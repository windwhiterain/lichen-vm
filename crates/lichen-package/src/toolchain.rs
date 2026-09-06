//! Toolchain binaries the package manager installs into Lichen Home.
//!
//! There are two classes, and they live in different places:
//!
//! * **Plugin-sensitive** (compiler, language server): a native plugin extends the
//!   compiler's vocabulary, so these are composed *per plugin set* and cached under
//!   `<lichendir>/compilers/<plugin-set-key>/` (see [`crate::compiler_cache`]). The
//!   package manager installs the prebuilt **shipping** (no-extra-plugin) binaries
//!   into the base plugin-set slot.
//! * **Non-plugin-sensitive** (formatter, and the package manager itself): a single
//!   fixed binary, at `<lichendir>/tools/<name>`.
//!
//! Binaries are fetched as **prebuilt release assets** (never built on the user's
//! machine), from the GitHub **release at the package manager's own commit** — so
//! the toolchain and the package manager are always the same revision. The package
//! manager is only ever *run*; how it got installed (any way) is irrelevant.

use std::path::PathBuf;
use std::process::Command;

use lichen_preprocess::{Depend, lichendir};

use crate::compiler_cache;
use crate::plugin;

/// The default repository toolchain releases are fetched from.
pub const DEFAULT_REPO: &str = "https://github.com/windwhiterain/lichen-vm";

/// The package manager's own binary name.
pub const PACKAGE_MANAGER_BIN: &str = "liche";

/// The commit this `lichen` binary was compiled from, if known.
///
/// Set by [`build.rs`](crate::build) from `git rev-parse HEAD`; `None` when the
/// crate was built outside a git checkout (the package manager then falls back to
/// the repo's default-branch tip). The toolchain release is fetched at this commit
/// so the package manager and the toolchain are the same revision.
pub fn self_commit() -> Option<&'static str> {
    let commit = env!("LICHEN_BUILD_COMMIT");
    if commit.is_empty() {
        None
    } else {
        Some(commit)
    }
}

/// The `--rev` selector string for pinning a cargo install to this binary's own
/// commit (retained for the source-build fallback); `None` when unknown.
pub fn self_commit_rev() -> Option<String> {
    self_commit().map(|c| format!("rev:{c}"))
}

/// A toolchain binary the package manager can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// `lichen-compiler` — plugin-sensitive.
    Compiler,
    /// `lichen-language-server` — plugin-sensitive.
    LanguageServer,
    /// `lichen-fmt` — non-plugin-sensitive (not shipped yet; slot is reserved).
    Formatter,
}

impl Tool {
    /// The binary name for this tool.
    pub fn bin_name(&self) -> &'static str {
        match self {
            Tool::Compiler => "lichen-compiler",
            Tool::LanguageServer => "lichen-language-server",
            Tool::Formatter => "lichen-fmt",
        }
    }

    /// Whether this tool depends on a native plugin set (and so is cached
    /// plugin-set-keyed rather than in a flat `tools/` slot).
    pub fn is_plugin_sensitive(&self) -> bool {
        matches!(self, Tool::Compiler | Tool::LanguageServer)
    }

    /// Parse a tool by name, tolerating a leading `lichen-` or the short form.
    pub fn from_name(name: &str) -> Option<Tool> {
        match name {
            "compiler" | "liche-compiler" | "lichen-compiler" | "run" => Some(Tool::Compiler),
            "language-server" | "lichen-language-server" | "lsp" | "server" => {
                Some(Tool::LanguageServer)
            }
            "fmt" | "formatter" | "lichen-fmt" => Some(Tool::Formatter),
            _ => None,
        }
    }

    /// The plugin-sensitive tools an `all` install covers (`formatter` is not yet
    /// built, so it is excluded until its release asset exists).
    pub const ALL_PLUGIN_SENSITIVE: [Tool; 2] = [Tool::Compiler, Tool::LanguageServer];
}

// ---------------------------------------------------------------------------
// Local path naming and the lichen-home layout.
// ---------------------------------------------------------------------------

