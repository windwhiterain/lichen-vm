//! The Zed editor extension for Lichen.
//!
//! This is a *package-kind*-separate crate, not a *tool*-separate crate: it is
//! a WASM plugin that speaks `zed_extension_api`, so it cannot live in the same
//! binary target as the LSP server. It declares the `lichen` language and points
//! Zed's LSP integration at `lichen-language-server` (via `extension.toml`).
//!
//! The `zed` feature is on by default (`default = ["zed"]` in `Cargo.toml`), so
//! a plain `cargo build` — including the one Zed's own dev-extension builder
//! runs, which passes no `--features` — compiles the extension body and emits the
//! `zed:api-version` custom section Zed requires.
//!
//! The extension does **not** bundle `lichen-language-server` (Zed's publishing
//! rules forbid shipping a standalone LSP binary in the extension). On first
//! launch, if the server is not already on `$PATH`, the extension reports install
//! progress to Zed and asks the `lichen` package manager to ensure it is present:
//! `lichen path language-server` installs the **prebuilt** compiler + language
//! server into **Lichen Home** (`$LICHEN_HOME`, defaulting to `~/.lichen`) at the
//! package manager's own commit, and prints the binary path (see
//! `docs/notes/language-toolchain.md`).
//!
//! `lichen` is located on `$PATH`; on a machine with no `lichen` at all, the
//! extension downloads the prebuilt `lichen` binary from this repo's GitHub
//! release into its **own working directory** (`download_file` +
//! `make_file_executable`, mirroring how the Kotlin extension self-installs) and
//! runs that. Either way it then resolves the server through the package
//! manager.
//!
//! When the worktree root is available, the extension passes it as
//! `--project <root>` so that a project importing a **native plugin** composes
//! its own language server over that plugin set (`lichen path language-server
//! --project <root>` builds/caches the composed server into the plugin-set LSP
//! slot, understanding the plugin's leaves for diagnostics / hover / definition).

pub const LANGUAGE_NAME: &str = "Lichen";
pub const LANGUAGE_ID: &str = "lichen";
pub const FILE_EXTENSIONS: &[&str] = &["lichen"];
pub const GRAMMAR_SCOPE: &str = "source.lichen";
/// The LSP binary this extension instructs Zed to launch.
pub const LANGUAGE_SERVER_BINARY: &str = "lichen-language-server";

/// The extension type. Non-`zed` builds expose this as metadata only.
pub struct LichenExtension {
    /// The self-bootstrapped `lichen` package-manager path, cached once it has
    /// been downloaded into the extension's working directory. Cached so a
    /// fresh environment bootstraps once, not on every buffer open.
    cached_lichen: Option<String>,
}

#[cfg(feature = "zed")]
mod zed_impl {
    use zed_extension_api::{
        self as zed, Architecture, Command, DownloadedFileType, Extension, GithubReleaseOptions,
        LanguageServerId, LanguageServerInstallationStatus, Os, Worktree, current_platform,
        download_file, latest_github_release, make_file_executable,
        set_language_server_installation_status,
    };

    use crate::{LANGUAGE_SERVER_BINARY, LichenExtension};

    /// The GitHub `owner/repo` the prebuilt toolchain releases are published to.
    const RELEASE_REPO: &str = "windwhiterain/lichen-vm";
    /// The package-manager binary name (built as `lichen`).
    const LICHEN_BIN: &str = "lichen";

    impl Extension for LichenExtension {
        fn new() -> Self {
            LichenExtension {
                cached_lichen: None,
            }
        }

        fn language_server_command(
            &mut self,
            language_server_id: &LanguageServerId,
            worktree: &Worktree,
        ) -> zed::Result<Command> {
            // Fast path: the server is already installed on `$PATH`.
            if let Some(path) = worktree.which(LANGUAGE_SERVER_BINARY) {
                return Ok(Command::new(path));
            }

            set_language_server_installation_status(
                language_server_id,
                &LanguageServerInstallationStatus::Downloading,
            );

            // The toolchain is managed by the `lichen` package manager, which
            // installs the prebuilt compiler + language server into Lichen Home
            // at its own commit.  Ask it to ensure the server is present and
            // print its path, then hand that path to Zed.  On a fresh machine
            // with no `lichen`, it is first downloaded into the extension dir.
            match resolve_via_lichen(worktree, self) {
                Ok(path) => {
                    set_language_server_installation_status(
                        language_server_id,
                        &LanguageServerInstallationStatus::None,
                    );
                    Ok(Command::new(path))
                }
                Err(err) => {
                    set_language_server_installation_status(
                        language_server_id,
                        &LanguageServerInstallationStatus::Failed(err.clone()),
                    );
                    Err(err)
                }
            }
        }
    }

    /// Whether the current OS is Windows (affects the executable suffix).
    fn on_windows() -> bool {
        matches!(current_platform().0, Os::Windows)
    }

