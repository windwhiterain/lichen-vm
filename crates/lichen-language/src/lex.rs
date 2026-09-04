//! The lexer: source text -> tokens, each with a (line, column) span.
//!
//! Whitespace (space/tab/cr) is trivia and never reaches the token stream.
//! There are no comments at all in the language -- prose lives in the
//! preprocessor's `@{...@}` block as metadata strings.  A newline, comma, or
//! semicolon all lex as the same Separator token -- the language treats them
//! uniformly as a boundary (statement or list-element separator), and the
//! quantity never matters.
//!
//! The only whitespace-significance the grammar needs is adjacency: an
//! expression followed immediately (no trivia) by '(' '{' '<' or '[' is a
//! postfix form (a slot read, a table lookup, an array type, or an index).
//! So the lexer emits a Glue token immediately before one of those four
//! delimiters when it is directly glued to the previous token.  The parser
//! reads Glue to decide postfix vs application -- no hidden space_before flag.
//!
//! There is no comment masking or re-scan: the lexer sees exactly the code it
//! is given.  [lex] handles a whole source (byte 0); [lex_with] handles a
//! slice of a larger source, mapping every token's span and range back to the
//! original file via a base offset and the source's line starts.
//!
//! Int, Type, struct, table, let, if, then, and else lex as keywords.  '->'
//! is the function-type arrow, '=>' a lambda, '::' a table key/value
//! separator, '!' a prefix assert.  '~' with adjacent digits folds into
//! Tilde(n).  Any other character is a lex error -- errors accumulate (the
//! character is skipped).

use logos::Logos;

use lichen_highlevel::ir::Span;

use crate::diag::{Diag, Stage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// An integer literal.
    Int(usize),
    /// An identifier, never a keyword.
    Name(String),
    /// The Int type constant.
    KwInt,
    /// The Type type constant -- the universe.
    KwType,
    /// The struct keyword -- a nominal struct type.
    KwStruct,
    /// The table keyword -- a constant table literal.
    KwTable,
    /// The let keyword -- a restrictive binding.
    KwLet,
    /// The if keyword -- a conditional expression.
    KwIf,
    /// The then keyword -- the if's then-branch delimiter.
    KwThen,
    /// The else keyword -- the if's else-branch delimiter.
    KwElse,
    /// '->' -- a function type.
    Arrow,
    /// '=>' -- a lambda.
    FatArrow,
    /// ':' -- an annotation.
    Colon,
    /// '::' -- the table literal's key/value separator.
    DoubleColon,
    /// '#' -- the perspective annotation.
    Hash,
    /// '!' -- a prefix assert: `!e` asserts `e`.
    Bang,
    /// '$' -- a native-operator call prefix: `$jit(f)`.  Reserved for a
    /// plugin's own embedded source; a normal file never lexes it as a valid
    /// call (the checker's private registry resolves it, or it is an error).
    Dollar,
    /// '=' -- a statement binding.
    Equals,
    /// '==' -- equality.
    Eq,
    /// '<=' -- less-or-equal.
    Leq,
    /// '+' -- addition.
    Plus,
    /// '-' -- subtraction.
    Minus,
    /// '('.
    LParen,
    /// ')'.
    RParen,
    /// '['.
    LBracket,
    /// ']'.
    RBracket,
    /// '{'.
    LBrace,
    /// '}'.
    RBrace,
    /// '<' -- exclusively type-level.
    LAngle,
    /// '>' -- closes '<'.
    RAngle,
    /// A '~' shallow marker.
    Tilde(usize),
    /// A newline, comma, or semicolon -- a uniform boundary token.
    Separator,
    /// A zero-width marker: the next '(' '{' '<' or '[' is directly glued to
    /// the previous token, so it is a postfix form.
    Glue,
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
            TokenKind::KwTable => "'table'".to_string(),
            TokenKind::KwLet => "'let'".to_string(),
            TokenKind::KwIf => "'if'".to_string(),
            TokenKind::KwThen => "'then'".to_string(),
            TokenKind::KwElse => "'else'".to_string(),
            TokenKind::Arrow => "'->'".to_string(),
            TokenKind::FatArrow => "'=>'".to_string(),
            TokenKind::Colon => "':'".to_string(),
            TokenKind::DoubleColon => "'::'".to_string(),
            TokenKind::Hash => "'#'".to_string(),
            TokenKind::Bang => "'!'".to_string(),
            TokenKind::Dollar => "'$'".to_string(),
            TokenKind::Equals => "'='".to_string(),
            TokenKind::Eq => "'=='".to_string(),
            TokenKind::Leq => "'<='".to_string(),
            TokenKind::Plus => "'+'".to_string(),
            TokenKind::Minus => "'-'".to_string(),
            TokenKind::LParen => "'('".to_string(),
            TokenKind::RParen => "')'".to_string(),
            TokenKind::LBracket => "'['".to_string(),
            TokenKind::RBracket => "']'".to_string(),
            TokenKind::LBrace => "'{'".to_string(),
            TokenKind::RBrace => "'}'".to_string(),
            TokenKind::LAngle => "'<'".to_string(),
            TokenKind::RAngle => "'>'".to_string(),
            TokenKind::Tilde(_) => "'~'".to_string(),
            TokenKind::Separator => "a separator".to_string(),
            TokenKind::Glue => "a glued delimiter".to_string(),
            TokenKind::Eof => "the end of the program".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// (line, column), 1-based -- the token's start.
    pub span: Span,
    /// The token's byte range in the source, half-open.
    pub range: (u32, u32),
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.kind.describe())
    }
}

