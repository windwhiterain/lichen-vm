//! The LSP's handle to a specific artifact-cache slot under Lichen Home.
//!
//! The server persists the *settled imported packages* it compiles through the
//! same device store the `lichen` compiler uses, rooted at that vocabulary's
//! slot (`$LICHEN_HOME/compilers/<plugin-set-key>`, under `~/.lichen`).
//! This small type owns the "the slot must exist" half of the self-heal: hold
//! the root and (re)create it lazily.  The durability and per-artifact
//! recovery live in the store (`PackageStore` / `persist::DeviceRegistry`),
//! which the LSP drives by handing each request's `Doc` a cache root.
//!
//! Self-heal is deliberately minimal (see `docs/notes/liche-lsp-home.md`):
//!
//! - a **missing** home is created lazily (first use, `create_dir_all`);
//! - a **corrupt** home is repaired lazily by the store — `DeviceRegistry`
//!   reloads the last-known (or empty) state from an unparseable `registry` and
//!   repairs it on the next save, and a corrupt/missing artifact falls through to
//!   a recompile.  We never add an extra "reset" layer; the store already heals.
//! - we **never silently fall back to in-memory**.  An in-memory store is used
//!   only when it is *intended*: the program's artifact codec cannot serialize
//!   (`NoPersist`, `P::Codec::PERSISTENT == false`), or no root was supplied
//!   (tests / the `Doc::new_with_base` wrapper).  If a healthy home is not
//!   writable, the store simply keeps compiling from source (writes no-op via the
//!   store's `let _ =`), rather than the editor taking a separate in-memory path.

use std::path::{Path, PathBuf};

/// The server's handle to a specific artifact-cache slot under Lichen Home
/// (`~/.lichen/compilers/<plugin-set-key>/`).
///
/// Every vocabulary caches its settled imported packages under its own slot
/// (shipping = the empty plugin set, a plugin-composed server = its plugin set),
/// so two compilers/servers over different vocabularies never share or reuse
/// each other's artifacts for the same file ID.
pub struct LichenHome {
    dir: PathBuf,
}

impl LichenHome {
    /// Construct for an explicit cache root — the `compilers/<plugin-set-key>`
    /// slot this server's vocabulary caches into.  The LSP only needs the root;
    /// whether to create it — and the persistent store — is driven by the cache
    /// root passed to each `Doc`.
    pub fn at(cache_root: PathBuf) -> LichenHome {
        LichenHome { dir: cache_root }
    }

    /// Create the cache root's `artifacts/` subdir if missing, returning the
    /// resolved root.  This is the "missing home is created lazily" half of the
    /// self-heal: it runs when the server starts (a lichen buffer is opened), not
    /// at install time.  A failure to create (e.g. an unwritable root) is not a
    /// hard error here — the store's own fault-tolerant paths handle writes
    /// degrading silently.
    pub fn ensure(&self) -> &Path {
        let _ = std::fs::create_dir_all(self.dir.join("artifacts"));
        &self.dir
    }

    /// The cache root to hand each request's store — always the persistent root.
    /// `None` is never returned for a healthy home; in-memory is only chosen
    /// inside `Doc` when the codec cannot persist.
    pub fn cache_root(&self) -> &Path {
        &self.dir
    }
}
