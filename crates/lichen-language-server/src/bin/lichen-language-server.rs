//! The `lichen-language-server` binary: the shipping vocabulary's LSP server.
//! See `docs/notes/language-toolchain.md`.

use lichen_language::program::LangProgram;

fn main() {
    // One slot per plugin set, as the compiler uses: `docs/notes/liche-lsp-home.md` §4.
    let cache_root = lichen_language::persist::shipping_cache_root();
    lichen_language_server::server::main::<LangProgram>(&cache_root);
}
