//! The Zed editor extension for Lichen.
//!
//! This is a *package-kind*-separate crate, not a *tool*-separate crate: it is
//! a WASM plugin that speaks `zed_extension_api`, so it cannot live in the same
//! binary target as the LSP server. Everything editor-y that it needs — the
//! span↔position conversion, the name-resolution index, and above all the
//! shared frontend artifacts from `lichen-language` — comes from
//! `lichen-language-server`'s library, so the extension and the server agree
//! byte-for-byte. See `docs/notes/language-toolchain.md`.
//!
//! Under the default (no-feature) build this crate is a thin host stub (so a
//! plain `cargo build` stays green). With the `zed` feature it becomes the
//! extension: it declares the `lichen-language-server` command that Zed launches
//! from `extension.toml`'s `language_servers` table.
//!
//! The installable extension is this directory (`extension.toml` + the `zed`
//! feature). Build the WASM for the current Zed target (`wasm32-wasip2`) and
//! install it as a dev extension. A Tree-sitter `tree-sitter-lichen` grammar is
//! the one piece still missing for syntax highlighting (see `extension.toml`).

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
    use zed_extension_api::{self as zed, Command, Extension, LanguageServerId, Worktree};

    use crate::{LANGUAGE_SERVER_BINARY, LichenExtension};

    impl Extension for LichenExtension {
        fn new() -> Self {
            LichenExtension
        }

        fn language_server_command(
            &mut self,
            _language_server_id: &LanguageServerId,
            _worktree: &Worktree,
        ) -> zed::Result<Command> {
            Ok(Command {
                command: LANGUAGE_SERVER_BINARY.to_string(),
                args: Vec::new(),
                env: Vec::new(),
            })
        }
    }

    zed_extension_api::register_extension!(LichenExtension);
}
