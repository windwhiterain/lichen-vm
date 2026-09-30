//! The language server's LSP positions are the **protocol dialect** of the one
//! byte ↔ line/column conversion in `lichen-span`: a 0-based line and a UTF-16
//! `character`, computed from `lichen_span::line_col`'s 1-based byte column.
//!
//! These pin the boundary — including the two places LSP must clamp (a byte
//! past the source, a byte inside a multi-byte character) and the byte model
//! must not, and the out-of-range answer a span gets.

use lichen_language::lex::{line_col, line_starts};
use lichen_language_server::lsp::{
    Position, offset_from_position, offset_of_span, position_at_offset, span_of_offset,
};

/// `(source, offsets to probe)` — the edges the conversions answered
/// differently at before they shared one implementation.
const CASES: &[(&str, &[usize])] = &[
    ("", &[0, 1]),
    ("ab\n", &[0, 2, 3]),
    ("ab", &[0, 2, 5]),
    ("é=1", &[0, 1, 2, 3]),
    ("a\r\nb", &[0, 1, 2, 3, 4]),
    ("a\rb", &[0, 1, 2, 3]),
];

#[test]
fn the_lsp_span_is_the_one_line_col_conversion() {
    for &(source, offsets) in CASES {
        let starts = line_starts(source);
        for &offset in offsets {
            assert_eq!(
                span_of_offset(&starts, offset),
                line_col(&starts, offset as u32),
                "span_of_offset must be `lichen_span::line_col`: {source:?} at byte {offset}"
            );
        }
    }
}

#[test]
fn the_lsp_line_is_the_span_line_zero_based() {
    for &(source, offsets) in CASES {
        let starts = line_starts(source);
        for &offset in offsets {
            let (line, _) = line_col(&starts, offset as u32);
            assert_eq!(
                position_at_offset(source, &starts, offset).line,
                line - 1,
                "the LSP line must be the span's line, 0-based: {source:?} at byte {offset}"
            );
        }
    }
}

#[test]
fn the_lsp_character_counts_utf16_units_from_the_line_start() {
    // `é` is one UTF-16 unit in two bytes; `😀` is two units in four bytes.
    let source = "aé=1\nb";
    let starts = line_starts(source);
    for (offset, character) in [(0, 0), (1, 1), (3, 2), (4, 3), (6, 0)] {
        assert_eq!(
            position_at_offset(source, &starts, offset).character,
            character,
            "byte {offset} of {source:?}"
        );
    }
    let emoji = "😀x";
    let starts = line_starts(emoji);
    assert_eq!(position_at_offset(emoji, &starts, 0).character, 0);
    assert_eq!(position_at_offset(emoji, &starts, 4).character, 2);
}

#[test]
fn a_byte_inside_a_character_clamps_to_the_character_start() {
    let source = "é=1";
    let starts = line_starts(source);
    // Byte 1 is the trailing byte of `é`; LSP's `character` cannot name a
    // mid-character position, so the boundary clamps down to the start.
    assert_eq!(
        position_at_offset(source, &starts, 1),
        Position {
            line: 0,
            character: 0
        }
    );
    // The byte model keeps the byte column: the two do not disagree about
    // where the byte is, only about what LSP can say.
    assert_eq!(line_col(&starts, 1), (1, 2));
}

#[test]
fn a_byte_past_the_end_clamps_to_the_source_length() {
    let source = "ab";
    let starts = line_starts(source);
    assert_eq!(
        position_at_offset(source, &starts, 5),
        Position {
            line: 0,
            character: 2
        }
    );
    // No clamp in the byte model: it is given no source, so it keeps counting.
    assert_eq!(line_col(&starts, 5), (1, 6));
}

#[test]
fn end_of_file_is_a_valid_position() {
    let source = "ab\n";
    let starts = line_starts(source);
    // The newline starts an empty last line, and its first byte is the end.
    assert_eq!(line_col(&starts, source.len() as u32), (2, 1));
    assert_eq!(
        position_at_offset(source, &starts, source.len()),
        Position {
            line: 1,
            character: 0
        }
    );
    assert_eq!(offset_of_span(&starts, (2, 1)), source.len());
}

#[test]
fn an_out_of_range_span_saturates_instead_of_naming_another_line() {
    // A line before the first, or past the last, saturates to the first/last
    // line with its column kept — the answer no longer depends on the source's
    // line count alone.
    assert_eq!(offset_of_span(&[0], (99, 3)), 2);
    assert_eq!(offset_of_span(&[0, 3], (99, 3)), 5);
    assert_eq!(offset_of_span(&[0], (0, 3)), 2);
    assert_eq!(offset_of_span(&[0, 3], (0, 3)), 2);
}

#[test]
fn an_empty_line_start_table_is_a_total_input() {
    let starts: Vec<usize> = Vec::new();
    assert_eq!(line_col(&starts, 0), (1, 1));
    assert_eq!(span_of_offset(&starts, 0), (1, 1));
    assert_eq!(offset_of_span(&starts, (1, 1)), 0);
    assert_eq!(position_at_offset("ab", &starts, 1).line, 0);
}

#[test]
fn an_lsp_position_outside_the_source_has_no_offset() {
    let source = "ab\n";
    let starts = line_starts(source);
    assert_eq!(
        offset_from_position(
            source,
            &starts,
            Position {
                line: 2,
                character: 0
            }
        ),
        None
    );
    assert_eq!(
        offset_from_position(
            source,
            &starts,
            Position {
                line: 1,
                character: 99
            }
        ),
        Some(source.len())
    );
}
