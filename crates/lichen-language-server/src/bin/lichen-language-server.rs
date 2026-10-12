//! The `lichen-language-server` binary: the shipping vocabulary's LSP server.
//! See `docs/notes/language-toolchain.md`.

fn main() {
    // One slot per plugin set, as the compiler uses: `docs/notes/liche-lsp-home.md` §4.
    let cache_root = lichen_language::persist::shipping_cache_root();
    // The shipping server is the compute sub-language, so it resolves the same
    // `compute.lichen` the compiler does.
    lichen_language_server::server::main::<lichen_compute::language::LangProgram>(
        &cache_root,
        lichen_compute::language::native_packages().to_vec(),
    );
}
