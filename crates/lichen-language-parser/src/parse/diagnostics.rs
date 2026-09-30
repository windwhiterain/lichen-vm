//! chumsky's parse errors translated into this crate's diagnostic.

use super::*;
use chumsky::error::RichReason;

/// Convert a chumsky error (token-index span, found token, expected labels)
/// into this crate's diagnostic.
pub(super) fn diag_from(tokens: &[Token], e: &Rich<'_, Token, SimpleSpan<usize>>) -> ParseDiag {
    let span = span_at(tokens, e.span().start);
    let message = match e.reason() {
        // A custom error (from the parser's own checks, e.g. a recovered
        // binding value) carries its message directly.
        RichReason::Custom(message) => message.to_string(),
        RichReason::ExpectedFound { .. } => {
            let found = e
                .found()
                .map(|t| t.kind.describe())
                .unwrap_or_else(|| "the end of the program".to_string());
            let expected: Vec<String> = e.expected().map(|p| p.to_string()).collect();
            match expected.as_slice() {
                [] => format!("unexpected {found}"),
                [one] => format!("expected {one}, found {found}"),
                [a, b] => format!("expected {a} or {b}, found {found}"),
                _ => format!(
                    "expected {}, or {}, found {found}",
                    expected[..expected.len() - 1].join(", "),
                    expected[expected.len() - 1],
                ),
            }
        }
    };
    ParseDiag {
        span: Some(span),
        message,
    }
}
