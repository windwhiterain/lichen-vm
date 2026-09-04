//! The incremental [`BufferSession`]: an editable source buffer whose compile
//! reuses the established AST→IR→check when only a frontend *error* changed.
//!
//! The frontend absorbs every error at its own layer — a recovered parse error
//! and an *unresolved name* both lower to the **same** inert [`ExprKind::ErrorBlock`]
//! (see [`crate::compile::Compiler`]), so the lowering is total and the checker
//! sees stable, name-free effective content.  The session tracks a
//! **name-resolved content signature**: a hash over the tree *shape* with every
//! name replaced by the binding it resolves to (a stable slot, not its spelling)
//! or a single **unresolved** sentinel.  So an edit that only extends an
//! unresolved name (the editor's typing case), rewrites an error block, or
//! consistently renames a binding, leaves the signature unchanged and reuses the
//! established [`IR`] + [`Build`] — only the fresh frontend/resolve diagnostics
//! are re-derived.  This is the `T1` tier's user-facing behaviour: typing a new
//! unfinished piece never re-derives the established program.
//!
//! The reuse is content-addressed (a `signature → build` cache), so it is sound
//! for an arbitrary edit that leaves the resolved structure alone — not just an
//! append.  A general edit that changes the resolved structure falls back to a
//! full re-lower + re-check.

use std::collections::HashMap;
use std::sync::Arc;

use lichen_highlevel::checker::Build;
use lichen_highlevel::native::no_native_ops;
use sha2::Digest;

use crate::ast::{BinOp, Expr, Program, Stmt, TypeConst};
use crate::diag::{Diag, Stage};
use crate::lex;
use crate::parse;
use crate::program::LangProgram;
use crate::{build_report, Report};

/// The result of a [`BufferSession::compile`]: the checked build (shared, so it
/// is cheap to hold) plus every diagnostic, and the content signature the
/// compile ran under.
#[derive(Clone)]
pub struct SessionReport {
    /// The checked build — `Some` whenever the frontend resolved the program
    /// (including a partially recovered parse) and the checker ran on it.
    /// Shared, so reusing the session never requires rebuilding it.
    pub build: Option<Arc<Build<LangProgram>>>,
    /// Lex + parse (always fresh) and the checker's rendered failures (from the
    /// reused or freshly built [`Build`]).
    pub diagnostics: Vec<Diag>,
    /// The beyond-error content signature this report was compiled under —
    /// equal across edits that only change error blocks.
    pub signature: u64,
    /// Whether the established build was reused because the content signature
    /// was unchanged (`true`) rather than freshly re-lowered and re-checked.
    pub reused: bool,
}

impl SessionReport {
    /// No errors and the program checked.
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty() && self.build.as_ref().is_some_and(|b| b.ok)
    }
}

/// An editable source buffer with a diff-gated compile.
pub struct BufferSession {
    source: String,
    cache: Option<Cache>,
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

struct Cache {
    /// The beyond-error content signature the cached build was compiled under.
    signature: u64,
    /// The cached build, shared with any report that reused it.
    build: Option<Arc<Build<LangProgram>>>,
    /// The checker's rendered diagnostics for that build — static across edits
    /// that keep the clean content (the fresh frontend diagnostics are
    /// re-derived per call).
    check_diagnostics: Vec<Diag>,
}

impl BufferSession {
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

    /// The beyond-error content signature of the last compile.
    pub fn signature(&self) -> u64 {
        self.cache.as_ref().map(|c| c.signature).unwrap_or(0)
    }

    /// Compile and check the current buffer.
    ///
    /// Lexes and parses the source (to re-derive the frontend diagnostics and
    /// the current resolved structure), computes the **name-resolved** content
    /// signature, and **reuses the cached build when it is unchanged** — so an
    /// edit that only extends an unresolved name (or an error block, or a
    /// consistent rename) never re-lowers or re-checks the established program.
    /// A changed signature re-lowers and re-checks, then refreshes the cache.
    ///
    /// When the previous compile left a [`LastState`] snapshot, lexing is
    /// **incremental** (`lex::lex_resume` over the edit span) rather than a
    /// whole-buffer re-lex, so a keystroke in a long buffer is `O(edit)` in the
    /// regex work.  The result is identical to a full re-lex (`lex_resume` is
    /// proven equal to [`lex::lex_with`] — see the lex tests); only the cost
    /// changes.
    pub fn compile(&mut self) -> SessionReport {
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
        let (tokens, mut diagnostics) = match (&self.last, edit) {
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

        // Parse: re-parse only the statement window the edit touched and splice
        // it into the snapshot's program when that is safe; otherwise parse the
        // whole buffer (the result is identical either way).
        let (program, errors) = match (&self.last, edit) {
            (Some(prev), Some((a, b, delta))) => splice_program(
                &prev.tokens,
                &prev.program,
                &tokens,
                a,
                b,
                delta,
            )
            .unwrap_or_else(|| full_parse(&tokens)),
            _ => full_parse(&tokens),
        };
        diagnostics.extend(errors);

        // Resolution-aware signature of the *name-resolved* structure, plus the
        // current resolve diagnostics — computed from the parsed AST, without
        // lowering.
        let (signature, resolve_errors) = program_signature(&program);
        diagnostics.extend(resolve_errors);

        // Reuse: the name-resolved structure is unchanged, so the established
        // build is exactly right.  Only the (fresh, above) frontend/resolve
        // diagnostics moved; the lowering and check are skipped entirely.
        if let Some(cache) = &self.cache
            && cache.signature == signature
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
                    signature,
                    reused: true,
                };
            }
        }

