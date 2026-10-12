//! The README's example section is generated from `examples/` and rewritten on drift.

use lichen_tools::readme;
use std::fs;

#[test]
fn readme_embeds_the_current_example_programs() {
    let blob = readme::render_examples().unwrap_or_else(|e| panic!("{e}"));
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
