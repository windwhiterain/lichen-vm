//! The source-position protocol the frontend crates share: a 1-based
//! `(line, column-in-bytes)` span and the math on it.

/// A source span: 1-based `(line, column)`.
///
/// # Invariant
///
/// `column` counts **bytes** from the line's start, not characters and not
/// UTF-16 code units.  [`line_starts`], [`line_col`] and [`offset_of_span`] are
/// that model; see docs/notes/frontend-syntax-separation.md.
pub type Span = (u32, u32);

/// Byte offsets at which each line starts, line 1 at byte 0.  The result is
/// never empty.
///
/// # Invariant
///
/// Lines are broken by `\n` alone: a lone `\r` is ordinary line content, so in
/// a `\r\n` file the `\r` is the last byte of its line.  `source.len()` appears
/// as a start exactly when the source ends in `\n`, where it names the valid
/// empty line after the last newline.
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
/// # Invariant
///
/// Nothing here panics:
///
/// - A `pos` past the last recorded start stays on the last line, counting.
/// - A table not beginning at byte 0 saturates the column, never underflows.
/// - For a source ending in `\n`, `source.len()` is the empty line after it at
///   column 1; otherwise one past the last line.
/// - The column stays byte-exact, so a byte inside a character is ordinary.
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

/// The byte offset of a 1-based `(line, column-in-bytes)` span, the inverse of
/// [`line_col`].
///
/// # Invariant
///
/// An out-of-range span saturates on the line and keeps the column: a `line`
/// below 1 is line 1 and a `line` past the last is the last line.  The answer
/// never names a different line's start.
pub fn offset_of_span(starts: &[usize], span: Span) -> usize {
    let last = starts.len().max(1);
    let line = (span.0 as usize).clamp(1, last);
    let start = starts.get(line - 1).copied().unwrap_or(0);
    start + (span.1 as usize).saturating_sub(1)
}

/// The text of 1-based `line` without its terminator (`\n` or `\r\n`); `None`
/// when `line` is not a line of `source`.
pub fn line_text<'s>(source: &'s str, starts: &[usize], line: u32) -> Option<&'s str> {
    let index = (line as usize).checked_sub(1)?;
    let start = *starts.get(index)?;
    let end = starts.get(index + 1).copied().unwrap_or(source.len());
    let text = source.get(start..end)?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    Some(text.strip_suffix('\r').unwrap_or(text))
}