        // Rebuild: lower (total) and check.  The resolve diagnostics were
        // already produced by the signature pass (the frontend source of truth
        // for the session), so the lowering's own are discarded.
        let (ir, _) = crate::compile::compile_with_imports(&program, &[]);
        let report: Report = build_report(Some(ir), diagnostics, None, no_native_ops());
        let check_diagnostics: Vec<Diag> = report
            .diagnostics
            .iter()
            .filter(|d| d.stage == Stage::Check)
            .cloned()
            .collect();
        let build = report.build.map(Arc::new);
        self.cache = Some(Cache {
            signature,
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
            signature,
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
fn full_parse(tokens: &[lex::Token]) -> (Program, Vec<Diag>) {
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
) -> Option<(Program, Vec<Diag>)> {
    // Number of logical statements = `statements` + the final expression.
    let old_n = old_program.statements.len() + 1;
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
    let new_eb = if win_eb >= e_end { win_eb + delta } else { win_eb };

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
    let mut stmts: Vec<Stmt> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    stmts.extend(old_program.statements[..lo].iter().cloned());
    ranges.extend(old_program.stmt_ranges[..lo].iter().copied());
    stmts.extend(win_stmts.iter().cloned());
    ranges.extend(win_ranges.iter().copied());
    for i in hi..old_n {
        let stmt = if i < old_n - 1 {
            old_program.statements[i].clone()
        } else {
            Stmt::Expr(old_program.expr.clone())
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
    // whole-program parser: a trailing binding is itself an error, so the
    // caller falls back to a full parse (which reports it precisely).
    let last_stmt = stmts.pop()?;
    let last_range = ranges.pop()?;
    let (statements, expr) = match last_stmt {
        Stmt::Expr(e) => (stmts, e),
        Stmt::Binding(_) => return None,
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
    Some((program, win_errors))
}

/// The beyond-error content signature of a parsed program: a hash over its
/// **name-resolved** structure.
///
/// The checker/IR is name-free, so what determines the lowering is (a) the
/// tree *shape* and (b) each name's *resolution outcome* — the binding it
/// points to, or none.  This signature therefore signs the structure with
/// every `Name` replaced by the binding it resolves to (a stable slot, by
/// declaration order, *not* its spelling) or a single **unresolved** sentinel.
/// It is:
///
/// - **spelling-free**: a consistent rename (`f = 1; f` → `g = 1; g`) resolves
///   to the same slots and hashes identically, so the established build is
///   reused;
/// - **stable while a name is unresolved**: `v`, `ve`, `very_long…` — all in
///   flux but none yet bound — each resolve to the *unresolved* sentinel, so
///   an edit that only extends an unresolved name (the editor's typing case)
///   signs the same and reuses the build;
/// - **faithful**: an edit that actually changes which binding a use points to
///   (or adds/removes a binding) changes the slots and moves the signature.
fn program_signature(program: &Program) -> (u64, Vec<Diag>) {
    let mut sig = Sig::new();
    sig.hash_program(program);
    let signature = sig.combined();
    (signature, sig.diagnostics)
}

/// The signature walk, carrying the hasher + the name-resolution state.  The
/// scope handling mirrors [`crate::compile::Compiler`] exactly — a scope's
/// block-wide bindings are pre-entered before any value, restrictive `let`
/// bindings enter after their value (a fresh frame), a lambda enters its
/// parameter for the body, and blocks push/pop their own scope — so the
/// resolution this signature signs is the one the lowering actually uses.
///
/// It also emits the resolve-layer diagnostics for every unresolved name,
/// so the session reports the *current* name's error even when the build is
/// reused and the lowering is skipped.
///
/// The walk produces the signature as a **vector of per-statement resolved
/// hashes** (one per program statement, plus the final expression) so that the
/// session can re-derive *only* the statements a user edit touched and re-combine
/// the rest — the incremental information the high (AST) layer owns.  The
/// combined [`Sig::combined`] is an order-sensitive fold over those hashes.
struct Sig {
    /// The hasher for the statement currently being signed.
    cur: sha2::Sha256,
    /// Per-statement resolved hashes, in source order; the last is the
    /// program's final expression.
    stmt_hashes: Vec<u64>,
    scopes: Vec<HashMap<String, usize>>,
    next_slot: usize,
    diagnostics: Vec<Diag>,
}

impl Sig {
    fn new() -> Self {
        Sig {
            cur: sha2::Sha256::new(),
            stmt_hashes: Vec::new(),
            scopes: Vec::new(),
            next_slot: 0,
            diagnostics: Vec::new(),
        }
    }

    /// The order-sensitive combined signature over the per-statement hashes.
    fn combined(&self) -> u64 {
        let mut h = sha2::Sha256::new();
        for sh in &self.stmt_hashes {
            h.update(&sh.to_le_bytes());
        }
        u64::from_le_bytes(h.finalize().as_slice()[..8].try_into().unwrap())
    }

    fn slot(&mut self, name: &str) -> usize {
        let s = self.next_slot;
        self.next_slot += 1;
        self.scopes
            .last_mut()
            .expect("a scope frame is pushed before entering a binding")
            .insert(name.to_string(), s);
        s
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).copied())
    }

    /// Hash a *use* by its resolution outcome: the binding slot if it
    /// resolves, or a single spelling-free sentinel (plus a resolve diagnostic)
    /// if it does not.
    fn hash_name(&mut self, name: &str, span: &(u32, u32)) {
        match self.lookup(name) {
            Some(slot) => {
                self.cur.update(&[1]);
                self.cur.update(&slot.to_le_bytes());
            }
            None => {
                self.cur.update(&[0]);
                self.diagnostics.push(Diag::new(
                    Stage::Resolve,
                    *span,
                    format!("unresolved name '{name}'"),
                ));
            }
        }
    }

    /// A scope (a program, a `{ … }` block): pre-enter the block-wide
    /// binding names, then hash the statements and the value.  Each statement
    /// (and the final value) is signed into its own per-statement hash.
    fn hash_scope(&mut self, statements: &[Stmt], expr: Option<&Expr>) {
        let base = self.scopes.len();
        self.scopes.push(HashMap::new());
        for stmt in statements {
            if let Stmt::Binding(b) = stmt {
                if !b.restrictive {
                    self.slot(&b.name);
                }
            }
        }
        self.begin_stmt();
        for stmt in statements {
            self.hash_stmt(stmt);
            self.record_stmt();
        }
        if let Some(e) = expr {
            self.begin_stmt();
            self.cur.update(&[2]);
            self.hash_expr(e);
            self.record_stmt();
        }
        self.scopes.truncate(base);
    }

    /// Begin a new per-statement hash.
    fn begin_stmt(&mut self) {
        self.cur = sha2::Sha256::new();
    }

    /// Finalize the current statement's hash into the result list.
    fn record_stmt(&mut self) {
        let v = u64::from_le_bytes(
            self.cur
                .clone()
                .finalize()
                .as_slice()[..8]
                .try_into()
                .expect("sha256 is at least 8 bytes"),
        );
        self.stmt_hashes.push(v);
    }

    fn hash_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            // `let a = e` — the value is hashed before `a` is visible; a
            // fresh frame is pushed for the name (visible only to later
            // statements), mirroring the compiler.
            Stmt::Binding(b) if b.restrictive => {
                self.cur.update(&[0]);
                self.hash_expr(&b.value);
                self.scopes.push(HashMap::new());
                self.slot(&b.name);
            }
            // Block-wide binding — the name was pre-entered; only the value
            // (and its resolution) is signed.
            Stmt::Binding(b) => {
                self.cur.update(&[0]);
                self.hash_expr(&b.value);
            }
            Stmt::Expr(e) => {
                self.cur.update(&[1]);
                self.hash_expr(e);
            }
        }
    }

    fn hash_program(&mut self, program: &Program) {
        self.hash_scope(&program.statements, Some(&program.expr));
    }

    fn hash_opt_expr(&mut self, e: &Option<Box<Expr>>) {
        match e {
            Some(inner) => {
                self.cur.update(&[1]);
                self.hash_expr(inner);
            }
            None => self.cur.update(&[0]),
        }
    }

    /// Hash the structure of an AST expression, excluding source spans and
    /// the *content* of a recovered error block.  Names are signed by their
    /// resolution outcome; an [`Expr::Err`] and an unresolved name are both
    /// spelling-free sentinels.
    fn hash_expr(&mut self, e: &Expr) {
        match e {
            Expr::Int(n, _) => {
                self.cur.update(&[0]);
                self.cur.update(&n.to_le_bytes());
            }
            Expr::Str(s, _) => {
                self.cur.update(&[24]);
                self.cur.update(s.as_bytes());
            }
            Expr::TypeConst(c, _) => {
                self.cur.update(&[1]);
                self.cur.update(&[match c {
                    TypeConst::Int => 0,
                    TypeConst::Type => 1,
                    TypeConst::String => 2,
                }]);
            }
            Expr::Name(name, span) => {
                self.cur.update(&[2]);
                self.hash_name(name, span);
            }
            Expr::Placeholder(_) => self.cur.update(&[3]),
            // The recovered-error region / an unresolved name: content-free,
            // position-only.
            Expr::Err { .. } => self.cur.update(&[4]),
            Expr::Lambda {
                parameter,
                parameter_type,
                parameter_perspective,
                r#return,
                ..
            } => {
                self.cur.update(&[5]);
                let base = self.scopes.len();
                self.scopes.push(HashMap::new());
                self.slot(parameter);
                self.hash_opt_expr(parameter_type);
                self.cur.update(&[6]);
                self.hash_opt_expr(parameter_perspective);
                self.cur.update(&[7]);
                self.hash_expr(r#return);
                self.scopes.truncate(base);
            }
            Expr::Apply {
                function,
                argument,
                ..
            } => {
                self.cur.update(&[8]);
                self.hash_expr(function);
                self.hash_expr(argument);
            }
            Expr::BinOp {
                operator,
                left,
                right,
                ..
            } => {
                self.cur.update(&[9]);
                self.cur.update(&[match operator {
                    BinOp::Add => 0,
                    BinOp::Sub => 1,
                    BinOp::Leq => 2,
                    BinOp::Eq => 3,
                }]);
                self.hash_expr(left);
                self.hash_expr(right);
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.cur.update(&[10]);
                self.hash_expr(condition);
                self.hash_expr(then_branch);
                self.hash_expr(else_branch);
            }
            Expr::Assert { value, .. } => {
                self.cur.update(&[11]);
                self.hash_expr(value);
            }
            Expr::NativeCall { op, args, .. } => {
                self.cur.update(&[12]);
                self.cur.update(op.as_bytes());
                for a in args {
                    self.hash_expr(a);
                }
            }
            Expr::Index { array, index, .. } => {
                self.cur.update(&[13]);
                self.hash_expr(array);
                self.hash_expr(index);
            }
            Expr::FieldRead { container, key, .. } => {
                self.cur.update(&[14]);
                self.hash_expr(container);
                self.hash_expr(key);
            }
            Expr::TableFind { container, key, .. } => {
                self.cur.update(&[15]);
                self.hash_expr(container);
                self.hash_expr(key);
            }
            Expr::Annotation {
                value,
                r#type,
                perspective,
                ..
            } => {
                self.cur.update(&[16]);
                self.hash_expr(value);
                self.hash_opt_expr(r#type);
                self.hash_opt_expr(perspective);
            }
            Expr::Arrow {
                parameter,
                r#return,
                ..
            } => {
                self.cur.update(&[17]);
                self.hash_expr(parameter);
                self.hash_expr(r#return);
            }
            Expr::Tuple(elems, _)
            | Expr::TypeTuple(elems, _)
            | Expr::StructType(elems, _)
            | Expr::Array(elems, _) => {
                self.cur.update(&[18]);
                for el in elems {
                    self.hash_expr(el);
                }
            }
            Expr::StructInst { callee, fields, .. } => {
                self.cur.update(&[19]);
                self.hash_expr(callee);
                for f in fields {
                    self.hash_expr(f);
                }
            }
            Expr::Table(entries, _) => {
                self.cur.update(&[20]);
                for (k, v) in entries {
                    self.hash_expr(k);
                    self.hash_expr(v);
                }
            }
            Expr::Shallow(inner, depth, _) => {
                self.cur.update(&[21]);
                self.cur.update(&depth.to_le_bytes());
                self.hash_expr(inner);
            }
            Expr::TypeArray {
                element_type,
                length,
                ..
            } => {
                self.cur.update(&[22]);
                self.hash_expr(element_type);
                self.hash_expr(length);
            }
            Expr::Block { statements, expr, .. } => {
                self.cur.update(&[23]);
                self.hash_scope(statements, Some(expr));
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/session_tests.rs"]
mod tests;
