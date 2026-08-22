//! The minimal source language: text → highlevel IR → checked program.
//!
//! The pipeline is [`frontend`] (lex → parse → resolve → an
//! [`ExprTable`], the highlevel IR, with names pre-resolved to binder ids and
//! every expression carrying a source span) followed by
//! [`lichen_highlevel::checker::Checker::build`], which runs unchanged.
//! [`compile`] runs the whole pipeline and merges the frontend and checker
//! diagnostics; [`render`] prints them with source carets.
//!
//! See `docs/language.md` for the language spec.

pub mod ast;
pub mod compile;
pub mod diag;
pub mod lex;
pub mod parse;
pub mod readme;
pub mod render;
pub mod run;

use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::ir::IR;

pub use diag::{Diag, Stage};

/// The result of compiling and checking a source program.
///
/// `build` is `Some` iff the frontend succeeded (lex, parse, resolve); the
/// checker then ran on it.  `diagnostics` holds the first frontend error (if
/// any) or the checker's rendered failures (which may be many).
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
    match frontend(source) {
        Err(diagnostics) => Report {
            build: None,
            diagnostics,
        },
        Ok(ir) => {
            let build = Checker::build(ir);
            // The pretty rendering is shared across the whole report: the
            // raw node → span table (the flow lines' line numbers) and one
            // type printer, so a class keeps one `?a` name across
            // diagnostics.
            let node_spans = build.node_spans();
            let mut printer =
                crate::render::TypePrinter::new_with_arrows(&build.module, Some(&build.arrows));
            let diagnostics = build
                .diagnostics()
                .into_iter()
                .map(|d| Diag {
                    span: d.span,
                    message: crate::render::checker_message(&build, &node_spans, &mut printer, &d),
                    stage: Stage::Check,
                    check: Some(Box::new(d)),
                })
                .collect();
            Report {
                build: Some(build),
                diagnostics,
            }
        }
    }
}

/// The frontend only: text → IR (lex, parse, resolve).  The checker does not
/// run; on failure the first error is returned.
pub fn frontend(source: &str) -> Result<IR, Vec<Diag>> {
    let tokens = lex::lex(source).map_err(|d| vec![d])?;
    let ast = parse::parse(&tokens).map_err(|d| vec![d])?;
    let ir = compile::compile(&ast).map_err(|d| vec![d])?;
    Ok(ir)
}
