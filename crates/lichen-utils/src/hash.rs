//! Hashing shared by the language layer's artifact cache and the package
//! manager's compiler cache key.

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
