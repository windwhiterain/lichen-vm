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
//! progress to Zed and asks the `liche` package manager to ensure it is present:
//! `liche path language-server` installs the **prebuilt** compiler + language
//! server into **Lichen Home** (`$LICHEN_HOME`, defaulting to `~/.lichen`) at the
//! package manager's own commit, and prints the binary path. The package manager
//! is located on `$PATH` or at `$LICHEN_HOME/tools/liche`; how it was installed
//! does not matter (see `docs/notes/language-toolchain.md`).
//!
//! When the worktree root is available, the extension passes it as
//! `--project <root>` so that a project importing a **native plugin** composes
//! its own language server over that plugin set (`liche path language-server
//! --project <root>` builds/caches the composed server into the plugin-set LSP
//! slot, understanding the plugin's leaves for diagnostics / hover / definition).

pub const LANGUAGE_NAME: &str = "Lichen";
pub const LANGUAGE_ID: &str = "lichen";
pub const FILE_EXTENSIONS: &[&str] = &["lichen"];
pub const GRAMMAR_SCOPE: &str = "source.lichen";
/// The LSP binary this extension instructs Zed to launch.
pub const LANGUAGE_SERVER_BINARY: &str = "lichen-language-server";

/// The extension type. Non-`zed` builds expose this as metadata only.
pub struct LichenExtension;

#[cfg(feature = "zed")]
mod zed_impl {
    use zed_extension_api::{
        self as zed, Command, Extension, LanguageServerId, LanguageServerInstallationStatus, Os,
        Worktree, current_platform, set_language_server_installation_status,
    };

    use crate::{LANGUAGE_SERVER_BINARY, LichenExtension};

    impl Extension for LichenExtension {
        fn new() -> Self {
            LichenExtension
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

            // The toolchain is managed by the `liche` package manager, which
            // installs the prebuilt compiler + language server into Lichen Home
            // at its own commit. Ask it to ensure the server is present and print
            // its path, then hand that path to Zed.
            match resolve_via_liche(worktree) {
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

    /// Whether the current OS is Windows (affects the `liche` executable suffix).
    fn on_windows() -> bool {
        matches!(current_platform().0, Os::Windows)
    }

    /// The shell environment as `(key, value)` pairs.
    fn shell_env(worktree: &Worktree) -> Vec<(String, String)> {
        worktree.shell_env()
    }

    /// Resolve `$LICHEN_HOME`, defaulting to `~/.lichen` (per
    /// `liche_language::persist::lichendir`).
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

    /// Locate the `liche` package manager: on `$PATH`, else the canonical copy in
    /// `$LICHEN_HOME/tools/liche`.
    fn liche_binary(worktree: &Worktree) -> Result<String, String> {
        if let Some(path) = worktree.which("liche") {
            return Ok(path);
        }
        let exe = if on_windows() { "liche.exe" } else { "liche" };
        Ok(format!("{}/tools/{exe}", lichen_home(worktree)))
    }

    /// Ensure the server is installed (asking the `liche` package manager, which
    /// installs the prebuilt compiler + language server into Lichen Home at its
    /// own commit) and return its absolute path.  When the worktree root is
    /// available it is passed as `--project <root>` so a project with native
    /// plugins composes its own server; when no root is available the shipping
    /// server is resolved.
    fn resolve_via_liche(worktree: &Worktree) -> Result<String, String> {
        let liche = liche_binary(worktree)?;
        let mut command = Command::new(liche.as_str())
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
            .map_err(|e| format!("cannot run `{liche} path language-server`: {e}"))?;
        if out.status != Some(0) {
            return Err(format!(
                "`{liche} path language-server` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() {
            Err("`liche path language-server` returned an empty path".to_string())
        } else {
            Ok(path)
        }
    }

    zed_extension_api::register_extension!(LichenExtension);
}
