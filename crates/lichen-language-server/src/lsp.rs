//! Source-position ↔ LSP-position conversion, and the canonical LSP types.
//!
//! The frontend reports positions as [`lichen_highlevel::ir::Span`] — a
//! `(line, col)` pair, 1-based, where `col` counts **bytes** from the line's
//! start (see [`lichen_language::lex::line_col`]). LSP wants `line` /
//! `character`, both 0-based, with `character` in **UTF-16 code units**. This
//! module owns that conversion, since it is the single caller-facing dialect of
//! the shared span — a language server and a Zed extension must agree on it, so
//! it lives once, here.
//!
//! The protocol *types* are the canonical ones from `lsp-types` (re-exported
//! here and by the crate root), so the server and any extension speak the same
//! types the editor does. The JSON-RPC framing, request dispatch, cancellation
//! and error handling are provided by `tower-lsp` in the server binary; the
//! library only needs the span math and the protocol type set.

pub use lsp_types::{
    Diagnostic, DiagnosticSeverity, Position, Range, SemanticToken, SemanticTokenModifier,
    SemanticTokens, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensResult,
    SemanticTokenType,
};

use lichen_highlevel::ir::Span;

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
// Semantic tokens
//
// Lichen's own parser drives highlighting (see `analysis::Doc::semantic_tokens`):
// the frontend classifies each token (literal / keyword / operator / name), and
// an editor consumes it via the LSP `textDocument/semanticTokens/full` request.
// This module owns the protocol half — the legend and the delta encoding — so a
// Zed extension requesting semantic tokens from the server and the server itself
// agree on the same type set.  The grammar-optional path: when the tree-sitter
// grammar is absent, semantic tokens carry the highlighting instead.

/// One semantic token in *source* coordinates (a half-open byte range) with its
/// LSP legend classification — the pre-encoding view the frontend produces.
#[derive(Clone, Debug)]
pub struct SemanticTokenData {
    pub start: u32,
    pub end: u32,
    pub token_type: SemanticTokenType,
    pub modifiers: Vec<SemanticTokenModifier>,
}

/// The token types / modifiers this server emits.  The list order defines the
/// indices in the encoded `data` array, so it is the single source of truth for
/// both the capability advertisement and the encoder.
pub fn semantic_token_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::TYPE,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::KEYWORD,
            SemanticTokenType::OPERATOR,
            SemanticTokenType::NUMBER,
            SemanticTokenType::STRING,
            SemanticTokenType::COMMENT,
        ],
        token_modifiers: vec![SemanticTokenModifier::DECLARATION],
    }
}

fn index_of_type(legend: &SemanticTokensLegend, t: &SemanticTokenType) -> u32 {
    legend.token_types.iter().position(|x| x == t).unwrap_or(0) as u32
}

/// Delta-encode `tokens` (already in document order) into LSP `SemanticTokens`.
/// The first token's line/start are absolute; each later token's are deltas from
/// the previous one (a `delta_start` only when on the same line).  Lengths and
/// the inner `delta_start` are in UTF-16 code units (LSP's `character` unit).
pub fn encode_semantic_tokens(
    source: &str,
    line_starts: &[usize],
    tokens: &[SemanticTokenData],
) -> SemanticTokens {
    let legend = semantic_token_legend();
    let mut data = Vec::with_capacity(tokens.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    for t in tokens {
        let start = position_at_offset(source, line_starts, t.start as usize);
        let end = position_at_offset(source, line_starts, t.end as usize);
        let delta_line = start.line - prev_line;
        let delta_start = if delta_line == 0 {
            start.character.saturating_sub(prev_start)
        } else {
            start.character
        };
        let length = end.character.saturating_sub(start.character);
        let token_type = index_of_type(&legend, &t.token_type);
        let mut token_modifiers_bitset = 0u32;
        for m in &t.modifiers {
            if let Some(i) = legend.token_modifiers.iter().position(|x| x == m) {
                token_modifiers_bitset |= 1 << i;
            }
        }
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type,
            token_modifiers_bitset,
        });
        prev_line = start.line;
        prev_start = start.character;
    }
    // Serialize as the flat `[deltaLine, deltaStart, length, type, modifiers]*`.
    SemanticTokens { result_id: None, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_offset_round_trips() {
        let source = "a = 1\nb = a + 1\nb";
        let starts = lichen_language::lex::line_starts(source);
        // The `a` use in `b = a + 1` is at line 1, byte column 4 (0-based col 4).
        let pos = position_at_offset(source, &starts, starts[1] + 4);
        assert_eq!(pos, Position { line: 1, character: 4 });
        let back = offset_from_position(source, &starts, pos).expect("position in source");
        assert_eq!(back, starts[1] + 4);
    }

    #[test]
    fn span_of_offset_round_trips_to_1_based_span() {
        let source = "a = 1\nb";
        let starts = lichen_language::lex::line_starts(source);
        // Byte offset of the `b` on line 2 (col 1).
        let offset = starts[1];
        assert_eq!(span_of_offset(&starts, offset), (2, 1));
    }

    #[test]
    fn encode_semantic_tokens_delta_encodes_and_maps_legend_indices() {
        let source = "a = 1\nb = 2\na + b\n";
        let starts = lichen_language::lex::line_starts(source);
        let tokens = vec![
            // `a` at line 0 char 0.
            SemanticTokenData { start: 0, end: 1, token_type: SemanticTokenType::VARIABLE, modifiers: Vec::new() },
            // `1` at line 0 char 4 (same line, so a delta_start).
            SemanticTokenData { start: 4, end: 5, token_type: SemanticTokenType::NUMBER, modifiers: Vec::new() },
            // `2` at line 1 char 4 (a new line, so an absolute delta_start).
            SemanticTokenData { start: 10, end: 11, token_type: SemanticTokenType::NUMBER, modifiers: Vec::new() },
        ];
        let encoded = encode_semantic_tokens(source, &starts, &tokens);
        assert_eq!(encoded.data.len(), 3);
        let legend = semantic_token_legend();
        let ty = |t: &SemanticTokenType| legend.token_types.iter().position(|x| x == t).unwrap() as u32;
        assert_eq!(encoded.data[0].delta_line, 0);
        assert_eq!(encoded.data[0].delta_start, 0);
        assert_eq!(encoded.data[0].length, 1);
        assert_eq!(encoded.data[0].token_type, ty(&SemanticTokenType::VARIABLE));
        assert_eq!(encoded.data[1].delta_line, 0);
        assert_eq!(encoded.data[1].delta_start, 4);
        assert_eq!(encoded.data[1].token_type, ty(&SemanticTokenType::NUMBER));
        assert_eq!(encoded.data[2].delta_line, 1);
        assert_eq!(encoded.data[2].delta_start, 4);
        assert_eq!(encoded.data[2].token_type, ty(&SemanticTokenType::NUMBER));
    }
}
