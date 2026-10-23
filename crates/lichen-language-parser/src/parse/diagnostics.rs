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
    // The labels are the backtracking record (`docs/notes/parse-clean-probe.md`).
    let message = match unclosed_delimiter(tokens, e) {
        Some(unclosed) => format!("{}: {message}", unclosed.clause),
        None => message,
    };
    ParseDiag {
        span: Some(span),
        message,
    }
}

/// A delimiter left open before the reported failure, named with the line it
/// was opened on.
pub(super) struct Unclosed {
    /// The clause that names it: `unclosed '(' opened on line 3`.
    pub clause: String,
    /// The line the opener stands on, for choosing the earliest one.
    pub line: u32,
}

/// The delimiter this error expected to close and did not find.
pub(super) fn unclosed_delimiter(
    tokens: &[Token],
    e: &Rich<'_, Token, SimpleSpan<usize>>,
) -> Option<Unclosed> {
    e.expected()
        .map(|label| label.to_string())
        .filter_map(|label| closer_expr(&label))
        .filter_map(|expr| {
            let (closer, opener, printed) = expr;
            unmatched_opener(tokens, e.span().start, opener, closer).map(|span| (span, printed))
        })
        .min_by_key(|(span, _)| span.0)
        .map(|(span, printed)| Unclosed {
            clause: format!("unclosed {printed} opened on line {}", span.0),
            line: span.0,
        })
}

/// The opener and closer a closer label belongs to, as the `(closer, opener,
/// printed opener)` a scan needs.
fn closer_expr(label: &str) -> Option<(TokenKind, TokenKind, &'static str)> {
    match label {
        "')'" => Some((TokenKind::RParen, TokenKind::LParen, "'('")),
        "'}'" => Some((TokenKind::RBrace, TokenKind::LBrace, "'{'")),
        "']'" => Some((TokenKind::RBracket, TokenKind::LBracket, "'['")),
        _ => None,
    }
}

/// The nearest opener of `opener` no `closer` in `tokens[..=at]` balances.
fn unmatched_opener(
    tokens: &[Token],
    at: usize,
    opener: TokenKind,
    closer: TokenKind,
) -> Option<Span> {
    let last = at.min(tokens.len().checked_sub(1)?);
    let mut unmatched = 0usize;
    for token in tokens[..=last].iter().rev() {
        if token.kind == closer {
            unmatched += 1;
        } else if token.kind == opener {
            if unmatched == 0 {
                return Some(token.span);
            }
            unmatched -= 1;
        }
    }
    None
}
