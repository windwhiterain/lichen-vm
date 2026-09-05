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
pub type Span = (u32, u32);

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
