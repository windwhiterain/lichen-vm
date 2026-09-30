//! The generated `Cargo.toml` is a TOML document, not a concatenated string.
//!
//! Every value in it comes from a plugin's own manifest or from a `--repo`
//! argument: the preprocessor's string lexer allows a newline inside a string
//! (see `lichen-preprocess`'s lexer tests), and a `--repo` may be a Windows
//! directory path, so a value must never be able to end the string — or the
//! line — it is written into.

use super::{Depend, server_dep, write_cargo_toml};

/// A plugin whose package name, URL, and pinned revision carry the characters
/// that used to break the generated manifest: a newline (which the
/// preprocessor's string lexer permits) and an injected `[package]`/key.
fn hostile_plugin() -> Depend {
    Depend {
        url: "https://example.com/plug\n[package]".into(),
        name: "plug".into(),
        rev: Some("dead\nbeef".into()),
        branch: None,
        tag: None,
        package: Some("plug\nbad = 1".into()),
        sub: None,
        plugin: true,
    }
}

/// A `--repo` that is a Windows directory path: `\w` is not a TOML escape, so
/// an unescaped backslash makes the whole document unparseable.
const WINDOWS_CORE_REPO: &str = r"C:\work\lichen-vm";

fn write_and_read(tag: &str, extra_deps: &str) -> String {
    let dir = std::env::temp_dir().join(format!("lichen-manifest-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_cargo_toml(
        &dir,
        "lichen-compiler-probe",
        WINDOWS_CORE_REPO,
        &[hostile_plugin()],
        extra_deps,
    )
    .expect("write the generated manifest");
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).expect("read it back");
    let _ = std::fs::remove_dir_all(&dir);
    manifest
}

#[test]
fn a_manifest_value_that_carries_a_newline_or_a_backslash_still_parses() {
    let manifest = write_and_read("compiler", "");
    let parsed: toml::Value = toml::from_str(&manifest)
        .unwrap_or_else(|e| panic!("the generated manifest must parse as TOML: {e}\n{manifest}"));
    assert!(
        parsed["dependencies"].get("lichen-language").is_some(),
        "the core dependencies must survive:\n{manifest}"
    );
    assert!(
        parsed["dependencies"].get("bad").is_none(),
        "an injected key must stay inside the string it came from:\n{manifest}"
    );
}

#[test]
fn the_language_server_dependency_line_parses_too() {
    // `rebuild_lsp` passes exactly this fragment, which is the second
    // interpolation site inside `write_cargo_toml`.
    let extra = format!("\n{}", server_dep(WINDOWS_CORE_REPO));
    let manifest = write_and_read("server", &extra);
    let parsed: toml::Value = toml::from_str(&manifest)
        .unwrap_or_else(|e| panic!("the generated manifest must parse as TOML: {e}\n{manifest}"));
    assert!(
        parsed["dependencies"]
            .get("lichen-language-server")
            .is_some(),
        "the language-server dependency must survive:\n{manifest}"
    );
}
