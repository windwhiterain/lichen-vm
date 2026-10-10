//! Git dependency fetching into the lichen-home source cache.
//! See docs/notes/package-manager.md.
//!
//! # Invariant
//!
//! Every source-file value git receives — the `depend` URL and the `rev`/`branch`/`tag` revision —
//! is never read as an option: the URL is terminated by `--`, and any value starting with `-` is
//! refused (`D2`). Paths drop the Windows `\\?\` prefix `canonicalize` adds, which git refuses.

use std::path::{Path, PathBuf};
use std::process::Command;

use lichen_preprocess::{Depend, sources_root};

/// The alias a [`Depend`] resolves to: its binding name (`name = depend`).
pub fn alias_of(dep: &Depend) -> String {
    dep.alias()
}

/// The Rust crate package a native-plugin [`Depend`] is built under.
pub fn crate_name(dep: &Depend) -> String {
    dep.package.clone().unwrap_or_else(|| alias_of(dep))
}

/// What a checkout pins to: `rev`, else `branch`, else `tag`; `None` leaves the
/// default HEAD.
pub fn checkout(dep: &Depend) -> Option<&str> {
    dep.rev
        .as_deref()
        .or(dep.branch.as_deref())
        .or(dep.tag.as_deref())
}

/// Refuse a source-file value git would read as an option rather than an operand.
///
/// # Invariant
///
/// No legitimate URL or revision starts with `-`, and a leading `-` turns `--upload-pack=<command>`
/// into command execution or `-f` into a silent `HEAD` checkout (`D2`).
fn reject_option_like(value: &str, directive: &str) -> Result<(), String> {
    if value.starts_with('-') {
        return Err(format!(
            "dependency {directive} '{value}' is invalid: git would read its leading '-' as an option"
        ));
    }
    Ok(())
}

/// A path as git wants it: without the Windows `\\?\` extended-path prefix that
/// canonicalizing adds.
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
    crate::tool_available("git")
}

/// Clone or update a dependency into the source cache and return its vendored
/// directory.
pub fn fetch(dep: &Depend) -> Result<PathBuf, String> {
    // Reject an invalid `sub` before any git command or cache write.
    let vendored = dep.vendored_dir()?;
    // The URL and the revision are source-file text: neither may reach git as an
    // option. This also covers `checkout <rev>`.
    reject_option_like(&dep.url, "url")?;
    let rev = checkout(dep);
    if let Some(rev) = rev {
        let directive = if dep.rev.is_some() {
            "rev"
        } else if dep.branch.is_some() {
            "branch"
        } else {
            "tag"
        };
        reject_option_like(rev, directive)?;
    }
    if !git_available() {
        return Err(
            "the `git` CLI is required to fetch dependencies, but it is not on $PATH".into(),
        );
    }
    let dir = dep.sources_dir();
    let dir_git = git_path(&dir);
    // The cache root must exist; the clone lands under it.
    let root = sources_root();
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let root_git = git_path(&root);
    if !dir.join(".git").exists() {
        git(&["clone", "--", &dep.url, &dir_git], &root_git)?;
        if let Some(rev) = rev {
            git_in(&dir_git, &["checkout", rev])?;
        }
    } else {
        git_in(&dir_git, &["fetch", "--quiet", "--all", "--tags"])?;
        if let Some(rev) = rev {
            git_in(&dir_git, &["checkout", rev])?;
        }
    }
    Ok(vendored)
}

/// A fetched dependency's resolved version: the `HEAD` of its source-cache clone.
///
/// # Invariant
///
/// The dependency is fetched (or updated) before this is called, so the commit is concrete and
/// stable — the compiler cache keys on it, so a dependency's change is a new slot.
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

#[cfg(test)]
#[path = "tests/git_tests.rs"]
mod git_tests;
