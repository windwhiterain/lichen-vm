//! Git dependency fetching for the package manager.
//!
//! Dependencies are fetched with the `git` CLI (no libgit2 dependency) into a
//! per-project cache under `<project>/.lichen/deps/`.  `fetch` clones a
//! missing repository and otherwise `fetch`es + checks out the recorded
//! revision, so `lichen fetch` is idempotent and only pulls what changed.
//!
//! Paths handed to git are normalized to drop the Windows `\\?\` extended-path
//! prefix that [`std::fs::canonicalize`] produces — git refuses a `\\?\`
//! destination on clone ("could not create work tree dir").

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::manifest::{Dependency, PROJECT_DIR};

/// The vendored-deps directory name, under the project's `.lichen` dir.
pub const DEPS_DIR: &str = "deps";

/// The directory a dependency is vendored into.  The name is the dependency
/// alias, sanitized to a safe directory name.
pub fn dep_dir(project_dir: &Path, alias: &str) -> PathBuf {
    project_dir
        .join(PROJECT_DIR)
        .join(DEPS_DIR)
        .join(sanitize(alias))
}

/// A path as git wants it: without the `\\?\` (and `\\?\UNC\`) extended-path
/// prefix that canonicalized Windows paths carry.
fn git_path(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    }
}

/// Sanitize an alias into a filesystem-safe directory name.
fn sanitize(alias: &str) -> String {
    let mut out = String::new();
    for ch in alias.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => out.push(ch),
            _ => out.push('_'),
        }
    }
    if out.is_empty() {
        out.push_str("dep");
    }
    out
}

/// Whether the `git` CLI is available.
pub fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Clone or update `dep` into `<project_dir>/deps/<alias>`, checking out the
/// recorded revision, and return the vendored directory.
///
/// A fresh clone is followed by a checkout of the requested rev (a rev,
/// branch, or tag); an existing clone is `fetch`ed and the requested rev is
/// checked out.  When no rev is recorded, the clone is left at its default
/// HEAD.
pub fn fetch(project_dir: &Path, alias: &str, dep: &Dependency) -> Result<PathBuf, String> {
    if !git_available() {
        return Err(
            "the `git` CLI is required to fetch dependencies, but it is not on $PATH".into(),
        );
    }
    let dir = dep_dir(project_dir, alias);
    let rev = dep.checkout();
    // The git-visible forms of the paths (no `\\?\` prefix, forward slashes).
    let dir_git = git_path(&dir);
    let cwd_git = git_path(project_dir);
    // The clone target's parent must exist; create the `.lichen/deps` dir.
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    if !dir.join(".git").exists() {
        git(&["clone", &dep.git, &dir_git], &cwd_git)?;
        if let Some(rev) = rev {
            git_in(&dir_git, &["checkout", rev])?;
        }
    } else {
        git_in(&dir_git, &["fetch", "--quiet", "--all", "--tags"])?;
        if let Some(rev) = rev {
            git_in(&dir_git, &["checkout", rev])?;
        }
    }
    Ok(dir)
}

/// Run git with cwd `cwd`, returning the command's stderr on failure.
fn git(args: &[&str], cwd: &str) -> Result<(), String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Run git inside an existing cloned directory (given in git-visible form).
fn git_in(dir: &str, args: &[&str]) -> Result<(), String> {
    git(args, dir)
}
