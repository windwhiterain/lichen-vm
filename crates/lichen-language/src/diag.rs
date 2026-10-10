//! Diagnostics: a span plus a rendered message, per pipeline stage.
//!
//! See docs/notes/language-toolchain.md.

use std::sync::Arc;

use lichen_language_lex::Span;

/// Which stage of the pipeline produced a diagnostic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Preprocess,
    Lex,
    Parse,
    Resolve,
    Check,
    /// The filesystem, not the pipeline: carries no source span.
    Io,
}

/// A rendered diagnostic: a message plus the span it is grounded in, if any.
///
/// # Invariant
///
/// `None` renders without a caret; the message is always user-facing. `P`
/// appears only in `check`, so a frontend `Diag` is program-blind.
#[derive(Clone, Debug)]
pub struct Diag<P: lichen_lowlevel::Program> {
    pub span: Option<Span>,
    pub message: String,
    pub stage: Stage,
    /// The **package** `span` is a position in, when that is not the source
    /// being compiled.
    ///
    /// # Invariant
    ///
    /// `None` means the source this diagnostic was produced for, which is what
    /// every frontend and checker diagnostic means.
    pub file: Option<Arc<lichen_highlevel::program::PackageSource>>,
    /// The failing **use**'s position, present only alongside `file`.
    pub related: Option<Span>,
    /// The checker's structured facts — `None` for frontend errors, boxed to
    /// keep a diagnostic small.
    pub check: Option<Box<lichen_highlevel::diagnostic::Diag<P>>>,
}

impl<P: lichen_lowlevel::Program> Diag<P> {
    pub fn new(stage: Stage, span: Span, message: impl Into<String>) -> Self {
        Diag {
            span: Some(span),
            message: message.into(),
            stage,
            file: None,
            related: None,
            check: None,
        }
    }

    /// A diagnostic with no source position; rendered without a caret.
    pub fn unattributed(stage: Stage, message: impl Into<String>) -> Self {
        Diag {
            span: None,
            message: message.into(),
            stage,
            file: None,
            related: None,
            check: None,
        }
    }

    /// A filesystem failure at [`Stage::Io`] with no source span.
    pub fn io(message: impl Into<String>) -> Self {
        Self::unattributed(Stage::Io, message)
    }

    /// Widen a lexer diagnostic into the pipeline's [`Diag`] at `Stage::Lex`.
    pub fn from_lex(d: crate::LexDiag) -> Self {
        Diag {
            span: d.span,
            message: d.message,
            stage: Stage::Lex,
            file: None,
            related: None,
            check: None,
        }
    }

    /// Widen a parser diagnostic into the pipeline's [`Diag`] at `Stage::Parse`.
    pub fn from_parse(d: crate::ParseDiag) -> Self {
        Diag {
            span: d.span,
            message: d.message,
            stage: Stage::Parse,
            file: None,
            related: None,
            check: None,
        }
    }
}

impl<P: lichen_lowlevel::Program> Diag<P> {
    /// Widen a preprocessor diagnostic into the pipeline's [`Diag`].
    pub fn from_preprocess(d: lichen_preprocess::PreprocessDiag) -> Self {
        Diag {
            span: d.span,
            message: d.message,
            stage: Stage::Preprocess,
            file: None,
            related: None,
            check: None,
        }
    }

    /// Re-type a frontend `Diag` to any program marker's [`Diag`].
    ///
    /// # Invariant
    ///
    /// Only a checker-free diagnostic re-types: the `P` lives in the `check`
    /// slot, and a checker diagnostic's facts are program-specific.
    pub fn retype<Q: lichen_lowlevel::Program>(self) -> Diag<Q> {
        debug_assert!(
            self.check.is_none(),
            "only frontend diagnostics (check: None) can be re-typed"
        );
        Diag {
            span: self.span,
            message: self.message,
            stage: self.stage,
            file: self.file,
            related: self.related,
            check: None,
        }
    }
}
