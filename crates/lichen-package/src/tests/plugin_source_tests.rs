//! The generated `src/main.rs` is Rust source, not a concatenation of a
//! dependency's strings.
//!
//! A plugin's `package` and binding name come from a source file's
//! `depend`/`plug` declaration (or from a host that composes a [`Depend`]
//! directly), and the preprocessor's string lexer is `"[^"@]*"` — documented
//! "no escapes, may be multiline" (`lichen-preprocess`'s lexer) — so a `}` or
//! a newline in either used to close the generated item and inject Rust source
//! that `cargo build` then compiles and runs.

use super::{Depend, Leaves, write_compiler_main_rs};

/// A plugin whose `package` carries a closing brace and a newline: as a Rust
/// crate identifier it closed the generated item and injected a top-level
/// function beside it.
fn injected_package_plugin() -> Depend {
    Depend {
        url: "https://example.com/plug".into(),
        name: "plug".into(),
        rev: None,
        branch: None,
        tag: None,
        package: Some("x;\n}\nfn injected_by_a_package_name() {}\n//".into()),
        sub: None,
        plugin: true,
    }
}

/// A plugin whose binding name carries a `"`: interpolated into the generated
/// `"<alias>.lichen"` literal it ended the literal and injected code after it.
fn injected_alias_plugin() -> Depend {
    Depend {
        name: "evil\"); fn injected_by_an_alias() {} //".into(),
        package: Some("plug".into()),
        ..injected_package_plugin()
    }
}

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("lichen-plugin-source-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

#[test]
fn a_package_name_that_is_not_a_rust_identifier_is_refused() {
    let dir = workspace("injected-package");
    let outcome = write_compiler_main_rs(&dir, &[injected_package_plugin()], &Leaves::shipping());
    let generated = std::fs::read_to_string(dir.join("src/main.rs")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        outcome.is_err(),
        "a `package` that is not a Rust identifier must be refused, not written as source:\n{generated}"
    );
    assert!(
        !generated.contains("injected_by_a_package_name"),
        "the injected item must not reach the generated source:\n{generated}"
    );
    assert!(
        outcome.unwrap_err().contains("package"),
        "the refusal must name the package it refused"
    );
}

#[test]
fn an_alias_that_cannot_be_spelled_in_a_literal_is_refused() {
    let dir = workspace("injected-alias");
    let outcome = write_compiler_main_rs(&dir, &[injected_alias_plugin()], &Leaves::shipping());
    let generated = std::fs::read_to_string(dir.join("src/main.rs")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        outcome.is_err(),
        "an alias that is not a name must be refused, not written into a literal:\n{generated}"
    );
    assert!(
        !generated.contains("injected_by_an_alias"),
        "the injected item must not reach the generated source:\n{generated}"
    );
}

#[test]
fn a_well_formed_plugin_still_generates_its_identifiers() {
    let dep = Depend {
        name: "std".into(),
        package: Some("lichen-std-native".into()),
        ..injected_package_plugin()
    };
    let dir = workspace("well-formed");
    write_compiler_main_rs(&dir, &[dep], &Leaves::shipping())
        .expect("a well-formed plugin must generate source");
    let generated = std::fs::read_to_string(dir.join("src/main.rs")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        generated.contains("    lichen_std_native as lichen_std_native_leaves;\n"),
        "the hyphenated package must be spelled as the Rust identifier:\n{generated}"
    );
    assert!(
        generated.contains("(\"std.lichen\", lichen_std_native::WRAPPER_SOURCE,"),
        "the plugin's alias and wrapper must be registered:\n{generated}"
    );
}
