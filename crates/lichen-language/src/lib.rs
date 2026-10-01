//! The minimal source language: text → highlevel IR → checked program.
//!
//! The pipeline is [`frontend`] (lex → parse → resolve → an
//! [`lichen_highlevel::ir::IR`], the highlevel IR, with names pre-resolved to binder ids and
//! every expression carrying a source span) followed by
//! [`lichen_highlevel::checker::Checker::build`], which runs unchanged.
//! [`compile`] runs the whole pipeline and merges the frontend and checker
//! diagnostics; [`render`] prints them with source carets.
//!
//! The frontend does not stop at the first error: lex errors accumulate, the
//! parser *recovers* (a broken statement is skipped and reported, and the
//! partial program still compiles), and the checker runs on the partial
//! program — so one pass reports every problem it can find, and a bad input
//! never panics.  Even an *unresolved name* is absorbed at the resolve layer
//! (it lowers to the same inert [`ExprKind::ErrorBlock`] a parse error uses and
//! its diagnostic is reported), so the lowering is total and the checker runs
//! on the effective content.
//!
//! See `docs/language-spec.md` for the language spec.

// The lexer and parser live in their own crates; re-export them here so
// existing module paths (`lichen_language::lex::Token`, `lichen_language::ast::Expr`,
// `lichen_language::parse::parse`) resolve unchanged.
pub use lichen_language_lex as lex;
pub use lichen_language_lex::{LexDiag, Span};
pub use lichen_language_parser as parse;
pub use lichen_language_parser::ast;
pub use lichen_language_parser::path;
pub use lichen_language_parser::{ParseDiag, Parsed};

pub mod cells;
pub mod compile;
pub mod diag;
mod dirty;
pub mod package;
pub mod persist;
pub mod preprocess;
pub mod program;
pub mod render;
pub mod resolve;
pub mod run;
pub mod session;
pub mod suggest;

use std::sync::{Arc, PoisonError, RwLock};

use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::ir::{ExprId, IR};
use lichen_highlevel::program::{
    HighGlobalExt, HighProgram, HighProgramLiteral, TypeOperator, ValueType,
};
use lichen_highlevel::{NativeOps, no_native_ops};
use lichen_lowlevel::{Registry, StaticNodeId, is_unbound};

use crate::cells::CellStore;
use crate::path::Path;
use crate::persist::ProgramCodecOf;
pub use diag::{Diag, Stage};
use preprocess::ResolvedImport;
use program::{GcdOp, LangProgram, lang_attr_ext};

/// A program the language tooling drives: a [`ProgramCodecOf`] program that
/// additionally carries the language's own `LangAttr` attribute set, the
/// highlevel literal vocabulary, and the highlevel package-meta type — the
/// shape every [`lang_compose_vocabulary!`](crate::lang_compose_vocabulary)
/// program has.  The frontend/checker/build the store all speak in those
/// concrete types, so a single-`P` function needs these associated-type
/// equalities (plus `'static`, which `NativeOps`/`lang_attr_ext` demand) in
/// one bound.  A program implements this automatically when it satisfies the
/// equalities; only the composition site (the language's own program, a
/// plugin-built one) ever names it.
pub trait LangProgramShape:
    ProgramCodecOf
    + ::lichen_highlevel::program::HighProgram<
        Attr = program::LangAttr,
        Literal = ::lichen_highlevel::program::HighProgramLiteral,
    > + ::lichen_lowlevel::Program<PackageMeta = ::lichen_highlevel::program::HighPackageMeta>
    + 'static
{
}

impl<T> LangProgramShape for T
where
    T: ProgramCodecOf,
    T: ::lichen_highlevel::program::HighProgram<
            Attr = program::LangAttr,
            Literal = ::lichen_highlevel::program::HighProgramLiteral,
        >,
    T: ::lichen_lowlevel::Program<PackageMeta = ::lichen_highlevel::program::HighPackageMeta>,
    T: 'static,
{
}

