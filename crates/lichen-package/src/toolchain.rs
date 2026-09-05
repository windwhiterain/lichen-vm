//! Toolchain binaries the package manager fetches: the compiler
//! (`lichen-compiler`) and the language server (`lichen-language-server`).
//!
//! Both are distributed via Cargo ([`cargo install`](https://doc.rust-lang.org/cargo/commands/cargo-install.html)),
//! so "download" here means "fetch from the repository and build".  The
//! package manager drives `cargo install` against the configured repository
//! (or a local checkout) on the user's behalf.

use std::path::PathBuf;
use std::process::Command;

/// The default repository the toolchain binaries are fetched from.
pub const DEFAULT_REPO: &str = "https://github.com/windwhiterain/lichen-vm";

/// A toolchain binary the package manager can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// `lichen-compiler`
    Compiler,
    /// `lichen-language-server`
    LanguageServer,
}

impl Tool {
    /// The binary name produced by `cargo install`.
    pub fn bin_name(&self) -> &'static str {
        match self {
            Tool::Compiler => "lichen-compiler",
            Tool::LanguageServer => "lichen-language-server",
        }
    }

    /// The Cargo package (crate) name the binary is installed from.
    pub fn crate_name(&self) -> &'static str {
        match self {
            Tool::Compiler => "lichen-language",
            Tool::LanguageServer => "lichen-language-server",
        }
    }

    /// Parse a tool by name, tolerating a leading `lichen-` or the short form.
    pub fn from_name(name: &str) -> Option<Tool> {
        match name {
            "compiler" | "liche-compiler" | "lichen-compiler" | "run" => Some(Tool::Compiler),
            "language-server" | "lichen-language-server" | "lsp" | "server" => {
                Some(Tool::LanguageServer)
            }
            _ => None,
        }
    }
}

/// Install a toolchain binary from `source` (a git URL or a local path).
///
/// `rev` optionally pins the checked-out revision (defaulting to the repo's
/// default branch).  The install is force-refreshed so re-running upgrades the
/// binary.  Returns the install command's stderr on failure.
pub fn install(tool: Tool, source: &str, rev: Option<&str>) -> Result<(), String> {
    if !cargo_available() {
        return Err("`cargo` is required to install the toolchain, but it is not on $PATH".into());
    }
    let mut cmd = Command::new("cargo");
    cmd.arg("install").arg("--force").arg("--locked");
    if PathBuf::from(source).exists() {
        cmd.arg("--path").arg(source);
    } else {
        cmd.arg("--git").arg(source);
        if let Some(rev) = rev {
            if rev.starts_with("rev:") {
                cmd.arg("--rev").arg(&rev[4..]);
            } else {
                cmd.arg("--branch").arg(rev);
            }
        }
    }
    cmd.arg("--bin").arg(tool.bin_name()).arg(tool.crate_name());
    let out = cmd
        .output()
        .map_err(|e| format!("cannot run cargo install: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Whether `cargo` is on `$PATH`.
pub fn cargo_available() -> bool {
    Command::new("cargo")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Locate an installed toolchain binary on `$PATH` (searching `$CARGO_HOME/bin`
/// first), or its absolute path when already built in `target` dirs.
pub fn find(tool: Tool) -> Option<PathBuf> {
    let name = tool.bin_name();
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    // The Cargo bin dir, then the PATH.
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
            home.map(|h| PathBuf::from(h).join(".cargo"))
                .unwrap_or_default()
        });
    let candidates = [cargo_home.join("bin").join(&exe)];
    for c in candidates {
        if c.is_file() {
            return Some(c);
        }
    }
    find_on_path(&exe)
}

/// Search `$PATH` for an executable, returning its first match.
fn find_on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}
