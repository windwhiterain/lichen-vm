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
//! package manager's own commit, and prints the binary path.
//!
//! The package manager is the single canonical copy at
//! `$LICHEN_HOME/tools/lichen[.exe]` — exactly the file `liche update` refreshes
//! in place — or a `lichen` already on `$PATH`. On a machine with neither, the
//! extension downloads the prebuilt `lichen` for this host from this repo's GitHub
//! release **into that same `$LICHEN_HOME/tools` slot** via `curl` (mirroring the
//! package manager's own `toolchain::download`). This keeps a fresh environment
//! self-bootstrapping while later `liche update`s stay in sync: the extension
//! never keeps a private copy of its own.
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
    /// The located `lichen` package-manager path. This is the canonical
    /// `$LICHEN_HOME/tools/lichen` copy (or the `lichen` `which` found on
    /// `$PATH`) — never a private extension-dir download. Cached once located so
    /// a fresh environment bootstraps once rather than re-downloading on every
    /// buffer open. Only read/written under the `zed` feature; in the
    /// metadata-only (non-`zed`) build the field is intentionally dead.
    #[allow(dead_code)]
    cached_lichen: Option<String>,
}

#[cfg(feature = "zed")]
mod zed_impl {
    use zed_extension_api::{
        self as zed, Architecture, Command, Extension, GithubReleaseOptions, LanguageServerId,
        LanguageServerInstallationStatus, Os, Worktree, current_platform, latest_github_release,
        make_file_executable, set_language_server_installation_status,
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
            // print its path, then hand that path to Zed.
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

    /// The shell environment as `(key, value)` pairs.
    fn shell_env(worktree: &Worktree) -> Vec<(String, String)> {
        worktree.shell_env()
    }

    /// Resolve `$LICHEN_HOME`, defaulting to `~/.lichen` (per the package
    /// manager's `lichen_preprocess::lichendir`).
    fn lichen_home(worktree: &Worktree) -> String {
        let vars = shell_env(worktree);
        if let Some((_, home)) = vars.iter().find(|(k, _)| k == "LICHEN_HOME") {
            return home.clone();
        }
        let home = vars
            .iter()
            .find(|(k, _)| k == "HOME" || k == "USERPROFILE")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| ".".into());
        format!("{home}/.lichen")
    }

    /// The canonical package-manager path: `$LICHEN_HOME/tools/lichen[.exe]`,
    /// exactly the file `liche update` refreshes in place.
    fn canonical_lichen(worktree: &Worktree) -> String {
        format!("{}/tools/lichen{}", lichen_home(worktree), asset_suffix())
    }

    /// The outcome of running `lichen path language-server`.
    enum PmRun {
        /// The command succeeded; this is the server binary's absolute path.
        Path(String),
        /// The package manager binary could not be spawned (`output()` errored),
        /// meaning it is absent — try another location or bootstrap.
        Missing,
        /// The package manager ran but failed; this is a real error to report.
        Failed(String),
    }