/// The `.exe` suffix on Windows, else empty.
fn exe_suffix() -> &'static str {
    if cfg!(windows) { ".exe" } else { "" }
}

/// The binary file name on this host (e.g. `lichen-language-server.exe`).
pub fn local_name(bin: &str) -> String {
    format!("{bin}{}", exe_suffix())
}

/// The host target triple used to name release assets (matches how CI builds them).
pub fn host_target() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc".to_string(),
        ("windows", "aarch64") => "aarch64-pc-windows-msvc".to_string(),
        ("macos", "aarch64") => "aarch64-apple-darwin".to_string(),
        ("macos", "x86_64") => "x86_64-apple-darwin".to_string(),
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu".to_string(),
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu".to_string(),
        _ => format!("{arch}-unknown-{os}"),
    }
}

/// The release asset file name for a tool on this host.
pub fn asset_name(bin: &str) -> String {
    format!("{}-{}{}", bin, host_target(), exe_suffix())
}

/// `<lichendir>/tools` — non-plugin-sensitive tool slot (pm, formatter).
pub fn tools_dir() -> PathBuf {
    lichendir().join("tools")
}

/// `<lichendir>/compilers/<base-plugin-set-key>/bin` — the shipping (no-extra-plugin)
/// binaries for the plugin-sensitive tools. Project plugin sets are composed into
/// their own keyed sllot by [`crate::compiler_cache`].
fn shipping_dir() -> Result<PathBuf, String> {
    let key =
        compiler_cache::key(&[]).map_err(|e| format!("cannot key the base plugin set: {e}"))?;
    Ok(lichendir()
        .join(compiler_cache::COMPILERS_DIR)
        .join(key)
        .join("bin"))
}

/// The local destination path for a shipped binary in Lichen Home.
pub fn dest_path(bin: &str, plugin_sensitive: bool) -> Result<PathBuf, String> {
    if plugin_sensitive {
        Ok(shipping_dir()?.join(local_name(bin)))
    } else {
        Ok(tools_dir().join(local_name(bin)))
    }
}

/// The destination path for a tool in Lichen Home.
pub fn tool_dest_path(tool: Tool) -> Result<PathBuf, String> {
    dest_path(tool.bin_name(), tool.is_plugin_sensitive())
}

// ---------------------------------------------------------------------------
// Prebuilt-release fetch.
// ---------------------------------------------------------------------------

/// The commit the toolchain release should come from: this binary's own commit,
/// else the repo's default-branch tip.
fn toolchain_commit(repo: &str) -> Result<String, String> {
    if let Some(commit) = self_commit() {
        return Ok(commit.to_string());
    }
    default_branch_tip(repo)
}

/// The default-branch tip SHA of `repo` (as `git ls-remote <repo> HEAD`).
fn default_branch_tip(repo: &str) -> Result<String, String> {
    let out = Command::new("git")
        .args(["ls-remote", repo, "HEAD"])
        .output()
        .map_err(|e| format!("cannot query {repo}: {e}"))?;
    let sha = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    if sha.is_empty() {
        Err(format!("no default branch found for {repo}"))
    } else {
        Ok(sha)
    }
}

/// The GitHub release asset download URL for `bin` at `commit`.
///
/// GitHub downloads are `https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>`;
/// here the *tag* is the commit SHA (the release is tagged at the commit).
pub fn asset_url(repo: &str, commit: &str, bin: &str) -> String {
    format!("{repo}/releases/download/{commit}/{}", asset_name(bin))
}

/// Download `url` to `dest` (a temp sibling, then rename) using `curl`.
///
/// Windows ships `curl.exe`; Unix systems ship `curl`. Returns the download
/// command's stderr on failure.
fn download(url: &str, dest: &PathBuf) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let tmp = dest.with_extension("download.tmp");
    let out = Command::new("curl")
        .args(["-L", "--fail", "--output"])
        .arg(&tmp)
        .arg(url)
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "download failed ({url}): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    std::fs::rename(&tmp, dest).map_err(|e| format!("cannot move download into place: {e}"))?;
    make_executable(dest)?;
    Ok(())
}

