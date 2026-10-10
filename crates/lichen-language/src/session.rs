//! The incremental [`BufferSession`]: an editable buffer with retained cells.
//!
//! # Invariant
//!
//! Reuse is key-addressed on the resolved content key, so it is sound for any
//! edit that leaves the resolved structure alone; the cells are the finer cut
//! under that gate. See `docs/notes/incremental-update.md` §6.

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

/// The result of a [`BufferSession::compile`]: the build, diagnostics, tokens and key.
#[derive(Clone)]
pub struct SessionReport<P: HighProgram>
where
    P::Value: ValueType,
{
    /// The checked build, shared, so a reuse never rebuilds it.
    ///
    /// # Invariant
    ///
    /// `Some` whenever the frontend resolved the program — including a partially
    /// recovered parse — and the checker ran on it.
    pub build: Option<Arc<Build<P>>>,
    /// Lex and parse diagnostics (always fresh) plus the checker's failures.
    pub diagnostics: Vec<Diag<P>>,
    /// The token stream this report derives from, absolute in the original file.
    pub tokens: Arc<Vec<lex::Token>>,
    /// The resolved AST this report derives from; fresh on every compile.
    pub program: Arc<Program>,
    /// `ExprId` → span for [`Self::build`], in the original file's coordinates.
    pub span_index: Option<Arc<SpanIndex>>,
    /// The resolved content key this report was compiled under.
    pub key: Vec<u64>,
    /// Whether the established build was reused rather than re-lowered.
    pub reused: bool,
    /// What this compile did to the retained cells; zero on the reuse path.
    pub cells: CellEvents,
}

/// What one compile did to the retained cells — the session's event surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellEvents {
    /// Marked bindings read from a retained artifact instead of being compiled.
    pub reused: usize,
    /// Cells this compile froze into the registry, read back from the store.
    pub frozen: usize,
    /// Cells dropped before the lowering: the edit reached them, or the mark is gone.
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
    /// The code compiled: the whole buffer, or the text after a stripped block.
    source: String,
    /// Where `source` begins in the original file: `0` for a whole-file buffer.
    base: u32,
    /// The original file's line starts, for a view with a base offset.
    line_starts: Vec<usize>,
    /// The imports resolved for the view, seeded into every resolution.
    imports: Vec<ResolvedImport>,
    /// This buffer's identity as a source: caller-named, never derived from content.
    source_id: SourceId,
    /// This buffer's retained cells, and the registry their artifacts live in.
    ///
    /// # Invariant
    ///
    /// The session owns the registry, because a cell's artifact must outlive the
    /// build that made it.
    cells: CellStore,
    registry: Arc<RwLock<Registry<P>>>,
    /// The artifacts of the dropped cells; [`Self::evict_unreachable`] pays the debt.
    unreachable: Vec<ModuleKey>,
    cache: Option<Cache<P>>,
    /// The state the last compile ran under: the baseline the next edit is diffed against.
    last: Option<LastState>,
}

/// The snapshot of the buffer a compile ran under.
struct LastState {
    source: String,
    /// The view this snapshot was taken under; a moved view makes it stale.
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
    /// The checker's rendered diagnostics for the cached build.
    check_diagnostics: Vec<Diag<P>>,
    /// The build's `ExprId` → span index, and the same positions as byte offsets.
    ///
    /// # Invariant
    ///
    /// Both are always the latest mapping: an edit moves bytes, not `(line, col)`
    /// pairs, so a reuse that moves them writes both back, or the next reuse
    /// would shift an already-shifted position.
    span_index: Option<Arc<SpanIndex>>,
    span_bytes: Vec<Option<u32>>,
}

