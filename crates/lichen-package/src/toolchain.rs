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
pub const PACKAGE_MANAGER_BIN: &str = "lichen";

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
            "compiler" | "lichen-compiler" | "run" => Some(Tool::Compiler),
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

/// The commit the toolchain release should come from: this binary's own commit.
/// A `lichen` built outside a git checkout has no pinned commit, so it cannot
/// install a same-commit toolchain — the caller is told to `liche update` rather
/// than silently chasing the repo tip (which may have no release, since the user
/// publishes manually).
fn toolchain_commit() -> Result<String, String> {
    self_commit().map(str::to_string).ok_or_else(|| {
        "cannot pin the toolchain to a commit: this `lichen` was built outside a \
         git checkout; run `liche update` to align to a published release"
            .to_string()
    })
}

/// The tag of the newest published GitHub release for `repo` (a short-SHA
/// toolchain tag — see [`release_tag`]).
///
/// GitHub's release list is newest-first and includes pre-releases, so its first
/// entry is the latest.  This decouples `lichen update` from the repository tip,
/// which may have no release (the user publishes manually).
fn latest_release_tag(repo: &str) -> Result<String, String> {
    let slug = github_owner_repo(repo)?;
    let url = format!("https://api.github.com/repos/{slug}/releases?per_page=1");
    let out = Command::new("curl")
        .args(["-sS", "-L", "--fail", url.as_str()])
        .output()
        .map_err(|e| format!("cannot query GitHub releases: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot query GitHub releases: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let releases: Vec<Release> = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("cannot parse GitHub releases: {e}"))?;
    releases
        .into_iter()
        .next()
        .map(|r| r.tag_name)
        .ok_or_else(|| "no GitHub release published yet; publish the toolchain first".to_string())
}

/// `owner/repo` from a GitHub URL (`https://github.com/o/r[.git]`,
/// `git@github.com:o/r[.git]`, `ssh://git@github.com/o/r`).
fn github_owner_repo(repo: &str) -> Result<String, String> {
    let trimmed = repo.trim_end_matches('/');
    let cleaned = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let path = if let Some(idx) = cleaned.find("://") {
        let rest = &cleaned[idx + 3..];
        rest.split_once('/').map(|(_, p)| p).unwrap_or(rest)
    } else if let Some((_, path)) = cleaned.split_once(':') {
        path
    } else {
        cleaned
    };
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.split('/').count() == 2 && !path.starts_with('/') {
        Ok(path.to_string())
    } else {
        Err(format!(
            "unsupported repository URL for release lookup: {repo}"
        ))
    }
}

/// A single GitHub release entry (the fields we read).
#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
}

/// The number of leading hex characters of a commit used as a release tag.
///
/// GitHub rejects release/branch tags that are a bare 40- or 64-hex commit SHA,
/// so toolchain releases are tagged with the first 12 hex chars of the commit
/// instead of the raw SHA.
const RELEASE_TAG_LEN: usize = 12;

/// The GitHub release tag for a toolchain commit: its first [`RELEASE_TAG_LEN`]
/// hex chars (never the raw 40-hex SHA, which GitHub forbids as a tag).
pub fn release_tag(commit: &str) -> String {
    commit.chars().take(RELEASE_TAG_LEN).collect()
}

/// The GitHub release asset download URL for `bin` at the given release `tag`.
///
/// GitHub downloads are `https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>`;
/// here the *tag* is the toolchain release tag (a short-SHA tag derived from the
/// commit — see [`release_tag`]).
pub fn asset_url(repo: &str, tag: &str, bin: &str) -> String {
    format!("{repo}/releases/download/{tag}/{}", asset_name(bin))
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
/// commit into Lichen Home. Returns the installed binary path. If the release at
/// that commit is missing, the error hints at `lichen update`.
pub fn install(tool: Tool, repo: &str) -> Result<PathBuf, String> {
    let bin = tool.bin_name();
    let commit = toolchain_commit()?;
    let tag = release_tag(&commit);
    let url = asset_url(repo, &tag, bin);
    let dest = tool_dest_path(tool)?;
    match download(&url, &dest) {
        Ok(()) => Ok(dest),
        Err(e) => Err(format!(
            "{e}; if `{bin}` was not published at `{tag}` (this `lichen`'s own \
             commit `{commit}`), run `liche update`"
        )),
    }
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

/// Self-update the package manager to the repo's **latest published release**. The
/// updated binary is written to `$LICHEN_HOME/tools/lichen` (the canonical copy the
/// extension and the CLI resolve from); a copy on `$PATH` elsewhere is left for the
/// user to refresh. Returns `Ok(None)` when already current, else the new commit.
pub fn update(repo: &str) -> Result<Option<String>, String> {
    let tag = latest_release_tag(repo)?;
    if self_commit().is_some_and(|c| release_tag(c) == tag) {
        return Ok(None);
    }
    let dest = tools_dir().join(local_name(PACKAGE_MANAGER_BIN));
    let url = asset_url(repo, &tag, PACKAGE_MANAGER_BIN);
    download(&url, &dest)?;
    Ok(Some(tag))
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

#[cfg(test)]
mod tests {
    use super::{github_owner_repo, release_tag};

    #[test]
    fn parses_github_repo_urls() {
        assert_eq!(
            github_owner_repo("https://github.com/windwhiterain/lichen-vm").unwrap(),
            "windwhiterain/lichen-vm"
        );
        assert_eq!(
            github_owner_repo("https://github.com/windwhiterain/lichen-vm.git").unwrap(),
            "windwhiterain/lichen-vm"
        );
        assert_eq!(
            github_owner_repo("git@github.com:windwhiterain/lichen-vm").unwrap(),
            "windwhiterain/lichen-vm"
        );
        assert_eq!(
            github_owner_repo("ssh://git@github.com/windwhiterain/lichen-vm").unwrap(),
            "windwhiterain/lichen-vm"
        );
    }

    #[test]
    fn rejects_non_github_urls() {
        assert!(github_owner_repo("C:\\dev\\lichen-vm").is_err());
        assert!(github_owner_repo("https://example.com/just-one-segment").is_err());
    }

    #[test]
    fn release_tag_is_the_short_sha() {
        let commit = "15a12b39020cc3c093f193f79ddc30860cb041d0";
        // GitHub forbids tags that are a bare 40/64-hex SHA; the tag is a 12-char prefix.
        assert_eq!(release_tag(commit), "15a12b39020c");
        assert_ne!(release_tag(commit).len(), 40);
    }
}
