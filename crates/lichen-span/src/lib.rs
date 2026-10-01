//! The source-position protocol shared by the frontend crates.
//!
//! [`Span`] is the one source-position type every frontend crate agrees on —
//! the lexer, the parser, the language layer, and the preprocessor.  It lives
//! in this tiny dependency-free crate so a crate that only needs to *name* a
//! source position (or convert a byte offset to one) does not have to pull in
//! the whole lexer or the language crate.  The lexer is the one thing that
//! turns raw bytes into a source position, so it is the *producer* of the
//! type; this crate is its shared home.
//!
//! [`Span`] stays a **transparent alias** — a tuple, not a newtype.  That is
//! deliberate: it is cheap to copy, trivially comparable, and usable directly
//! wherever the source→span math lives, with no nominal break between the
//! crates that consume it.

/// A source span: 1-based `(line, column)`.
///
/// `column` counts **bytes** from the line's start — not characters and not
/// UTF-16 code units, both 1-based.  [`line_starts`], [`line_col`] and
/// [`offset_of_span`] are that model, and this crate is its only
/// implementation: the language server converts the byte column to LSP's
/// 0-based UTF-16 `character` at its own boundary, never by re-deriving the
/// line.
pub type Span = (u32, u32);

/// Byte offsets at which each line starts (line 1 begins at 0).
///
/// Lines are broken by `\n` alone: a lone `\r` is ordinary line content, so in
/// a `\r\n` file the `\r` is the last byte of its line.  The result is never
/// empty, and `source.len()` appears as a start exactly when the source ends in
/// `\n` — there it names the valid empty line after the last newline.
pub fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Map a byte offset to its 1-based `(line, column-in-bytes)`.
///
/// Contract:
///
/// - **Total.**  A `pos` past the last recorded start belongs to the last line
///   and keeps counting the column; an empty `starts` is read as "line 1 starts
///   at byte 0", and a table that does not begin at byte 0 saturates the column
///   rather than underflowing.  Nothing here panics, for any table.
/// - **End of file is a valid position.**  For a source ending in `\n`, the
///   offset `source.len()` names the empty line after it at column 1; otherwise
///   it is one byte past the last line's end.
/// - **No character clamping.**  `pos` is a byte offset and the column is
///   byte-exact, so a byte inside a multi-byte character has an ordinary
///   column.  LSP's `character` cannot name that, so the language server clamps
///   to the character start at that boundary.
pub fn line_col(starts: &[usize], pos: u32) -> Span {
    let pos = pos as usize;
    // `Ok(i)` is exactly a line start; `Err(i)` is a byte inside line `i`.
    let line = match starts.binary_search(&pos) {
        Ok(i) => i + 1,
        Err(i) => i.max(1),
    };
    let start = starts.get(line - 1).copied().unwrap_or(0);
    (line as u32, (pos.saturating_sub(start) + 1) as u32)
}

/// The byte offset of a 1-based `(line, column-in-bytes)` span — the inverse of
/// [`line_col`].
///
/// The out-of-range answer is **saturation on the line, the column kept**: a
/// `line` below 1 is line 1, a `line` past the last is the last line, and an
/// empty `starts` is line 1 at byte 0.  So the answer never silently names a
/// different line's start, and a span [`line_col`] produced maps back to the
/// offset it came from.
pub fn offset_of_span(starts: &[usize], span: Span) -> usize {
    let last = starts.len().max(1);
    let line = (span.0 as usize).clamp(1, last);
    let start = starts.get(line - 1).copied().unwrap_or(0);
    start + (span.1 as usize).saturating_sub(1)
}

/// The text of 1-based `line` without its line terminator (`\n`, or the `\r\n`
/// pair) — the display view of the model [`line_starts`] defines.  `None` when
/// `line` is not a line of `source`.
pub fn line_text<'s>(source: &'s str, starts: &[usize], line: u32) -> Option<&'s str> {
    let index = (line as usize).checked_sub(1)?;
    let start = *starts.get(index)?;
    let end = starts.get(index + 1).copied().unwrap_or(source.len());
    let text = source.get(start..end)?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    Some(text.strip_suffix('\r').unwrap_or(text))
}
