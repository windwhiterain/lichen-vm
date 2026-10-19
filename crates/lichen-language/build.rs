//! Stamp the build revision the `link` loader gates on.
//!
//! # Invariant
//! A library's entries use this build's `Module` and value layouts, so the tag a
//! generated library carries must name the tree that compiled both. An external
//! `LICHEN_BUILD_REV` (a release build) wins; a local build falls back to the
//! checkout's `git describe`, dirty state included
//! (docs/notes/native-link-library.md §12).

use std::process::Command;

fn main() {
    let revision = std::env::var("LICHEN_BUILD_REV")
        .ok()
        .filter(|revision| !revision.is_empty())
        .or_else(|| {
            let output = Command::new("git")
                .args(["describe", "--always", "--dirty", "--abbrev=12"])
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let text = String::from_utf8(output.stdout).ok()?;
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        });
    if let Some(revision) = revision {
        println!("cargo:rustc-env=LICHEN_BUILD_REV={revision}");
    }
}
