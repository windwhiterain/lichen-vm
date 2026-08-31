//! The minimal source language: text → highlevel IR → checked program.
//!
//! The pipeline is [`frontend`] (lex → parse → resolve → an
//! [`ExprTable`], the highlevel IR, with names pre-resolved to binder ids and
//! every expression carrying a source span) followed by
//! [`lichen_highlevel::checker::Checker::build`], which runs unchanged.
//! [`compile`] runs the whole pipeline and merges the frontend and checker
//! diagnostics; [`render`] prints them with source carets.
//!
//! The frontend does not stop at the first error: lex errors accumulate, the
//! parser *recovers* (a broken statement is skipped and reported, and the
//! partial program still compiles), and the checker runs on the partial
//! program — so one pass reports every problem it can find, and a bad input
//! never panics.  Only an unresolved name (the resolve stage) still stops
//! the pipeline.
//!
//! See `docs/language.md` for the language spec.

pub mod ast;
pub mod compile;
pub mod diag;
pub mod lex;
pub mod package;
pub mod parse;
pub mod persist;
pub mod preprocess;
pub mod readme;
pub mod render;
pub mod run;

use std::sync::{Arc, RwLock};

use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::ir::IR;
use lichen_highlevel::program::{HighProgram, HighProgramValue};
use lichen_lowlevel::Registry;

pub use diag::{Diag, Stage};
use preprocess::ResolvedImport;

/// The result of compiling and checking a source program.
///
/// `build` is `Some` whenever the frontend resolved the program (lex, parse,
/// resolve) — including a *partially recovered* parse — and the checker ran
/// on it; it is `None` only when the resolve stage failed (an unresolved
/// name), so no IR exists to check.  `diagnostics` holds the frontend's
/// errors (which may be many — lex errors accumulate and parse errors are
/// recovered) and the checker's rendered failures.
pub struct Report {
    pub build: Option<Build>,
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
    registry: Option<Arc<RwLock<Registry<HighProgram<HighProgramValue>>>>>,
) -> Report {
    let Frontend {
        ir,
        mut diagnostics,
    } = frontend_with_imports(source, imports);
    let Some(ir) = ir else {
        return Report {
            build: None,
            diagnostics,
        };
    };
    let build = match registry {
        Some(registry) => Checker::build_in(ir, registry),
        None => Checker::build(ir),
    };
    // The pretty rendering is shared across the whole report: one type
    // printer, so a class keeps one `?a` name across diagnostics.  The
    // message carries no `?a` journey — the user inspects an expression's
    // type directly rather than reading a source trace.
    // Only render diagnostics when the build actually failed.  For a clean
    // build this is empty; skipping it also avoids descending into static
    // refs that a successful import may contain.
    if !build.ok {
        diagnostics.extend(
            build
                .diagnostics()
                .into_iter()
                .map(|d| Diag {
                    span: d.span,
                    message: d.message.clone(),
                    stage: Stage::Check,
                    check: Some(Box::new(d)),
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
/// run.  The frontend recovers: `ir` is `Some` unless the resolve stage
/// failed (an unresolved name); `diagnostics` holds every lex and parse
/// error encountered.
pub struct Frontend {
    pub ir: Option<IR>,
    pub diagnostics: Vec<Diag>,
}

/// The frontend: text → IR.  See [`Frontend`].
pub fn frontend(source: &str) -> Frontend {
    frontend_with_imports(source, &[])
}

/// [`frontend`] with resolved imports seeded into the compiler's first frame.
pub fn frontend_with_imports(source: &str, imports: &[ResolvedImport]) -> Frontend {
    let lex::Lexed {
        tokens,
        errors: mut diagnostics,
    } = lex::lex(source);
    let parse::Parsed {
        program,
        errors: parse_errors,
    } = parse::parse(&tokens);
    diagnostics.extend(parse_errors);
    match compile::compile_with_imports(&program, imports) {
        Ok(ir) => Frontend {
            ir: Some(ir),
            diagnostics,
        },
        Err(resolve) => {
            diagnostics.push(resolve);
            Frontend {
                ir: None,
                diagnostics,
            }
        }
    }
}
