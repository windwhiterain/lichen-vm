//! The lexer: source text to tokens with `(line, column)` spans.  Grammar:
//! `docs/language-spec.md`; postfix Glue is §2.1.

use logos::Logos;

pub use lichen_span::{Span, line_col, line_starts, line_text, offset_of_span};

/// A lex diagnostic: a message plus its source position; widened to the
/// language crate's `Diag` at `Stage::Lex`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexDiag {
    pub span: Option<Span>,
    pub message: String,
}

// No `Eq`: the `f32` payload (`docs/notes/floating-point.md` §3.3).
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// An integer literal.
    Int(usize),
    /// A float literal — one `f32`, the value [`raw_to_kind`] read from the
    /// matched digits.
    Float(f32),
    /// A string literal — the immutable builtin `string` value's content.
    Str(String),
    /// An identifier, never a keyword.
    Name(String),
    /// `_` — an inference placeholder hole in any position; never a name.
    Placeholder,
    /// The Int type constant.
    KwInt,
    /// The `Float` type constant — a keyword like `Int`
    /// (`docs/notes/floating-point.md` §3.3).
    KwFloat,
    /// The string type constant.
    KwString,
    /// The Type type constant -- the universe.
    KwType,
    /// The struct keyword -- a nominal struct type.
    KwStruct,
    /// The table keyword -- a constant table literal.
    KwTable,
    /// The `set` keyword — a set value, `set{a, b}` (`docs/language-spec.md` §2).
    KwSet,
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
    /// `@loop` — a binding whose recursion may become a loop; the first keyword
    /// at the `@` sigil.
    KwLoop,
    /// `@assert` — the prefix assert, `@assert e`; it replaced the `!` sigil,
    /// now a refinement annotation.
    KwAssert,
    /// `@in` — set membership, `value @in set`; the one keyword that is an infix
    /// operator.
    KwIn,
    /// The pub keyword -- a block statement marked as a struct field.
    KwPub,
    /// The `cache` keyword — a binding whose value is a retained cell
    /// (`docs/notes/incremental-update.md` §3).
    KwCache,
    /// The `array` keyword — a keyword-led array type, `array<T, n>`.
    KwArray,
    /// The `int2float` keyword — the prefix `Int` to `Float` conversion, at the
    /// prefix level beside `!`.
    KwInt2Float,
    /// The `float2int` keyword -- the prefix conversion from `Float` to `Int`,
    /// truncating toward zero.
    KwFloat2Int,
    /// '->' -- a function type.
    Arrow,
    /// '=>' -- a lambda.
    FatArrow,
    /// ':' -- an annotation.
    Colon,
    /// '::' -- the raw named field read on a TypeStruct value (`X::a`).
    DoubleColon,
    /// '==>' -- the table literal's key/value separator (`table { k ==> v }`).
    TableArrow,
    /// '#' -- the perspective annotation.
    Hash,
    /// '?' -- the label (doc) annotation: `e ? expr`.
    Question,
    /// `!` — the refinement annotation's sigil, `e : T ! p`; the assert it used
    /// to spell is `@assert`.
    Bang,
    /// '$' — a native-operator call prefix, `$jit(f)`, for a plugin's own source.
    Dollar,
    /// '=' -- a statement binding.
    Equals,
    /// '==' -- equality.
    Eq,
    /// '!=' -- inequality (the same generalized equality as `==`, negated).
    Neq,
    /// '<=' -- less-or-equal.
    Leq,
    /// '>=' -- greater-or-equal.
    Geq,
    /// '+' -- addition.
    Plus,
    /// '-' -- subtraction.
    Minus,
    /// '*' -- multiplication.
    Star,
    /// '/' -- integer division.
    Slash,
    /// '%' -- remainder.
    Percent,
    /// '&' -- bitwise and; over two `0`/`1` comparison results it is `and`.
    Amp,
    /// '|' -- bitwise or; over two `0`/`1` comparison results it is `or`.
    Pipe,
    /// '^' -- bitwise exclusive or.
    Caret,
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
    /// '<' — a type delimiter, or the less-than comparison; Glue decides
    /// (`docs/language-spec.md` §2.1).
    LAngle,
    /// '>' — closes an angle bracket, or is the greater-than comparison.
    RAngle,
    /// A '~' shallow marker.
    Tilde(usize),
    /// A newline, comma, or semicolon -- a uniform boundary token.
    Separator,
    /// A zero-width marker: the next '(' '{' '<' '[' or '::' is glued to the
    /// previous token, so it is a postfix form.
    Glue,
    Eof,
}

