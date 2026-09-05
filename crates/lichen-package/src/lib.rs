//! The lichen package manager.
//!
//! [`Project`] ties a directory of lichen programs to their per-file
//! `name = depend "url"` / `name = plug "url"` directives ([`preprocess`]),
//! fetches those git dependencies into the lichen-home source cache ([`git`]),
//! and drives the compiler binary to run/build a program.  The package manager
//! **never compiles a program in-process** — it delegates the actual
//! compilation to `lichen-compiler` ([`project`]), which resolves the
//! `depend`/`plug` directives against the cache the package manager just
//! populated.  It also fetches the toolchain binaries ([`toolchain`]) and, for
//! a native plugin ([`plugin`]), builds a plugin-composed compiler into the
//! lichen-home compiler cache ([`compiler_cache`]) and drives that.
//!
//! The package manager depends only on the isolated [`lichen_preprocess`]
//! crate for the `@{…@}` block grammar and its `Depend` import-path type (see
//! [`preprocess`]); it never links the language or VM crates.  The binary is
//! named `lichen` ([`main`]); the language compiler is the renamed
//! `lichen-compiler` in `crates/lichen-language`.

pub mod compiler_cache;
pub mod git;
pub mod plugin;
pub mod preprocess;
pub mod project;
pub mod toolchain;

pub use lichen_preprocess::Depend;
pub use project::Project;

/// The repository the core crates and toolchain binaries are fetched from.
/// Overridable per command with a `--repo` flag / config value.
pub const DEFAULT_REPO: &str = crate::toolchain::DEFAULT_REPO;
