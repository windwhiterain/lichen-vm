//! The preprocessor's block-interior parser.
//!
//! It turns the interior token stream into a list of [Directive]s.  The
//! grammar is a set of statements, Separator-separated (blank lines and a
//! stray trailing separator are tolerated): `name = import "path"` is an
//! import binding, `name = depend "url"` is a git dependency, and
//! `name = "value"` is a string metadata entry.  The block
//! is small and line-oriented, so a simple scan gives precise, uniform
//! errors (expected X found Y) with a byte offset into the interior, which
//! the caller turns into a (line, col) diagnostic against the original file.

use super::lex::{Token, TokenKind};

/// A parsed block-interior statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Directive {
    /// `name = import "path"` -- an import binding.
    Import { name: String, path: String },
    /// `name = "value"` -- a string metadata entry.
    Metadata { name: String, value: String },
    /// `name = depend "url" [rev/branch/tag/package/sub = "x"] [plugin]` -- a
    /// git dependency bound to `name`, fetched by the package manager (not
    /// resolved here).
    Depend {
        url: String,
        name: String,
        rev: Option<String>,
        branch: Option<String>,
        tag: Option<String>,
        package: Option<String>,
        sub: Option<String>,
        plugin: bool,
    },
}

/// Parse a block-interior token stream.  Returns each directive with the
/// byte offset (within the interior) of its first token, or the
/// (byte-offset, message) for the first error (the block is small, so
/// stopping at the first problem is acceptable).
pub fn parse(tokens: &[Token]) -> Result<Vec<(Directive, u32)>, Vec<(u32, String)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        // Separators are idempotent; skip any run of them.
        if matches!(tokens[i].kind, TokenKind::Separator) {
            i += 1;
            continue;
        }

        // A statement starts with a name: an import binding, a metadata entry,
        // or a dependency binding (`name = depend "url"`).
        let stmt_start = tokens[i].range.0;
        let name = match &tokens[i].kind {
            TokenKind::Name(n) => n.clone(),
            k => {
                return Err(vec![(
                    tokens[i].range.0,
                    format!("expected a name, found {}", k.describe()),
                )]);
            }
        };
        i += 1;

        // Then '='.
        let eq = tokens.get(i);
        if eq.map(|t| &t.kind) != Some(&TokenKind::Equals) {
            return Err(vec![err_at(tokens, i, "expected '='".to_string())]);
        }
        i += 1;

        // The right-hand side: `import "path"`, `depend "url" [options]`, or a
        // bare string value.
        match tokens.get(i).map(|t| &t.kind) {
            Some(TokenKind::KwImport) => {
                i += 1;
                let path = match tokens.get(i).map(|t| &t.kind) {
                    Some(TokenKind::String(p)) => p.clone(),
                    k => {
                        return Err(vec![err_at(
                            tokens,
                            i,
                            format!("expected a path after import, found {}", found(k)),
                        )]);
                    }
                };
                i += 1;
                out.push((Directive::Import { name, path }, stmt_start));
            }
            Some(TokenKind::KwDepend) => {
                i += 1;
                let depend = parse_depend(tokens, &mut i, name)?;
                out.push((depend, stmt_start));
            }
            Some(TokenKind::String(v)) => {
                let v = v.clone();
                i += 1;
                out.push((Directive::Metadata { name, value: v }, stmt_start));
            }
            k => {
                return Err(vec![err_at(
                    tokens,
                    i,
                    format!("expected a value, found {}", found(k)),
                )]);
            }
        }
    }
    Ok(out)
}

/// Parse `depend "url" [options...]` starting at `*i` (already past the
/// `depend` keyword), binding the dependency under `name` (the statement's
/// left-hand side).  Reads a URL string then zero or more options:
/// `rev|branch|tag|package|sub = "..."`, `plugin`.  Stops at a Separator (the
/// outer loop consumes it) or the end of the block.
fn parse_depend(
    tokens: &[Token],
    i: &mut usize,
    name: String,
) -> Result<Directive, Vec<(u32, String)>> {
    let url = match tokens.get(*i).map(|t| &t.kind) {
        Some(TokenKind::String(u)) => u.clone(),
        k => {
            return Err(vec![err_at(
                tokens,
                *i,
                format!("expected a url after depend, found {}", found(k)),
            )]);
        }
    };
    *i += 1;

    let mut rev = None;
    let mut branch = None;
    let mut tag = None;
    let mut package = None;
    let mut sub = None;
    let mut plugin = false;

    while let Some(tok) = tokens.get(*i) {
        if matches!(tok.kind, TokenKind::Separator) {
            break;
        }
        let keyword = match &tok.kind {
            TokenKind::Name(k) => k.clone(),
            k => {
                return Err(vec![err_at(
                    tokens,
                    *i,
                    format!(
                        "expected a depend option (rev/branch/tag/package/sub/plugin), found {}",
                        k.describe()
                    ),
                )]);
            }
        };
        *i += 1;
        match keyword.as_str() {
            "rev" => rev = Some(expect_string_after_eq(tokens, i, &keyword)?),
            "branch" => branch = Some(expect_string_after_eq(tokens, i, &keyword)?),
            "tag" => tag = Some(expect_string_after_eq(tokens, i, &keyword)?),
            "package" => package = Some(expect_string_after_eq(tokens, i, &keyword)?),
            "sub" => sub = Some(expect_string_after_eq(tokens, i, &keyword)?),
            "plugin" => plugin = true,
            other => {
                return Err(vec![err_at(
                    tokens,
                    *i - 1,
                    format!("unknown depend option '{other}'"),
                )]);
            }
        }
    }

    Ok(Directive::Depend {
        url,
        name,
        rev,
        branch,
        tag,
        package,
        sub,
        plugin,
    })
}