impl TokenKind {
    /// The human-readable spelling used in parse-error messages.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Int(_) => "an integer literal".to_string(),
            TokenKind::Float(_) => "a float literal".to_string(),
            TokenKind::Str(_) => "a string literal".to_string(),
            TokenKind::Name(_) => "a name".to_string(),
            TokenKind::Placeholder => "'_'".to_string(),
            TokenKind::KwInt => "'Int'".to_string(),
            TokenKind::KwFloat => "'Float'".to_string(),
            TokenKind::KwString => "'string'".to_string(),
            TokenKind::KwType => "'Type'".to_string(),
            TokenKind::KwStruct => "'struct'".to_string(),
            TokenKind::KwTable => "'table'".to_string(),
            TokenKind::KwSet => "'set'".to_string(),
            TokenKind::KwLet => "'let'".to_string(),
            TokenKind::KwIf => "'if'".to_string(),
            TokenKind::KwThen => "'then'".to_string(),
            TokenKind::KwElse => "'else'".to_string(),
            TokenKind::KwReturn => "'return'".to_string(),
            TokenKind::KwLoop => "'@loop'".to_string(),
            TokenKind::KwAssert => "'@assert'".to_string(),
            TokenKind::KwIn => "'@in'".to_string(),
            TokenKind::KwPub => "'pub'".to_string(),
            TokenKind::KwCache => "'cache'".to_string(),
            TokenKind::KwArray => "'array'".to_string(),
            TokenKind::KwInt2Float => "'int2float'".to_string(),
            TokenKind::KwFloat2Int => "'float2int'".to_string(),
            TokenKind::Arrow => "'->'".to_string(),
            TokenKind::FatArrow => "'=>'".to_string(),
            TokenKind::Colon => "':'".to_string(),
            TokenKind::DoubleColon => "'::'".to_string(),
            TokenKind::TableArrow => "'==>'".to_string(),
            TokenKind::Hash => "'#'".to_string(),
            TokenKind::Question => "'?'".to_string(),
            TokenKind::Bang => "'!'".to_string(),
            TokenKind::Dollar => "'$'".to_string(),
            TokenKind::Equals => "'='".to_string(),
            TokenKind::Eq => "'=='".to_string(),
            TokenKind::Neq => "'!='".to_string(),
            TokenKind::Leq => "'<='".to_string(),
            TokenKind::Geq => "'>='".to_string(),
            TokenKind::Plus => "'+'".to_string(),
            TokenKind::Minus => "'-'".to_string(),
            TokenKind::Star => "'*'".to_string(),
            TokenKind::Slash => "'/'".to_string(),
            TokenKind::Percent => "'%'".to_string(),
            TokenKind::Amp => "'&'".to_string(),
            TokenKind::Pipe => "'|'".to_string(),
            TokenKind::Caret => "'^'".to_string(),
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

// Same reason as `TokenKind`: the float payload is `f32`, not `Eq`.
#[derive(Clone, Debug, PartialEq)]
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

/// The result of lexing: the tokens (always ending with Eof) plus any errors.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<LexDiag>,
}

