//! The preprocessor for the dedicated `@import` line format.
//!
//! The language parser/lexer never sees `@import`: this module performs a
//! line-level scan, loads the referenced packages through the
//! [`PackageStore`], and returns a cleaned source (same line numbering) plus
//! resolved import bindings that the compiler injects as the first scope
//! frame.  A package's own `@import` lines resolve the same way — the store
//! loads each dependency recursively before the importer compiles, which is
//! how transitive dependencies work.

use std::path::{Path, PathBuf};

use lichen_highlevel::ir::Span;
use lichen_lowlevel::StaticNodeId;

use crate::diag::{Diag, Stage};
use crate::package::PackageStore;

/// One resolved `@import "path" as name` directive.
#[derive(Clone, Debug)]
pub struct ResolvedImport {
    pub name: String,
    /// The `(line, column)` of the original `@import` directive.
    pub span: Span,
    /// The package's exported final `[value, type]` pair node (static ref in
    /// the shared registry).
    pub export: StaticNodeId,
    /// The canonical path of the imported package — the device registry's
    /// dependency-graph edge records it alongside the dependency's key.
    pub path: PathBuf,
}

/// The preprocessor output: cleaned lichen source plus resolved imports.
#[derive(Clone, Debug)]
pub struct Preprocessed {
    pub source: String,
    pub imports: Vec<ResolvedImport>,
}

/// A syntactically parsed `@import` line (without resolving the package).
struct ImportLine {
    name: String,
    path: String,
}

/// Preprocess `raw`: scan line by line, resolve each `@import` through the
/// shared package store, and blank out directive lines while preserving line
/// numbers.
pub fn preprocess(
    raw: &str,
    base: Option<&Path>,
    store: &mut PackageStore,
) -> (Preprocessed, Vec<Diag>) {
    let mut imports = Vec::new();
    let mut diags = Vec::new();
    let mut source = String::with_capacity(raw.len());

    for (idx, line) in raw.split_inclusive('\n').enumerate() {
        let line_no = idx as u32 + 1;
        let parsed = parse_import_line(line);
        if let Some(parsed) = parsed {
            let body = body_without_newline(line);
            let leading_len = body.len() - body.trim_start_matches([' ', '\t']).len();
            // Keep the line count (and intentionally leave a blank line where
            // the directive was) so parser/checker diagnostics point at the
            // original source coordinates.
            push_line(&mut source, line, true);
            match store.resolve_import(base, &parsed.path) {
                Ok(handle) => imports.push(ResolvedImport {
                    name: parsed.name,
                    span: (line_no, (leading_len + 1) as u32),
                    export: handle.export,
                    path: handle.path,
                }),
                Err(mut diag) => {
                    // A failed load (missing path, cycle, or diagnostics from
                    // inside the package) is reported at the directive that
                    // pulled it: the caret belongs to this file, not the
                    // package's own coordinates.
                    diag.span = Some((line_no, (leading_len + 1) as u32));
                    diag.message = format!("cannot load package '{}': {}", parsed.path, diag.message);
                    diags.push(diag);
                }
            }
        } else {
            let body = body_without_newline(line);
            let trimmed = body.trim_start_matches([' ', '\t']);
            if trimmed.starts_with("@import") {
                let leading_len = body.len() - trimmed.len();
                diags.push(Diag::new(
                    Stage::Preprocess,
                    (line_no, (leading_len + 1) as u32),
                    format!("invalid @import directive: `{}`", body.trim_end()),
                ));
            }
            push_line(&mut source, line, false);
        }
    }

    (Preprocessed { source, imports }, diags)
}

fn body_without_newline(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn push_line(out: &mut String, line: &str, blank: bool) {
    if blank {
        if let Some(stripped) = line.strip_suffix('\n') {
            if let Some(_stripped) = stripped.strip_suffix('\r') {
                out.push_str("\r\n");
            } else {
                out.push('\n');
            }
        }
        // An unterminated final line is replaced by nothing (it was a full
        // directive with no following newline).
    } else {
        out.push_str(line);
    }
}

/// Parse a single physical line.  `line` may include its trailing newline.
fn parse_import_line(line: &str) -> Option<ImportLine> {
    let body = body_without_newline(line);
    let rest = body.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix("@import")?;
    let rest = rest.trim_start_matches([' ', '\t']);

    // Path: a double-quoted string.  v1 deliberately keeps escaping minimal.
    let path = rest.strip_prefix('"')?;
    let end = path.find('"')?;
    let import_path = &path[..end];
    if import_path.is_empty() {
        return None;
    }
    let after = path[end + 1..].trim_start_matches([' ', '\t']);
    let after = after.strip_prefix("as")?;
    let after = after.trim_start_matches([' ', '\t']);
    let name_end = after
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    if name_end == 0 {
        return None;
    }
    let tail = after[name_end..].trim_matches([' ', '\t']);
    // A trailing `;` is accepted as ordinary lichen statement punctuation;
    // the cleaned source blanks the whole line, so it never reaches the
    // parser.
    if tail.is_empty() || tail == ";" {
        Some(ImportLine {
            name: after[..name_end].to_string(),
            path: import_path.to_string(),
        })
    } else {
        None
    }
}
