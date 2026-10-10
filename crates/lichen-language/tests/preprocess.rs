//! An unfetched `depend`/`plug` is a missing directory, not a source problem, so
//! its diagnostic carries no position.

use lichen_language::package::PackageStore;
use lichen_language::preprocess::stage_depends;
use lichen_language::program::LangProgram;

#[test]
fn an_unfetched_dependency_is_not_reported_at_line_one() {
    // The alias is unique, so the store cannot have staged it.
    let source = "---p5_13_unfetched_probe = depend \"https://example.invalid/probe.git\"---\n42\n";
    let mut store = PackageStore::<LangProgram>::new();
    let diagnostics = stage_depends(&mut store, source);
    let diagnostic = diagnostics
        .first()
        .expect("an unfetched dependency is reported");
    assert!(
        diagnostic.message.contains("p5_13_unfetched_probe")
            && diagnostic.message.contains("lichen fetch"),
        "the missing directory must be named: {}",
        diagnostic.message
    );
    assert!(
        diagnostic.span.is_none(),
        "a missing directory is not a source position: {diagnostic:?}"
    );
    let rendered = lichen_language::render::render(source, diagnostic);
    assert!(
        !rendered.contains("-->") && !rendered.contains('^'),
        "a missing directory renders without a line-1 caret:\n{rendered}"
    );
}

#[test]
fn a_dependency_sub_path_outside_its_clone_is_not_reported_at_line_one() {
    // `sub` is free-form text, rejected as not a relative path inside the clone.
    let source = "---p5_13_sub_probe = depend \"https://example.invalid/probe.git\" \
                  sub = \"../escape\"---\n42\n";
    let mut store = PackageStore::<LangProgram>::new();
    let diagnostics = stage_depends(&mut store, source);
    let diagnostic = diagnostics.first().expect("the bad sub path is reported");
    assert!(
        diagnostic.message.contains("not a relative path"),
        "the bad `sub` must be named: {}",
        diagnostic.message
    );
    assert!(
        diagnostic.span.is_none(),
        "a rejected `sub` path is not a source position: {diagnostic:?}"
    );
}