/// The result of lexing: the tokens (always ending with Eof) plus any lex
/// errors.  Errors accumulate -- an unexpected character is skipped and
/// lexing continues.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<Diag>,
}

/// The token kinds logos recognizes.  Payloads are read from the matched
/// slice in the loop so integer overflow can be its own error.
#[derive(Logos, Clone, Debug, PartialEq, Eq)]
#[logos(skip r"[ \t\r]+")]
enum RawToken {
    #[regex(r"\n|,|;")]
    Separator,
    #[regex(r"[0-9]+")]
    IntLit,
    #[token("Int")]
    KwInt,
    #[token("Type")]
    KwType,
    #[token("struct")]
    KwStruct,
    #[token("table")]
    KwTable,
    #[token("let")]
    KwLet,
    #[token("if")]
    KwIf,
    #[token("then")]
    KwThen,
    #[token("else")]
    KwElse,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    NameLit,
    #[token("->")]
    Arrow,
    #[token("=>")]
    FatArrow,
    #[token("::")]
    DoubleColon,
    #[token(":")]
    Colon,
    #[token("#")]
    Hash,
    #[token("!")]
    Bang,
    #[token("$")]
    Dollar,
    #[token("=")]
    Equals,
    #[token("==")]
    Eq,
    #[token("<=")]
    Leq,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("<")]
    LAngle,
    #[token(">")]
    RAngle,
    #[regex(r"~[0-9]*")]
    TildeLit,
}

impl RawToken {
    /// Whether this kind is one of the postfix-capable delimiters that a Glue
    /// marker can precede.
    fn is_postfix_delim(&self) -> bool {
        matches!(
            &self,
            RawToken::LParen | RawToken::LBracket | RawToken::LBrace | RawToken::LAngle
        )
    }
}

pub fn lex(source: &str) -> Lexed {
    let line_starts = line_starts(source);
    lex_with(source, &line_starts, 0)
}

/// Lex `code`, a slice of a larger source (whose line starts are
/// `line_starts`) beginning at byte `base` within it.  Token ranges and
/// spans are absolute positions in the full source (`base + local`), so
/// diagnostics and LSP positions point at the real source even when `code`
/// is only a suffix of it (e.g. the code after a stripped `@{...@}`
/// preprocessor block).
pub fn lex_with(code: &str, line_starts: &[usize], base: u32) -> Lexed {
    let mut tokens: Vec<Token> = Vec::new();
    let mut errors: Vec<Diag> = Vec::new();
    let mut lexer = RawToken::lexer(code);
    // End of the last real token (not a Separator/Glue).  Used to decide
    // whether the next delimiter is glued to it.  Reset to None at a boundary
    // (a Separator or an error) so a following delimiter is a fresh atom.
    let mut prev_end: Option<u32> = None;
    while let Some(result) = lexer.next() {
        match result {
            Ok(raw) => {
                let span = lexer.span();
                let (start, end) = (base + span.start as u32, base + span.end as u32);
                let lc = line_col(line_starts, start);
                if raw == RawToken::Separator {
                    tokens.push(Token {
                        kind: TokenKind::Separator,
                        span: lc,
                        range: (start, end),
                    });
                    prev_end = None;
                    continue;
                }
                let glued = raw.is_postfix_delim() && prev_end == Some(start);
                if glued {
                    tokens.push(Token {
                        kind: TokenKind::Glue,
                        span: lc,
                        range: (start, start),
                    });
                }
                match raw_to_kind(&raw, lexer.slice(), lc, &mut errors) {
                    Some(kind) => {
                        tokens.push(Token { kind, span: lc, range: (start, end) });
                        prev_end = Some(end);
                    }
                    None => {
                        // integer overflow: the run was consumed, no token.
                        prev_end = Some(end);
                    }
                }
            }
            Err(..) => {
                let span = lexer.span();
                let start = base + span.start as u32;
                let lc = line_col(line_starts, start);
                let ch = lexer.slice().chars().next().unwrap_or('?');
                errors.push(Diag::new(Stage::Lex, lc, format!("unexpected character '{ch}'")));
                prev_end = None;
            }
        }
    }
    let pos = base + code.len() as u32;
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: line_col(line_starts, pos),
        range: (pos, pos),
    });
    Lexed { tokens, errors }
}



