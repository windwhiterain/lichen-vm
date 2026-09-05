//! The lichen package manager.
//!
//! [`Project`] ties a directory of lichen programs to their per-file
//! `depend "url"` directives ([`preprocess`]), fetches those git dependencies
//! into the lichen-home source cache ([`git`]), and drives the compiler binary
//! to run/build a program.  The package manager **never compiles a program
//! in-process** — it delegates the actual compilation to `lichen-compiler`
//! ([`project`]), which resolve the `depend` directives against the cache the
//! package manager just populated.  It also fetches the toolchain binaries
//! ([`toolchain`]) and rebuilds the compiler for a native plugin ([`plugin`]).
//!
//! The binary is named `lichen` ([`main`]); the language compiler is the
//! renamed `lichen-compiler` in `crates/lichen-language`.

pub mod git;
pub mod plugin;
pub mod preprocess;
pub mod project;
pub mod toolchain;

pub use lichen_language::preprocess::Depend;
pub use project::Project;

/// The repository the core crates and toolchain binaries are fetched from.
/// Overridable per command with a `--repo` flag / config value.
pub const DEFAULT_REPO: &str = crate::toolchain::DEFAULT_REPO;
