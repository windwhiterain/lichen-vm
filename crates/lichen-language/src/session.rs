//! The incremental [`BufferSession`]: an editable source buffer whose compile
//! reuses the established AST→IR→check when only a frontend *error* changed.
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
//! full re-lower + re-check.

use std::sync::Arc;

use lichen_highlevel::checker::Build;
use lichen_highlevel::native::no_native_ops;
use lichen_highlevel::program::{HighProgram, TypeOperator, ValueType};
use lichen_lowlevel::{LowOperator, OperatorExt};
use lichen_utils::extend::AsEnum;

use crate::ast::{BlockStmt, Program, Stmt};
use crate::diag::{Diag, Stage};
use crate::lex;
use crate::parse;
use crate::program::GcdOp;
use crate::{CompiledProgram, ParseDiag, Report, build_report};

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
pub struct BufferSession<V: ValueType, O: OperatorExt<CompiledProgram<V, O>>>
where
    O: AsEnum<LowOperator>
        + From<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq
        + From<GcdOp>
        + From<TypeOperator>
        + 'static,
    V: 'static,
{
    source: String,
    cache: Option<Cache<CompiledProgram<V, O>>>,
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

impl<V, O> BufferSession<V, O>
where
    V: ValueType + 'static,
    O: OperatorExt<CompiledProgram<V, O>>
        + AsEnum<LowOperator>
        + From<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq
        + From<GcdOp>
        + From<TypeOperator>
        + 'static,
{
    /// A new session over `source`.
    pub fn new(source: impl Into<String>) -> Self {
        BufferSession {
            source: source.into(),
            cache: None,
            last: None,
        }
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
    pub fn compile(&mut self) -> SessionReport<CompiledProgram<V, O>> {
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
        let mut diagnostics: Vec<Diag<CompiledProgram<V, O>>> =
            lex_errors.into_iter().map(Diag::from_lex).collect();

        // Parse: re-parse only the statement window the edit touched and splice
        // it into the snapshot's program when that is safe; otherwise parse the
        // whole buffer (the result is identical either way).
        let (mut program, errors, _window) = match (&self.last, edit) {
            (Some(prev), Some((a, b, delta))) => {
                match splice_program(&prev.tokens, &prev.program, &tokens, a, b, delta) {
                    Some(out) => {
                        let SpliceOut {
                            program,
                            errors,
                            lo,
                            hi,
                        } = out;
                        (program, errors, Some((lo, hi)))
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
        // moved; the lowering and check are skipped entirely.
        if let Some(cache) = &self.cache
            && cache.key == key
        {
            if let Some(build) = &cache.build {
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
                };
            }
        }

        // Rebuild: lower the already-resolved program (total) and check.  The
        // session ran the resolver itself, so it lowers via `compile_resolved`
        // rather than `compile_with_imports` (which would resolve again).
        let (ir, span_index) = crate::compile::compile_resolved(&program, &resolved.import_binders);
        let report: Report<CompiledProgram<V, O>> = build_report::<V, O>(
            Some(ir),
            Some(span_index),
            diagnostics,
            None,
            no_native_ops(),
        );
        let check_diagnostics: Vec<Diag<CompiledProgram<V, O>>> = report
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
        }
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
        if let Some((sb, eb)) = byte_range(i) {
            if eb > e_start && sb < e_end {
                lo = lo.min(i);
                hi = hi.max(i + 1);
            }
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

    // Project the window's byte range into the new source: positions at/before
    // the edit start are unchanged; positions at/after the edit end shift by
    // the length delta.
    let new_ob = win_ob;
    let new_eb = if win_eb >= e_end {
        win_eb + delta
    } else {
        win_eb
    };

    // New token index range [ns, ne) covering the window.
    let ns = new_tokens
        .iter()
        .position(|t| t.range.1 as isize > new_ob)
        .unwrap_or(new_tokens.len());
    let mut ne = new_tokens.len();
    for (k, t) in new_tokens.iter().enumerate() {
        if t.range.0 as isize >= new_eb {
            ne = k;
            break;
        }
    }
    let eof = new_tokens.len().saturating_sub(1); // the lexer's trailing Eof.
    if ns > eof {
        return None;
    }
    let ns = ns.min(eof);
    // When the window is the tail (it reaches the last statement), extend it to
    // the end of the buffer so an append that adds statements is fully parsed.
    let mut ne = ne.clamp(ns, eof);
    if hi == old_n {
        ne = eof;
    }
    if ns >= ne {
        return None;
    }

    let (win_stmts, win_ranges, win_errors) =
        crate::parse::parse_statement_region_traced(new_tokens, ns, ne);

    // Spliced logical statements: the unchanged prefix, the freshly re-parsed
    // window, then the unchanged suffix.  The suffix's *content* is untouched
    // but its token indices shift by `dk`, since the window's token count may
    // have changed (an insertion/removal before it).
    let dk = ne as isize - old_th as isize;
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
    let mut ranges = ranges;
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
        hi: lo + win_stmts.len(),
    })
}

/// The outcome of a window splice: the fresh frontend plus the window extent
/// (in the new program's logical-statement index space).
struct SpliceOut {
    program: Program,
    errors: Vec<ParseDiag>,
    lo: usize,
    hi: usize,
}

#[cfg(test)]
#[path = "tests/session_tests.rs"]
mod tests;
