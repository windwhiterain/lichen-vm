//! The lichen package manager: it drives the compiler binary for a project.
//! See docs/notes/package-manager.md.

pub mod compiler_cache;
pub mod git;
pub mod plugin;
pub mod preprocess;
pub mod project;
pub mod toolchain;

pub use lichen_preprocess::Depend;
pub use project::Project;

/// The repository the core crates and toolchain binaries are fetched from;
/// overridable per command with `--repo`.
pub const DEFAULT_REPO: &str = crate::toolchain::DEFAULT_REPO;

/// Whether `tool` answers `--version` — the one probe every external-tool check
/// goes through.
pub(crate) fn tool_available(tool: &str) -> bool {
    std::process::Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}