/// Make `path` executable (a no-op on Windows).
fn make_executable(path: &PathBuf) -> Result<(), String> {
    #[cfg(windows)]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)
            .map_err(|e| format!("cannot chmod {}: {e}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public operations.
// ---------------------------------------------------------------------------

/// Install (refresh) `tool` from the prebuilt release at the package manager's own
/// commit into Lichen Home. Returns the installed binary path.
pub fn install(tool: Tool, repo: &str) -> Result<PathBuf, String> {
    let bin = tool.bin_name();
    let commit = toolchain_commit(repo)?;
    let url = asset_url(repo, &commit, bin);
    let dest = tool_dest_path(tool)?;
    download(&url, &dest)?;
    Ok(dest)
}

/// Resolve the shipped binary for `tool`: the Lichen Home copy first, then `$PATH`.
pub fn resolve(tool: Tool) -> Option<PathBuf> {
    if let Ok(dest) = tool_dest_path(tool) {
        if dest.is_file() {
            return Some(dest);
        }
    }
    find_on_path(tool.bin_name())
}

/// Resolve the language-server binary for a plugin set.
///
/// With an **empty** plugin set, the *shipping* `lichen-language-server` is
/// resolved (the Lichen Home copy first, then `$PATH`), installed into Lichen
/// Home from the prebuilt release if absent.  With a **non-empty** plugin set,
/// a language server composed over those plugins is ensured via the
/// plugin-set LSP cache (see [`compiler_cache::ensure_lsp`]) and returned —
/// an editor then runs the composed server so it understands the plugins'
/// leaves for diagnostics / hover / go-to-definition.
///
/// Returns `Ok(Some(binary))` on success.  The `Option` is always `Some` in
/// practice (a shipping server can always be resolved or installed); a caller
/// that only wants a composed server can `filter` the shipped out.
pub fn resolve_lsp_for(plugins: &[Depend]) -> Result<Option<PathBuf>, String> {
    if plugins.is_empty() {
        if let Some(path) = resolve(Tool::LanguageServer) {
            return Ok(Some(path));
        }
        let dest = install(Tool::LanguageServer, DEFAULT_REPO)?;
        return Ok(Some(dest));
    }
    println!(
        "composing a language server over the project's {} native plugin(s)...",
        plugins.len()
    );
    let leaves = plugin::Leaves::shipping();
    let bin = compiler_cache::ensure_lsp(DEFAULT_REPO, plugins, &leaves)?;
    Ok(Some(bin))
}

/// Self-update the package manager to the repo's latest commit. The updated binary
/// is written to `$LICHEN_HOME/tools/liche` (the canonical copy the extension and
/// the CLI resolve from); a copy on `$PATH` elsewhere is left for the user to
/// refresh. Returns `Ok(None)` when already current, else the new commit.
pub fn update(repo: &str) -> Result<Option<String>, String> {
    let commit = default_branch_tip(repo)?;
    if self_commit().is_some_and(|c| c == commit) {
        return Ok(None);
    }
    let dest = tools_dir().join(local_name(PACKAGE_MANAGER_BIN));
    let url = asset_url(repo, &commit, PACKAGE_MANAGER_BIN);
    download(&url, &dest)?;
    Ok(Some(commit))
}

/// Whether the given installed binary for `bin` exists in Lichen Home (used to
/// decide whether to refresh the toolchain for a project).
pub fn installed(bin: &str, plugin_sensitive: bool) -> bool {
    dest_path(bin, plugin_sensitive).is_ok_and(|p| p.is_file())
}

/// Search `$PATH` for an executable, returning its first match.
pub fn find_on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}

/// Locate an installed toolchain binary on `$PATH` (searching `$CARGO_HOME/bin`
/// first). Kept for the source-build dev path.
pub fn find(tool: Tool) -> Option<PathBuf> {
    let name = tool.bin_name();
    let exe = local_name(name);
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
            home.map(|h| PathBuf::from(h).join(".cargo"))
                .unwrap_or_default()
        });
    let candidate = cargo_home.join("bin").join(&exe);
    if candidate.is_file() {
        Some(candidate)
    } else {
        find_on_path(&exe)
    }
}
