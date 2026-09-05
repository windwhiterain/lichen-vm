//! Shared hashing helpers: SHA-256 and its hex encoding.
//!
//! These are used by the language layer's artifact cache (the device store)
//! and by the package manager's compiler cache key — two crates that must not
//! depend on each other — so they live here, in the leaf utility crate.

use sha2::Digest as _;

/// The content hash of an artifact — 32 bytes of SHA-256.
pub type Hash = [u8; 32];

/// SHA-256 over `bytes`.
pub fn sha256(bytes: &[u8]) -> Hash {
    use sha2::Sha256;
    Sha256::digest(bytes).into()
}

/// The hex encoding of a hash — the artifact file name.
pub fn hex(hash: &Hash) -> String {
    let mut out = String::with_capacity(64);
    for byte in hash {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}
