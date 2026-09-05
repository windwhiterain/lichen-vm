//! The preprocessor's block-interior lexer.
//!
//! The `@{...@}` block at the top of a file holds a set of statements, one
//! per line (Separator-separated): `name = import "path"` binds an import,
//! and `name = "value"` defines a string metadata entry.  The block is cut
//! out of the source by a pure byte scan (see [super]) before this lexer
//! runs, so it only ever sees the block's interior -- no `@{` / `@}` and no
//! `@` at all (`@` is reserved for the block delimiters, so it cannot appear
//! here or inside a string).
//!
//! Like the main lexer, whitespace (space/tab/cr) is trivia and newline,
//! comma, and semicolon all lex as the same Separator token (uniform
//! boundary).  A string is `"…"` with no escape characters and may span
//! newlines; its content is any character except `"` or `@`.

use logos::Logos;

/// A block-interior token.
#[derive(Logos, Clone, Debug, PartialEq, Eq)]
#[logos(skip r"[ \t\r]+")]
pub enum TokenKind {
    /// A newline, comma, or semicolon -- a uniform boundary.
    #[regex(r"\n|,|;")]
    Separator,
    /// '=' -- separates a statement's name from its value.
    #[token("=")]
    Equals,
    /// The `import` keyword: `name = import "path"`.
    #[token("import")]
    KwImport,
    /// The `depend` keyword: `depend "url" [options...]` — declare a git
    /// dependency fetched from `url` (handled by the package manager).
    #[token("depend")]
    KwDepend,
    /// A string literal (quotes stripped); no escapes, may be multiline.
    /// `@` is reserved, so it is excluded from the content.
    #[regex(r#""[^"@]*""#, |lex| {
        let s = lex.slice();
        s[1..s.len() - 1].to_string()
    })]
    String(String),
    /// An identifier -- the name of a binding or a metadata entry.
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", |lex| lex.slice().to_string())]
    Name(String),
}

impl TokenKind {
    /// The human-readable spelling used in parse-error messages.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Separator => "a separator".to_string(),
            TokenKind::Equals => "'='".to_string(),
            TokenKind::KwImport => "'import'".to_string(),
            TokenKind::KwDepend => "'depend'".to_string(),
            TokenKind::String(_) => "a string".to_string(),
            TokenKind::Name(_) => "a name".to_string(),
        }
    }
}

/// A token plus the byte span it covers within the block interior.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// The half-open byte range in the interior [start, end).
    pub range: (u32, u32),
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.kind.describe())
    }
}

/// A lex error: a message plus the byte offset of the offending character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    pub message: String,
    /// Byte offset in the interior (0-based).
    pub offset: u32,
}

/// The result of tokenizing a block interior.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<LexError>,
}

/// Tokenize a block interior (the bytes between `@{` and `@}`, no
/// delimiters).  An unexpected character makes the block unusable, so the
/// first one is reported and lexing stops -- the caller blanks/ignores the
/// whole preprocessor block.
pub fn tokenize(interior: &str) -> Lexed {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut lexer = TokenKind::lexer(interior);
    while let Some(result) = lexer.next() {
        match result {
            Ok(kind) => {
                let span = lexer.span();
                tokens.push(Token {
                    kind,
                    range: (span.start as u32, span.end as u32),
                });
            }
            Err(..) => {
                let start = lexer.span().start as u32;
                let ch = lexer.slice().chars().next().unwrap_or('?');
                errors.push(LexError {
                    message: format!("unexpected character '{ch}'"),
                    offset: start,
                });
                break;
            }
        }
    }
    Lexed { tokens, errors }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(interior: &str) -> Vec<TokenKind> {
        tokenize(interior)
            .tokens
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn an_import_binding_lexes() {
        assert_eq!(
            kinds("math = import \"math.lichen\""),
            vec![
                TokenKind::Name("math".to_string()),
                TokenKind::Equals,
                TokenKind::KwImport,
                TokenKind::String("math.lichen".to_string()),
            ]
        );
    }

    #[test]
    fn a_depend_directive_lexes() {
        assert_eq!(
            kinds("depend \"https://example.com/foo.git\" as foo"),
            vec![
                TokenKind::KwDepend,
                TokenKind::String("https://example.com/foo.git".to_string()),
                TokenKind::Name("as".to_string()),
                TokenKind::Name("foo".to_string()),
            ]
        );
    }

    #[test]
    fn separates_on_newline_comma_and_semicolon() {
        use TokenKind as K;
        assert_eq!(
            kinds("a = \"1\"\nb = \"2\";"),
            vec![
                K::Name("a".into()),
                K::Equals,
                K::String("1".into()),
                K::Separator,
                K::Name("b".into()),
                K::Equals,
                K::String("2".into()),
                K::Separator,
            ]
        );
    }

    #[test]
    fn a_string_may_span_lines_and_hold_commas() {
        assert_eq!(
            kinds("doc = \"line one\nline two, with a comma\""),
            vec![
                TokenKind::Name("doc".to_string()),
                TokenKind::Equals,
                TokenKind::String("line one\nline two, with a comma".to_string()),
            ]
        );
    }

    #[test]
    fn an_at_sign_is_rejected_even_inside_a_string() {
        // `@` is excluded from string content, so a string containing it
        // cannot lex as a string -- logos errors instead of matching it.
        let lexed = tokenize("title = \"has @ inside\"");
        assert_eq!(lexed.tokens.len(), 2, "string token is not produced");
        assert!(lexed.errors.len() >= 1, "at least one error");
    }

    #[test]
    fn whitespace_is_trivia() {
        assert_eq!(
            kinds("  a  =  \"x\"  "),
            vec![
                TokenKind::Name("a".to_string()),
                TokenKind::Equals,
                TokenKind::String("x".to_string()),
            ]
        );
    }
}
