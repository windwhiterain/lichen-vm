//! The LSP's handle to a specific artifact-cache slot under Lichen Home.
//! See `docs/notes/liche-lsp-home.md`.

use std::path::{Path, PathBuf};

/// The server's handle to one artifact-cache slot, `compilers/<plugin-set-key>`.
///
/// # Invariant
/// Two vocabularies must never share a slot, or one reuses the other's
/// artifacts for the same file ID. See `docs/notes/liche-lsp-home.md` §4.
pub struct LichenHome {
    dir: PathBuf,
}

impl LichenHome {
    /// Construct for an explicit `compilers/<plugin-set-key>` cache root.
    ///
    /// # Invariant
    /// `cache_root` is this vocabulary's slot, never a shared one.
    pub fn at(cache_root: PathBuf) -> LichenHome {
        LichenHome { dir: cache_root }
    }

    /// Create the cache root's `artifacts/` subdir if missing; return the root.
    ///
    /// # Invariant
    /// A failed create leaves the root usable: the store degrades to no-persist
    /// writes rather than failing the server start.
    pub fn ensure(&self) -> &Path {
        let _ = std::fs::create_dir_all(self.dir.join("artifacts"));
        &self.dir
    }

    /// The cache root to hand each request's store.
    pub fn cache_root(&self) -> &Path {
        &self.dir
    }
}
