//! The one byte ↔ line/column model: 1-based `(line, column-in-bytes)`, lines
//! broken by `\n` alone, end of file a valid position, nothing panics.  Every
//! edge here was measured against the other conversions before they were made
//! to delegate to this one.

use lichen_span::{Span, line_col, line_starts, line_text, offset_of_span};

#[test]
fn line_starts_breaks_on_newline_only() {
    assert_eq!(line_starts(""), vec![0]);
    assert_eq!(line_starts("ab"), vec![0]);
    assert_eq!(line_starts("ab\n"), vec![0, 3]);
    // A lone `\r` is content; in `\r\n` the `\r` ends the line's bytes.
    assert_eq!(line_starts("a\rb"), vec![0]);
    assert_eq!(line_starts("a\r\nb"), vec![0, 3]);
}

#[test]
fn end_of_file_is_a_valid_position() {
    // A trailing newline opens the empty last line; its first byte is the end.
    let starts = line_starts("ab\n");
    assert_eq!(line_col(&starts, 3), (2, 1));
    // Without one, the end is a column past the last line's last byte.
    let starts = line_starts("ab");
    assert_eq!(line_col(&starts, 2), (1, 3));
    // An empty source still has line 1.
    let starts = line_starts("");
    assert_eq!(line_col(&starts, 0), (1, 1));
}

#[test]
fn a_byte_inside_a_character_keeps_its_byte_column() {
    // `é` is two bytes: byte 1 is the trailing byte, column 2.
    let starts = line_starts("é=1");
    assert_eq!(line_col(&starts, 1), (1, 2));
    assert_eq!(line_col(&starts, 2), (1, 3));
    // A four-byte character is four ordinary byte columns.
    let starts = line_starts("😀x");
    assert_eq!(line_col(&starts, 2), (1, 3));
}

#[test]
fn a_byte_past_the_end_is_not_clamped() {
    let starts = line_starts("ab");
    assert_eq!(line_col(&starts, 5), (1, 6));
    // The last line absorbs it, at any distance past the end.
    let starts = line_starts("ab\ncd");
    assert_eq!(line_col(&starts, 99), (2, 97));
}

#[test]
fn an_empty_line_start_table_is_a_total_input() {
    // The one panic the old implementation had: `starts[line - 1]` with an
    // empty table.
    assert_eq!(line_col(&[], 0), (1, 1));
    assert_eq!(line_col(&[], 7), (1, 8));
    assert_eq!(offset_of_span(&[], (1, 1)), 0);
    assert_eq!(offset_of_span(&[], (1, 4)), 3);
}

#[test]
fn a_table_that_does_not_begin_at_zero_is_total_too() {
    // Not a table `line_starts` produces, but the conversion must still not
    // underflow on one: a byte before the first recorded start is column 1 of
    // line 1.
    assert_eq!(line_col(&[5], 0), (1, 1));
    assert_eq!(line_col(&[5, 9], 0), (1, 1));
    assert_eq!(line_col(&[5], 5), (1, 1));
}

#[test]
fn offset_of_span_inverts_line_col() {
    for source in ["", "ab", "ab\n", "a\r\nb", "é=1\nx"] {
        let starts = line_starts(source);
        for offset in 0..=(source.len() as u32 + 2) {
            let span = line_col(&starts, offset);
            assert_eq!(
                offset_of_span(&starts, span),
                offset as usize,
                "{source:?} at byte {offset} through {span:?}"
            );
        }
    }
}

#[test]
fn an_out_of_range_span_saturates_on_the_line_and_keeps_the_column() {
    let one = line_starts("ab");
    let two = line_starts("ab\ncd");
    // `(99, 3)` names line 1 + 2 bytes in the one-line source, and line 2 + 2
    // bytes in the two-line one — never "another line's start".
    assert_eq!(offset_of_span(&one, (99, 3)), 2);
    assert_eq!(offset_of_span(&two, (99, 3)), 5);
    // Below the first line saturates to line 1.
    assert_eq!(offset_of_span(&one, (0, 3)), 2);
    assert_eq!(offset_of_span(&two, (0, 3)), 2);
}

#[test]
fn offset_of_span_saturates_the_column_at_zero() {
    // A zero column names the line's first byte, never a byte before it.
    let starts = line_starts("ab\ncd");
    assert_eq!(offset_of_span(&starts, (2, 0)), 3);
}

#[test]
fn line_text_is_the_line_without_its_terminator() {
    let source = "ab\ncd";
    let starts = line_starts(source);
    assert_eq!(line_text(source, &starts, 1), Some("ab"));
    assert_eq!(line_text(source, &starts, 2), Some("cd"));
    // Past the last line there is no text.
    assert_eq!(line_text(source, &starts, 3), None);
    assert_eq!(line_text(source, &starts, 0), None);
}

#[test]
fn line_text_keeps_a_lone_carriage_return_and_drops_a_crlf_terminator() {
    let lone = "a\rb";
    assert_eq!(line_text(lone, &line_starts(lone), 1), Some("a\rb"));
    let crlf = "a\r\nb";
    assert_eq!(line_text(crlf, &line_starts(crlf), 1), Some("a"));
    let crlf = "a\r\nb";
    assert_eq!(line_text(crlf, &line_starts(crlf), 2), Some("b"));
}

#[test]
fn line_text_names_the_valid_empty_line_after_a_trailing_newline() {
    let source = "ab\n";
    let starts = line_starts(source);
    assert_eq!(line_text(source, &starts, 1), Some("ab"));
    // End of file is a position, and this is its (empty) line.
    assert_eq!(line_text(source, &starts, 2), Some(""));
    // An empty source is one empty line, not none.
    assert_eq!(line_text("", &line_starts(""), 1), Some(""));
}

#[test]
fn the_span_type_stays_a_transparent_pair() {
    // The alias is load-bearing: these conversions and every consumer pass the
    // tuple around directly.
    let span: Span = line_col(&line_starts("ab\n"), 3);
    assert_eq!(span, (2, 1));
}
