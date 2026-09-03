//! The example programs in `examples/programs/` are the living spec, and the
//! top-level README embeds them.  This test keeps everything in sync
//! automatically: each example's `output =` metadata entry is rewritten to its
//! actual output (appended when missing), and when the README's embedded
//! section drifts from the files it is rendered from, the README is rewritten
//! in place (exactly what the `sync-readme` binary does), so the suite never
//! fails on a stale README or stale output metadata — a changed example simply
//! resyncs both on the next `cargo test`.

use lichen_language::readme;
use std::fs;

#[test]
fn readme_embeds_the_current_example_programs() {
    if readme::sync_output_comments() {
        eprintln!("example programs: output comments out of sync — rewrote them");
    }
    let blob = readme::render_examples();
    let path = readme::readme_path();
    let content = readme::read_normalized(&path);
    let expected = readme::replace_examples(&content, &blob)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if content != expected {
        fs::write(&path, expected).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        eprintln!(
            "{}: example section out of sync with examples/programs/ — rewrote it",
            path.display()
        );
    }
}
