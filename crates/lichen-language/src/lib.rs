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
pub use lichen_language_parser::{ParseDiag, Parsed};

pub mod cli;
pub mod compile;
pub mod diag;
pub mod package;
pub mod persist;
pub mod preprocess;
pub mod program;
pub mod readme;
pub mod render;
pub mod run;
pub mod session;

use std::sync::{Arc, RwLock};

use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::ir::IR;
use lichen_highlevel::program::{
    HighGlobalExt, HighProgram, HighProgramLiteral, TypeOperator, ValueType,
};
use lichen_highlevel::{NativeOps, no_native_ops};
use lichen_lowlevel::{LowOperator, OperatorExt, Registry};
use lichen_utils::extend::AsEnum;

pub use diag::{Diag, Stage};
use preprocess::ResolvedImport;
use program::{GcdOp, LangProgram, lang_attr_ext};

/// The concrete program marker the language's tooling drives: the value and
/// operator vocabularies vary (`V`, `O`), while the compile-time attribute set
/// is fixed to the language's [`program::LangAttr`] and the literal /
/// global-ext defaults are the highlevel's own.  A plugin-built compiler uses a
/// `ProgramImpl` of exactly this shape, so the store/run/CLI machinery is
/// generic over precisely these two vocabularies and the frontend / checker
/// already agree on `IR<LangAttr>`.
pub type CompiledProgram<V, O> = lichen_highlevel::program::ProgramImpl<
    V,
    O,
    program::LangAttr,
    HighProgramLiteral,
    HighGlobalExt,
>;

/// The version of the lichen library (`liche-language`).  The package manager
/// keys its compiler cache by this — a change to the library means any
/// previously built compiler binary is stale and must be rebuilt.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The result of compiling and checking a source program.
///
/// `build` is `Some` whenever the frontend resolved the program (lex, parse,
/// resolve) — including a *partially recovered* parse — and the checker ran
/// on it; it is `None` only when the resolve stage failed (an unresolved
/// name), so no IR exists to check.  `diagnostics` holds the frontend's
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
    compile_with_imports_at::<program::LangValue, program::LangOperator>(
        source,
        imports,
        registry,
        0,
        &line_starts,
        no_native_ops(),
    )
}

/// Compile and check a program over any value/operator vocabulary `V`/`O`
/// (the language's attribute set is fixed to [`program::LangAttr`]).  The
/// program is a slice of a larger source starting at byte `base`, whose line
/// starts are `line_starts`.  Token spans are absolute positions in that
/// larger source, so diagnostics point at the real file even when `code` is
/// only a suffix of it (the code after a stripped `@{...@}` block).
pub fn compile_with_imports_at<V, O>(
    code: &str,
    imports: &[ResolvedImport],
    registry: Option<Arc<RwLock<Registry<CompiledProgram<V, O>>>>>,
    base: u32,
    line_starts: &[usize],
    native_ops: NativeOps<CompiledProgram<V, O>>,
) -> Report<CompiledProgram<V, O>>
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
    let Frontend {
        ir,
        span_index,
        diagnostics,
    } = frontend_at(code, base, line_starts, imports);
    // The frontend diagnostics carry no checker build (they are program-blind),
    // so re-type them onto the caller's program marker before the report.
    let diagnostics: Vec<Diag<CompiledProgram<V, O>>> =
        diagnostics.into_iter().map(|d| d.retype()).collect();
    build_report(ir, Some(span_index), diagnostics, registry, native_ops)
}

/// The shared tail of the pipeline: run the checker on an [`IR`] (if the
/// frontend resolved one) and render the checker's diagnostics (only when the
/// build fails).  [`compile_with_imports_at`] and the incremental
/// [`BufferSession`] both end here — the session reuses this for its cached
/// rebuild path, so the rendering is centralized.
pub fn build_report<V, O>(
    ir: Option<IR<program::LangAttr>>,
    span_index: Option<compile::SpanIndex>,
    mut diagnostics: Vec<Diag<CompiledProgram<V, O>>>,
    registry: Option<Arc<RwLock<Registry<CompiledProgram<V, O>>>>>,
    native_ops: NativeOps<CompiledProgram<V, O>>,
) -> Report<CompiledProgram<V, O>>
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
    let Some(ir) = ir else {
        return Report {
            build: None,
            diagnostics,
            span_index,
        };
    };
    let registry = registry.unwrap_or_else(|| Arc::new(RwLock::new(Registry::new())));
    let build = Checker::<CompiledProgram<V, O>>::build_in_attr_native(
        ir,
        registry,
        lang_attr_ext::<CompiledProgram<V, O>>(),
        native_ops,
    );
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
    let lex::Lexed {
        tokens,
        errors: lex_errors,
    } = lex::lex_with(code, line_starts, base);
    let mut diagnostics: Vec<Diag<LangProgram>> =
        lex_errors.into_iter().map(Diag::from_lex).collect();
    let parse::Parsed {
        program,
        errors: parse_errors,
    } = parse::parse(&tokens);
    diagnostics.extend(parse_errors.into_iter().map(Diag::from_parse));
    // The lowering is total: an unresolved name lowers to the same inert
    // `ErrorBlock` the parse layer uses, so the frontend always produces an IR
    // and the resolve errors ride in `diagnostics`.
    let (ir, span_index, resolve_errors) = compile::compile_with_imports(&program, imports);
    diagnostics.extend(resolve_errors);
    Frontend {
        ir: Some(ir),
        span_index,
        diagnostics,
    }
}
