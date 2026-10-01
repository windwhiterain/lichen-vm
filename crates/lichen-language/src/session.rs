//! The incremental [`BufferSession`]: an editable source buffer whose compile
//! reuses the established AST→IR→check when only a frontend *error* changed,
//! and whose `cache`d bindings are **retained cells** across a rebuild.
//!
//! The frontend absorbs every error at its own layer — a recovered parse error
//! and an *unresolved name* both lower to the **same** inert [`ExprKind::ErrorBlock`]
//! (see [`crate::compile`]), so the lowering is total and the checker sees
//! stable, name-free effective content.  The session runs the resolver
//! ([`crate::resolve`]) on every compile and tracks the **resolved content key**:
//! a name-free serialization over the resolver's `BinderId`s (names encoded by
//! the binding they resolve to, error blocks opaque, spans dropped).  An edit
//! that only extends an unresolved name (the editor's typing case), rewrites an
//! error block, or consistently renames a binding leaves the key unchanged and
//! reuses the established [`IR`] + [`Build`] — only the fresh frontend/resolve
//! diagnostics are re-derived.  This is the `T1` tier's user-facing behaviour:
//! typing a new unfinished piece never re-derives the established program.
//!
//! The reuse is key-addressed (a `content-key → build` cache), so it is sound
//! for an arbitrary edit that leaves the resolved structure alone — not just an
//! append.  A general edit that changes the resolved structure falls back to a
//! full re-lower + re-check; the cells are the **finer cut** under that
//! fallback: a marked binding the edit did not reach is lowered to a read of its
//! frozen artifact instead of being compiled, and only the cells dirty
//! propagation reaches are dropped (`crate::dirty`).
//!
//! # A caller's view
//!
//! A session over a **file** is not necessarily over a whole buffer: a real
//! source begins with a `@{…@}` block whose `@import`s resolve through the
//! package store, so a caller that runs the preprocessor itself (the language
//! server — it owns the store, and the block's directive spans) compiles the
//! text *after* the block.  [`BufferSession::set_view`] is how that caller hands
//! the session the region, where it begins in the original file, the file's line
//! starts, and the imports resolved for it, so every span the session produces
//! is absolute in the file the user is editing.
//!
//! The session's buffer is the **code**, and the imports are seeded into the
//! resolution ([`crate::resolve`]) — so a cell whose value reads an import
//! freezes a closure that resolves through the same registry the import lives in
//! ([`BufferSession::with_registry`], and the cell key space that keeps the two
//! allocators apart).

use std::collections::HashSet;
use std::sync::{Arc, PoisonError, RwLock};

use lichen_language_lex::{line_col, offset_of_span};
use lichen_lowlevel::{Eviction, ModuleKey, Registry};

use crate::LangProgramShape;
use crate::ast::{BlockStmt, Program, Stmt};
use crate::cells::{CellStore, SourceId};
use crate::compile::SpanIndex;
use crate::diag::{Diag, Stage};
use crate::lex;
use crate::parse;
use crate::path::Path;
use crate::persist::ProgramCodecOf;
use crate::preprocess::ResolvedImport;
use crate::program::GcdOp;
use crate::{ParseDiag, Report, build_report};
use lichen_highlevel::checker::Build;
use lichen_highlevel::native::no_native_ops;
use lichen_highlevel::program::{HighProgram, TypeOperator, ValueType};

/// The result of a [`BufferSession::compile`]: the checked build (shared, so it
/// is cheap to hold) plus every diagnostic, and the resolved content key the
/// compile ran under.
#[derive(Clone)]
pub struct SessionReport<P: HighProgram>
where
    P::Value: ValueType,
{
    /// The checked build — `Some` whenever the frontend resolved the program
    /// (including a partially recovered parse) and the checker ran on it.
    /// Shared, so reusing the session never requires rebuilding it.
    pub build: Option<Arc<Build<P>>>,
    /// Lex + parse (always fresh) and the checker's rendered failures (from the
    /// reused or freshly built [`Build`]).
    pub diagnostics: Vec<Diag<P>>,
    /// The token stream this report was derived from — every compile lexes (only
    /// the region an edit touched is re-lexed), and a caller that reads tokens
    /// (an editor's semantic tokens and offsets) reads them here rather than
    /// lexing the same text again.  Positions are absolute in the original file.
    pub tokens: Arc<Vec<lex::Token>>,
    /// The **resolved** AST this report was derived from: binder ids written,
    /// spans absolute in the original file.  Fresh on every compile — the parse
    /// runs even when the build is reused — so it is the current text's AST, not
    /// the cached build's.
    pub program: Arc<Program>,
    /// `ExprId → span` for [`Self::build`], in the original file's coordinates.
    /// `None` only when no build was produced.  On a reuse this is the retained
    /// index **moved** through the edit, so it describes the current text.
    pub span_index: Option<Arc<SpanIndex>>,
    /// The resolved content key (a name-free serialization over the resolver's
    /// `BinderId`s) this report was compiled under — equal across edits that
    /// only change an error block, extend an unresolved name, or consistently
    /// rename a binding.
    pub key: Vec<u64>,
    /// Whether the established build was reused because the resolved content key
    /// was unchanged (`true`) rather than freshly re-lowered and re-checked.
    pub reused: bool,
    /// What this compile did to the retained cells.  Zero on the reuse path:
    /// nothing was lowered at all, so no cell was read or frozen.
    pub cells: CellEvents,
}

