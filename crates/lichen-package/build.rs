//! Emit the revision this crate was compiled from so `lichen install` can address
//! the toolchain release by the tag derived from that commit — the commit the
//! `lichen` binary itself was built from.
//!
//! Runs `git rev-parse HEAD` from the package directory (git walks up to find the
//! enclosing checkout). When the crate is built outside a git checkout — e.g. a
//! published package, which cargo strips of `.git` — it emits an empty commit, and
//! `lichen install` refuses because it has no commit to derive a release tag from
//! (it tells the caller to run `lichen update`).

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
