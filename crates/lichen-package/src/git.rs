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
//! Paths handed to git are normalized to drop the Windows `\\?\` extended-path
//! prefix that `std::fs::canonicalize` produces — git refuses a `\\?\`
//! destination on clone ("could not create work tree dir").

use std::path::{Path, PathBuf};
use std::process::Command;

use lichen_language::persist::lichendir;
use lichen_language::preprocess::Depend;

/// The source-cache subdir name, under the lichen home.
pub const SOURCES_DIR: &str = "sources";

/// The cache root for fetched git sources (the lichen home's `sources/`).
pub fn sources_root() -> PathBuf {
    lichendir().join(SOURCES_DIR)
}

/// The directory a dependency's source is cached in, keyed by its alias.
pub fn dep_dir(alias: &str) -> PathBuf {
    sources_root().join(sanitize(alias))
}

/// The alias a [`Depend`] resolves to: `as NAME`, else the URL's repo name.
pub fn alias_of(dep: &Depend) -> String {
    dep.name.clone().unwrap_or_else(|| repo_name(&dep.url))
}

/// The repository's last path component, sans `.git`.
fn repo_name(url: &str) -> String {
    let name = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("dep");
    name.strip_suffix(".git").unwrap_or(name).to_string()
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
    let dir = dep_dir(&alias_of(dep));
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
    pkg_dir(&dir, dep)
}

/// The package directory a [`Depend`] resolves to: the fetched clone's root,
/// or its `sub` subdirectory (a monorepo source).
fn pkg_dir(clone: &Path, dep: &Depend) -> Result<PathBuf, String> {
    let Some(sub) = &dep.sub else {
        return Ok(clone.to_path_buf());
    };
    let subdir = clone.join(sub);
    if !subdir.is_dir() {
        return Err(format!(
            "dependency '{}' has no subdirectory '{}' in the fetched source",
            alias_of(dep),
            sub
        ));
    }
    Ok(subdir)
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