/// What one compile did to the retained cells — the session's event surface.
///
/// Without it a caller cannot tell a rebuild that reused nine cells from one
/// that reused none, and the mechanism would be silent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellEvents {
    /// Marked bindings read from a retained artifact instead of being compiled.
    pub reused: usize,
    /// Cells this compile froze into the registry — a marked binding it compiled
    /// and could solve.  A failed build freezes none, so this is read from the
    /// store after the build rather than from what the lowering compiled.
    pub frozen: usize,
    /// Cells dropped before the lowering: the edit reached them, or the program
    /// no longer marks their position.
    pub dropped: usize,
}

impl<P: HighProgram> SessionReport<P>
where
    P::Value: ValueType,
{
    /// No errors and the program checked.
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty() && self.build.as_ref().is_some_and(|b| b.ok)
    }
}

/// An editable source buffer with a diff-gated compile.
pub struct BufferSession<P: ProgramCodecOf>
where
    P::Value: ValueType,
{
    /// The code compiled — the whole buffer, or the text after a stripped
    /// `@{…@}` block ([`Self::set_view`]).  This is what an edit changes and what
    /// the diff, the re-lex and the window splice are all over.
    source: String,
    /// Where [`Self::source`] begins in the original file: `0` for a whole-file
    /// buffer, the block's end for a view.
    base: u32,
    /// The **original file's** line starts, for a view with a base offset (a
    /// suffix's own line starts are not the file's).  Unused when `base` is zero,
    /// where they are derived from the buffer.
    line_starts: Vec<usize>,
    /// The imports resolved for the view, seeded into every resolution.
    imports: Vec<ResolvedImport>,
    /// The identity of this buffer **as a source** — what a cell recorded from it
    /// belongs to.  Caller-named, never derived from content
    /// ([`crate::cells::SourceId`]).
    source_id: SourceId,
    /// The retained cells of this buffer, and the registry their artifacts live
    /// in.  The registry is the session's because a cell's artifact must outlive
    /// the build that made it, exactly as a package's does for an import.
    cells: CellStore,
    registry: Arc<RwLock<Registry<P>>>,
    /// The artifacts of the cells this session dropped — its debt to the
    /// registry, taken by [`BufferSession::take_unreachable`].  The session does
    /// not evict them itself; see that method for why.
    unreachable: Vec<ModuleKey>,
    cache: Option<Cache<P>>,
    /// The state the last compile ran under — the baseline the *next* edit is
    /// diffed against and, for lexing, the token stream it resumes from.
    last: Option<LastState>,
}

/// The snapshot of the buffer a compile ran under.  Diffing
/// [`LastState::source`] against the current [`BufferSession::source`] yields
/// the edit span, `lex::lex_resume` reuses [`LastState::tokens`] to re-lex only
/// that span, and [`LastState::program`] is the AST the window splice re-parses
/// into.
struct LastState {
    source: String,
    /// The view this snapshot was taken under.  A view that moved makes every
    /// token range and span in the snapshot stale, so neither the incremental lex
    /// nor a reuse may read it.
    base: u32,
    line_starts: Vec<usize>,
    tokens: Arc<Vec<lex::Token>>,
    program: Arc<Program>,
}

struct Cache<P: HighProgram>
where
    P::Value: ValueType,
{
    /// The resolved content key the cached build was compiled under.
    key: Vec<u64>,
    /// The cached build, shared with any report that reused it.
    build: Option<Arc<Build<P>>>,
    /// The checker's rendered diagnostics for that build — static across edits
    /// that keep the resolved content (the fresh frontend diagnostics are
    /// re-derived per call).
    check_diagnostics: Vec<Diag<P>>,
    /// The build's `ExprId → span` index, and the same positions as **byte
    /// offsets** in the original file.
    ///
    /// Both are kept because an edit moves bytes, not `(line, col)` pairs: the
    /// offsets are what a reuse shifts, and the pair is what it returns.  They are
    /// always the *latest* mapping, so a reuse that moves them writes both back —
    /// otherwise the next reuse would shift an already-shifted position.
    span_index: Option<Arc<SpanIndex>>,
    span_bytes: Vec<Option<u32>>,
}

impl<P> Cache<P>
where
    P: HighProgram,
    P::Value: ValueType,
{
    /// The moved positions of a reuse: whether every retained position survives
    /// the edit, and if so what it moves to.
    ///
    /// A position **inside** the replaced text `[a, b_old)` has no counterpart in
    /// the new source — its bytes are gone — so a check diagnostic that points
    /// there cannot be moved, and the caller must rebuild rather than point it
    /// somewhere it does not mean.  A *span-index* entry inside the region is a
    /// different matter: it can only be an error block's (a check diagnostic never
    /// points into one, the lowering masks it), and such an entry becomes `None`
    /// — "no position" is exactly what an inert node has.
    fn moved(
        &self,
        a: u32,
        b_old: u32,
        delta: isize,
        old_starts: &[usize],
        new_starts: &[usize],
    ) -> Option<Moved<P>> {
        let mut diagnostics = self.check_diagnostics.clone();
        for diagnostic in &mut diagnostics {
            let Some(span) = diagnostic.span else {
                continue;
            };
            let byte = offset_of_span(old_starts, span) as u32;
            diagnostic.span = Some(line_col(
                new_starts,
                crate::spans::moved_offset(byte, a, b_old, delta)?,
            ));
        }
        let bytes: Vec<Option<u32>> = self
            .span_bytes
            .iter()
            .map(|byte| byte.and_then(|byte| crate::spans::moved_offset(byte, a, b_old, delta)))
            .collect();
        let spans: SpanIndex = bytes
            .iter()
            .map(|byte| byte.map(|byte| line_col(new_starts, byte)))
            .collect();
        Some(Moved {
            spans,
            bytes,
            diagnostics,
        })
    }
}

