//! The LSP's handle to Lichen Home.
//!
//! The server persists the *settled imported packages* it compiles through the
//! same device store the `lichen` compiler uses, rooted at Lichen Home
//! (`$LICHEN_HOME`, else `~/.lichen` — `persist::lichendir`).  This small type
//! owns the "the home must exist" half of the self-heal: resolve the root and
//! (re)create it lazily.  The durability and per-artifact recovery live in the
//! store (`PackageStore` / `persist::DeviceRegistry`), which the LSP drives by
//! handing each request's `Doc` a cache root.
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

use lichen_language::persist::lichendir;

/// The server's handle to the Lichen Home directory.
pub struct LichenHome {
    dir: PathBuf,
}

impl LichenHome {
    /// Resolve the home (`$LICHEN_HOME`, else `~/.lichen`) without creating it.
    /// The LSP server only needs the root; whether to create/home — and the
    /// persistent store — is driven by the cache root passed to each `Doc`.
    pub fn resolve() -> LichenHome {
        LichenHome { dir: lichendir() }
    }

    /// Create the home (and its `artifacts/` subdir) if missing, returning the
    /// resolved root.  This is the "missing home is created lazily" half of the
    /// self-heal: it runs when the server starts (a lichen buffer is opened), not
    /// at install time.  A failure to create (e.g. an unwritable home) is not a
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
