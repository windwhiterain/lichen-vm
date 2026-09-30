//! The compiler-cache slot key, derived once for the two crates that need it.
//!
//! The package manager builds a compiler into `<lichendir>/compilers/<key>/`
//! and the compiler locates its own artifact-cache root in that same slot;
//! neither crate can see the other's derivation, so it is derived here, in the
//! leaf crate both link — a second derivation is what let the two disagree.

use crate::hash::{hex, sha256};

/// The repository the toolchain's core crates and its released binaries come
/// from by default.  It is also the fixed core-repository identity the shipping
/// compiler slot is keyed by, whichever repository a particular install
/// downloaded its bytes from: the slot names the toolchain a home holds, not
/// the address it arrived from.
pub const DEFAULT_CORE_REPO: &str = "https://github.com/windwhiterain/lichen-vm";

/// The `<lichendir>/compilers/<key>` slot key of a compiler built from
/// `core_repo` over `plugins`.
///
/// The version is **this** crate's `CARGO_PKG_VERSION`.  Every caller links
/// this crate, so it is the one version value both derivations share; a caller
/// substituting its own crate's version reintroduces exactly the coupling this
/// function exists to remove (two crates at `0.1.0` agreeing by coincidence).
///
/// `plugins` are the resolved `name@version` parts; they are sorted here, so
/// the same set in any order keys identically.
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
