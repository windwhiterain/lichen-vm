//! The example programs in `examples/programs/` are the living spec, and the
//! top-level README embeds them.  This test fails the suite if that embedded
//! section ever drifts from the files it is rendered from.

use lichen_language::readme;

#[test]
fn readme_embeds_the_current_example_programs() {
    let blob = readme::render_examples();
    let path = readme::readme_path();
    let content = readme::read_normalized(&path);
    let expected =
        readme::replace_examples(&content, &blob).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(
        content == expected,
        "{} is out of sync with examples/programs/ — run `cargo run -p lichen-language --bin sync-readme`",
        path.display()
    );
}
