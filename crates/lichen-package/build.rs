//! Emit the revision this crate was compiled from so `lichen install` can pin the
//! toolchain binaries (compiler, language-server) to the **same** commit as the
//! `lichen` binary itself.
//!
//! Runs `git rev-parse HEAD` from the package directory (git walks up to find the
//! enclosing checkout). When the crate is built outside a git checkout — e.g. a
//! published package, which cargo strips of `.git` — it emits an empty commit and
//! the pin is skipped, so `lichen install` falls back to the repo's default branch.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();

    println!("cargo:rustc-env=LICHEN_BUILD_COMMIT={commit}");
}
