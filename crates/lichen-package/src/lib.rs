//! The lichen package manager.
//!
//! [`Project`] ties a directory of lichen programs to a manifest
//! ([`manifest`]), fetches its git dependencies ([`git`]), and stages them
//! onto the preprocessor import path ([`preprocess`], [`project::Project::stage`])
//! before compiling.  It also fetches the toolchain binaries
//! ([`toolchain`]) and rebuilds the compiler for a native plugin
//! ([`plugin`]).
//!
//! The binary is named `lichen` ([`main`]); the language compiler is the
//! renamed `lichen-compiler` in `crates/lichen-language`.

pub mod git;
pub mod manifest;
pub mod plugin;
pub mod preprocess;
pub mod project;
pub mod toolchain;

pub use manifest::{Dependency, Manifest, Package};
pub use project::Project;

/// The repository the core crates and toolchain binaries are fetched from.
/// Overridable per command with a `--repo` flag / config value.
pub const DEFAULT_REPO: &str = crate::toolchain::DEFAULT_REPO;
