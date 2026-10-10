//! The Zed extension for Lichen: a WASM plugin launching
//! `lichen-language-server`; see docs/notes/language-toolchain.md.

pub const LANGUAGE_NAME: &str = "Lichen";
pub const LANGUAGE_ID: &str = "lichen";
pub const FILE_EXTENSIONS: &[&str] = &["lichen"];
pub const GRAMMAR_SCOPE: &str = "source.lichen";
/// The LSP binary this extension instructs Zed to launch.
pub const LANGUAGE_SERVER_BINARY: &str = "lichen-language-server";

/// The extension type. Non-`zed` builds expose this as metadata only.
pub struct LichenExtension {
    /// The `lichen` package-manager path cached from a previous call: the
    /// canonical copy, never a private download.
    #[allow(dead_code)]
    cached_lichen: Option<String>,
}

#[cfg(feature = "zed")]
mod zed_impl {
    use zed_extension_api::{
        self as zed, Architecture, Command, Extension, GithubReleaseOptions, LanguageServerId,
        LanguageServerInstallationStatus, Os, Worktree, current_platform, latest_github_release,
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

            // `lichen path language-server` installs the server if needed and
            // prints its path; see docs/notes/language-toolchain.md.
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

    /// The canonical package-manager path: `$LICHEN_HOME/tools/lichen[.exe]` —
    /// the file `liche update` refreshes in place.
    fn canonical_lichen(worktree: &Worktree) -> String {
        format!("{}/tools/lichen{}", lichen_home(worktree), asset_suffix())
    }

    /// The outcome of running `lichen path language-server`.
    enum PmRun {
        /// The command succeeded; this is the server binary's absolute path.
        Path(String),
        /// The package manager could not be spawned (`output()` errored), so it is
        /// absent — try another location or bootstrap.
        Missing,
        /// The package manager ran but failed; this is a real error to report.
        Failed(String),
    }

    /// Run `lichen path language-server [--project <root>]` for `pm` and classify
    /// the result; see [`PmRun`].
    fn run_server(worktree: &Worktree, pm: &str) -> PmRun {
        let mut command = Command::new(pm).arg("path").arg("language-server");
        // A native-plugin project composes its own server over the plugin set;
        // an empty root is skipped as unavailable.
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

    /// Download the prebuilt `lichen` for this host into `$LICHEN_HOME/tools`
    /// via `curl` and mark it executable.
    fn bootstrap_lichen(home: &str) -> Result<(), String> {
        let asset_name = format!("{}-{}{}", LICHEN_BIN, host_target(), asset_suffix());
        let release = latest_github_release(
            RELEASE_REPO,
            GithubReleaseOptions {
                require_assets: true,
                // Toolchain releases are full releases; requiring a pre-release
                // would find nothing (see the release-lichen workflow).
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
        // Lichen Home is outside the extension's sandbox, so
        // `make_file_executable` cannot touch it — spawn a real `chmod`.
        if !on_windows() {
            let out = Command::new("chmod")
                .args(["+x", home])
                .output()
                .map_err(|e| format!("cannot run chmod on `{LICHEN_BIN}`: {e}"))?;
            if out.status != Some(0) {
                return Err(format!(
                    "cannot mark `{LICHEN_BIN}` executable: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
        }
        Ok(())
    }

    /// Ensure the server is installed and return its absolute path —
    /// `$PATH`, then the canonical `lichen`, then bootstrap.
    fn resolve_via_lichen(
        worktree: &Worktree,
        self_: &mut LichenExtension,
    ) -> Result<String, String> {
        // Fast path: a previously located package manager.  If it has become
        // unusable it is cleared below, and we re-locate.
        if let Some(pm) = self_.cached_lichen.clone() {
            match run_server(worktree, &pm) {
                PmRun::Path(path) => return Ok(path),
                PmRun::Missing => self_.cached_lichen = None,
                PmRun::Failed(e) => return Err(e),
            }
        }

        // 1. A `lichen` already on `$PATH` — `which` only returns a path the
        //    shell can run, so no spawn probe is needed here.
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

        // 2. The canonical Lichen Home copy — `liche update` refreshes this
        //    exact file, so the extension stays in sync.
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
