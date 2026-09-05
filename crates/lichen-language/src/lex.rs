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
    /// A string literal — the immutable builtin `string` value's content.
    Str(String),
    /// An identifier, never a keyword.
    Name(String),
    /// The Int type constant.
    KwInt,
    /// The string type constant.
    KwString,
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
    /// The return keyword -- a block's explicit tail expression marker.
    KwReturn,
    /// The pub keyword -- a block statement marked as a struct field.
    KwPub,
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
    /// '?' -- the label (doc) annotation: `e ? expr`.
    Question,
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
    /// '.' -- a named field read `a.b`.
    Dot,
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
            TokenKind::Str(_) => "a string literal".to_string(),
            TokenKind::Name(_) => "a name".to_string(),
            TokenKind::KwInt => "'Int'".to_string(),
            TokenKind::KwString => "'string'".to_string(),
            TokenKind::KwType => "'Type'".to_string(),
            TokenKind::KwStruct => "'struct'".to_string(),
            TokenKind::KwTable => "'table'".to_string(),
            TokenKind::KwLet => "'let'".to_string(),
            TokenKind::KwIf => "'if'".to_string(),
            TokenKind::KwThen => "'then'".to_string(),
            TokenKind::KwElse => "'else'".to_string(),
            TokenKind::KwReturn => "'return'".to_string(),
            TokenKind::KwPub => "'pub'".to_string(),
            TokenKind::Arrow => "'->'".to_string(),
            TokenKind::FatArrow => "'=>'".to_string(),
            TokenKind::Colon => "':'".to_string(),
            TokenKind::DoubleColon => "'::'".to_string(),
            TokenKind::Hash => "'#'".to_string(),
            TokenKind::Question => "'?'".to_string(),
            TokenKind::Bang => "'!'".to_string(),
            TokenKind::Dollar => "'$'".to_string(),
            TokenKind::Equals => "'='".to_string(),
            TokenKind::Eq => "'=='".to_string(),
            TokenKind::Leq => "'<='".to_string(),
            TokenKind::Plus => "'+'".to_string(),
            TokenKind::Minus => "'-'".to_string(),
            TokenKind::Dot => "'.'".to_string(),
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
    /// A `"..."` string literal (no escapes; may span newlines).  The trailing
    /// quote is optional so an unterminated string lexes as one unit and is
    /// diagnosed as a whole, rather than as a run of single-char errors.
    #[regex("\"[^\"]*\"?")]
    StrLit,
    #[token("Int")]
    KwInt,
    #[token("string")]
    KwString,
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
    #[token("return")]
    KwReturn,
    #[token("pub")]
    KwPub,
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
    #[token("?")]
    Question,
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
    #[token(".")]
    Dot,
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

