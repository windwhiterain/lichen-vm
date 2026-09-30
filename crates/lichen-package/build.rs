//! Emit the revision this crate was compiled from so `lichen install` can address
//! the toolchain release by the tag derived from that commit — the commit the
//! `lichen` binary itself was built from.
//!
//! Runs `git rev-parse HEAD` from the package directory (git walks up to find the
//! enclosing checkout). When the crate is built outside a git checkout — e.g. a
//! published package, which cargo strips of `.git` — it emits an empty commit, and
//! `lichen install` refuses because it has no commit to derive a release tag from
//! (it tells the caller to run `lichen update`).
//!
//! The rebuild trigger names the **resolved** git directory and the directory
//! holding the branch refs, never `<checkout>/.git/HEAD`.  In a git worktree
//! `.git` is a *file* holding a `gitdir:` pointer, so that literal path does not
//! exist — and cargo re-runs a build script on **every** build when one of its
//! `rerun-if-changed` paths is missing, so the named path costs a recompile per
//! build and still never observes the ref that moves.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        // `HEAD` moves on a branch switch or a detached checkout.
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
    }
    if let Some(branch_refs) = git(&["rev-parse", "--git-path", "refs/heads"]) {
        // The ref behind `HEAD` moves on a commit, rebase or reset.  A commit
        // on a **packed** ref writes a loose file under this directory and
        // leaves `packed-refs` untouched, so the directory — which cargo scans
        // recursively — is what catches it.  `--git-path` relocates it into the
        // main checkout's git directory for a worktree, where the refs live.
        println!("cargo:rerun-if-changed={branch_refs}");
    }

    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=LICHEN_BUILD_COMMIT={commit}");
}

/// `git <args>` run from the build script's working directory — the package
/// root, which is also what a relative `rerun-if-changed` path is resolved
/// against — trimmed, or `None` when git is unavailable or the command failed.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