/// Map a raw token plus its matched slice to a TokenKind.  Integer literals
/// are parsed here: overflow records an error and returns None (no token).
fn raw_to_kind(
    raw: &RawToken,
    slice: &str,
    lc: Span,
    errors: &mut Vec<Diag>,
) -> Option<TokenKind> {
    match raw {
        RawToken::IntLit => {
            let mut value: usize = 0;
            for byte in slice.bytes() {
                let digit = (byte - b'0') as usize;
                match value.checked_mul(10).and_then(|v| v.checked_add(digit)) {
                    Some(v) => value = v,
                    None => {
                        errors.push(Diag::new(Stage::Lex, lc, "integer literal out of range"));
                        return None;
                    }
                }
            }
            Some(TokenKind::Int(value))
        }
        RawToken::NameLit => Some(match slice {
            "Int" => TokenKind::KwInt,
            "Type" => TokenKind::KwType,
            "struct" => TokenKind::KwStruct,
            "table" => TokenKind::KwTable,
            "let" => TokenKind::KwLet,
            "if" => TokenKind::KwIf,
            "then" => TokenKind::KwThen,
            "else" => TokenKind::KwElse,
            _ => TokenKind::Name(slice.to_string()),
        }),
        RawToken::TildeLit => {
            let digits = &slice[1..];
            let n = if digits.is_empty() {
                usize::MAX
            } else {
                let mut n: usize = 0;
                for byte in digits.bytes() {
                    n = n.saturating_mul(10).saturating_add((byte - b'0') as usize);
                }
                n
            };
            Some(TokenKind::Tilde(n))
        }
        RawToken::KwInt => Some(TokenKind::KwInt),
        RawToken::KwType => Some(TokenKind::KwType),
        RawToken::KwStruct => Some(TokenKind::KwStruct),
        RawToken::KwTable => Some(TokenKind::KwTable),
        RawToken::KwLet => Some(TokenKind::KwLet),
        RawToken::KwIf => Some(TokenKind::KwIf),
        RawToken::KwThen => Some(TokenKind::KwThen),
        RawToken::KwElse => Some(TokenKind::KwElse),
        RawToken::Arrow => Some(TokenKind::Arrow),
        RawToken::FatArrow => Some(TokenKind::FatArrow),
        RawToken::DoubleColon => Some(TokenKind::DoubleColon),
        RawToken::Colon => Some(TokenKind::Colon),
        RawToken::Hash => Some(TokenKind::Hash),
        RawToken::Bang => Some(TokenKind::Bang),
        RawToken::Dollar => Some(TokenKind::Dollar),
        RawToken::Equals => Some(TokenKind::Equals),
        RawToken::Eq => Some(TokenKind::Eq),
        RawToken::Leq => Some(TokenKind::Leq),
        RawToken::Plus => Some(TokenKind::Plus),
        RawToken::Minus => Some(TokenKind::Minus),
        RawToken::LParen => Some(TokenKind::LParen),
        RawToken::RParen => Some(TokenKind::RParen),
        RawToken::LBracket => Some(TokenKind::LBracket),
        RawToken::RBracket => Some(TokenKind::RBracket),
        RawToken::LBrace => Some(TokenKind::LBrace),
        RawToken::RBrace => Some(TokenKind::RBrace),
        RawToken::LAngle => Some(TokenKind::LAngle),
        RawToken::RAngle => Some(TokenKind::RAngle),
        RawToken::Separator => Some(TokenKind::Separator),
    }
}

/// Byte offsets at which each line starts (line 1 begins at 0).
pub fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Map a byte offset to its 1-based (line, column).
pub fn line_col(starts: &[usize], pos: u32) -> Span {
    let pos = pos as usize;
    let line = match starts.binary_search(&pos) {
        Ok(i) => i + 1,
        Err(i) => i,
    };
    let col = pos - starts[line - 1] + 1;
    (line as u32, col as u32)
}

#[cfg(test)]
#[path = "tests/lex_tests.rs"]
mod tests;