/// Re-lex a source **incrementally**, given the previous full token stream and
/// a single edit.
///
/// The edit replaces the bytes `[a, b)` **of the old source** (a pure insertion
/// has `a == b`; a pure deletion removes `[a, b)` and inserts nothing) with new
/// text, turning `old_source` into `new_source` (so positions `>= a` shift by
/// `delta = new_source.len() - old_source.len()`).  `prev` is the full token
/// stream of `old_source` (including its `Eof`), with byte ranges in absolute
/// source coordinates.
///
/// The result reuses the *prefix* of `prev` unchanged, re-lexes only the
/// affected region, and re-uses the *suffix* by re-synchronizing against the
/// old stream once lexing has passed the changed region and produced a token
/// that is byte-identical to an old one at the shifted position.  This is
/// `O(edit)` in the regex work; materializing the returned `Vec` copies the
/// prefix and suffix.
///
/// The lexer is stateless except for `Glue` (an immediately-preceding token's
/// byte-end equals this delimiter's start), so the only cross-token dependency
/// is local.  Re-synchronization must compare (**kind, byte range**) — not
/// byte offset alone — because an edit can *merge* two tokens (`a b` -> `ab`)
/// or *split* one (`ab` -> `a b`).
pub fn lex_resume(
    prev: &[Token],
    old_source: &str,
    new_source: &str,
    line_starts: &[usize],
    a: usize,
    b: usize,
) -> Lexed {
    let delta = new_source.len() as isize - old_source.len() as isize;
    let mut tokens: Vec<Token> = Vec::new();
    let mut errors: Vec<Diag> = Vec::new();

    // The first old token that could be affected: the one whose byte range
    // ends at or after `a`.  This is the token that would absorb an insertion
    // just before `a`, or that contains the edit.
    let i = prev
        .iter()
        .position(|t| t.range.1 >= a as u32)
        .unwrap_or(prev.len());
    // The byte offset (in `old_source`/`new_source`) at which to start re-lexing.
    // `prev[i].range.0 <= a`, so this offset is identical in old and new.
    let s: u32 = if i < prev.len() { prev[i].range.0 } else { a as u32 };

    // Reuse the intact prefix (tokens before index `i` are before the edit).
    tokens.extend_from_slice(&prev[..i]);

    // Seed the `Glue` decision: the byte end of the last real token before the
    // region, or `None` after a separator/error/bos.
    let mut prev_end = seed_prev_end(prev, i);

    let code = &new_source[s as usize..];
    let mut lexer = RawToken::lexer(code);

    // Probe index into `prev` for the re-sync search.
    let mut j = i;
    let mut resynced_at: Option<usize> = None;

    'lex: while let Some(result) = lexer.next() {
        match result {
            Ok(raw) => {
                let span = lexer.span();
                let start_abs = s + span.start as u32;
                let end_abs = s + span.end as u32;
                let lc = line_col(line_starts, start_abs);
                if raw == RawToken::Separator {
                    let t = Token { kind: TokenKind::Separator, span: lc, range: (start_abs, end_abs) };
                    if let Some(jj) = resync(&prev, &mut j, &t, delta, b) {
                        resynced_at = Some(jj);
                        break 'lex;
                    }
                    tokens.push(t);
                    prev_end = None;
                    continue;
                }
                let glued = raw.is_postfix_delim() && prev_end == Some(start_abs);
                if glued {
                    let g = Token { kind: TokenKind::Glue, span: lc, range: (start_abs, start_abs) };
                    if let Some(jj) = resync(&prev, &mut j, &g, delta, b) {
                        resynced_at = Some(jj);
                        break 'lex;
                    }
                    tokens.push(g);
                }
                match raw_to_kind(&raw, lexer.slice(), lc, &mut errors) {
                    Some(kind) => {
                        let t = Token { kind, span: lc, range: (start_abs, end_abs) };
                        if let Some(jj) = resync(&prev, &mut j, &t, delta, b) {
                            resynced_at = Some(jj);
                            break 'lex;
                        }
                        tokens.push(t);
                        prev_end = Some(end_abs);
                    }
                    None => prev_end = Some(end_abs),
                }
            }
            Err(..) => {
                let span = lexer.span();
                let start_abs = s + span.start as u32;
                let lc = line_col(line_starts, start_abs);
                let ch = lexer.slice().chars().next().unwrap_or('?');
                errors.push(Diag::new(Stage::Lex, lc, format!("unexpected character '{ch}'")));
                prev_end = None;
            }
        }
    }

    if let Some(jj) = resynced_at {
        // Reuse the old suffix, shifted to the new source (positions at or past
        // the edit move by `delta`); recompute spans so line/col stay correct
        // even if the edit added/removed newlines.
        for tk in &prev[jj..] {
            let r = (tk.range.0 as isize + delta) as u32;
            let re = (tk.range.1 as isize + delta) as u32;
            tokens.push(Token {
                kind: tk.kind.clone(),
                span: line_col(line_starts, r),
                range: (r, re),
            });
        }
    } else {
        // The region ran to the end without re-synchronizing (no unchanged
        // suffix): push a fresh Eof at the new end.
        let pos = s + code.len() as u32;
        tokens.push(Token {
            kind: TokenKind::Eof,
            span: line_col(line_starts, pos),
            range: (pos, pos),
        });
    }

    Lexed { tokens, errors }
}