/// The token kinds logos recognizes; payloads are read from the matched slice.
// Same reason as `TokenKind`: the float payload is `f32`, not `Eq`.
#[derive(Logos, Clone, Debug, PartialEq)]
#[logos(skip r"[ \t\r]+")]
enum RawToken {
    #[regex(r"\n|,|;")]
    Separator,
    #[regex(r"[0-9]+")]
    IntLit,
    /// A float literal `[0-9]+\.[0-9]+`; a digit before the dot is required
    /// (`docs/notes/floating-point.md` §3.3).
    #[regex(r"[0-9]+\.[0-9]+")]
    FloatLit,
    /// A `"..."` string literal: no escapes, may span newlines, the closing
    /// quote optional.
    #[regex("\"[^\"]*\"?")]
    StrLit,
    #[token("Int")]
    KwInt,
    #[token("Float")]
    KwFloat,
    #[token("string")]
    KwString,
    #[token("Type")]
    KwType,
    #[token("struct")]
    KwStruct,
    #[token("table")]
    KwTable,
    #[token("set")]
    KwSet,
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
    #[token("cache")]
    KwCache,
    /// A `@`-prefixed word; an unknown one is a lex error, which reserves `@`.
    #[regex(r"@[A-Za-z_][A-Za-z0-9_]*")]
    AtNameLit,
    #[token("array")]
    KwArray,
    #[token("int2float")]
    KwInt2Float,
    #[token("float2int")]
    KwFloat2Int,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    NameLit,
    #[token("->")]
    Arrow,
    #[token("=>")]
    FatArrow,
    #[token("::")]
    DoubleColon,
    #[token("==>")]
    TableArrow,
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
    #[token("!=")]
    Neq,
    #[token("<=")]
    Leq,
    #[token(">=")]
    Geq,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("&")]
    Amp,
    #[token("|")]
    Pipe,
    #[token("^")]
    Caret,
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
            RawToken::LParen
                | RawToken::LBracket
                | RawToken::LBrace
                | RawToken::LAngle
                | RawToken::DoubleColon
        )
    }
}

pub fn lex(source: &str) -> Lexed {
    let line_starts = line_starts(source);
    lex_with(source, &line_starts, 0)
}

