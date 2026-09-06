//! The `lichen-language-server` binary: an LSP server for Lichen over stdio,
//! serving the **shipping** vocabulary.
//!
//! All of the actual editor behavior — span↔position conversion, name
//! resolution, the shared frontend artefacts — lives in the
//! `lichen-language-server` library (see [`lichen_language_server::server`] and
//! [`lichen_language_server::analysis::Doc`]).  This binary is a thin wrapper
//! that drives the shared generic server over the shipping
//! [`LangProgram`](lichen_language::program::LangProgram); a plugin-built server
//! (composed by the `lichen` package manager over a project's plugin set) is the
//! same [`lichen_language_server::server::main`] instantiated with its composed
//! program instead.
//!
//! Run it directly, or install it as the LSP binary that a Zed extension launches:
//!
//! ```text
//! cargo run -p lichen-language-server --bin lichen-language-server
//! ```

use lichen_language::program::LangProgram;

fn main() {
    // The shipping server caches under the empty plugin set's slot
    // (`compilers/<toolchain-key>`), consistent with every other vocabulary.
    let cache_root = lichen_language::persist::shipping_cache_root();
    lichen_language_server::server::main::<LangProgram>(&cache_root);
}