/// The `Glue` seed for the token at index `i`: the byte end of the last *real*
/// token before it (skipping `Glue`s, which do not set the flag), or `None`
/// after a separator / error / beginning of the stream.
fn seed_prev_end(prev: &[Token], i: usize) -> Option<u32> {
    let mut k = i;
    while k > 0 {
        k -= 1;
        match prev[k].kind {
            TokenKind::Glue => continue,
            TokenKind::Separator => return None,
            _ => return Some(prev[k].range.1),
        }
    }
    None
}

/// Attempt to re-synchronize the re-lexed token `t` against the old stream,
/// probing from `j`.  Only re-sync on a token at or past the old edit end `b`
/// (the only tokens whose new position is `old + delta`); an earlier token is
/// still inside the changed region.  Advances `j` past old tokens entirely
/// before the target so the probe is amortized `O(edit)`.
fn resync(prev: &[Token], j: &mut usize, t: &Token, delta: isize, b: usize) -> Option<usize> {
    let target = t.range.0 as isize - delta;
    if target < b as isize {
        return None;
    }
    while *j < prev.len() && (prev[*j].range.1 as isize) < target {
        *j += 1;
    }
    if *j < prev.len()
        && prev[*j].range.0 as isize == target
        && prev[*j].range.1 as isize == t.range.1 as isize - delta
        && prev[*j].kind == t.kind
    {
        Some(*j)
    } else {
        None
    }
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
        RawToken::StrLit => {
            // The matched text is `"…"` (or a bare `"` / an unterminated
            // `"…` when the closing quote is missing).  An unterminated
            // string is a lex error and the token is dropped; a terminated
            // one keeps its content (the quotes stripped).
            if slice.len() < 2 || !slice.ends_with('"') {
                errors.push(Diag::new(Stage::Lex, lc, "unterminated string literal"));
                return None;
            }
            Some(TokenKind::Str(slice[1..slice.len() - 1].to_string()))
        }
        RawToken::NameLit => Some(match slice {
            "Int" => TokenKind::KwInt,
            "string" => TokenKind::KwString,
            "Type" => TokenKind::KwType,
            "struct" => TokenKind::KwStruct,
            "table" => TokenKind::KwTable,
            "let" => TokenKind::KwLet,
            "if" => TokenKind::KwIf,
            "then" => TokenKind::KwThen,
            "else" => TokenKind::KwElse,
            "return" => TokenKind::KwReturn,
            "pub" => TokenKind::KwPub,
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
        RawToken::KwString => Some(TokenKind::KwString),
        RawToken::KwType => Some(TokenKind::KwType),
        RawToken::KwStruct => Some(TokenKind::KwStruct),
        RawToken::KwTable => Some(TokenKind::KwTable),
        RawToken::KwLet => Some(TokenKind::KwLet),
        RawToken::KwIf => Some(TokenKind::KwIf),
        RawToken::KwThen => Some(TokenKind::KwThen),
        RawToken::KwElse => Some(TokenKind::KwElse),
        RawToken::KwReturn => Some(TokenKind::KwReturn),
        RawToken::KwPub => Some(TokenKind::KwPub),
        RawToken::Arrow => Some(TokenKind::Arrow),
        RawToken::FatArrow => Some(TokenKind::FatArrow),
        RawToken::DoubleColon => Some(TokenKind::DoubleColon),
        RawToken::Colon => Some(TokenKind::Colon),
        RawToken::Hash => Some(TokenKind::Hash),
        RawToken::Question => Some(TokenKind::Question),
        RawToken::Bang => Some(TokenKind::Bang),
        RawToken::Dollar => Some(TokenKind::Dollar),
        RawToken::Equals => Some(TokenKind::Equals),
        RawToken::Eq => Some(TokenKind::Eq),
        RawToken::Leq => Some(TokenKind::Leq),
        RawToken::Plus => Some(TokenKind::Plus),
        RawToken::Minus => Some(TokenKind::Minus),
        RawToken::Dot => Some(TokenKind::Dot),
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
