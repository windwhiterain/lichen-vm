//! The workspace's test layout: a crate carries one external test binary, not one per area.
//!
//! # Invariant
//! Cargo links a test binary per `tests/*.rs` and per `tests/<dir>/main.rs`, so an area
//! per file is a binary per area. The rule is at most one external binary per crate,
//! beside the lib's own inline one. The census is `cargo metadata --no-deps`, because it
//! names the targets Cargo will build — the authority — not the working tree's shape.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

/// The workspace root: the nearest ancestor whose manifest declares `[workspace]`.
fn workspace_root() -> PathBuf {
    let mut directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let manifest = directory.join("Cargo.toml");
        if manifest.is_file()
            && std::fs::read_to_string(&manifest).is_ok_and(|text| text.contains("[workspace]"))
        {
            return directory;
        }
        assert!(
            directory.pop(),
            "an ancestor of the manifest holds the workspace"
        );
    }
}

/// Every package in the workspace, with the external test binaries it carries.
fn external_test_binaries() -> Vec<(String, Vec<String>)> {
    let metadata = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(workspace_root())
        .output()
        .expect("cargo metadata runs");
    assert!(
        metadata.status.success(),
        "cargo metadata succeeds: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let value: Value = serde_json::from_slice(&metadata.stdout).expect("cargo metadata is json");
    value["packages"]
        .as_array()
        .expect("the metadata names its packages")
        .iter()
        .map(|package| {
            let name = package["name"]
                .as_str()
                .expect("a package names itself")
                .to_owned();
            let binaries = package["targets"]
                .as_array()
                .expect("a package names its targets")
                .iter()
                .filter(|target| {
                    target["kind"]
                        .as_array()
                        .is_some_and(|kinds| kinds.iter().any(|kind| kind == "test"))
                })
                .map(|target| {
                    target["name"]
                        .as_str()
                        .expect("a target names itself")
                        .to_owned()
                })
                .collect();
            (name, binaries)
        })
        .collect()
}

#[test]
fn a_crate_carries_at_most_one_external_test_binary() {
    let offenders: Vec<String> = external_test_binaries()
        .into_iter()
        .filter(|(_, binaries)| binaries.len() > 1)
        .map(|(name, binaries)| {
            format!(
                "{name} carries {} external test binaries: {binaries:?}",
                binaries.len()
            )
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "a crate carries at most one external test binary, beside the lib's own inline one \
         (docs/notes/test-binary-layout.md).\n{}",
        offenders.join("\n")
    );
}
