//! The lexer: source text → tokens, each with a `(line, column)` span.
//!
//! `Int` and `Type` lex as keywords; everything else that starts an
//! identifier is a name.  `<` `>` and `struct { }` build type-level forms
//! (the array type `Int<3>`, the tuple type `<Int, Type>`, the struct type
//! `struct { Int, Type }`); `[` `]` and `(` `)` stay value-level.  `;`
//! separates statements, `=` binds a name (`a = [1, 2]`); `=>` is still the
//! lambda.  `--` starts a line comment.  Any other character is a lex error
//! — the first one stops the pipeline.

use lichen_highlevel::ir::Span;

use crate::diag::{Diag, Stage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// An integer literal.
    Int(usize),
    /// An identifier, never `Int`/`Type` (those are keywords).
    Name(String),
    /// The `Int` type constant.
    KwInt,
    /// The `Type` type constant — the universe.
    KwType,
    /// The `struct` keyword — a nominal struct type.
    KwStruct,
    /// `->` — a function type.
    Arrow,
    /// `=>` — a lambda.
    FatArrow,
    /// `:` — an annotation.
    Colon,
    /// `=` — a statement binding.
    Equals,
    /// `;` — the statement separator.
    Semicolon,
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
    /// `<` — the array-type postfix after an expression, the tuple-type
    /// prefix at expression start.  Exclusively type-level.
    LAngle,
    /// `>` — closes `<`.
    RAngle,
    /// `{` — opens a struct type's field list.
    LBrace,
    /// `}` — closes `{`.
    RBrace,
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
            TokenKind::Arrow => "'->'".to_string(),
            TokenKind::FatArrow => "'=>'".to_string(),
            TokenKind::Colon => "':'".to_string(),
            TokenKind::Equals => "'='".to_string(),
            TokenKind::Semicolon => "';'".to_string(),
            TokenKind::Comma => "','".to_string(),
            TokenKind::LParen => "'('".to_string(),
            TokenKind::RParen => "')'".to_string(),
            TokenKind::LBracket => "'['".to_string(),
            TokenKind::RBracket => "']'".to_string(),
            TokenKind::LAngle => "'<'".to_string(),
            TokenKind::RAngle => "'>'".to_string(),
            TokenKind::LBrace => "'{'".to_string(),
            TokenKind::RBrace => "'}'".to_string(),
            TokenKind::Eof => "the end of the program".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    /// `(line, column)`, 1-based.
    pub span: Span,
}

pub fn lex(source: &str) -> Result<Vec<Token>, Diag> {
    let mut lexer = Lexer {
        source,
        bytes: source.as_bytes(),
        pos: 0,
        line: 1,
        col: 1,
        tokens: Vec::new(),
    };
    lexer.run()?;
    lexer.tokens.push(Token {
        kind: TokenKind::Eof,
        span: (lexer.line, lexer.col),
    });
    Ok(lexer.tokens)
}

struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
    tokens: Vec<Token>,
}

impl Lexer<'_> {
    fn run(&mut self) -> Result<(), Diag> {
        loop {
            self.skip_trivia();
            let (line, col) = (self.line, self.col);
            let Some(&b) = self.bytes.get(self.pos) else {
                return Ok(());
            };
            match b {
                b'0'..=b'9' => self.int_literal(line, col)?,
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.name_or_keyword(line, col),
                b'(' => self.push(line, col, 1, TokenKind::LParen),
                b')' => self.push(line, col, 1, TokenKind::RParen),
                b'[' => self.push(line, col, 1, TokenKind::LBracket),
                b']' => self.push(line, col, 1, TokenKind::RBracket),
                b'<' => self.push(line, col, 1, TokenKind::LAngle),
                b'>' => self.push(line, col, 1, TokenKind::RAngle),
                b'{' => self.push(line, col, 1, TokenKind::LBrace),
                b'}' => self.push(line, col, 1, TokenKind::RBrace),
                b',' => self.push(line, col, 1, TokenKind::Comma),
                b':' => self.push(line, col, 1, TokenKind::Colon),
                b';' => self.push(line, col, 1, TokenKind::Semicolon),
                b'-' if self.bytes.get(self.pos + 1) == Some(&b'>') => {
                    self.push(line, col, 2, TokenKind::Arrow)
                }
                b'=' if self.bytes.get(self.pos + 1) == Some(&b'>') => {
                    self.push(line, col, 2, TokenKind::FatArrow)
                }
                b'=' => self.push(line, col, 1, TokenKind::Equals),
                _ => {
                    let ch = self.source[self.pos..].chars().next().unwrap();
                    return Err(self.error(line, col, format!("unexpected character '{ch}'")));
                }
            }
        }
    }

    /// Whitespace and `--` line comments.
    fn skip_trivia(&mut self) {
        loop {
            match self.bytes.get(self.pos) {
                Some(b' ') | Some(b'\t') | Some(b'\r') => self.step(1),
                Some(b'\n') => {
                    self.step(1);
                    self.line += 1;
                    self.col = 1;
                }
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

    fn int_literal(&mut self, line: u32, col: u32) -> Result<(), Diag> {
        let mut value: usize = 0;
        while let Some(&b) = self.bytes.get(self.pos) {
            if !b.is_ascii_digit() {
                break;
            }
            let digit = (b - b'0') as usize;
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(digit))
                .ok_or_else(|| self.error(line, col, "integer literal out of range".to_string()))?;
            self.step(1);
        }
        self.tokens.push(Token {
            kind: TokenKind::Int(value),
            span: (line, col),
        });
        Ok(())
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
            _ => TokenKind::Name(text.to_string()),
        };
        self.tokens.push(Token {
            kind,
            span: (line, col),
        });
    }

    fn push(&mut self, line: u32, col: u32, len: usize, kind: TokenKind) {
        self.step(len);
        self.tokens.push(Token { kind, span: (line, col) });
    }

    fn step(&mut self, len: usize) {
        for _ in 0..len {
            self.pos += 1;
            self.col += 1;
        }
    }

    fn error(&self, line: u32, col: u32, message: String) -> Diag {
        Diag::new(Stage::Lex, (line, col), message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex(source).unwrap().into_iter().map(|t| t.kind).collect()
    }

    fn lex_one(source: &str) -> Token {
        let tokens = lex(source).unwrap();
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
        assert_eq!(
            kinds("5 -- a comment\n-- another\n 6"),
            vec![TokenKind::Int(5), TokenKind::Int(6), TokenKind::Eof]
        );
    }

    #[test]
    fn spans_track_line_and_column() {
        let token = lex_one("  x");
        assert_eq!(token.span, (1, 3));
        let token = lex_one("\n\n  y");
        assert_eq!(token.span, (3, 3));
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
    fn an_unexpected_character_is_a_lex_error() {
        let err = lex("x @ y").unwrap_err();
        assert_eq!(err.stage, Stage::Lex);
        assert_eq!(err.message, "unexpected character '@'");
        assert_eq!(err.span, Some((1, 3)));
    }

    #[test]
    fn an_overflowing_literal_is_a_lex_error() {
        let err = lex("99999999999999999999999999999999999999").unwrap_err();
        assert_eq!(err.message, "integer literal out of range");
    }
}