    /// Run `lichen path language-server [--project <root>]` for `pm` and classify
    /// the result: `Path` on success, `Missing` when the binary cannot be spawned
    /// (absent), `Failed` for a genuine command error.
    fn run_server(worktree: &Worktree, pm: &str) -> PmRun {
        let mut command = Command::new(pm).arg("path").arg("language-server");
        // A project with native plugins composes its own server over the plugin
        // set (`--project <root>`); the fallback (no `--project`) resolves the
        // shipping server.  `root_path()` always returns a string, so an empty
        // root is treated as "unavailable" and skipped.
        let root = worktree.root_path();
        if !root.is_empty() {
            command = command.arg("--project").arg(root);
        }
        let out = match command.output() {
            Ok(out) => out,
            // A spawn error (`output()` errs) means the binary is not present.
            Err(_) => return PmRun::Missing,
        };
        if out.status != Some(0) {
            return PmRun::Failed(format!(
                "`{pm} path language-server` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() {
            PmRun::Failed(format!(
                "`{pm} path language-server` returned an empty path"
            ))
        } else {
            PmRun::Path(path)
        }
    }

    /// Download the prebuilt `lichen` for this host into `$LICHEN_HOME/tools` via
    /// `curl` (creating the directory as needed) and mark it executable.
    fn bootstrap_lichen(home: &str) -> Result<(), String> {
        let asset_name = format!("{}-{}{}", LICHEN_BIN, host_target(), asset_suffix());
        let release = latest_github_release(
            RELEASE_REPO,
            GithubReleaseOptions {
                require_assets: true,
                // Toolchain releases are published as full/latest releases (see the
                // release-lichen workflow's `prerelease` input); don't require a
                // pre-release, or nothing is found once the newest release is real.
                pre_release: false,
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
        let mut curl = Command::new("curl")
            .arg("--create-dirs")
            .arg("-L")
            .arg("--fail")
            .arg("--silent")
            .arg("--show-error")
            .arg("--output")
            .arg(home)
            .arg(asset.download_url.as_str());
        let out = curl.output().map_err(|e| format!("cannot run curl: {e}"))?;
        if out.status != Some(0) {
            return Err(format!(
                "cannot download `{LICHEN_BIN}` ({asset_name}): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        make_file_executable(home)
            .map_err(|e| format!("cannot mark `{LICHEN_BIN}` executable: {e}"))?;
        Ok(())
    }

    /// Ensure the server is installed (asking the `lichen` package manager, which
    /// installs the prebuilt compiler + language server into Lichen Home at its
    /// own commit) and return its absolute path.  When the worktree root is
    /// available it is passed as `--project <root>` so a project with native
    /// plugins composes its own server; when no root is available the shipping
    /// server is resolved.
    fn resolve_via_lichen(
        worktree: &Worktree,
        self_: &mut LichenExtension,
    ) -> Result<String, String> {
        // Fast path: a package manager located on a previous call.  If that path
        // has since become unusable (cleared below), re-locate rather than
        // surfacing a confusing spawn error.
        if let Some(pm) = self_.cached_lichen.clone() {
            match run_server(worktree, &pm) {
                PmRun::Path(path) => return Ok(path),
                PmRun::Missing => self_.cached_lichen = None,
                PmRun::Failed(e) => return Err(e),
            }
        }

        // 1. A `lichen` already on `$PATH`.  `which` is a reliable presence check
        //    (it only returns a path the shell can actually run), so the common
        //    case needs no spawn-probe.
        if let Some(p) = worktree.which(LICHEN_BIN) {
            match run_server(worktree, &p) {
                PmRun::Path(path) => {
                    self_.cached_lichen = Some(p.clone());
                    return Ok(path);
                }
                PmRun::Missing => {}
                PmRun::Failed(e) => return Err(e),
            }
        }

        // 2. The canonical Lichen Home copy — `liche update` refreshes this exact
        //    file, so using it keeps the extension in sync with the package
        //    manager (the fix for the stale "private extension-dir copy" bug).
        //    `run_server` doubles as the presence probe: if it cannot spawn, the
        //    copy is absent and we fall through.
        let home = canonical_lichen(worktree);
        match run_server(worktree, &home) {
            PmRun::Path(path) => {
                self_.cached_lichen = Some(home.clone());
                return Ok(path);
            }
            PmRun::Missing => {}
            PmRun::Failed(e) => return Err(e),
        }

        // 3. Nothing anywhere: bootstrap the canonical home copy, then run it.
        bootstrap_lichen(&home)?;
        self_.cached_lichen = Some(home.clone());
        match run_server(worktree, &home) {
            PmRun::Path(path) => Ok(path),
            PmRun::Missing => Err(format!(
                "bootstrapped `{LICHEN_BIN}` is not runnable at `{home}`"
            )),
            PmRun::Failed(e) => Err(e),
        }
    }

    zed_extension_api::register_extension!(LichenExtension);
}
