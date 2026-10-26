//! An unclosed delimiter names itself and the line it was opened on.

use super::*;
use lichen_language_lex::lex;

/// Every diagnostic a source produces, in emission order.
fn diagnostics(source: &str) -> Vec<ParseDiag> {
    let tokens = lex(source).tokens;
    parse(&tokens).errors
}

/// The first diagnostic, asserting the source produced one.
fn first_diagnostic(source: &str) -> ParseDiag {
    let all = diagnostics(source);
    assert!(!all.is_empty(), "expected a diagnostic for {source:?}");
    all.into_iter().next().unwrap()
}

/// The unclosed clause a diagnostic carries, if any.
fn unclosed_clause(message: &str) -> Option<&str> {
    let head = message.split_once(':').map(|(h, _)| h).unwrap_or(message);
    head.starts_with("unclosed ").then_some(head)
}

#[test]
fn an_unclosed_paren_names_the_delimiter_and_the_line_it_was_opened_on() {
    let diag = first_diagnostic("x := (1 +\n2\n");
    assert_eq!(
        unclosed_clause(&diag.message),
        Some("unclosed '(' opened on line 1"),
        "the message must name the delimiter and its open line: {message}",
        message = diag.message,
    );
}

#[test]
fn an_unclosed_brace_names_the_delimiter_and_the_line_it_was_opened_on() {
    let diag = first_diagnostic("x := { a := 1;\n");
    assert_eq!(
        unclosed_clause(&diag.message),
        Some("unclosed '{' opened on line 1"),
        "the message must name the delimiter and its open line: {message}",
        message = diag.message,
    );
}

#[test]
fn an_unclosed_bracket_names_the_delimiter_and_the_line_it_was_opened_on() {
    let diag = first_diagnostic("x := [1, 0,\n");
    assert_eq!(
        unclosed_clause(&diag.message),
        Some("unclosed '[' opened on line 1"),
        "the message must name the delimiter and its open line: {message}",
        message = diag.message,
    );
}

#[test]
fn the_open_line_is_the_line_the_delimiter_stands_on_not_the_line_it_ends() {
    let diag = first_diagnostic("x := 1;\ny := (2 +\n3\n");
    assert_eq!(
        unclosed_clause(&diag.message),
        Some("unclosed '(' opened on line 2"),
        "the clause must name the line the delimiter was opened on: {message}",
        message = diag.message,
    );
}

#[test]
fn the_head_of_a_cascade_names_the_unclosed_delimiter_when_a_later_one_does() {
    // The `(1` on line 1 is never closed.
    let all = diagnostics("x = (1 +\n2;\ny = [3\n");
    let head = &all[0];
    assert!(
        unclosed_clause(&head.message).is_some(),
        "the head diagnostic must name the unclosed delimiter: {message}",
        message = head.message,
    );
}

#[test]
fn a_balanced_delimiter_never_reports_as_unclosed() {
    assert!(diagnostics("x := (1 + 2)").is_empty());
    assert!(diagnostics("x := [1, 2]").is_empty());
    assert!(diagnostics("x := { a := 1; }").is_empty());
    for diag in diagnostics("x := (1) + [2, 0] * { a := 3; }") {
        assert!(
            unclosed_clause(&diag.message).is_none(),
            "a balanced delimiter must not be called unclosed: {message}",
            message = diag.message,
        );
    }
}

#[test]
fn a_wrong_token_where_no_delimiter_is_missing_is_not_called_unclosed() {
    let diag = first_diagnostic("x := (1 + )");
    assert_eq!(
        unclosed_clause(&diag.message),
        None,
        "the delimiters balance, so this is not an unclosed-delimiter report: {message}",
        message = diag.message,
    );
}

#[test]
fn the_unclosed_clause_is_added_to_the_message_and_not_stolen_from_the_labels() {
    // Naming the delimiter prepends a clause; the label record must survive.
    let diag = first_diagnostic("x := (1 +\n2\n");
    assert!(
        diag.message.contains("expected"),
        "the label record must survive: {message}",
        message = diag.message,
    );
}