/// The inner program shape a composed program wraps: the value and operator
/// vocabularies vary (`V`, `O`), while the compile-time attribute set is fixed
/// to the language's [`program::LangAttr`] and the literal / global-ext
/// defaults are the highlevel's own.  This is the `ProgramImpl` that the
/// [`lang_compose_vocabulary!`](crate::lang_compose_vocabulary) newtype
/// `LangProgram` wraps; downstream tooling is generic over `LangProgram`
/// (the single associated-type collector), not over this alias.
pub type CompiledProgram<V, O> = lichen_highlevel::program::ProgramImpl<
    V,
    O,
    program::LangAttr,
    HighProgramLiteral,
    HighGlobalExt,
>;

/// The version of the lichen library (`lichen-language`).  The package manager
/// keys its compiler cache by this — a change to the library means any
/// previously built compiler binary is stale and must be rebuilt.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The result of compiling and checking a source program.
///
/// `build` is `Some` on every path this crate produces: the frontend's
/// lowering is total, so even an unresolved name yields an IR and its error
/// rides in `diagnostics`.  `None` is reserved for a caller of [`build_report`]
/// that has no IR to check.  `diagnostics` holds the frontend's
/// errors (which may be many — lex errors accumulate and parse errors are
/// recovered) and the checker's rendered failures.
pub struct Report<P: HighProgram>
where
    P::Value: ValueType,
{
    pub build: Option<Build<P>>,
    pub diagnostics: Vec<Diag<P>>,
    /// The source span of each IR node, keyed by [ExprId] — the frontend's own
    /// position index (highlevel is span-free).  Present whenever `build` is
    /// `Some`; the caller maps a checker `Loc` back to a source caret through it.
    pub span_index: Option<compile::SpanIndex>,
}

impl<P: HighProgram> Report<P>
where
    P::Value: ValueType,
{
    /// No errors and the program checked: no diagnostics at all and the
    /// checker reported no unification failures.
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty() && self.build.as_ref().is_some_and(|b| b.ok)
    }
}

/// Compile and check a source program with a **cell store** (shipping
/// vocabulary): a `cache`d binding whose cell is clean is lowered to a read of its
/// frozen artifact — its body is not lowered, checked or evaluated — and a marked
/// binding that *is* compiled is frozen into `registry` and recorded under its
/// occurrence path.
///
/// `source_id` names the file the cells belong to; it is what
/// [`CellStore::invalidate`] drops, so the caller names the edit.  The registry is
/// the caller's because a cell's artifact must outlive the build that made it,
/// exactly as a package's artifact does for an import.
pub fn compile_with_cells(
    source_id: &str,
    source: &str,
    cells: &mut CellStore,
    registry: Arc<RwLock<Registry<LangProgram>>>,
) -> Report<LangProgram> {
    let line_starts = lex::line_starts(source);
    compile_with_imports_at_with_cells::<LangProgram>(
        source,
        &[],
        Some(registry),
        0,
        &line_starts,
        no_native_ops(),
        Some(cells),
        Vec::new(),
        source_id,
    )
}

/// Freeze the marked bindings a build compiled, and record each under its
/// occurrence path.
///
/// Only a **solved** pair is retained: a `Parameterized` one has no answer to
/// keep, so filing it would record an artifact that says nothing — the cell is
/// left out and the next build compiles the binding again, which is the honest
/// answer rather than a silent freeze of nothing.
///
/// And only from a **clean** build.  A cell is read back by skipping its body —
/// its lowering, its check and its evaluation — so a cell frozen from a build
/// that failed would carry that failure's silence: the error would be reported
/// once, and then disappear the moment the cell is read.  A failed build has no
/// answer to keep, so nothing is filed and every binding is compiled again.
fn freeze_cells<P>(
    build: &Build<P>,
    cells: &mut CellStore,
    registry: &Arc<RwLock<Registry<P>>>,
    compiled_cells: Vec<(Path, ExprId)>,
    source_id: &str,
) where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    if !build.ok {
        return;
    }
    let mut registry = registry.write().unwrap_or_else(PoisonError::into_inner);
    for (path, expr) in compiled_cells {
        let Some(pair) = build.state[expr.0 as usize].term else {
            continue;
        };
        if is_unbound(build.module.class_value(pair)) {
            continue;
        }
        let key = cells.allocate_key();
        // The hash is a placeholder on this path: a cell's reuse is decided by its
        // **path**, never by content (`docs/notes/incremental-update.md` §4.3).
        let freeze = registry.freeze_closure_mapped(&build.module, key, &[pair], [0; 32]);
        let index = freeze.node_map[&pair];
        cells.record(path, source_id, StaticNodeId { module: key, index });
    }
}

