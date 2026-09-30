//! The example programs in `examples/` are the living spec, and the
//! top-level README embeds them.  This test keeps the README's embedded
//! section in sync automatically: when the section drifts from the files it
//! is rendered from, the README is rewritten in place (exactly what the
//! `sync-readme` binary does), so the suite never fails on a stale README —
//! a changed example simply resyncs it on the next `cargo test`.
//!
//! The README is *derived documentation*, and rewriting it is the right
//! response to drift.  A program's own `output = "..."` metadata is not: it is
//! a claim about observable behaviour, and drift there is a behaviour change,
//! so `tests/examples.rs` asserts it rather than rewriting it.  That is why
//! [`readme::sync_output_comments`] is deliberately **not** called here — see
//! its documentation for when it is the right tool.

use lichen_language::readme;
use std::fs;

#[test]
fn readme_embeds_the_current_example_programs() {
    let blob = readme::render_examples();
    let path = readme::readme_path();
    let content = readme::read_normalized(&path);
    let expected = readme::replace_examples(&content, &blob)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if content != expected {
        fs::write(&path, expected).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        eprintln!(
            "{}: example section out of sync with examples/ — rewrote it",
            path.display()
        );
    }
}
