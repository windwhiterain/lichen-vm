//! A minimal LSP type set plus the source-position ↔ LSP-position conversion.
//!
//! The frontend reports positions as [`lichen_highlevel::ir::Span`] — a
//! `(line, col)` pair, 1-based, where `col` counts **bytes** from the line's
//! start (see [`lichen_language::lex::line_col`]). LSP wants `line` /
//! `character`, both 0-based, with `character` in **UTF-16 code units**. This
//! module owns that conversion, since it is the single caller-facing dialect of
//! the shared span — a language server and a Zed extension must agree on it, so
//! it lives once, here.
//!
//! The JSON-RPC envelope and the handful of LSP message shapes the server
//! sends/handles are also defined here (serde-backed), keeping the server
//! binary free of any LSP framework dependency.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use lichen_highlevel::ir::Span;

/// A source position: 0-based line, `character` in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// A half-open source range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// LSP diagnostic severities (the protocol's numbers).
pub mod severity {
    pub const ERROR: u32 = 1;
    pub const WARNING: u32 = 2;
    pub const INFORMATION: u32 = 3;
    pub const HINT: u32 = 4;
}

/// A rendered diagnostic ready to publish.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: u32,
    pub source: String,
    pub message: String,
}

/// A `(uri, range)` definition/hover location target.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

// ---------------------------------------------------------------------------
// Span ↔ position conversion
//
// All helpers take the source text and its precomputed line-start byte offsets
// (`lichen_language::lex::line_starts`), so they are pure and reusable.

/// The byte offset of a 1-based `(line, col)` span in `source`.
fn offset_of_span(line_starts: &[usize], span: Span) -> usize {
    let (line, col) = (span.0 as usize, span.1 as usize);
    if line == 0 || line > line_starts.len() {
        return line_starts.len().checked_sub(1).map(|i| line_starts[i]).unwrap_or(0);
    }
    line_starts[line - 1] + (col.saturating_sub(1))
}

/// The number of UTF-16 code units in `text` (LSP's `character` unit).
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Convert a byte offset to a 0-based LSP `Position`.
pub fn position_at_offset(source: &str, line_starts: &[usize], offset: usize) -> Position {
    let offset = offset.min(source.len());
    // `line` is the largest index with `line_starts[line] <= offset`.
    let line = match line_starts.binary_search(&offset) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let line_start = line_starts.get(line).copied().unwrap_or(0);
    let character = utf16_len(&source[line_start.min(source.len())..offset]);
    Position {
        line: line as u32,
        character: character as u32,
    }
}

/// Convert an LSP `Position` to a byte offset, if it is within `source`.
pub fn offset_from_position(source: &str, line_starts: &[usize], pos: Position) -> Option<usize> {
    let line = pos.line as usize;
    let line_start = *line_starts.get(line)?;
    let line_end = line_starts
        .get(line + 1)
        .copied()
        .unwrap_or(source.len());
    let text = &source[line_start..line_end.min(source.len())];
    // Walk `text` counting UTF-16 code units until we reach `character`.
    let mut units = 0usize;
    let mut byte = 0usize;
    for ch in text.chars() {
        if units == pos.character as usize {
            return Some(line_start + byte);
        }
        units += ch.len_utf16();
        byte += ch.len_utf8();
    }
    // `character` may land at (or just past) the end of the line.
    Some(line_start + byte)
}

/// A 1-based `(line, col-byte)` span for a byte offset (the reverse of
/// [`offset_of_span`]).
pub fn span_of_offset(line_starts: &[usize], offset: usize) -> Span {
    let line = match line_starts.binary_search(&offset) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let line_start = line_starts.get(line).copied().unwrap_or(0);
    ((line + 1) as u32, (offset - line_start + 1) as u32)
}

/// An LSP `Range` for a 1-based `(line, col)` span, expanded to one character
/// (so a diagnostic squiggle is visible).
pub fn range_from_span(source: &str, line_starts: &[usize], span: Span) -> Range {
    let start = offset_of_span(line_starts, span).min(source.len());
    // Advance one code point past `start` for the end.
    let end = start + source[start..].chars().next().map(|c| c.len_utf8()).unwrap_or_else(|| source.len().saturating_sub(start).min(1));
    Range {
        start: position_at_offset(source, line_starts, start),
        end: position_at_offset(source, line_starts, end.min(source.len())),
    }
}

/// An LSP `Range` for a half-open byte range in `source`.
pub fn range_from_byte_range(source: &str, line_starts: &[usize], range: (u32, u32)) -> Range {
    let (start, end) = (range.0 as usize, range.1 as usize);
    Range {
        start: position_at_offset(source, line_starts, start.min(source.len())),
        end: position_at_offset(source, line_starts, end.min(source.len())),
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC / LSP wire shapes
//
// A deliberately small, serde-backed subset of the protocol. The server parses
// the JSON body into these; everything unrecognized is dropped.

/// An inbound request/notification envelope.
#[derive(Debug, Deserialize)]
pub struct Envelope {
    #[serde(rename = "jsonrpc")]
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// `None` for a notification (no response expected).
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// The `textDocument` object carried by most requests.
#[derive(Debug, Deserialize)]
pub struct TextDocumentItem {
    pub uri: String,
    #[serde(rename = "languageId", default)]
    #[allow(dead_code)]
    pub language_id: Option<String>,
    #[serde(default)]
    pub version: Option<u32>,
    pub text: String,
}

/// A single content change in a `didChange` `contentChanges` array. Under full
/// sync the change has no `range` and replaces the whole document.
#[derive(Debug, Deserialize)]
pub struct ContentChange {
    #[serde(default)]
    pub range: Option<Range>,
    pub text: String,
}

/// The `TextDocumentPositionParams`: where the cursor is.
#[derive(Debug, Deserialize)]
pub struct TextDocumentPositionParams {
    pub textDocument: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Debug, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

/// Build a `textDocument/publishDiagnostics` notification.
pub fn publish_diagnostics(uri: &str, version: Option<u32>, diagnostics: Vec<Diagnostic>) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri,
            "version": version,
            "diagnostics": diagnostics,
        }
    })
}
