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

pub mod ast;
pub mod compile;
pub mod diag;
pub mod lex;
pub mod package;
pub mod parse;
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
use lichen_highlevel::{no_native_ops, NativeOps};
use lichen_lowlevel::Registry;

pub use diag::{Diag, Stage};
use preprocess::ResolvedImport;
use program::{LangProgram, persp_attr_ext};

/// The result of compiling and checking a source program.
///
/// `build` is `Some` whenever the frontend resolved the program (lex, parse,
/// resolve) — including a *partially recovered* parse — and the checker ran
/// on it; it is `None` only when the resolve stage failed (an unresolved
/// name), so no IR exists to check.  `diagnostics` holds the frontend's
/// errors (which may be many — lex errors accumulate and parse errors are
/// recovered) and the checker's rendered failures.
pub struct Report {
    pub build: Option<Build<LangProgram>>,
    pub diagnostics: Vec<Diag>,
}

impl Report {
    /// No errors and the program checked: no diagnostics at all and the
    /// checker reported no unification failures.
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty() && self.build.as_ref().is_some_and(|b| b.ok)
    }
}

/// Compile and check a source program: the full pipeline.
pub fn compile(source: &str) -> Report {
    compile_with_imports(source, &[])
}

/// Compile and check a source program with resolved package imports.
pub fn compile_with_imports(source: &str, imports: &[ResolvedImport]) -> Report {
    compile_with_imports_in(source, imports, None)
}

/// [`compile_with_imports`] with an optional shared registry.  `None` uses a
/// fresh private registry; `Some` binds the importer module to the package
/// store's registry so `ExprKind::Static` refs resolve in place.
pub fn compile_with_imports_in(
    source: &str,
    imports: &[ResolvedImport],
    registry: Option<Arc<RwLock<Registry<LangProgram>>>>,
) -> Report {
    let line_starts = lex::line_starts(source);
    compile_with_imports_at(source, imports, registry, 0, &line_starts, no_native_ops())
}

/// Compile and check a program that is a slice of a larger source starting at
/// byte `base`, whose line starts are `line_starts`.  Token spans are absolute
/// positions in that larger source, so diagnostics point at the real file even
/// when `code` is only a suffix of it (the code after a stripped `@{...@}`
/// block).
pub fn compile_with_imports_at(
    code: &str,
    imports: &[ResolvedImport],
    registry: Option<Arc<RwLock<Registry<LangProgram>>>>,
    base: u32,
    line_starts: &[usize],
    native_ops: NativeOps<LangProgram>,
) -> Report {
    let Frontend {
        ir,
        diagnostics,
    } = frontend_at(code, base, line_starts, imports);
    build_report(ir, diagnostics, registry, native_ops)
}

/// The shared tail of the pipeline: run the checker on an [`IR`] (if the
/// frontend resolved one) and render the checker's diagnostics (only when the
/// build fails).  [`compile_with_imports_at`] and the incremental
/// [`BufferSession`] both end here — the session reuses this for its cached
/// rebuild path, so the rendering is centralized.
pub fn build_report(
    ir: Option<IR<program::Perspective>>,
    mut diagnostics: Vec<Diag>,
    registry: Option<Arc<RwLock<Registry<LangProgram>>>>,
    native_ops: NativeOps<LangProgram>,
) -> Report {
    let Some(ir) = ir else {
        return Report {
            build: None,
            diagnostics,
        };
    };
    let registry = registry.unwrap_or_else(|| Arc::new(RwLock::new(Registry::new())));
    let build = Checker::<LangProgram>::build_in_attr_native(
        ir,
        registry,
        persp_attr_ext(),
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
                    // IR (which still carries spans at this stage).
                    let span = d.loc().and_then(|loc| build.ir[loc.expr].span);
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
    }
}

/// The frontend only: text → IR (lex, parse, resolve).  The checker does not
/// run.  The frontend recovers from every frontend error — lex, parse, *and*
/// resolve: an unresolved name lowers to the same inert `ErrorBlock` the parse
/// layer uses, so `ir` is always `Some` and `diagnostics` carries every lex,
/// parse, and resolve error encountered.
pub struct Frontend {
    pub ir: Option<IR<program::Perspective>>,
    pub diagnostics: Vec<Diag>,
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
        errors: mut diagnostics,
    } = lex::lex_with(code, line_starts, base);
    let parse::Parsed {
        program,
        errors: parse_errors,
    } = parse::parse(&tokens);
    diagnostics.extend(parse_errors);
    // The lowering is total: an unresolved name lowers to the same inert
    // `ErrorBlock` the parse layer uses, so the frontend always produces an IR
    // and the resolve errors ride in `diagnostics`.
    let (ir, resolve_errors) = compile::compile_with_imports(&program, imports);
    diagnostics.extend(resolve_errors);
    Frontend {
        ir: Some(ir),
        diagnostics,
    }
}
