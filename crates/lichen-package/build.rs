//! Emits `LICHEN_BUILD_COMMIT`, the revision `lichen install` addresses a release by.
//!
//! # Invariant
//!
//! Empty when the crate is built outside a git checkout — `lichen install` then refuses.
//! Each `rerun-if-changed` path exists: `<checkout>/.git/HEAD` is a file in a worktree, and a
//! missing path re-runs this script on every build.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        // `HEAD` moves on a branch switch or a detached checkout.
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
    }
    if let Some(branch_refs) = git(&["rev-parse", "--git-path", "refs/heads"]) {
        // A packed ref moves by writing a loose file here; `--git-path` points
        // at the worktree's real git dir.
        println!("cargo:rerun-if-changed={branch_refs}");
    }

    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=LICHEN_BUILD_COMMIT={commit}");
}

/// `git <args>` run from the package root, trimmed; `None` when git is missing
/// or the command failed.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