/// Expect `= "string"` following a keyword option, returning the string.
fn expect_string_after_eq(
    tokens: &[Token],
    i: &mut usize,
    keyword: &str,
) -> Result<String, Vec<(u32, String)>> {
    if tokens.get(*i).map(|t| &t.kind) != Some(&TokenKind::Equals) {
        return Err(vec![err_at(
            tokens,
            *i,
            format!("expected '=' after {keyword}"),
        )]);
    }
    *i += 1;
    match tokens.get(*i).map(|t| &t.kind) {
        Some(TokenKind::String(v)) => {
            let v = v.clone();
            *i += 1;
            Ok(v)
        }
        k => Err(vec![err_at(
            tokens,
            *i,
            format!("expected a string after '{keyword} =', found {}", found(k)),
        )]),
    }
}

/// An error pointing at `idx` (or the end of the block when out of range).
fn err_at(tokens: &[Token], idx: usize, message: String) -> (u32, String) {
    let offset = tokens
        .get(idx)
        .map(|t| t.range.0)
        .unwrap_or_else(|| tokens.last().map(|t| t.range.1).unwrap_or(0));
    (offset, message)
}

/// The "found" portion of an error message.
fn found(k: Option<&TokenKind>) -> String {
    k.map(|k| k.describe())
        .unwrap_or_else(|| "the end of the block".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preprocess::lex::tokenize;

    fn parse_ok(interior: &str) -> Vec<Directive> {
        let lexed = tokenize(interior);
        assert!(
            lexed.errors.is_empty(),
            "unexpected lex errors: {:?}",
            lexed.errors
        );
        parse(&lexed.tokens)
            .expect("expected a clean parse")
            .into_iter()
            .map(|(d, _)| d)
            .collect()
    }

    #[test]
    fn an_import_binding_parses() {
        let dirs = parse_ok("math = import \"math.lichen\"");
        assert_eq!(
            dirs,
            vec![Directive::Import {
                name: "math".to_string(),
                path: "math.lichen".to_string()
            }]
        );
    }

    #[test]
    fn a_metadata_string_parses() {
        let dirs = parse_ok("order = \"3\"");
        assert_eq!(
            dirs,
            vec![Directive::Metadata {
                name: "order".to_string(),
                value: "3".to_string()
            }]
        );
    }

    #[test]
    fn a_depend_directive_parses() {
        let dirs = parse_ok("foo = depend \"https://example.com/foo.git\"");
        assert_eq!(
            dirs,
            vec![Directive::Depend {
                url: "https://example.com/foo.git".to_string(),
                name: "foo".to_string(),
                rev: None,
                branch: None,
                tag: None,
                package: None,
                sub: None,
                plugin: false,
            }]
        );
    }

    #[test]
    fn a_depend_directive_with_options_parses() {
        let dirs = parse_ok(
            "foo = depend \"https://example.com/foo.git\" rev = \"abc123\" \
             branch = \"main\" package = \"foo-crate\" sub = \"lichen-std\" plugin",
        );
        assert_eq!(
            dirs,
            vec![Directive::Depend {
                url: "https://example.com/foo.git".to_string(),
                name: "foo".to_string(),
                rev: Some("abc123".to_string()),
                branch: Some("main".to_string()),
                tag: None,
                package: Some("foo-crate".to_string()),
                sub: Some("lichen-std".to_string()),
                plugin: true,
            }]
        );
    }

    #[test]
    fn a_depend_without_a_url_is_an_error() {
        let lexed = tokenize("foo = depend");
        let errors = parse(&lexed.tokens).expect_err("expected an error");
        assert!(
            errors[0].1.contains("expected a url"),
            "got: {}",
            errors[0].1
        );
    }

    #[test]
    fn a_depend_without_a_name_is_an_error() {
        let lexed = tokenize("depend \"https://example.com/foo.git\"");
        let errors = parse(&lexed.tokens).expect_err("expected an error");
        assert!(
            errors[0].1.contains("expected a name"),
            "got: {}",
            errors[0].1
        );
    }

    #[test]
    fn an_as_option_is_rejected() {
        let lexed = tokenize("foo = depend \"https://example.com/foo.git\" as bar");
        let errors = parse(&lexed.tokens).expect_err("expected an error");
        assert!(
            errors[0].1.contains("unknown depend option"),
            "got: {}",
            errors[0].1
        );
    }

    #[test]
    fn multiple_statements_split_on_separators() {
        let dirs = parse_ok("a = \"1\"\nb = import \"p.lichen\"\n\nc = \"three\"");
        assert_eq!(
            dirs,
            vec![
                Directive::Metadata {
                    name: "a".to_string(),
                    value: "1".to_string()
                },
                Directive::Import {
                    name: "b".to_string(),
                    path: "p.lichen".to_string()
                },
                Directive::Metadata {
                    name: "c".to_string(),
                    value: "three".to_string()
                },
            ]
        );
    }

    #[test]
    fn a_missing_value_is_an_error() {
        let lexed = tokenize("a =");
        let errors = parse(&lexed.tokens).expect_err("expected an error");
        assert!(errors[0].1.contains("expected"), "got: {}", errors[0].1);
    }

    #[test]
    fn a_missing_equals_is_an_error() {
        let lexed = tokenize("a \"1\"");
        let errors = parse(&lexed.tokens).expect_err("expected an error");
        assert!(errors[0].1.contains("expected '='"), "got: {}", errors[0].1);
    }

    #[test]
    fn an_empty_block_parses_to_nothing() {
        let lexed = tokenize(" \n \n ");
        assert_eq!(parse(&lexed.tokens).expect("empty is fine"), vec![]);
    }
}
