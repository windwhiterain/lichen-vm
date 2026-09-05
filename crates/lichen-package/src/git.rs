//! Git dependency fetching for the package manager.
//!
//! Dependencies are declared per-file as `depend "url"` directives in the
//! `@{…@}` block.  Each is fetched with the `git` CLI (no libgit2 dependency)
//! into a **source cache under the lichen home** (`$LICHEN_HOME` or
//! `~/.lichen`, the same root the compiler's static-module cache uses; see
//! [`lichen_language::persist::lichendir`]).  A missing source is cloned, an
//! existing one is `fetch`ed and checked out to the pinned revision, so
//! `lichen fetch` is idempotent and only pulls what changed.  The source cache
//! lives in `sources/`, a sibling of the compiler's `artifacts/`/`registry`.
//!
//! The cache path and the import alias a dependency resolves to come from
//! [`Depend`] (see [`Depend::sources_dir`], [`Depend::vendored_dir`],
//! [`Depend::alias`]) — the same derivation the compiler uses when it stages
//! the vendored alias, so fetch and run agree by construction.
//!
//! Paths handed to git are normalized to drop the Windows `\\?\` extended-path
//! prefix that `std::fs::canonicalize` produces — git refuses a `\\?\`
//! destination on clone ("could not create work tree dir").

use std::path::{Path, PathBuf};
use std::process::Command;

use lichen_language::persist;
use lichen_language::preprocess::Depend;

/// The cache root for fetched git sources (the lichen home's `sources/`).
pub fn sources_root() -> PathBuf {
    persist::sources_root()
}

/// The alias a [`Depend`] resolves to: its binding name (`name = depend`).
pub fn alias_of(dep: &Depend) -> String {
    dep.alias()
}

/// The Rust crate package a native-plugin [`Depend`] is built under.
pub fn crate_name(dep: &Depend) -> String {
    dep.package.clone().unwrap_or_else(|| alias_of(dep))
}

/// The right-hand side of a `git clone`/checkout: an explicit `rev`, else
/// `branch`, else `tag`; `None` leaves the clone at its default HEAD.
pub fn checkout(dep: &Depend) -> Option<&str> {
    dep.rev
        .as_deref()
        .or(dep.branch.as_deref())
        .or(dep.tag.as_deref())
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

/// Whether the `git` CLI is available.
pub fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Clone or update a dependency into the lichen-home source cache and return
/// its vendored directory.
///
/// A fresh source is cloned (under `sources/`) then checked out to the pinned
/// revision; an existing source is `fetch`ed and the pinned revision is
/// checked out.
pub fn fetch(dep: &Depend) -> Result<PathBuf, String> {
    if !git_available() {
        return Err(
            "the `git` CLI is required to fetch dependencies, but it is not on $PATH".into(),
        );
    }
    let dir = dep.sources_dir();
    let rev = checkout(dep);
    let dir_git = git_path(&dir);
    // The cache root must exist; the clone lands under it.
    let root = sources_root();
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let root_git = git_path(&root);
    if !dir.join(".git").exists() {
        git(&["clone", &dep.url, &dir_git], &root_git)?;
        if let Some(rev) = rev {
            git_in(&dir_git, &["checkout", rev])?;
        }
    } else {
        git_in(&dir_git, &["fetch", "--quiet", "--all", "--tags"])?;
        if let Some(rev) = rev {
            git_in(&dir_git, &["checkout", rev])?;
        }
    }
    Ok(dep.vendored_dir())
}

/// The resolved version (commit hash) of a fetched dependency: the current
/// `HEAD` of its source-cache clone.  A dependency is fetched (or updated)
/// before this is called, so the commit is concrete and stable — which is
/// what the compiler cache keys on, so a dependency's change is a new cache
/// slot.  The source must have been fetched into the cache.
pub fn resolved_version(dep: &Depend) -> Result<String, String> {
    let dir = dep.sources_dir();
    let dir_git = git_path(&dir);
    let out = Command::new("git")
        .args(["-C", &dir_git, "rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("cannot resolve {} version: {e}", dep.alias()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "cannot resolve {} version: {}",
            dep.alias(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
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
