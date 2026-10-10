//! The compiler-cache slot key, derived once here because the package manager
//! and the compiler both key on it.

use crate::hash::{hex, sha256};

/// The repository the toolchain's core crates come from, and the identity the
/// shipping compiler slot is keyed by.
pub const DEFAULT_CORE_REPO: &str = "https://github.com/windwhiterain/lichen-vm";

/// The `<lichendir>/compilers/<key>` slot key of a compiler built from
/// `core_repo` over `plugins`.
///
/// # Invariant
///
/// The version is this crate's `CARGO_PKG_VERSION`, the one value every caller
/// shares; `plugins` is sorted, so the same set in any order keys identically.
pub fn compiler_slot_key(core_repo: &str, plugins: &[String]) -> String {
    let mut parts: Vec<&str> = plugins.iter().map(String::as_str).collect();
    parts.sort_unstable();
    let mut spec = format!("lichen-utils={}", env!("CARGO_PKG_VERSION"));
    spec.push_str("&core_repo=");
    spec.push_str(core_repo);
    for part in parts {
        spec.push('&');
        spec.push_str(part);
    }
    hex(&sha256(spec.as_bytes()))
}
