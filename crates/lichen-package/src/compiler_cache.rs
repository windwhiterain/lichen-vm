//! The compiler cache under the lichen home.
//!
//! A native plugin extends the compiler's vocabulary, so running a program
//! that uses one needs a compiler built over that plugin set.  Rather than
//! building into the project, the package manager builds that compiler into a
//! **cache under the lichen home** (`<lichendir>/compilers/<key>/`), keyed by
//! the lichen-library version and every plugin's *resolved* version (the
//! commit `HEAD` in the fetched source cache).  The same plugin set + library
//! version reuses the cached binary; a change to any plugin (or to the
//! library) keys a new slot.
//!
//! `ensure` builds on a miss and returns the cached binary on a hit, so each
//! `lichen run`/`build` can gather the project's native plugins and drive the
//! matching compiler — the plugin-built compiler actually takes effect because
//! it is the one the package manager spawns (delegation, not in-process
//! linking, see `crate::main`).

use std::path::PathBuf;

use lichen_language::persist;
use lichen_language::persist::lichendir;
use lichen_language::preprocess::Depend;

use crate::git;
use crate::plugin::{self, Leaves};

/// The cache subdir name, under the lichen home.
pub const COMPILERS_DIR: &str = "compilers";

/// The cache root: `<lichendir>/compilers`.
pub fn root() -> PathBuf {
    lichendir().join(COMPILERS_DIR)
}

/// The compiler name a cached build is produced under (the `name` baked into
/// the `lichen-compiler-<name>` binary; the cache key already separates plugin
/// sets, so a fixed name is unambiguous in each slot).
pub const COMPILER_NAME: &str = "project";

/// The cache key for a plugin set: a stable hash of the lichen-library version
/// and every plugin's (name, resolved version), sorted so the same set in any
/// order keys identically.  Each plugin must already be fetched so its source
/// cache `HEAD` is resolvable.
pub fn key(plugins: &[Depend]) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    for dep in plugins {
        let version = git::resolved_version(dep)?;
        parts.push(format!("{}@{version}", dep.name));
    }
    parts.sort();
    let mut spec = format!("liche-language={}", lichen_language::VERSION);
    for part in &parts {
        spec.push('&');
        spec.push_str(part);
    }
    Ok(persist::hex(&persist::sha256(spec.as_bytes())))
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

/// Ensure a compiler built over `plugins` (with `leaves`) is cached and return
/// its binary path.  On a cache hit it is reused; on a miss it is built (a
/// `cargo build` of a generated crate) into the lichen-home cache slot.
///
/// Each plugin must already be fetched (its source-cache `HEAD` is read for
/// the cache key).  `core_repo` is the repository (or local checkout path) the
/// core crates and toolchain come from.
pub fn ensure(core_repo: &str, plugins: &[Depend], leaves: &Leaves) -> Result<PathBuf, String> {
    let key = key(plugins).map_err(|e| format!("cannot key the compiler cache: {e}"))?;
    if let Some(bin) = resolve(&key) {
        return Ok(bin);
    }
    let dir = dir(&key);
    let build = plugin::rebuild(&dir, COMPILER_NAME, core_repo, plugins, leaves)?;
    Ok(build.bin)
}