/// The positions of a [`Cache`] after a reuse moved them.
struct Moved<P: HighProgram>
where
    P::Value: ValueType,
{
    spans: SpanIndex,
    bytes: Vec<Option<u32>>,
    diagnostics: Vec<Diag<P>>,
}

impl<P> BufferSession<P>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    /// A new session over `source`, whose cells belong to the **empty** source
    /// name — an unnamed buffer, which is the single-buffer case.
    pub fn new(source: impl Into<String>) -> Self {
        Self::with_source_id(source, "")
    }

    /// A new session over `source`, whose cells belong to `source_id`: the
    /// caller's name for the file (a path, a URI).  A cell's identity is its
    /// occurrence path *plus* this name, so one store can hold several buffers
    /// without confusing them, and nothing about it is derived from content.
    pub fn with_source_id(source: impl Into<String>, source_id: impl Into<String>) -> Self {
        Self::with_registry(source, source_id, Arc::new(RwLock::new(Registry::new())))
    }

    /// [`Self::with_source_id`] over a **caller-owned registry** — the one a
    /// package store registered the imported packages in.
    ///
    /// A cell's artifact is frozen as a closure, and a closure that reads an
    /// import names that import's module key; the registry it is filed in must
    /// already hold the import, or the artifact would not resolve for the
    /// importer that reads it ([`Registry::freeze_closure_mapped`] asserts
    /// exactly that).  So a session that compiles a file with imports shares the
    /// store's registry, and the cell keys live in their own space
    /// ([`crate::cells`]) so the two allocators cannot meet.
    pub fn with_registry(
        source: impl Into<String>,
        source_id: impl Into<String>,
        registry: Arc<RwLock<Registry<P>>>,
    ) -> Self {
        BufferSession {
            source: source.into(),
            base: 0,
            line_starts: Vec::new(),
            imports: Vec::new(),
            source_id: source_id.into(),
            cells: CellStore::new(),
            registry,
            unreachable: Vec::new(),
            cache: None,
            last: None,
        }
    }

    /// The registry this session's cells are filed in — the handle a caller that
    /// shares it (a package store, another session) holds.
    pub fn registry(&self) -> Arc<RwLock<Registry<P>>> {
        Arc::clone(&self.registry)
    }

    /// The source name this session's cells are recorded under.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// How many cells the session currently retains.
    pub fn retained_cells(&self) -> usize {
        self.cells.len()
    }

    /// How many artifacts the session dropped and has not managed to evict — its
    /// outstanding debt to the registry ([`Self::evict_unreachable`]).
    ///
    /// A number that only grows is the leak §8 warns about: the caller is not
    /// evicting, or is still holding a report that keeps a refusal in place.
    pub fn pending_evictions(&self) -> usize {
        self.unreachable.len()
    }

    /// Evict the artifacts of the cells this session dropped, and report how many
    /// were freed.
    ///
    /// **The caller's precondition, and it cannot be checked here:** a static ref is
    /// a raw handle into an artifact's arena, and one survives in every
    /// [`SessionReport`] the caller still holds — a `build` is an `Arc`, so the
    /// session never sees the last clone die.  Call this only once those are
    /// dropped.  What *is* checked is the other half: an artifact that a surviving
    /// cell's artifact still references is refused ([`Eviction::StillReferenced`])
    /// and kept for a later call, so a cell that read another cell is not a dangle.
    ///
    /// Safe to call at any time and repeatedly; a refusal is retried on the next
    /// call, once the artifact that referenced it is gone too.  Two artifacts that
    /// reference each other are never freed — the honest answer for a cycle.
    pub fn evict_unreachable(&mut self) -> usize {
        let mut pending = std::mem::take(&mut self.unreachable);
        let mut registry = self
            .registry
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let mut freed = 0;
        // Repeated passes, because a key that is still referenced may be free once
        // the artifact referencing it is gone — and that artifact may be in this
        // same list, later in it.  A pass that frees nothing has reached the fixed
        // point: what is left is referenced by something still filed.
        loop {
            let before = pending.len();
            let mut refused = Vec::new();
            for key in pending.drain(..) {
                match registry.evict(key) {
                    Eviction::Freed => freed += 1,
                    // Nothing filed under it: an earlier pass freed it, or it never
                    // reached the registry.  Not a debt any more.
                    Eviction::NotRegistered => {}
                    Eviction::StillReferenced => refused.push(key),
                }
            }
            let progressed = refused.len() < before;
            pending = refused;
            if !progressed {
                break;
            }
        }
        self.unreachable = pending;
        freed
    }

    /// The current source.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The source's length in bytes.
    pub fn len(&self) -> usize {
        self.source.len()
    }

    /// Whether the source is empty.
    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    /// Insert `text` at byte `byte`.
    pub fn insert(&mut self, byte: usize, text: &str) {
        self.source.insert_str(byte, text);
    }

    /// Append `text` at the end of the buffer.
    pub fn push(&mut self, text: &str) {
        self.source.push_str(text);
    }

    /// Remove the bytes in `range`.
    pub fn remove(&mut self, range: std::ops::Range<usize>) {
        self.source.replace_range(range, "");
    }

    /// Replace the bytes in `range` with `text`.
    pub fn replace(&mut self, range: std::ops::Range<usize>, text: &str) {
        self.source.replace_range(range, text);
    }

    /// Replace the whole buffer with `source` — what a caller that re-reads a
    /// **file** has, rather than an edit range.
    ///
    /// This is the file-diff entry point, and it is deliberately the same thing
    /// the byte-level edits do: it only sets the buffer.  The edit itself is still
    /// the session's to derive — the next [`Self::compile`] diffs this against the
    /// last state — so a caller that re-reads a file gets the incremental lex, the
    /// statement-window splice and the cell reuse without describing the change,
    /// and a caller that has a range uses [`Self::replace`].  Both are the same
    /// path from there on.
    ///
    /// It sets the **whole-file** view: base `0` and no imports, because a whole
    /// file's imports are the ones its own `@{…@}` block resolves — which is the
    /// caller's stage, not the session's.  A caller that runs the preprocessor
    /// itself uses [`Self::set_view`] instead.
    pub fn set_source(&mut self, source: impl Into<String>) {
        self.source = source.into();
        self.base = 0;
        self.line_starts.clear();
        self.imports.clear();
    }

    /// Point the session at a **caller's frontend view**: the code to compile,
    /// where it begins in the original file, that file's line starts, and the
    /// imports resolved for it.
    ///
    /// A real source begins with a `@{…@}` block whose `@import`s resolve through
    /// the package store, so the caller that owns the store (the language server)
    /// runs the preprocessor and compiles what follows it.  `code` is that text,
    /// `base` is its byte offset in the original file, `line_starts` is the
    /// **original file's** (a suffix's own are not the file's), and `imports` are
    /// the block's resolved bindings — seeded into every resolution, so a name
    /// that resolves to an import is not reported unresolved.
    ///
    /// The session's buffer *is* the code, so an edit anywhere in it — including
    /// the block's own text, which moves `base` — is diffed like any other.  A
    /// view whose mapping moved drops the incremental snapshot (its token ranges
    /// and spans are in the old coordinates) and re-parses whole; the cells are
    /// untouched, since their identity is a path, not a position.
    ///
    /// This is the whole-buffer entry point for a caller with a view: the
    /// byte-edit methods ([`Self::insert`], [`Self::replace`], …) keep the view
    /// they were given and re-derive nothing, so a caller with a base offset feeds
    /// the next buffer through `set_view` too.
    pub fn set_view(
        &mut self,
        code: impl Into<String>,
        base: u32,
        line_starts: &[usize],
        imports: &[ResolvedImport],
    ) {
        self.source = code.into();
        self.base = base;
        self.line_starts = line_starts.to_vec();
        self.imports = imports.to_vec();
    }

    /// The resolved content key of the last compile.
    pub fn key(&self) -> Vec<u64> {
        self.cache
            .as_ref()
            .map(|c| c.key.clone())
            .unwrap_or_default()
    }

    /// Compile and check the current buffer.
    ///
    /// Lexes and parses the source (to re-derive the frontend diagnostics and
    /// the current resolved structure), runs the resolver (assigning `BinderId`s
    /// and emitting the resolve diagnostics), computes the **resolved content
    /// key**, and **reuses the cached build when it is unchanged** — so an edit
    /// that only extends an unresolved name (or an error block, or a consistent
    /// rename) never re-lowers or re-checks the established program.  A changed
    /// key re-lowers and re-checks, then refreshes the cache.
    ///
    /// When the previous compile left a [`LastState`] snapshot, lexing is
    /// **incremental** (`lex::lex_resume` over the edit span) rather than a
    /// whole-buffer re-lex, so a keystroke in a long buffer is `O(edit)` in the
    /// regex work.  The result is identical to a full re-lex (`lex_resume` is
    /// proven equal to [`lex::lex_with`] — see the lex tests); only the cost
    /// changes.
    pub fn compile(&mut self) -> SessionReport<P> {
        // The line starts every span this compile produces is measured against:
        // the **original file's**.  A whole-file buffer derives them from the
        // buffer (the byte-edit methods mutate it directly); a view with a base
        // offset carries them, because a suffix's line starts are not the file's.
        let line_starts = view_line_starts(&self.source, self.base, &self.line_starts);

        // Whether the previous snapshot is in the same coordinate space.  A view
        // that moved (an edited `@{…@}` block) leaves every token range and span in
        // the snapshot describing the old file, so neither the incremental lex nor
        // a reuse may read it — the cells are untouched either way, since a cell's
        // identity is a path, not a position.
        let view_same = self.last.as_ref().is_some_and(|prev| {
            prev.base == self.base && (self.base == 0 || prev.line_starts == self.line_starts)
        });

        // Whether a prior snapshot plus a changed source lets us re-derive only
        // the touched region (both lex and parse become incremental); otherwise
        // the whole buffer is re-lexed and re-parsed.
        let edit = self
            .last
            .as_ref()
            .filter(|prev| view_same && prev.source != self.source)
            .map(|prev| edit_span(&prev.source, &self.source));
        // The same edit in the original file's coordinates: token ranges and spans
        // are absolute, so the diff's code-relative span is what has to move.
        let absolute_edit =
            edit.map(|(a, b, delta)| (a + self.base as usize, b + self.base as usize, delta));
        // The line starts the snapshot's spans are in — needed to locate a
        // retained position's byte before the edit moves it.
        let previous_starts = self
            .last
            .as_ref()
            .filter(|_| view_same)
            .map(|prev| view_line_starts(&prev.source, prev.base, &prev.line_starts));

        // Lex: resume from the snapshot's token stream over the edit span; on the
        // first compile (or a reset without an edit) lex the whole buffer.
        let (tokens, lex_errors) = match (&self.last, absolute_edit) {
            (Some(prev), Some((a, b, _delta))) => {
                let lexed = lex::lex_resume(
                    &prev.tokens,
                    &prev.source,
                    &self.source,
                    &line_starts,
                    self.base,
                    a,
                    b,
                );
                (lexed.tokens, lexed.errors)
            }
            _ => {
                let lexed = lex::lex_with(&self.source, &line_starts, self.base);
                (lexed.tokens, lexed.errors)
            }
        };
        let mut diagnostics: Vec<Diag<P>> = lex_errors.into_iter().map(Diag::from_lex).collect();

        // Parse: re-parse only the statement window the edit touched and splice
        // it into the snapshot's program when that is safe; otherwise parse the
        // whole buffer (the result is identical either way).  The window comes
        // back in **both** index spaces — the statements the edit replaced in the
        // snapshot's program and the statements it re-parsed in the fresh one —
        // because dirty propagation runs over both (see `crate::dirty`).
        let (mut program, errors, windows) = match (&self.last, absolute_edit) {
            (Some(prev), Some((a, b, delta))) => {
                let previous_starts = previous_starts.as_deref().unwrap_or(&line_starts);
                match splice_program(
                    &prev.tokens,
                    &prev.program,
                    previous_starts,
                    &tokens,
                    &line_starts,
                    a,
                    b,
                    delta,
                ) {
                    Some(out) => {
                        let SpliceOut {
                            program,
                            errors,
                            lo,
                            old_hi,
                            hi,
                        } = out;
                        (program, errors, Some((lo..old_hi, lo..hi)))
                    }
                    None => {
                        let p = full_parse(&tokens);
                        (p.0, p.1, None)
                    }
                }
            }
            _ => {
                let p = full_parse(&tokens);
                (p.0, p.1, None)
            }
        };
        diagnostics.extend(errors.into_iter().map(Diag::from_parse));

        // Resolve: the single resolution authority — assigns a `BinderId` to
        // each binder, writes it into the AST's resolve fields, and emits the
        // resolve diagnostics.  Then the resolved content key: a name-free,
        // digest-free serialization over those `BinderId`s, which is what the
        // lowering actually consumes.  The view's imports are the base scope, so a
        // name that resolves to an import is not an unresolved name.
        let resolved = crate::resolve::resolve(&mut program, &self.imports);
        let key = crate::resolve::content_key(&program);
        diagnostics.extend(resolved.diagnostics.iter().cloned().map(|d| d.retype()));
        let tokens = Arc::new(tokens);
        let program = Arc::new(program);

        // Reuse: the resolved content is unchanged, so the established build is
        // exactly right.  Only the (fresh, above) frontend/resolve diagnostics
        // moved; the lowering and check are skipped entirely.  The store is left
        // alone too: no cell is consulted when no lowering happens, and the
        // build being reused *is* the one that was correct for this content — the
        // reconciliation belongs to the next lowering, against the program it
        // will lower.
        //
        // The retained positions must also *survive* the edit: a rendered
        // diagnostic is a `(line, col)` pair, and the content key is span-free, so
        // an edit that moves text without changing the resolved structure leaves
        // the cached spans describing the old file.  They are moved through the
        // edit here, and a position the edit replaced outright — which no honest
        // mapping exists for — falls through to the rebuild below instead.
        if view_same
            && let Some(cache) = &self.cache
            && cache.key == key
            && cache.build.is_some()
        {
            let moved = match absolute_edit {
                // Nothing moved: the retained positions are already current.
                None => Some(None),
                Some((a, b, delta)) => {
                    let old_starts = previous_starts.as_deref().unwrap_or(&line_starts);
                    cache
                        .moved(
                            a as u32,
                            (b as isize - delta) as u32,
                            delta,
                            old_starts,
                            &line_starts,
                        )
                        .map(Some)
                }
            };
            if let Some(moved) = moved {
                let cache = self
                    .cache
                    .as_mut()
                    .expect("the reuse was decided from this cache");
                if let Some(moved) = moved {
                    cache.span_index = Some(Arc::new(moved.spans));
                    cache.span_bytes = moved.bytes;
                    cache.check_diagnostics = moved.diagnostics;
                }
                let mut all = diagnostics;
                all.extend(cache.check_diagnostics.iter().cloned());
                let build = Arc::clone(
                    cache
                        .build
                        .as_ref()
                        .expect("a reuse needs the build it reuses"),
                );
                let span_index = cache.span_index.clone();
                self.last = Some(LastState {
                    source: self.source.clone(),
                    base: self.base,
                    line_starts: self.line_starts.clone(),
                    tokens: Arc::clone(&tokens),
                    program: Arc::clone(&program),
                });
                return SessionReport {
                    build: Some(build),
                    diagnostics: all,
                    tokens,
                    program,
                    span_index,
                    key,
                    reused: true,
                    cells: CellEvents::default(),
                };
            }
        }

        // Rebuild: the cells first, because the lowering reads them.  The store
        // is reconciled with the program about to be lowered — a cell the edit
        // reached, or a position the program no longer marks, must not be read —
        // and what that drops is the session's debt to the registry.  A full
        // re-parse (or the first compile) has no window: the whole program is
        // the dirty region, which is the honest answer.
        let marked: HashSet<Path> = crate::compile::cached_bindings(&program)
            .into_values()
            .collect();
        let dirty = match &self.last {
            Some(last) => {
                let (previous_window, current_window) = windows.unwrap_or_else(|| {
                    (
                        0..logical_statements(&last.program),
                        0..logical_statements(&program),
                    )
                });
                crate::dirty::dirty_marked_paths(
                    &last.program,
                    &program,
                    previous_window,
                    current_window,
                )
            }
            None => HashSet::new(),
        };
        let mut unreachable = self.cells.invalidate_paths(&dirty);
        unreachable.extend(self.cells.retain_marked(&marked));
        let dropped = unreachable.len();
        self.unreachable.extend(unreachable);
        // Counted *after* the reconciliation: what the lowering will actually
        // read back is what survived it.
        let reused_cells = marked
            .iter()
            .filter(|path| self.cells.reference(path).is_some())
            .count();
        // Lower the already-resolved program (total) and check.  The session ran
        // the resolver itself, so it lowers via `compile_resolved_with_cells`
        // rather than `compile_with_imports` (which would resolve again) — with
        // the cells, so a clean one becomes a static read of its frozen artifact
        // and the marked bindings that *were* compiled come back to be frozen
        // once the build is solved.
        let (ir, span_index, compiled_cells) = crate::compile::compile_resolved_with_cells(
            &program,
            &resolved.import_binders,
            Some(&self.cells),
        );
        let mut report: Report<P> = build_report::<P>(
            Some(ir),
            Some(span_index),
            diagnostics,
            Some(Arc::clone(&self.registry)),
            no_native_ops(),
            Some(&mut self.cells),
            compiled_cells,
            &self.source_id,
        );
        let check_diagnostics: Vec<Diag<P>> = report
            .diagnostics
            .iter()
            .filter(|d| d.stage == Stage::Check)
            .cloned()
            .collect();
        // The build's span index, and the same positions as byte offsets: an edit
        // moves bytes, so the offsets are what a later reuse shifts (see `Cache`).
        let span_index = report.span_index.take();
        let span_bytes: Vec<Option<u32>> = span_index
            .as_ref()
            .map(|index| {
                index
                    .iter()
                    .map(|span| span.map(|span| offset_of_span(&line_starts, span) as u32))
                    .collect()
            })
            .unwrap_or_default();
        let span_index = span_index.map(Arc::new);
        let build = report.build.map(Arc::new);
        self.cache = Some(Cache {
            key: key.clone(),
            build: build.clone(),
            check_diagnostics,
            span_index: span_index.clone(),
            span_bytes,
        });
        self.last = Some(LastState {
            source: self.source.clone(),
            base: self.base,
            line_starts: self.line_starts.clone(),
            tokens: Arc::clone(&tokens),
            program: Arc::clone(&program),
        });
        SessionReport {
            build,
            diagnostics: report.diagnostics,
            tokens,
            program,
            span_index,
            key,
            reused: false,
            cells: CellEvents {
                reused: reused_cells,
                // The store now holds exactly the cells that were reused plus the
                // ones this build froze, and both are subsets of `marked`.
                frozen: self.cells.len().saturating_sub(reused_cells),
                dropped,
            },
        }
    }
}