/// Compile and check a source program: the full pipeline (shipping vocabulary).
pub fn compile(source: &str) -> Report<LangProgram> {
    compile_with_imports(source, &[])
}

/// Compile and check a source program with resolved package imports
/// (shipping vocabulary).
pub fn compile_with_imports(source: &str, imports: &[ResolvedImport]) -> Report<LangProgram> {
    compile_with_imports_in(source, imports, None)
}

/// [`compile_with_imports`] with an optional shared registry (shipping
/// vocabulary).  `None` uses a fresh private registry; `Some` binds the
/// importer module to the package store's registry so `ExprKind::Static` refs
/// resolve in place.
pub fn compile_with_imports_in(
    source: &str,
    imports: &[ResolvedImport],
    registry: Option<Arc<RwLock<Registry<LangProgram>>>>,
) -> Report<LangProgram> {
    let line_starts = lex::line_starts(source);
    compile_with_imports_at::<LangProgram>(
        source,
        imports,
        registry,
        0,
        &line_starts,
        no_native_ops(),
    )
}

/// Compile and check a program over any program `P` (the associated-type
/// collector; the language's attribute set is fixed to [`program::LangAttr`]).
/// The program is a slice of a larger source starting at byte `base`, whose line
/// starts are `line_starts`.  Token spans are absolute positions in that
/// larger source, so diagnostics point at the real file even when `code` is
/// only a suffix of it (the code after a stripped `@{...@}` block).
pub fn compile_with_imports_at<P>(
    code: &str,
    imports: &[ResolvedImport],
    registry: Option<Arc<RwLock<Registry<P>>>>,
    base: u32,
    line_starts: &[usize],
    native_ops: NativeOps<P>,
) -> Report<P>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    compile_with_imports_at_with_cells(
        code,
        imports,
        registry,
        base,
        line_starts,
        native_ops,
        None,
        Vec::new(),
        "",
    )
}

/// [`compile_with_imports_at`] with a **cell store**: `cache`d bindings whose cell
/// is clean are lowered to a read of their frozen artifact instead of being
/// compiled, and the marked bindings that *were* compiled are frozen into
/// `registry` and recorded under their occurrence paths, owned by `source_id`.
pub fn compile_with_imports_at_with_cells<P>(
    code: &str,
    imports: &[ResolvedImport],
    registry: Option<Arc<RwLock<Registry<P>>>>,
    base: u32,
    line_starts: &[usize],
    native_ops: NativeOps<P>,
    cells: Option<&mut CellStore>,
    compiled_cells: Vec<(Path, ExprId)>,
    source_id: &str,
) -> Report<P>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let (frontend, compiled_cells) = frontend_at_with_cells(
        code,
        base,
        line_starts,
        imports,
        cells.as_ref().map(|cells| &**cells),
        compiled_cells,
    );
    let Frontend {
        ir,
        span_index,
        diagnostics,
    } = frontend;
    // The frontend diagnostics carry no checker build (they are program-blind),
    // so re-type them onto the caller's program marker before the report.
    let diagnostics: Vec<Diag<P>> = diagnostics.into_iter().map(|d| d.retype()).collect();
    build_report(
        ir,
        Some(span_index),
        diagnostics,
        registry,
        native_ops,
        cells,
        compiled_cells,
        source_id,
    )
}