impl<P> Cache<P>
where
    P: HighProgram,
    P::Value: ValueType,
{
    /// The moved positions of a reuse: whether every retained position survives.
    ///
    /// # Invariant
    ///
    /// A position inside the replaced text has no counterpart in the new source,
    /// so a check diagnostic that points there cannot be moved and the caller
    /// rebuilds; a span-index entry inside the region becomes `None` (it can only
    /// be an error block's, for which "no position" is already right).
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
    /// A new session over `source`, whose cells belong to the empty source name.
    pub fn new(source: impl Into<String>) -> Self {
        Self::with_source_id(source, "")
    }

    /// A new session over `source`, whose cells belong to `source_id`.
    ///
    /// # Invariant
    ///
    /// A cell's identity is its occurrence path plus this name, and nothing about
    /// it is derived from content, so one store can hold several buffers.
    pub fn with_source_id(source: impl Into<String>, source_id: impl Into<String>) -> Self {
        Self::with_registry(source, source_id, Arc::new(RwLock::new(Registry::new())))
    }

    /// [`Self::with_source_id`] over a caller-owned registry, shared with imports.
    ///
    /// # Invariant
    ///
    /// A cell's artifact is frozen as a closure, and a closure that reads an
    /// import names that import's module key, so the registry it is filed in must
    /// already hold the import; the cell keys live in their own space
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

    /// The registry this session's cells are filed in.
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

    /// How many artifacts the session dropped and has not managed to evict.
    ///
    /// # Invariant
    ///
    /// A number that only grows is the leak: the caller is not evicting, or is
    /// still holding a report that keeps a refusal in place.
    pub fn pending_evictions(&self) -> usize {
        self.unreachable.len()
    }

    /// Evict the artifacts of the dropped cells, reporting how many were freed.
    ///
    /// # Invariant
    ///
    /// A static ref survives in every [`SessionReport`] the caller still holds, so
    /// this is sound only once those are dropped; an artifact that a surviving
    /// artifact still references is refused ([`Eviction::StillReferenced`]) and
    /// retried later, so a cell that read another cell is not a dangle.
    pub fn evict_unreachable(&mut self) -> usize {
        let mut pending = std::mem::take(&mut self.unreachable);
        let mut registry = self
            .registry
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let mut freed = 0;
        // Repeated passes: a key still referenced may free once its referrer is gone.
        loop {
            let before = pending.len();
            let mut refused = Vec::new();
            for key in pending.drain(..) {
                match registry.evict(key) {
                    Eviction::Freed => freed += 1,
                    // Nothing filed under it: an earlier pass freed it, so not a debt.
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

    /// Replace the whole buffer with `source`, the file-diff entry point.
    ///
    /// # Invariant
    ///
    /// It only sets the buffer: the edit is still derived by the next compile
    /// diffing against the last state, and the cells are untouched, since
    /// identity is a path rather than a position.
    pub fn set_source(&mut self, source: impl Into<String>) {
        self.source = source.into();
        self.base = 0;
        self.line_starts.clear();
        self.imports.clear();
    }

    /// Point the session at a caller's view: code, base, line starts, imports.
    ///
    /// # Invariant
    ///
    /// A view whose mapping moved drops the incremental snapshot, because every
    /// token range and span in it is in the old coordinates; the cells are
    /// untouched, since identity is a path, not a position. See
    /// `docs/notes/incremental-update.md` §6.2.
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

    /// Compile and check the current buffer, reusing the cached build when the key holds.
    ///
    /// # Invariant
    ///
    /// Lexing resumes from the last snapshot over the edit span, so a keystroke is
    /// `O(edit)` in the regex work; the result is identical to a full re-lex.
    pub fn compile(&mut self) -> SessionReport<P> {
        // The line starts every span is measured against: the original file's.
        let line_starts = view_line_starts(&self.source, self.base, &self.line_starts);

        // Whether the previous snapshot is in the same coordinate space.
        let view_same = self.last.as_ref().is_some_and(|prev| {
            prev.base == self.base && (self.base == 0 || prev.line_starts == self.line_starts)
        });

        // The edit span, when a prior snapshot and a changed source allow it.
        let edit = self
            .last
            .as_ref()
            .filter(|prev| view_same && prev.source != self.source)
            .map(|prev| edit_span(&prev.source, &self.source));
        // The same edit in the original file's absolute coordinates.
        let absolute_edit =
            edit.map(|(a, b, delta)| (a + self.base as usize, b + self.base as usize, delta));
        // The line starts the snapshot's spans are in.
        let previous_starts = self
            .last
            .as_ref()
            .filter(|_| view_same)
            .map(|prev| view_line_starts(&prev.source, prev.base, &prev.line_starts));

        // Lex: resume from the snapshot over the edit span, else lex it all.
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

        // Parse the statement window the edit touched, else the whole buffer.
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

        // Resolve, then compute the resolved content key.
        let resolved = crate::resolve::resolve(&mut program, &self.imports);
        let key = crate::resolve::content_key(&program);
        diagnostics.extend(resolved.diagnostics.iter().cloned().map(|d| d.retype()));
        let tokens = Arc::new(tokens);
        let program = Arc::new(program);

        // Reuse: the resolved content is unchanged and the build is still right.
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

        // Rebuild: reconcile the cells with the program about to be lowered.
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
        // Lower the already-resolved program with the cells, then check.
        let (ir, span_index, compiled_cells) = crate::compile::compile_resolved_with_cells(
            &program,
            &resolved.import_binders,
            &resolved.prelude,
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
        // The build's span index, and the same positions as byte offsets.
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
                // Frozen: the store's cells minus the ones that were reused.
                frozen: self.cells.len().saturating_sub(reused_cells),
                dropped,
            },
        }
    }
}

/// The logical statement count of a program: its statements plus its tail.
fn logical_statements(program: &Program) -> usize {
    program.statements.len() + usize::from(program.expr.is_some())
}

/// The line starts a view's spans are measured against: the original file's.
///
/// # Invariant
///
/// A whole-file view (`base == 0`) derives them from the buffer, so the byte-edit
/// methods stay in step; a view with a base offset carries the file's, because a
/// suffix's own line starts are not the file's.
fn view_line_starts(code: &str, base: u32, line_starts: &[usize]) -> Vec<usize> {
    if base == 0 {
        lex::line_starts(code)
    } else {
        line_starts.to_vec()
    }
}

/// The minimal byte span `[a, b)` of `new` that differs from `old`, plus the delta.
///
/// # Invariant
///
/// `a` is a byte position valid in both sources (every byte before it is
/// identical) and `b` is the position in `new` where the unchanged tail begins,
/// which is the coordinate pair `lex::lex_resume` expects.
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

/// Parse the whole stream: the fallback when no window can be spliced.
fn full_parse(tokens: &[lex::Token]) -> (Program, Vec<ParseDiag>) {
    let parsed = parse::parse(tokens);
    (parsed.program, parsed.errors)
}

/// Re-parse the statement window an edit touched and splice it into `old_program`.
///
/// # Invariant
///
/// The result is exactly what a whole-buffer parse of `new_tokens` produces. Any
/// case that cannot be confirmed — a degenerate program, an error block outside
/// the window, a trailing binding, a negative token shift — returns `None` and
/// the caller re-parses the whole buffer.
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
    // The logical statement count: statements plus the tail when there is one.
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

    // First/last logical statements whose body overlaps the edit.
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
        // No body overlaps: the window is the two statements around the edit.
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

    // An error block outside the window would lose its diagnostic: fall back.
    for block in &old_program.error_blocks {
        let (b0, b1) = (block.range.0 as isize, block.range.1 as isize);
        if b1 > win_ob && b0 < win_eb {
            continue; // inside the window; re-derived by the region parse.
        }
        return None;
    }

    // The window's byte range in the new source: clamp the start, project the end.
    let suffix_ob = if hi < old_n {
        old_tokens[old_program.stmt_ranges[hi].0].range.0 as isize
    } else {
        // The window reaches the end, so its new end is the new source's end.
        old_tokens
            .last()
            .map_or(win_eb, |token| token.range.1 as isize)
    };
    let new_ob = win_ob.min(e_start);
    let new_eb = suffix_ob + delta;

    // New token range covering the window, from the first token the edit reaches.
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
    // An empty window is legal: the prefix and suffix meet with nothing between.
    let ne = ne.clamp(ns, eof);
    if ns > ne {
        return None;
    }

    // ... but it is not a parse: the region parser needs one statement at least.
    let (win_stmts, win_ranges, win_errors) = if ns == ne {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        crate::parse::parse_statement_region_traced(new_tokens, ns, ne)
    };

    // Spliced statements: unchanged prefix, re-parsed window, shifted suffix.
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
            // The tail expression, as a bare-expression block statement.
            debug_assert!(old_program.expr.is_some());
            BlockStmt {
                stmt: Stmt::Expr(old_program.expr.clone().unwrap()),
                public: false,
            }
        };
        // A clone's position moved: shift its spans by the edit's delta.
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

    // Pop the final logical statement as the program's value or tail.
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

/// The outcome of a window splice: the fresh frontend plus the window extent.
///
/// # Invariant
///
/// The extent is reported in both index spaces (`lo..old_hi` in the snapshot's
/// program, `lo..hi` in the new one), which differ whenever the edit added or
/// removed statements.
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