/// The logical statement count of a program: its statements plus its tail
/// expression when it has one (the index space [`Program::stmt_ranges`] uses).
fn logical_statements(program: &Program) -> usize {
    program.statements.len() + usize::from(program.expr.is_some())
}

/// The line starts a view's spans are measured against: the original file's.
///
/// A whole-file view (`base == 0`) *is* the file, so its line starts are the
/// buffer's and are derived here — that is also what keeps the byte-edit methods
/// (`insert`/`replace`/…), which mutate the buffer directly, in step with the
/// model.  A view with a base offset carries the file's, because a suffix's own
/// line starts are not the file's.
fn view_line_starts(code: &str, base: u32, line_starts: &[usize]) -> Vec<usize> {
    if base == 0 {
        lex::line_starts(code)
    } else {
        line_starts.to_vec()
    }
}

/// The minimal byte span `[a, b)` of `new` that differs from `old`, plus the
/// length delta (`new.len() - old.len()`).
///
/// `a` is the common-prefix length — a byte position valid in *both* sources,
/// because every byte before it is identical.  `b` is `new.len()` minus the
/// common-suffix length, i.e. the byte position in `new` where the unchanged
/// tail begins.  Together they describe the smallest region an edit changed, in
/// the coordinates `lex::lex_resume` expects (`a` against the old token stream,
/// `b` against the new source, with the delta converting between them).
fn edit_span(old: &str, new: &str) -> (usize, usize, isize) {
    let delta = new.len() as isize - old.len() as isize;
    let ob = old.as_bytes();
    let nb = new.as_bytes();
    let max = ob.len().min(nb.len());
    let mut a = 0;
    while a < max && ob[a] == nb[a] {
        a += 1;
    }
    let mut suf = 0;
    while suf < max - a && ob[ob.len() - 1 - suf] == nb[nb.len() - 1 - suf] {
        suf += 1;
    }
    (a, nb.len() - suf, delta)
}