/// The shared tail of the pipeline: run the checker on an [`IR`] (if the
/// frontend resolved one) and render the checker's diagnostics (only when the
/// build fails).  [`compile_with_imports_at`] and the incremental
/// [`BufferSession`] both end here — the session reuses this for its cached
/// rebuild path, so the rendering is centralized.
pub fn build_report<P>(
    ir: Option<IR<program::LangAttr>>,
    span_index: Option<compile::SpanIndex>,
    mut diagnostics: Vec<Diag<P>>,
    registry: Option<Arc<RwLock<Registry<P>>>>,
    native_ops: NativeOps<P>,
    cells: Option<&mut CellStore>,
    compiled_cells: Vec<(Path, ExprId)>,
    source_id: &str,
) -> Report<P>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let Some(ir) = ir else {
        return Report {
            build: None,
            diagnostics,
            span_index,
        };
    };
    let registry = registry.unwrap_or_else(|| Arc::new(RwLock::new(Registry::new())));
    let build =
        Checker::<P>::build_in_attr_native(ir, registry.clone(), lang_attr_ext::<P>(), native_ops);
    // The cells this build compiled: frozen now, because the build is solved (the
    // definition pass ran) and a frozen artifact must be complete.
    if let Some(cells) = cells {
        freeze_cells(&build, cells, &registry, compiled_cells, source_id);
    }
    // The pretty rendering is shared across the whole report: one type
    // printer, so a class keeps one `?a` name across diagnostics.  The
    // message carries no `?a` journey — the user inspects an expression's
    // type directly rather than reading a source trace.
    // Only render diagnostics when the build actually failed.  For a clean
    // build this is empty; skipping it also avoids descending into static
    // refs that a successful import may contain.
    if !build.ok {
        let mut printer =
            crate::render::TypePrinter::new_with_arrows(&build.module, Some(&build.arrows));
        // The diagnostic printer shows a struct's nominal id (`struct<…>#n`) so
        // two structs with the same field shape stay distinguishable in a
        // conflict; the value/type output printer leaves it off.
        printer.show_struct_ids();
        diagnostics.extend(
            build
                .diagnostics()
                .into_iter()
                .map(|d| {
                    // The highlevel is source-blind: a diagnostic carries a
                    // structured `Loc` (an IR expression + position), and the
                    // frontend maps that back to a source span through its own
                    // `span_index` (highlevel nodes carry none).
                    let span = d.loc().and_then(|loc| {
                        span_index
                            .as_ref()
                            .and_then(|s| s.get(loc.expr.0 as usize).copied().flatten())
                    });
                    Diag {
                        span,
                        message: crate::render::checker_message(&mut printer, &d),
                        stage: Stage::Check,
                        check: Some(Box::new(d)),
                    }
                })
                .collect::<Vec<_>>(),
        );
        // Refusals a layer above the lowlevel recorded on its general channel
        // while the check ran — a `$jit` whose body is outside the kernel-safe
        // subset, say.  The channel carries the layer's **own rendered text**,
        // because the lowlevel has no vocabulary for it, so this is the host
        // doing the rendering the channel's contract leaves to it
        // (`docs/notes/compiler-plugin.md`).  No span: the entry names a
        // lowlevel node, not an IR expression, so there is nothing to point at.
        diagnostics.extend(build.module.extension_diagnostics.iter().map(|entry| {
            Diag::unattributed(
                Stage::Check,
                format!("{}: {}", entry.category, entry.message),
            )
        }));
        // The invariant every consumer of a `Report` relies on: a failed build
        // carries at least one diagnostic.  `Build::diagnostics` skips a
        // recorded failure it cannot attribute to an expression — an assert
        // cloned out of an imported module has no entry in this build's node
        // tables — so without this the state `!ok && diagnostics.is_empty()`
        // reaches `Err(report.diagnostics)` as an error rendering *nothing at
        // all*.  Synthesise exactly one, here, so `run`, the package store and
        // the editor all inherit it instead of each inventing their own.
        if diagnostics.is_empty() {
            let unattributed = lichen_highlevel::diagnostic::Diag::unattributed_failure();
            diagnostics.push(Diag {
                span: None,
                message: crate::render::checker_message(&mut printer, &unattributed),
                stage: Stage::Check,
                check: Some(Box::new(unattributed)),
            });
        }
    }
    Report {
        build: Some(build),
        diagnostics,
        span_index,
    }
}

