//! Diagnostics: a span plus a rendered message, per pipeline stage.
//!
//! This crate's diagnostics are the frontend's (lex, parse, resolve) merged
//! with the checker's into one list by [`crate::compile`].  The checker's
//! messages are re-rendered pretty — the same type printer as the CLI output
//! ([`crate::render::checker_message`]); the boxed highlevel `Diag` in
//! `check` stays raw for tests and tooling.

use lichen_language_lex::Span;

/// Which stage of the pipeline produced a diagnostic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Preprocess,
    Lex,
    Parse,
    Resolve,
    Check,
}

/// A rendered diagnostic: a message plus the source position it is grounded
/// in.  Every frontend error carries a span — the highlevel bar, no panics,
/// no "internal" messages.
///
/// `P` is the program marker the diagnostic belongs to.  It only appears in
/// the `check` slot (the checker's structured facts, `None` for a frontend
/// error), so a frontend `Diag` is program-blind and can be re-typed with
/// [`Diag::retype`].
#[derive(Clone, Debug)]
pub struct Diag<P: lichen_lowlevel::Program> {
    pub span: Option<Span>,
    pub message: String,
    pub stage: Stage,
    /// The checker's structured facts — `None` for frontend errors.  Boxed
    /// so a diagnostic stays small (these are the `Err` payload of the
    /// frontend functions).  `message` is the pretty rendering for display;
    /// tests and tooling match on this instead of the message.
    pub check: Option<Box<lichen_highlevel::diagnostic::Diag<P>>>,
}

impl<P: lichen_lowlevel::Program> Diag<P> {
    pub fn new(stage: Stage, span: Span, message: impl Into<String>) -> Self {
        Diag {
            span: Some(span),
            message: message.into(),
            stage,
            check: None,
        }
    }

    /// Widen a lexer diagnostic into the pipeline's [`Diag`] at `Stage::Lex`.
    pub fn from_lex(d: crate::LexDiag) -> Self {
        Diag {
            span: d.span,
            message: d.message,
            stage: Stage::Lex,
            check: None,
        }
    }

    /// Widen a parser diagnostic into the pipeline's [`Diag`] at `Stage::Parse`.
    pub fn from_parse(d: crate::ParseDiag) -> Self {
        Diag {
            span: d.span,
            message: d.message,
            stage: Stage::Parse,
            check: None,
        }
    }
}

impl<P: lichen_lowlevel::Program> Diag<P> {
    /// Widen a preprocessor diagnostic into the pipeline's [`Diag`] at
    /// `Stage::Preprocess`.  A preprocess diagnostic is program-blind
    /// (`check: None`), so it re-types to any program marker.
    pub fn from_preprocess(d: lichen_preprocess::PreprocessDiag) -> Self {
        Diag {
            span: d.span,
            message: d.message,
            stage: Stage::Preprocess,
            check: None,
        }
    }

    /// Re-type a *frontend* diagnostic (which carries no checker facts) to any
    /// program marker's [`Diag`].  A frontend `Diag` has `check: None` — the
    /// `P` is only in the `check` slot, so a checker-free diagnostic is
    /// program-blind and re-types freely.  A checker diagnostic must not be
    /// re-typed (its facts are program-specific).
    pub fn retype<Q: lichen_lowlevel::Program>(self) -> Diag<Q> {
        debug_assert!(
            self.check.is_none(),
            "only frontend diagnostics (check: None) can be re-typed"
        );
        Diag {
            span: self.span,
            message: self.message,
            stage: self.stage,
            check: None,
        }
    }
}