/// The full frontend for `tokens`: `parse` the whole stream (the fallback used
/// when a window cannot be spliced incrementally).
fn full_parse(tokens: &[lex::Token]) -> (Program, Vec<ParseDiag>) {
    let parsed = parse::parse(tokens);
    (parsed.program, parsed.errors)
}

/// Re-parse the statement window an edit touched and splice it into the previous
/// program, returning the fresh frontend `(program, parse_errors)`, or `None`
/// when the edit cannot be handled incrementally (the caller re-parses the whole
/// buffer instead).
///
/// `old_program` is the program the previous compile ran under, `old_tokens` its
/// token stream, and `new_tokens` the (already incrementally re-lexed) stream
/// for the current source; `a`,`b` are the edit span in the new source and
/// `delta = new.len() - old.len()`.
///
/// The window is chosen conservatively and the result must be exactly what a
/// whole-buffer parse of `new_tokens` produces.  If any invariant cannot be
/// confirmed — a degenerate program, an error block lying outside the window
/// (whose diagnostic would be dropped), a trailing binding, a token shift that
/// goes negative — `None` is returned and the caller falls back, so correctness
/// never depends on the borderline cases.
fn splice_program(
    old_tokens: &[lex::Token],
    old_program: &Program,
    old_starts: &[usize],
    new_tokens: &[lex::Token],
    new_starts: &[usize],
    a: usize,
    b: usize,
    delta: isize,
) -> Option<SpliceOut> {
    // Number of logical statements = `statements`, plus the tail expression
    // when there is one (a record program has no tail).
    let old_n = old_program.statements.len() + usize::from(old_program.expr.is_some());
    if old_program.stmt_ranges.len() != old_n || old_n == 0 {
        return None;
    }

    // The old byte range the edit replaced: [e_start, e_end).
    let e_start = a as isize;
    let e_end = b as isize - delta;
    if e_start < 0 || e_end < e_start {
        return None;
    }

    // The old byte range [sb, eb) of each logical statement, from old tokens.
    let byte_range = |i: usize| -> Option<(isize, isize)> {
        let (ts, te) = old_program.stmt_ranges[i];
        if te <= ts {
            return None;
        }
        let sb = old_tokens.get(ts)?.range.0 as isize;
        let eb = old_tokens.get(te - 1)?.range.1 as isize;
        Some((sb, eb))
    };

    // First/last logical statements whose body overlaps the edit; when none do
    // (a separator-only edit or an append at/after the last statement), the
    // window is the tail from the last statement before the edit to the end.
    let mut lo = old_n;
    let mut hi = 0usize;
    for i in 0..old_n {
        if let Some((sb, eb)) = byte_range(i)
            && eb > e_start
            && sb < e_end
        {
            lo = lo.min(i);
            hi = hi.max(i + 1);
        }
    }
    if lo > hi {
        // No statement body overlaps: the edit sits in a separator, or appends
        // at/after the last statement.  What the edit produced lands between the
        // statement ending at/before it and the one after, so the window is those
        // **two** — not, as it once was, everything from there to the end of the
        // buffer (measured: an append to a line re-parsed and re-dirtied the whole
        // tail, 82-90% of a rebuild).
        //
        // It must be two rather than one: an insertion at the boundary is inside
        // the byte range spanning them, and the region parse is byte-bounded, so
        // every statement the edit inserted there is re-parsed as well.  An
        // insertion at the very end of the buffer has no statement after it, so
        // `hi` clamps to the end — which is also the `prev == None` case's floor.
        let prev = (0..old_n)
            .rev()
            .find(|&i| byte_range(i).is_some_and(|(_, eb)| eb <= e_start));
        lo = prev.unwrap_or(0);
        hi = (lo + 2).min(old_n);
    }
    // The old token range and byte range of the window.
    let old_tl = old_program.stmt_ranges[lo].0;
    let old_th = old_program.stmt_ranges[hi - 1].1;
    if old_th <= old_tl {
        return None;
    }
    let win_ob = old_tokens[old_tl].range.0 as isize;
    let win_eb = old_tokens[old_th - 1].range.1 as isize;

    // An error block outside the window means a parse diagnostic for an
    // untouched statement would be dropped by the splice — fall back so no
    // diagnostic is lost.
    for block in &old_program.error_blocks {
        let (b0, b1) = (block.range.0 as isize, block.range.1 as isize);
        if b1 > win_ob && b0 < win_eb {
            continue; // inside the window; re-derived by the region parse.
        }
        return None;
    }

    // The window's byte range in the new source.  Its **start** is at or before
    // the edit start (the window's first statement is one the edit reached, and a
    // statement starting *inside* the replaced region was replaced, so it clamps
    // to the edit start); its **end** is where the untouched suffix begins, which
    // is at or after the edit end, because a statement overlapping the edit is in
    // the window and the first one after it therefore cannot be.
    //
    // Projecting the end from the *suffix* rather than from the window's own last
    // token is what keeps an edit that deletes whole statements from cutting the
    // window through the statement that follows: the deleted text includes the
    // separator between them, so the window's last token ends *inside* the
    // replaced region and projecting it would leave half a statement in the
    // window — and a window that reaches past the edit would drop the new
    // statements entirely.
    let suffix_ob = if hi < old_n {
        old_tokens[old_program.stmt_ranges[hi].0].range.0 as isize
    } else {
        // The window reaches the end, so the "suffix" is the end of the old
        // source and the window's new end is the end of the new one — which is
        // what puts an append inside the window.
        old_tokens
            .last()
            .map_or(win_eb, |token| token.range.1 as isize)
    };
    let new_ob = win_ob.min(e_start);
    let new_eb = suffix_ob + delta;

    // New token index range [ns, ne) covering the window: from the first token
    // the edit reaches to the suffix's first **statement** token.  A separator at
    // the boundary is not a statement, so it is skipped — and the index `ne`
    // lands on is exactly the token the suffix shift below is measured against,
    // in both the old and the new stream.
    let ns = new_tokens
        .iter()
        .position(|t| t.range.1 as isize > new_ob)
        .unwrap_or(new_tokens.len());
    let mut ne = new_tokens.len();
    for (k, t) in new_tokens.iter().enumerate() {
        if t.range.0 as isize >= new_eb && t.kind != lex::TokenKind::Separator {
            ne = k;
            break;
        }
    }
    let eof = new_tokens.len().saturating_sub(1); // the lexer's trailing Eof.
    if ns > eof {
        return None;
    }
    let ns = ns.min(eof);
    // An **empty** window is a legal splice: the edit deleted statements outright,
    // so the prefix and the suffix meet with nothing between them.  (`ns > ne`
    // cannot arise from the two scans above; the clamp only makes the empty case
    // explicit.)
    let ne = ne.clamp(ns, eof);
    if ns > ne {
        return None;
    }

    // An empty window is a legal splice — the edit deleted statements outright, so
    // the prefix and the suffix meet with nothing between them — but it is not a
    // *parse*: the region parser requires at least one statement, so asking it for
    // an empty region reports "found the end of the program".
    let (win_stmts, win_ranges, win_errors) = if ns == ne {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        crate::parse::parse_statement_region_traced(new_tokens, ns, ne)
    };

    // Spliced logical statements: the unchanged prefix, the freshly re-parsed
    // window, then the unchanged suffix.  The suffix's *content* is untouched
    // but its token indices shift by `dk`, since the window's token count may
    // have changed (an insertion/removal before it).  `dk` is measured from the
    // suffix's **own** first token in each stream, which is the same token on
    // both sides — not from the window's end, which the edit may have deleted
    // along with the separator that used to sit there.
    let dk = if hi < old_n {
        ne as isize - old_program.stmt_ranges[hi].0 as isize
    } else {
        0
    };
    let mut stmts: Vec<BlockStmt> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    stmts.extend(old_program.statements[..lo].iter().cloned());
    ranges.extend(old_program.stmt_ranges[..lo].iter().copied());
    stmts.extend(win_stmts.iter().cloned());
    ranges.extend(win_ranges.iter().copied());
    for i in hi..old_n {
        let mut statement = if i < old_program.statements.len() {
            old_program.statements[i].clone()
        } else {
            // The tail expression (only in a tail program) — represented here
            // as a bare-expression block statement so the pop below can pick
            // it out like the whole-program parser does.
            debug_assert!(old_program.expr.is_some());
            BlockStmt {
                stmt: Stmt::Expr(old_program.expr.clone().unwrap()),
                public: false,
            }
        };
        // A clone's bytes are unchanged but its *position* is not: an edit that
        // adds or removes a line moves every statement after it, and a span is a
        // `(line, col)` pair rather than a byte offset.  Without this shift a
        // diagnostic inside a cloned statement renders on the wrong line
        // (`crate::spans`).
        crate::spans::shift_stmt(&mut statement.stmt, old_starts, new_starts, delta);
        stmts.push(statement);
        let (r0, r1) = old_program.stmt_ranges[i];
        let s0 = r0 as isize + dk;
        let s1 = r1 as isize + dk;
        if s0 < 0 {
            return None;
        }
        ranges.push((s0 as usize, s1 as usize));
    }

    // Pop the final logical statement as the program's value, mirroring the
    // whole-program parser: a trailing bare expression is the tail (the
    // program's value); a trailing binding leaves no tail — the program is a
    // record program (a module), so every statement is a field.
    let last_stmt = stmts.pop()?;
    let last_range = ranges.pop()?;
    let (statements, expr) = match last_stmt.stmt {
        Stmt::Expr(e) => (stmts, Some(e)),
        Stmt::Binding(_) => {
            stmts.push(last_stmt);
            (stmts, None)
        }
    };
    ranges.push(last_range);
    let mut program = Program {
        statements,
        expr,
        error_blocks: Vec::new(),
        stmt_ranges: ranges,
    };
    program.error_blocks = crate::parse::collect_error_blocks(&program);
    Some(SpliceOut {
        program,
        errors: win_errors,
        lo,
        old_hi: hi,
        hi: lo + win_stmts.len(),
    })
}

/// The outcome of a window splice: the fresh frontend plus the window extent in
/// **both** index spaces — the statements the edit replaced in the snapshot's
/// program (`lo..old_hi`) and the statements it re-parsed in the new one
/// (`lo..hi`).  The two differ whenever the edit added or removed statements,
/// which is exactly what dirty propagation has to see.
struct SpliceOut {
    program: Program,
    errors: Vec<ParseDiag>,
    lo: usize,
    old_hi: usize,
    hi: usize,
}

#[cfg(test)]
#[path = "tests/session_tests.rs"]
mod tests;