/// The frontend only: text → IR (lex, parse, resolve).  The checker does not
/// run.  The frontend recovers from every frontend error — lex, parse, *and*
/// resolve: an unresolved name lowers to the same inert `ErrorBlock` the parse
/// layer uses, so `ir` is always `Some` and `diagnostics` carries every lex,
/// parse, and resolve error encountered.
///
/// The frontend is concrete over [`LangProgram`]: its diagnostics carry no
/// checker build (so they are program-blind) and it feeds the shipping
/// compiler unchanged.
pub struct Frontend {
    pub ir: Option<IR<program::LangAttr>>,
    /// The `ExprId → span` index built during lowering (highlevel is span-free).
    pub span_index: compile::SpanIndex,
    pub diagnostics: Vec<Diag<LangProgram>>,
}

/// The frontend: text → IR.  See [`Frontend`].
pub fn frontend(source: &str) -> Frontend {
    let line_starts = lex::line_starts(source);
    frontend_at(source, 0, &line_starts, &[])
}

/// [`frontend`] with resolved imports seeded into the compiler's first frame.
pub fn frontend_with_imports(source: &str, imports: &[ResolvedImport]) -> Frontend {
    let line_starts = lex::line_starts(source);
    frontend_at(source, 0, &line_starts, imports)
}

/// The frontend over a slice of a larger source: `code` starts at byte `base`
/// in the source whose line starts are `line_starts`.  Token spans are
/// absolute positions in the larger source.
pub fn frontend_at(
    code: &str,
    base: u32,
    line_starts: &[usize],
    imports: &[ResolvedImport],
) -> Frontend {
    frontend_at_with_cells(code, base, line_starts, imports, None, Vec::new()).0
}

/// [`frontend_at`] with a cell store: a marked binding whose cell is clean lowers
/// to a static read of its frozen pair, and the marked bindings that *were*
/// compiled come back with their paths — together with whatever the caller had
/// already collected, so a pipeline can thread one list through several stages.
pub fn frontend_at_with_cells(
    code: &str,
    base: u32,
    line_starts: &[usize],
    imports: &[ResolvedImport],
    cells: Option<&CellStore>,
    mut compiled_cells: Vec<(Path, ExprId)>,
) -> (Frontend, Vec<(Path, ExprId)>) {
    let lex::Lexed {
        tokens,
        errors: lex_errors,
    } = lex::lex_with(code, line_starts, base);
    let mut diagnostics: Vec<Diag<LangProgram>> =
        lex_errors.into_iter().map(Diag::from_lex).collect();
    let parse::Parsed {
        mut program,
        errors: parse_errors,
    } = parse::parse(&tokens);
    diagnostics.extend(parse_errors.into_iter().map(Diag::from_parse));
    // The lowering is total: an unresolved name lowers to the same inert
    // `ErrorBlock` the parse layer uses, so the frontend always produces an IR
    // and the resolve errors ride in `diagnostics`.
    let (ir, span_index, resolve_errors, compiled) =
        compile::compile_with_imports_with_cells(&mut program, imports, cells);
    compiled_cells.extend(compiled);
    diagnostics.extend(resolve_errors);
    (
        Frontend {
            ir: Some(ir),
            span_index,
            diagnostics,
        },
        compiled_cells,
    )
}