    /// The host target triple used to name release assets (matches
    /// `lichen-package`'s `toolchain::host_target`).
    fn host_target() -> String {
        match current_platform() {
            (Os::Windows, Architecture::X8664) => "x86_64-pc-windows-msvc",
            (Os::Windows, Architecture::Aarch64) => "aarch64-pc-windows-msvc",
            (Os::Mac, Architecture::Aarch64) => "aarch64-apple-darwin",
            (Os::Mac, Architecture::X8664) => "x86_64-apple-darwin",
            (Os::Linux, Architecture::X8664) => "x86_64-unknown-linux-gnu",
            (Os::Linux, Architecture::Aarch64) => "aarch64-unknown-linux-gnu",
            _ => "unknown-unknown",
        }
        .to_string()
    }

    /// The `.exe` suffix a Windows release asset carries (matches
    /// `lichen-package`'s `toolchain::asset_name`), else empty.
    fn asset_suffix() -> &'static str {
        if on_windows() { ".exe" } else { "" }
    }

    /// The extension's working directory (the WASM process CWD, which Zed sets
    /// via `PWD`).  This is the only directory the extension may read/write, so
    /// the self-bootstrapped `lichen` is cached here.
    fn extension_dir() -> String {
        std::env::var("PWD").unwrap_or_else(|_| ".".into())
    }

    /// The relative path (under the extension dir) of the self-bootstrapped
    /// `lichen`.  Relative is used for `download_file` / `std::fs::metadata`,
    /// which operate relative to the extension's working directory.
    fn cached_lichen_rel() -> String {
        format!("{}", if on_windows() { "lichen.exe" } else { "lichen" })
    }

    /// Locate `lichen`: on `$PATH`, else the self-bootstrapped copy in the
    /// extension's working directory, downloading it from the repo's GitHub
    /// release on first use (a truly fresh machine).  Cached in `cached`.
    fn ensure_lichen(worktree: &Worktree, cached: &mut Option<String>) -> Result<String, String> {
        if let Some(p) = cached.as_ref() {
            return Ok(p.clone());
        }
        if let Some(p) = worktree.which(LICHEN_BIN) {
            *cached = Some(p.clone());
            return Ok(p);
        }
        // Fresh machine with no `lichen` at all: download the prebuilt binary
        // for this host into the extension dir.
        let rel = cached_lichen_rel();
        if !std::path::Path::new(&rel).exists() {
            let asset_name = format!("{}-{}{}", LICHEN_BIN, host_target(), asset_suffix());
            let release = latest_github_release(
                RELEASE_REPO,
                GithubReleaseOptions {
                    require_assets: true,
                    pre_release: true,
                },
            )
            .map_err(|e| format!("cannot query lichen-vm releases: {e}"))?;
            let asset = release
                .assets
                .iter()
                .find(|a| a.name == asset_name)
                .ok_or_else(|| {
                    format!("no release asset named `{asset_name}`; publish the toolchain first")
                })?;
            download_file(&asset.download_url, &rel, DownloadedFileType::Uncompressed)
                .map_err(|e| format!("cannot download `{LICHEN_BIN}` toolchain: {e}"))?;
            make_file_executable(&rel)
                .map_err(|e| format!("cannot mark `{LICHEN_BIN}` executable: {e}"))?;
        }
        // Return an absolute path so the spawned subprocess resolves it exactly.
        let abs = format!("{}/{}", extension_dir(), rel);
        *cached = Some(abs.clone());
        Ok(abs)
    }

    /// Ensure the server is installed (asking the `lichen` package manager,
    /// which installs the prebuilt compiler + language server into Lichen Home
    /// at its own commit) and return its absolute path.  When the worktree root
    /// is available it is passed as `--project <root>` so a project with native
    /// plugins composes its own server; when no root is available the shipping
    /// server is resolved.
    fn resolve_via_lichen(
        worktree: &Worktree,
        self_: &mut LichenExtension,
    ) -> Result<String, String> {
        let lichen = ensure_lichen(worktree, &mut self_.cached_lichen)?;
        let mut command = Command::new(lichen.as_str())
            .arg("path")
            .arg("language-server");
        // A project with native plugins composes its own server over the plugin
        // set (`--project <root>`); the fallback (no `--project`) resolves the
        // shipping server.  `root_path()` always returns a string, so an empty
        // root is treated as "unavailable" and skipped.
        let root = worktree.root_path();
        if !root.is_empty() {
            command = command.arg("--project").arg(root);
        }
        let out = command
            .output()
            .map_err(|e| format!("cannot run `{lichen} path language-server`: {e}"))?;
        if out.status != Some(0) {
            return Err(format!(
                "`{lichen} path language-server` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() {
            Err("`lichen path language-server` returned an empty path".to_string())
        } else {
            Ok(path)
        }
    }

    zed_extension_api::register_extension!(LichenExtension);
}
