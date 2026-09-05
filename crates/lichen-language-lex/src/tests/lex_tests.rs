use super::*;

/// The significant tokens: Glue and Separator are layout/boundary markers the
/// parser consumes; the token-vocabulary tests focus on the real tokens.
fn kinds(source: &str) -> Vec<TokenKind> {
    lex(source)
        .tokens
        .into_iter()
        .map(|t| t.kind)
        .filter(|k| !matches!(k, TokenKind::Glue | TokenKind::Separator))
        .collect()
}

/// The full token stream, including Glue and Separator.
fn kinds_all(source: &str) -> Vec<TokenKind> {
    lex(source).tokens.into_iter().map(|t| t.kind).collect()
}

fn lex_one(source: &str) -> Token {
    let tokens = lex(source).tokens;
    assert_eq!(tokens.len(), 2, "one token plus Eof");
    tokens.into_iter().next().unwrap()
}

#[test]
fn tokens_of_a_small_program() {
    assert_eq!(
        kinds("x => 5 : Int"),
        vec![
            TokenKind::Name("x".to_string()),
            TokenKind::FatArrow,
            TokenKind::Int(5),
            TokenKind::Colon,
            TokenKind::KwInt,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn a_double_colon_is_one_table_separator() {
    assert_eq!(
        kinds("table{ 1 :: 2 }"),
        vec![
            TokenKind::KwTable,
            TokenKind::LBrace,
            TokenKind::Int(1),
            TokenKind::DoubleColon,
            TokenKind::Int(2),
            TokenKind::RBrace,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn arrows_and_punctuation() {
    assert_eq!(
        kinds("(a, b) : A -> B <3>"),
        vec![
            TokenKind::LParen,
            TokenKind::Name("a".to_string()),
            TokenKind::Name("b".to_string()),
            TokenKind::RParen,
            TokenKind::Colon,
            TokenKind::Name("A".to_string()),
            TokenKind::Arrow,
            TokenKind::Name("B".to_string()),
            TokenKind::LAngle,
            TokenKind::Int(3),
            TokenKind::RAngle,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn int_and_type_are_keywords_but_not_prefixes() {
    assert_eq!(
        kinds("Int Type Int2 int"),
        vec![
            TokenKind::KwInt,
            TokenKind::KwType,
            TokenKind::Name("Int2".to_string()),
            TokenKind::Name("int".to_string()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn spans_track_line_and_column() {
    let token = lex_one("  x");
    assert_eq!(token.span, (1, 3));
    let tokens = lex("\n\n  y").tokens;
    let y = tokens
        .iter()
        .find(|t| t.kind == TokenKind::Name("y".to_string()))
        .unwrap();
    assert_eq!(y.span, (3, 3));
    assert_eq!(y.range, (4, 5));
}

#[test]
fn a_newline_is_a_separator() {
    assert_eq!(
        kinds("a = 1\nb = 2"),
        vec![
            TokenKind::Name("a".to_string()),
            TokenKind::Equals,
            TokenKind::Int(1),
            TokenKind::Name("b".to_string()),
            TokenKind::Equals,
            TokenKind::Int(2),
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        kinds_all("a = 1;\nb = 2"),
        vec![
            TokenKind::Name("a".to_string()),
            TokenKind::Equals,
            TokenKind::Int(1),
            TokenKind::Separator,
            TokenKind::Separator,
            TokenKind::Name("b".to_string()),
            TokenKind::Equals,
            TokenKind::Int(2),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn operators_and_keywords_lex() {
    assert_eq!(
        kinds("let fib = n => if n <= 1 then n else fib (n - 1) + fib (n - 2)"),
        vec![
            TokenKind::KwLet,
            TokenKind::Name("fib".to_string()),
            TokenKind::Equals,
            TokenKind::Name("n".to_string()),
            TokenKind::FatArrow,
            TokenKind::KwIf,
            TokenKind::Name("n".to_string()),
            TokenKind::Leq,
            TokenKind::Int(1),
            TokenKind::KwThen,
            TokenKind::Name("n".to_string()),
            TokenKind::KwElse,
            TokenKind::Name("fib".to_string()),
            TokenKind::LParen,
            TokenKind::Name("n".to_string()),
            TokenKind::Minus,
            TokenKind::Int(1),
            TokenKind::RParen,
            TokenKind::Plus,
            TokenKind::Name("fib".to_string()),
            TokenKind::LParen,
            TokenKind::Name("n".to_string()),
            TokenKind::Minus,
            TokenKind::Int(2),
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        kinds("a == 1 a <= 2"),
        vec![
            TokenKind::Name("a".to_string()),
            TokenKind::Eq,
            TokenKind::Int(1),
            TokenKind::Name("a".to_string()),
            TokenKind::Leq,
            TokenKind::Int(2),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn angle_brackets_lex_as_their_own_tokens() {
    assert_eq!(
        kinds("Int<3> <Int, Type>"),
        vec![
            TokenKind::KwInt,
            TokenKind::LAngle,
            TokenKind::Int(3),
            TokenKind::RAngle,
            TokenKind::LAngle,
            TokenKind::KwInt,
            TokenKind::KwType,
            TokenKind::RAngle,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn braces_lex_as_their_own_tokens() {
    assert_eq!(
        kinds("{a = 1; a}"),
        vec![
            TokenKind::LBrace,
            TokenKind::Name("a".to_string()),
            TokenKind::Equals,
            TokenKind::Int(1),
            TokenKind::Name("a".to_string()),
            TokenKind::RBrace,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn an_unexpected_character_is_a_lex_error() {
    let Lexed { errors, .. } = lex("x @ y");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "unexpected character '@'");
    assert_eq!(errors[0].span, Some((1, 3)));
    assert_eq!(
        kinds("x @ y"),
        vec![
            TokenKind::Name("x".to_string()),
            TokenKind::Name("y".to_string()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn an_overflowing_literal_is_a_lex_error() {
    let errors = lex("99999999999999999999999999999999999999").errors;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "integer literal out of range");
}

#[test]
fn a_tilde_is_a_shallow_marker_token() {
    assert_eq!(
        kinds("[1, ~ f (x + 1), ~2 y]"),
        vec![
            TokenKind::LBracket,
            TokenKind::Int(1),
            TokenKind::Tilde(usize::MAX),
            TokenKind::Name("f".to_string()),
            TokenKind::LParen,
            TokenKind::Name("x".to_string()),
            TokenKind::Plus,
            TokenKind::Int(1),
            TokenKind::RParen,
            TokenKind::Tilde(2),
            TokenKind::Name("y".to_string()),
            TokenKind::RBracket,
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        kinds("~0 x"),
        vec![
            TokenKind::Tilde(0),
            TokenKind::Name("x".to_string()),
            TokenKind::Eof
        ]
    );
    assert_eq!(
        kinds("~ 1"),
        vec![
            TokenKind::Tilde(usize::MAX),
            TokenKind::Int(1),
            TokenKind::Eof
        ]
    );
}

#[test]
fn a_bang_is_a_prefix_assert_token() {
    assert_eq!(
        kinds("! (1 == 1)"),
        vec![
            TokenKind::Bang,
            TokenKind::LParen,
            TokenKind::Int(1),
            TokenKind::Eq,
            TokenKind::Int(1),
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn tokens_carry_their_byte_range() {
    let token = lex_one("  x");
    assert_eq!(token.span, (1, 3));
    assert_eq!(token.range, (2, 3));
}

#[test]
fn a_separator_covers_newline_comma_and_semicolon() {
    for src in ["a\nb", "a,b", "a;b"] {
        let k = kinds_all(src);
        assert_eq!(
            &k[..],
            &[
                TokenKind::Name("a".to_string()),
                TokenKind::Separator,
                TokenKind::Name("b".to_string()),
                TokenKind::Eof,
            ],
            "source: {src:?}"
        );
    }
}

#[test]
fn glue_marks_a_glued_delimiter_only() {
    assert_eq!(
        kinds_all("f(1)"),
        vec![
            TokenKind::Name("f".to_string()),
            TokenKind::Glue,
            TokenKind::LParen,
            TokenKind::Int(1),
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        kinds_all("f (1)"),
        vec![
            TokenKind::Name("f".to_string()),
            TokenKind::LParen,
            TokenKind::Int(1),
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        kinds_all("f\n(1)"),
        vec![
            TokenKind::Name("f".to_string()),
            TokenKind::Separator,
            TokenKind::LParen,
            TokenKind::Int(1),
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
}

// ---------------------------------------------------------------------------
// Incremental re-lex (`lex_resume`) — the output must be *identical* to a full
// re-lex (`lex`) of the new source, for every edit shape: token growth, split,
// merge, deletion, append, and a newline insertion that shifts the suffix.
// ---------------------------------------------------------------------------

/// Apply the edit `replace [a, b) of old with text`, re-lex both whole and
/// incrementally, and assert they agree (tokens and errors).
fn assert_incremental_eq(old: &str, a: usize, b: usize, text: &str) {
    let mut new = String::with_capacity(old.len() + text.len());
    new.push_str(&old[..a]);
    new.push_str(text);
    new.push_str(&old[b..]);

    let ls = line_starts(&new);
    let prev = lex(old).tokens;
    let expected = lex(&new);
    let got = lex_resume(&prev, old, &new, &ls, a, b);

    assert_eq!(
        got.tokens,
        expected.tokens,
        "incremental tokens differ for edit {:?} on {:?} -> {:?}",
        (a, b, text),
        old,
        new
    );
    assert_eq!(
        got.errors.len(),
        expected.errors.len(),
        "incremental error count differs for {:?} on {:?} -> {:?}",
        (a, b, text),
        old,
        new
    );
}

#[test]
fn incremental_resume_matches_full_relex() {
    let cases: &[(&str, usize, usize, &str)] = &[
        // Grow an identifier (`x` -> `xy`): the token absorbs the insert.
        ("x = 1", 1, 1, "y"),
        // Split an identifier (`ab` -> `a b`): one token becomes two.
        ("ab = 1", 1, 1, " "),
        // Merge two identifiers (`a b` -> `ab`): delete the space.
        ("a b = 1", 1, 2, ""),
        // Delete a character inside an identifier.
        ("xy = 1", 1, 2, ""),
        // Insert a separator (a newline) mid-file — shifts the suffix AND its
        // line numbers.
        ("a = 1\nb = 2\nc = 3", 6, 6, "\n"),
        // Delete a separator (merge two statements onto one line).
        ("a = 1\nb = 2", 5, 6, ""),
        // Append at the very end.
        ("a = 1\nf = x => a + x", 20, 20, "\nf 2"),
        // Insert an integer literal's digit.
        ("a = 12\nb = 3", 4, 4, "3"),
        // Insert a parenthesis so a `(` becomes glued to the previous token.
        ("f x", 1, 1, "("),
        // Go from glued to un-glued parenthesis.
        ("f(1)\ng", 1, 1, " "),
        // Edit whitespace that is trivia (no token change).
        ("a = 1\nb = 2", 4, 4, " "),
        // Delete a brace (an unbalanced region is one error block).
        ("{ a = 1 }", 0, 1, ""),
    ];
    for &(old, a, b, text) in cases {
        assert_incremental_eq(old, a, b, text);
    }
}

#[test]
fn incremental_resume_append_to_large_prefix() {
    let mut old = String::new();
    for i in 0..5_000 {
        old.push_str(&format!("x{i} = {i}\n"));
    }
    let a = old.len();
    let new = format!("{old}tail = x0 + x1\n");
    let ls = line_starts(&new);
    let prev = lex(&old).tokens;
    let expected = lex(&new);
    let got = lex_resume(&prev, &old, &new, &ls, a, old.len());
    assert_eq!(got.tokens, expected.tokens);
    assert_eq!(got.errors.len(), expected.errors.len());
}
