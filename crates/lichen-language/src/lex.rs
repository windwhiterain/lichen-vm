//! The lexer: source text → tokens, each with a `(line, column)` span.
//!
//! `Int`, `Type`, `struct`, `let`, `if`, `then`, and `else` lex as
//! keywords; everything else that starts an identifier is a name.  `<` `>`
//! build type-level forms (the array type `Int<3>`, the tuple type
//! `<Int, Type>`, the struct type `struct<Int, Type>`); `[` `]` and `(`
//! `)` stay value-level; `{` `}` delimit a block — scoped bindings followed
//! by the block's value.  `;` separates statements, and so does a newline —
//! both lex as the same `Semicolon` token.  `=` binds a name (`a = [1, 2]`);
//! `=>` is still the lambda; `==` compares, `<=` compares, `+` and `-` add
//! and subtract (and `->` is the function-type arrow).  `--` starts a line
//! comment.  Any other character is a lex error — errors accumulate (the
//! bad character is skipped and lexing continues), so a single stray
//! character does not hide the rest of the program's errors.

use lichen_highlevel::ir::Span;

use crate::diag::{Diag, Stage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// An integer literal.
    Int(usize),
    /// An identifier, never `Int`/`Type`/`struct`/`let`/`if`/`then`/`else`
    /// (those are keywords).
    Name(String),
    /// The `Int` type constant.
    KwInt,
    /// The `Type` type constant — the universe.
    KwType,
    /// The `struct` keyword — a nominal struct type.
    KwStruct,
    /// The `let` keyword — a restrictive binding (`let a = e`): the name is
    /// visible only to later bindings, never to itself.
    KwLet,
    /// The `if` keyword — a conditional expression.
    KwIf,
    /// The `then` keyword — the `if`'s then-branch delimiter.
    KwThen,
    /// The `else` keyword — the `if`'s else-branch delimiter.
    KwElse,
    /// `->` — a function type.
    Arrow,
    /// `=>` — a lambda.
    FatArrow,
    /// `:` — an annotation.
    Colon,
    /// `=` — a statement binding.
    Equals,
    /// `==` — equality (yields `USize(0/1)`).
    Eq,
    /// `<=` — less-or-equal (yields `USize(0/1)`).
    Leq,
    /// `+` — addition.
    Plus,
    /// `-` — subtraction.
    Minus,
    /// `;` or a newline — the statement separator.
    Semicolon,
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
    /// `{` — opens a block.
    LBrace,
    /// `}` — closes a block.
    RBrace,
    /// `<` — the array-type postfix after an expression, the tuple-type
    /// and struct-type prefixes at expression start.  Exclusively type-level.
    LAngle,
    /// `>` — closes `<`.
    RAngle,
    /// A `~` shallow marker on an array position.  `~` with adjacent digits
    /// folds into `Tilde(n)` (`~2` marks two spine levels); a bare `~` (no
    /// digits, or a space after) is `Tilde(usize::MAX)` — the whole subtree.
    Tilde(usize),
    Eof,
}

impl TokenKind {
    /// The human-readable spelling used in parse-error messages.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Int(_) => "an integer literal".to_string(),
            TokenKind::Name(_) => "a name".to_string(),
            TokenKind::KwInt => "'Int'".to_string(),
            TokenKind::KwType => "'Type'".to_string(),
            TokenKind::KwStruct => "'struct'".to_string(),
            TokenKind::KwLet => "'let'".to_string(),
            TokenKind::KwIf => "'if'".to_string(),
            TokenKind::KwThen => "'then'".to_string(),
            TokenKind::KwElse => "'else'".to_string(),
            TokenKind::Arrow => "'->'".to_string(),
            TokenKind::FatArrow => "'=>'".to_string(),
            TokenKind::Colon => "':'".to_string(),
            TokenKind::Equals => "'='".to_string(),
            TokenKind::Eq => "'=='".to_string(),
            TokenKind::Leq => "'<='".to_string(),
            TokenKind::Plus => "'+'".to_string(),
            TokenKind::Minus => "'-'".to_string(),
            TokenKind::Semicolon => "';'".to_string(),
            TokenKind::Comma => "','".to_string(),
            TokenKind::LParen => "'('".to_string(),
            TokenKind::RParen => "')'".to_string(),
            TokenKind::LBracket => "'['".to_string(),
            TokenKind::RBracket => "']'".to_string(),
            TokenKind::LBrace => "'{'".to_string(),
            TokenKind::RBrace => "'}'".to_string(),
            TokenKind::LAngle => "'<'".to_string(),
            TokenKind::RAngle => "'>'".to_string(),
            TokenKind::Tilde(_) => "'~'".to_string(),
            TokenKind::Eof => "the end of the program".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// `(line, column)`, 1-based — the token's start.
    pub span: Span,
    /// The token's byte range in the source, half-open — the offset-based
    /// twin of `span`, for tooling (an LSP) that works in byte offsets.
    pub range: (u32, u32),
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.kind.describe())
    }
}

