//! Temporary: whether a whitespace-only edit reuses the build through `set_view`.
//! See heavy-value-cache.md §9.7.
//!
//! # Invariant
//!
//! The two arms differ only in `base`, so a difference between them is the view path and not
//! the program.

use lichen_language::program::LangProgram;
use lichen_language::session::BufferSession;

const CODE: &str = "cache a = 1 + 2\ncache b = a + 3\nb\n";

fn arm(base: u32) {
    let starts = lichen_language::lex::line_starts(CODE);
    let mut session = BufferSession::<LangProgram>::new(CODE);
    session.set_view(CODE, base, &starts, &[]);
    let first = session.compile();

    let spaced = CODE.replace("cache a", "cache  a");
    session.set_view(&spaced, base, &starts, &[]);
    let whitespace = session.compile();

    let again = session.compile();
    println!(
        "base {base}: first reused {}, whitespace reused {} (key equal {}), same-source reused {}",
        first.reused,
        whitespace.reused,
        first.key == whitespace.key,
        again.reused,
    );
    println!(
        "  whitespace cells {:?}, same-source cells {:?}",
        whitespace.cells, again.cells
    );
}

fn main() {
    arm(0);
    // A preprocessed view's code starts after the `---` block, so its base is not zero.
    arm(7);
}
