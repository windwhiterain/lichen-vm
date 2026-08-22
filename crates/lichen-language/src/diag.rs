//! Diagnostics: a span plus a rendered message, per pipeline stage.
//!
//! The highlevel checker renders its own failures (the expected/found flow);
//! this crate's diagnostics are the frontend's (lex, parse, resolve) merged
//! with the checker's into one list by [`crate::compile`].

use lichen_highlevel::ir::Span;

/// Which stage of the pipeline produced a diagnostic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Lex,
    Parse,
    Resolve,
    Check,
}

/// A rendered diagnostic: a message plus the source position it is grounded
/// in.  Every frontend error carries a span — the highlevel bar, no panics,
/// no "internal" messages.
#[derive(Clone, Debug)]
pub struct Diag {
    pub span: Option<Span>,
    pub message: String,
    pub stage: Stage,
    /// The checker's structured facts — `None` for frontend errors.  Boxed
    /// so a diagnostic stays small (these are the `Err` payload of the
    /// frontend functions).  The rendered fields (`span`, `message`) mirror
    /// it for display; tests match on this instead of the message.
    pub check: Option<Box<lichen_highlevel::diagnostic::Diag>>,
}

impl Diag {
    pub fn new(stage: Stage, span: Span, message: impl Into<String>) -> Self {
        Diag {
            span: Some(span),
            message: message.into(),
            stage,
            check: None,
        }
    }
}