/// The result of lexing: the tokens (always ending with `Eof`) plus any
/// lex errors.  Errors accumulate — an unexpected character is skipped and
/// lexing continues, so a stray character does not hide the rest of the
/// program's errors.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<Diag>,
}

pub fn lex(source: &str) -> Lexed {
    let mut lexer = Lexer {
        source,
        bytes: source.as_bytes(),
        pos: 0,
        line: 1,
        col: 1,
        tokens: Vec::new(),
        errors: Vec::new(),
    };
    lexer.run();
    lexer.tokens.push(Token {
        kind: TokenKind::Eof,
        span: (lexer.line, lexer.col),
        range: (lexer.pos as u32, lexer.pos as u32),
    });
    Lexed {
        tokens: lexer.tokens,
        errors: lexer.errors,
    }
}

struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
    tokens: Vec<Token>,
    errors: Vec<Diag>,
}

impl Lexer<'_> {
    fn run(&mut self) {
        loop {
            self.skip_trivia();
            let (line, col) = (self.line, self.col);
            let Some(&b) = self.bytes.get(self.pos) else {
                return;
            };
            match b {
                b'0'..=b'9' => self.int_literal(line, col),
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.name_or_keyword(line, col),
                b'(' => self.push(line, col, 1, TokenKind::LParen),
                b')' => self.push(line, col, 1, TokenKind::RParen),
                b'[' => self.push(line, col, 1, TokenKind::LBracket),
                b']' => self.push(line, col, 1, TokenKind::RBracket),
                b'{' => self.push(line, col, 1, TokenKind::LBrace),
                b'}' => self.push(line, col, 1, TokenKind::RBrace),
                b'<' if self.bytes.get(self.pos + 1) == Some(&b'=') => {
                    self.push(line, col, 2, TokenKind::Leq)
                }
                b'<' => self.push(line, col, 1, TokenKind::LAngle),
                b'>' => self.push(line, col, 1, TokenKind::RAngle),
                b',' => self.push(line, col, 1, TokenKind::Comma),
                b':' => self.push(line, col, 1, TokenKind::Colon),
                b';' => self.push(line, col, 1, TokenKind::Semicolon),
                b'+' => self.push(line, col, 1, TokenKind::Plus),
                b'\n' => {
                    self.push(line, col, 1, TokenKind::Semicolon);
                    self.line += 1;
                    self.col = 1;
                }
                b'-' if self.bytes.get(self.pos + 1) == Some(&b'>') => {
                    self.push(line, col, 2, TokenKind::Arrow)
                }
                b'-' => self.push(line, col, 1, TokenKind::Minus),
                b'=' if self.bytes.get(self.pos + 1) == Some(&b'>') => {
                    self.push(line, col, 2, TokenKind::FatArrow)
                }
                b'=' if self.bytes.get(self.pos + 1) == Some(&b'=') => {
                    self.push(line, col, 2, TokenKind::Eq)
                }
                b'=' => self.push(line, col, 1, TokenKind::Equals),
                b'~' => self.tilde(line, col),
                _ => {
                    let ch = self.source[self.pos..].chars().next().unwrap();
                    self.errors.push(Diag::new(
                        Stage::Lex,
                        (line, col),
                        format!("unexpected character '{ch}'"),
                    ));
                    self.step(ch.len_utf8());
                }
            }
        }
    }

    /// Whitespace and `--` line comments.  A newline is *not* trivia: it
    /// lexes as a `Semicolon` (see [`Lexer::run`]).
    fn skip_trivia(&mut self) {
        loop {
            match self.bytes.get(self.pos) {
                Some(b' ') | Some(b'\t') | Some(b'\r') => self.step(1),
                Some(b'-') if self.bytes.get(self.pos + 1) == Some(&b'-') => {
                    while let Some(&b) = self.bytes.get(self.pos) {
                        if b == b'\n' {
                            break;
                        }
                        self.step(1);
                    }
                }
                _ => return,
            }
        }
    }

    fn int_literal(&mut self, line: u32, col: u32) {
        let start = self.pos;
        let mut value: usize = 0;
        while let Some(&b) = self.bytes.get(self.pos) {
            if !b.is_ascii_digit() {
                break;
            }
            let digit = (b - b'0') as usize;
            match value.checked_mul(10).and_then(|v| v.checked_add(digit)) {
                Some(v) => value = v,
                None => {
                    self.errors.push(Diag::new(
                        Stage::Lex,
                        (line, col),
                        "integer literal out of range".to_string(),
                    ));
                    // Skip the remaining digits so the overflowed number does
                    // not re-lex as separate literals; no token is emitted.
                    while self.bytes.get(self.pos).is_some_and(|b| b.is_ascii_digit()) {
                        self.step(1);
                    }
                    return;
                }
            }
            self.step(1);
        }
        self.tokens.push(Token {
            kind: TokenKind::Int(value),
            span: (line, col),
            range: (start as u32, self.pos as u32),
        });
    }

    fn name_or_keyword(&mut self, line: u32, col: u32) {
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.step(1);
            } else {
                break;
            }
        }
        let text = &self.source[start..self.pos];
        let kind = match text {
            "Int" => TokenKind::KwInt,
            "Type" => TokenKind::KwType,
            "struct" => TokenKind::KwStruct,
            "let" => TokenKind::KwLet,
            "if" => TokenKind::KwIf,
            "then" => TokenKind::KwThen,
            "else" => TokenKind::KwElse,
            _ => TokenKind::Name(text.to_string()),
        };
        self.tokens.push(Token {
            kind,
            span: (line, col),
            range: (start as u32, self.pos as u32),
        });
    }

    fn push(&mut self, line: u32, col: u32, len: usize, kind: TokenKind) {
        let start = self.pos;
        self.step(len);
        self.tokens.push(Token {
            kind,
            span: (line, col),
            range: (start as u32, self.pos as u32),
        });
    }

    /// A `~` shallow marker: `~` with adjacent digits folds into `Tilde(n)`
    /// (`~2` marks two spine levels); a bare `~` (a space or a non-digit
    /// follows) is `Tilde(usize::MAX)` — the whole subtree.
    fn tilde(&mut self, line: u32, col: u32) {
        let start = self.pos;
        self.step(1);
        let mut n: usize = 0;
        let mut digits = 0;
        while let Some(&b) = self.bytes.get(self.pos) {
            if !b.is_ascii_digit() {
                break;
            }
            digits += 1;
            n = n.saturating_mul(10).saturating_add((b - b'0') as usize);
            self.step(1);
        }
        let kind = if digits == 0 {
            TokenKind::Tilde(usize::MAX)
        } else {
            TokenKind::Tilde(n)
        };
        self.tokens.push(Token {
            kind,
            span: (line, col),
            range: (start as u32, self.pos as u32),
        });
    }

    fn step(&mut self, len: usize) {
        for _ in 0..len {
            self.pos += 1;
            self.col += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
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
    fn arrows_and_punctuation() {
        assert_eq!(
            kinds("(a, b) : A -> B <3>"),
            vec![
                TokenKind::LParen,
                TokenKind::Name("a".to_string()),
                TokenKind::Comma,
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
    fn comments_are_skipped() {
        // A comment is dropped, but the newline after it still lexes as the
        // statement separator.
        assert_eq!(
            kinds("5 -- a comment\n-- another\n 6"),
            vec![
                TokenKind::Int(5),
                TokenKind::Semicolon,
                TokenKind::Semicolon,
                TokenKind::Int(6),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn spans_track_line_and_column() {
        let token = lex_one("  x");
        assert_eq!(token.span, (1, 3));
        // Each newline is a Semicolon and advances the line.
        let tokens = lex("\n\n  y").tokens;
        let y = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Name("y".to_string()))
            .unwrap();
        assert_eq!(y.span, (3, 3));
    }

    #[test]
    fn a_newline_lexes_as_a_semicolon() {
        assert_eq!(
            kinds("a = 1\nb = 2"),
            vec![
                TokenKind::Name("a".to_string()),
                TokenKind::Equals,
                TokenKind::Int(1),
                TokenKind::Semicolon,
                TokenKind::Name("b".to_string()),
                TokenKind::Equals,
                TokenKind::Int(2),
                TokenKind::Eof,
            ]
        );
        // `;` plus a newline are two separators in a row.
        assert_eq!(
            kinds("a = 1;\nb = 2"),
            vec![
                TokenKind::Name("a".to_string()),
                TokenKind::Equals,
                TokenKind::Int(1),
                TokenKind::Semicolon,
                TokenKind::Semicolon,
                TokenKind::Name("b".to_string()),
                TokenKind::Equals,
                TokenKind::Int(2),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn operators_and_keywords_lex() {
        // A restrictive-`let` fibonacci binding — every new token in one line.
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
        // `==` and `<=` are two-char tokens, distinct from `=` and `<`.
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
        // `Int<3>` — the array type; `<Int, Type>` — the tuple type.  Both
        // are plain tokens; no adjacency rule exists.
        assert_eq!(
            kinds("Int<3> <Int, Type>"),
            vec![
                TokenKind::KwInt,
                TokenKind::LAngle,
                TokenKind::Int(3),
                TokenKind::RAngle,
                TokenKind::LAngle,
                TokenKind::KwInt,
                TokenKind::Comma,
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
                TokenKind::Semicolon,
                TokenKind::Name("a".to_string()),
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn an_unexpected_character_is_a_lex_error() {
        // The error is recorded and the character skipped — the rest of the
        // line still lexes.
        let Lexed { errors, .. } = lex("x @ y");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].stage, Stage::Lex);
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
        // `~` folds adjacent digits; a bare `~` is `Tilde(usize::MAX)`.
        assert_eq!(
            kinds("[1, ~ f (x + 1), ~2 y]"),
            vec![
                TokenKind::LBracket,
                TokenKind::Int(1),
                TokenKind::Comma,
                TokenKind::Tilde(usize::MAX),
                TokenKind::Name("f".to_string()),
                TokenKind::LParen,
                TokenKind::Name("x".to_string()),
                TokenKind::Plus,
                TokenKind::Int(1),
                TokenKind::RParen,
                TokenKind::Comma,
                TokenKind::Tilde(2),
                TokenKind::Name("y".to_string()),
                TokenKind::RBracket,
                TokenKind::Eof,
            ]
        );
        // `~0` is the unmarked no-op; a space after `~` separates the
        // marker from the element.
        assert_eq!(kinds("~0 x"), vec![TokenKind::Tilde(0), TokenKind::Name("x".to_string()), TokenKind::Eof]);
        assert_eq!(
            kinds("~ 1"),
            vec![TokenKind::Tilde(usize::MAX), TokenKind::Int(1), TokenKind::Eof]
        );
    }

    #[test]
    fn tokens_carry_their_byte_range() {
        let token = lex_one("  x");
        assert_eq!(token.span, (1, 3));
        assert_eq!(token.range, (2, 3));
    }
}
