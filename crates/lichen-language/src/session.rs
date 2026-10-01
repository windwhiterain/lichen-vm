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

use std::collections::HashSet;
use std::sync::{Arc, PoisonError, RwLock};

use lichen_lowlevel::{Eviction, ModuleKey, Registry};

use crate::LangProgramShape;
use crate::ast::{BlockStmt, Program, Stmt};
use crate::cells::{CellStore, SourceId};
use crate::diag::{Diag, Stage};
use crate::lex;
use crate::parse;
use crate::path::Path;
use crate::persist::ProgramCodecOf;
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
    source: String,
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
    tokens: Vec<lex::Token>,
    program: Program,
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
        BufferSession {
            source: source.into(),
            source_id: source_id.into(),
            cells: CellStore::new(),
            registry: Arc::new(RwLock::new(Registry::new())),
            unreachable: Vec::new(),
            cache: None,
            last: None,
        }
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
        let line_starts = lex::line_starts(&self.source);

        // Whether a prior snapshot plus a changed source lets us re-derive only
        // the touched region (both lex and parse become incremental); otherwise
        // the whole buffer is re-lexed and re-parsed.
        let edit = self
            .last
            .as_ref()
            .filter(|prev| prev.source != self.source)
            .map(|prev| edit_span(&prev.source, &self.source));

        // Lex: resume from the snapshot's token stream over the edit span; on the
        // first compile (or a reset without an edit) lex the whole buffer.
        let (tokens, lex_errors) = match (&self.last, edit) {
            (Some(prev), Some((a, b, _delta))) => {
                let lexed =
                    lex::lex_resume(&prev.tokens, &prev.source, &self.source, &line_starts, a, b);
                (lexed.tokens, lexed.errors)
            }
            _ => {
                let lexed = lex::lex_with(&self.source, &line_starts, 0);
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
        let (mut program, errors, windows) = match (&self.last, edit) {
            (Some(prev), Some((a, b, delta))) => {
                match splice_program(&prev.tokens, &prev.program, &tokens, a, b, delta) {
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
        // lowering actually consumes.
        let resolved = crate::resolve::resolve(&mut program, &[]);
        let key = crate::resolve::content_key(&program);
        diagnostics.extend(resolved.diagnostics.iter().cloned().map(|d| d.retype()));

        // Reuse: the resolved content is unchanged, so the established build is
        // exactly right.  Only the (fresh, above) frontend/resolve diagnostics
        // moved; the lowering and check are skipped entirely.  The store is left
        // alone too: no cell is consulted when no lowering happens, and the
        // build being reused *is* the one that was correct for this content — the
        // reconciliation belongs to the next lowering, against the program it
        // will lower.
        if let Some(cache) = &self.cache
            && cache.key == key
            && let Some(build) = &cache.build
        {
            let mut all = diagnostics;
            all.extend(cache.check_diagnostics.iter().cloned());
            self.last = Some(LastState {
                source: self.source.clone(),
                tokens,
                program,
            });
            return SessionReport {
                build: Some(Arc::clone(build)),
                diagnostics: all,
                key,
                reused: true,
                cells: CellEvents::default(),
            };
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
        let report: Report<P> = build_report::<P>(
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
        let build = report.build.map(Arc::new);
        self.cache = Some(Cache {
            key: key.clone(),
            build: build.clone(),
            check_diagnostics: check_diagnostics.clone(),
        });
        self.last = Some(LastState {
            source: self.source.clone(),
            tokens,
            program,
        });
        SessionReport {
            build,
            diagnostics: report.diagnostics,
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
    new_tokens: &[lex::Token],
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
        // at/after the last statement.  Re-parse from the statement that ends
        // at/before the edit to the end of the buffer.
        let prev = (0..old_n)
            .rev()
            .find(|&i| byte_range(i).is_some_and(|(_, eb)| eb <= e_start));
        lo = prev.unwrap_or(0);
        hi = old_n;
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
        let stmt = if i < old_program.statements.len() {
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
        stmts.push(stmt);
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