/// Lex `code`, a slice of a larger source at `base`; ranges and spans are
/// absolute positions in that source.
pub fn lex_with(code: &str, line_starts: &[usize], base: u32) -> Lexed {
    let mut tokens: Vec<Token> = Vec::new();
    let mut errors: Vec<LexDiag> = Vec::new();
    let mut lexer = RawToken::lexer(code);
    // Byte end of the last real token; None after a Separator or error.
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
                        tokens.push(Token {
                            kind,
                            span: lc,
                            range: (start, end),
                        });
                        prev_end = Some(end);
                    }
                    None => {
                        // literal overflow: the run was consumed, no token.
                        prev_end = Some(end);
                    }
                }
            }
            Err(..) => {
                let span = lexer.span();
                let start = base + span.start as u32;
                let lc = line_col(line_starts, start);
                let ch = lexer.slice().chars().next().unwrap_or('?');
                errors.push(LexDiag {
                    span: Some(lc),
                    message: format!("unexpected character '{ch}'"),
                });
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

/// Incremental re-lex: reuse `prev`'s prefix, re-lex the edit, re-sync
/// (`docs/notes/incremental-parse-compile.md`).
///
/// # Invariant
/// `a`, `b` and every token range are absolute in the larger source, and two
/// tokens match only on (kind, byte range) — an edit can merge or split one.
pub fn lex_resume(
    prev: &[Token],
    old_source: &str,
    new_source: &str,
    line_starts: &[usize],
    base: u32,
    a: usize,
    b: usize,
) -> Lexed {
    let delta = new_source.len() as isize - old_source.len() as isize;
    let mut tokens: Vec<Token> = Vec::new();
    let mut errors: Vec<LexDiag> = Vec::new();

    // The first old token whose byte range ends at or after `a`.
    let i = prev
        .iter()
        .position(|t| t.range.1 >= a as u32)
        .unwrap_or(prev.len());
    // Start re-lexing here; `prev[i].range.0 <= a`, so old and new agree.
    let s: u32 = if i < prev.len() {
        prev[i].range.0
    } else {
        a as u32
    };

    // Reuse the intact prefix (tokens before index `i` are before the edit).
    tokens.extend_from_slice(&prev[..i]);

    // Seed the `Glue` decision: byte end of the last real token before the region.
    let mut prev_end = seed_prev_end(prev, i);

    // `s` is absolute, but the text it indexes starts at `base`; a code-relative
    // `a` is caught here.
    let local = s
        .checked_sub(base)
        .expect("lex_resume: the edit span is absolute, like the token ranges");
    let code = &new_source[local as usize..];
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
                    let t = Token {
                        kind: TokenKind::Separator,
                        span: lc,
                        range: (start_abs, end_abs),
                    };
                    if let Some(jj) = resync(prev, &mut j, &t, delta, b) {
                        resynced_at = Some(jj);
                        break 'lex;
                    }
                    tokens.push(t);
                    prev_end = None;
                    continue;
                }
                let glued = raw.is_postfix_delim() && prev_end == Some(start_abs);
                if glued {
                    let g = Token {
                        kind: TokenKind::Glue,
                        span: lc,
                        range: (start_abs, start_abs),
                    };
                    if let Some(jj) = resync(prev, &mut j, &g, delta, b) {
                        resynced_at = Some(jj);
                        break 'lex;
                    }
                    tokens.push(g);
                }
                match raw_to_kind(&raw, lexer.slice(), lc, &mut errors) {
                    Some(kind) => {
                        let t = Token {
                            kind,
                            span: lc,
                            range: (start_abs, end_abs),
                        };
                        if let Some(jj) = resync(prev, &mut j, &t, delta, b) {
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
                errors.push(LexDiag {
                    span: Some(lc),
                    message: format!("unexpected character '{ch}'"),
                });
                prev_end = None;
            }
        }
    }

    if let Some(jj) = resynced_at {
        // Reuse the old suffix shifted by `delta`, recomputing spans so line/col
        // survive newline edits.
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

/// The `Glue` seed for the token at `i`: the byte end of the last real token
/// before it, or `None`.
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

/// Re-synchronize re-lexed token `t` against `prev`, probing from `j`.
///
/// # Invariant
/// `j` only advances, so a whole re-lex is `O(edit)`; a token before the old
/// edit end `b` never matches.
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

/// Map a raw token and its matched slice to a `TokenKind`; an overflow records
/// an error and yields no token.
fn raw_to_kind(
    raw: &RawToken,
    slice: &str,
    lc: Span,
    errors: &mut Vec<LexDiag>,
) -> Option<TokenKind> {
    match raw {
        RawToken::IntLit => {
            let mut value: usize = 0;
            for byte in slice.bytes() {
                let digit = (byte - b'0') as usize;
                match value.checked_mul(10).and_then(|v| v.checked_add(digit)) {
                    Some(v) => value = v,
                    None => {
                        errors.push(LexDiag {
                            span: Some(lc),
                            message: "integer literal out of range".to_string(),
                        });
                        return None;
                    }
                }
            }
            Some(TokenKind::Int(value))
        }
        RawToken::FloatLit => {
            // `from_str` always succeeds; an overflow is an infinity, not an
            // error.
            Some(TokenKind::Float(
                slice.parse::<f32>().unwrap_or(f32::INFINITY),
            ))
        }
        RawToken::StrLit => {
            // The matched text is `"…"`; an unterminated string is an error, a
            // terminated one keeps its content.
            if slice.len() < 2 || !slice.ends_with('"') {
                errors.push(LexDiag {
                    span: Some(lc),
                    message: "unterminated string literal".to_string(),
                });
                return None;
            }
            Some(TokenKind::Str(slice[1..slice.len() - 1].to_string()))
        }
        RawToken::AtNameLit => match &slice[1..] {
            "loop" => Some(TokenKind::KwLoop),
            "assert" => Some(TokenKind::KwAssert),
            "in" => Some(TokenKind::KwIn),
            other => {
                errors.push(LexDiag {
                    span: Some(lc),
                    message: format!(
                        "'@{other}' is not a keyword. `@` prefixes keywords, and `@loop`, \
                         `@assert` and `@in` are the ones that exist"
                    ),
                });
                None
            }
        },
        RawToken::NameLit => Some(match slice {
            "Int" => TokenKind::KwInt,
            "Float" => TokenKind::KwFloat,
            "string" => TokenKind::KwString,
            "Type" => TokenKind::KwType,
            "struct" => TokenKind::KwStruct,
            "table" => TokenKind::KwTable,
            "set" => TokenKind::KwSet,
            "let" => TokenKind::KwLet,
            "if" => TokenKind::KwIf,
            "then" => TokenKind::KwThen,
            "else" => TokenKind::KwElse,
            "return" => TokenKind::KwReturn,
            "pub" => TokenKind::KwPub,
            "cache" => TokenKind::KwCache,
            "array" => TokenKind::KwArray,
            "int2float" => TokenKind::KwInt2Float,
            "float2int" => TokenKind::KwFloat2Int,
            "_" => TokenKind::Placeholder,
            _ => TokenKind::Name(slice.to_string()),
        }),
        RawToken::TildeLit => {
            let digits = &slice[1..];
            // A bare `~` is the unbounded depth, encoded as `usize::MAX` in
            // the token's payload (`TokenKind::Tilde`).
            if digits.is_empty() {
                return Some(TokenKind::Tilde(usize::MAX));
            }
            let mut n: usize = 0;
            for byte in digits.bytes() {
                let digit = (byte - b'0') as usize;
                // Saturating would make an out-of-range depth the bare `~`, a
                // different marker.
                match n.checked_mul(10).and_then(|v| v.checked_add(digit)) {
                    Some(v) => n = v,
                    None => {
                        errors.push(LexDiag {
                            span: Some(lc),
                            message: "shallow marker depth out of range".to_string(),
                        });
                        return None;
                    }
                }
            }
            Some(TokenKind::Tilde(n))
        }
        RawToken::KwInt => Some(TokenKind::KwInt),
        RawToken::KwFloat => Some(TokenKind::KwFloat),
        RawToken::KwString => Some(TokenKind::KwString),
        RawToken::KwType => Some(TokenKind::KwType),
        RawToken::KwStruct => Some(TokenKind::KwStruct),
        RawToken::KwTable => Some(TokenKind::KwTable),
        RawToken::KwSet => Some(TokenKind::KwSet),
        RawToken::KwLet => Some(TokenKind::KwLet),
        RawToken::KwIf => Some(TokenKind::KwIf),
        RawToken::KwThen => Some(TokenKind::KwThen),
        RawToken::KwElse => Some(TokenKind::KwElse),
        RawToken::KwReturn => Some(TokenKind::KwReturn),
        RawToken::KwPub => Some(TokenKind::KwPub),
        RawToken::KwCache => Some(TokenKind::KwCache),
        RawToken::KwArray => Some(TokenKind::KwArray),
        RawToken::KwInt2Float => Some(TokenKind::KwInt2Float),
        RawToken::KwFloat2Int => Some(TokenKind::KwFloat2Int),
        RawToken::Arrow => Some(TokenKind::Arrow),
        RawToken::FatArrow => Some(TokenKind::FatArrow),
        RawToken::DoubleColon => Some(TokenKind::DoubleColon),
        RawToken::TableArrow => Some(TokenKind::TableArrow),
        RawToken::Colon => Some(TokenKind::Colon),
        RawToken::Hash => Some(TokenKind::Hash),
        RawToken::Question => Some(TokenKind::Question),
        RawToken::Bang => Some(TokenKind::Bang),
        RawToken::Dollar => Some(TokenKind::Dollar),
        RawToken::Equals => Some(TokenKind::Equals),
        RawToken::Eq => Some(TokenKind::Eq),
        RawToken::Neq => Some(TokenKind::Neq),
        RawToken::Leq => Some(TokenKind::Leq),
        RawToken::Geq => Some(TokenKind::Geq),
        RawToken::Plus => Some(TokenKind::Plus),
        RawToken::Minus => Some(TokenKind::Minus),
        RawToken::Star => Some(TokenKind::Star),
        RawToken::Slash => Some(TokenKind::Slash),
        RawToken::Percent => Some(TokenKind::Percent),
        RawToken::Amp => Some(TokenKind::Amp),
        RawToken::Pipe => Some(TokenKind::Pipe),
        RawToken::Caret => Some(TokenKind::Caret),
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

#[cfg(test)]
#[path = "tests/lex_tests.rs"]
mod tests;
