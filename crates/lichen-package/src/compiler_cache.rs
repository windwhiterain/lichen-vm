//! The compiler cache under the lichen home.
//! See docs/notes/package-manager.md.
//!
//! # Invariant
//!
//! A slot key covers the toolchain version, the core repository, and every plugin's resolved
//! `HEAD`; the derivation lives once in `lichen_utils::cache::compiler_slot_key`, because the
//! compiler that reads the slot derives it too and the two crates cannot see each other.

use std::path::PathBuf;

use lichen_preprocess::{Depend, lichendir};

use crate::git;
use crate::plugin::{self, Leaves};

/// The cache subdir name, under the lichen home.
pub const COMPILERS_DIR: &str = "compilers";

/// The cache root: `<lichendir>/compilers`.
pub fn root() -> PathBuf {
    lichendir().join(COMPILERS_DIR)
}

/// The name a cached compiler is produced under; the slot key separates the
/// plugin sets, so a fixed name is unambiguous.
pub const COMPILER_NAME: &str = "project";

/// The name a cached language server is produced under.
pub const LSP_NAME: &str = "project";

/// The cache key for a plugin set: the toolchain version, the core repository and
/// each plugin's `(name, version)`.
///
/// # Invariant
///
/// Each plugin is already fetched, so its resolved `HEAD` is readable; the parts are sorted, so
/// the same set in any order keys one slot.
pub fn key(core_repo: &str, plugins: &[Depend]) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    for dep in plugins {
        let version = git::resolved_version(dep)?;
        parts.push(format!("{}@{version}", dep.name));
    }
    // `lichen_utils::cache::compiler_slot_key` derives it for both sides, on
    // `lichen-utils`'s own version plus `core_repo`.
    Ok(lichen_utils::cache::compiler_slot_key(core_repo, &parts))
}

/// The cache slot directory for `key`.
fn dir(key: &str) -> PathBuf {
    root().join(key)
}

/// The path of a cached compiler binary for `key`, when it has been built.
fn resolve(key: &str) -> Option<PathBuf> {
    let bin = dir(key)
        .join("target")
        .join("release")
        .join(plugin::bin_name(COMPILER_NAME));
    if bin.is_file() { Some(bin) } else { None }
}

/// The path of a cached language-server binary for `key`, when it has been built.
fn resolve_lsp(key: &str) -> Option<PathBuf> {
    let bin = dir(key)
        .join("target")
        .join("release")
        .join(plugin::server_bin_name(LSP_NAME));
    if bin.is_file() { Some(bin) } else { None }
}

/// Ensure a compiler built over `plugins` and `leaves` is cached, returning its path.
///
/// # Invariant
///
/// Each plugin is already fetched (its source-cache `HEAD` keys the cache), and `core_repo` is
/// the repository (or local checkout) the core crates and toolchain come from.
pub fn ensure(core_repo: &str, plugins: &[Depend], leaves: &Leaves) -> Result<PathBuf, String> {
    let key = key(core_repo, plugins).map_err(|e| format!("cannot key the compiler cache: {e}"))?;
    if let Some(bin) = resolve(&key) {
        return Ok(bin);
    }
    let dir = dir(&key);
    let build = plugin::rebuild(&dir, COMPILER_NAME, core_repo, plugins, leaves)?;
    Ok(build.bin)
}

/// Ensure a language server composed over `plugins` and `leaves` is cached, returning
/// its path.
///
/// # Invariant
///
/// It shares the compiler's slot (`core_repo` and the plugin versions key both); only a non-empty
/// plugin set needs a composed server, the empty one resolving to the shipping server.
pub fn ensure_lsp(core_repo: &str, plugins: &[Depend], leaves: &Leaves) -> Result<PathBuf, String> {
    let key = key(core_repo, plugins).map_err(|e| format!("cannot key the LSP cache: {e}"))?;
    if let Some(bin) = resolve_lsp(&key) {
        return Ok(bin);
    }
    let dir = dir(&key);
    let build = plugin::rebuild_lsp(&dir, LSP_NAME, core_repo, plugins, leaves)?;
    Ok(build.bin)
}
