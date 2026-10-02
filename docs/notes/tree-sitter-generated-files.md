# The tree-sitter generated-files testing method

> Status: current — the settled testing method.  The generated parser files are **not committed**; they are
> regenerated from `grammar.js` at build time.  The grammar tooling is **fully decoupled**
> from the workspace: the default `cargo test --workspace` and the Zed WASM build never build
> the grammar and never need the tree-sitter CLI.  Grammar changes are tested locally by the
> developer who changes `grammar.js`.

## What `tree-sitter-lichen` is, and how it is tested

`tree-sitter-lichen` is a tree-sitter grammar for highlighting/outline.  The **source** is
`grammar.js`; `tree-sitter generate` turns it into `src/parser.c` (plus
`src/grammar.json`, `src/node-types.json`, and the `src/tree_sitter/*.h` runtime headers).

The crate **is not a workspace member**.  Zed builds the grammar itself from the committed
`grammar.js` (it runs `tree-sitter generate` in-tree), so the generated files are never shipped
and never need to be committed.  The only in-repo consumer that builds the grammar is the
opt-in `grammar-consistency` guard in `lichen-language-zed`.

## The decision

The generated files are **build outputs** of `grammar.js`, not sources.  Committing them means
every language-syntax change forces a large, mostly-irrelevant diff, and `tree-sitter generate`
is not reproducible without pinning the exact CLI version (the repo was generated with
**tree-sitter v0.25.10**; regenerating with the current **v0.27.0** CLI churns `src/parser.c`
by ~6882 lines that are *development noise*, not the grammar change).  So they are **gitignored**
(`src/parser.c`, `src/grammar.json`, `src/node-types.json`, `src/tree_sitter/`) and regenerated
in-tree only when the grammar is actually built:

- Regeneration needs a `tree-sitter` CLI on PATH (e.g. `cargo install tree-sitter-cli`, any way
  you prefer).  Because the generated `parser.c` is no longer committed, the **exact version no
  longer matters** for reviewability — the only constraint is ABI compatibility with the
  `tree-sitter` *runtime* crate (0.25).  There is no Node/npm dependency in the repo.
- `bindings/rust/build.rs` regenerates the parser **when `src/parser.c` is missing or older
  than `grammar.js`**, by running `tree-sitter generate`.  It fails with a clear message to
  install a `tree-sitter` CLI on PATH if it is missing.

## Decoupled from the workspace

`tree-sitter-lichen` is in the workspace **`exclude`** list (not a member), and
`lichen-language-zed` pulls it in only through the **opt-in** `grammar-consistency` feature.
So a plain `cargo test --workspace` (i.e. CI) and the WASM extension build never compile the
grammar and **never need Node.js / the tree-sitter CLI**.

The developer who changes `grammar.js` tests it locally, on demand:

```sh
# build + run the samples test on the grammar itself (regenerates as needed)
cargo test --manifest-path tree-sitter-lichen/Cargo.toml

# validate the .scm queries + the pinned grammar rev (needs the grammar crate)
cargo test -p lichen-language-zed --features grammar-consistency
```

The samples test runs by default when testing the grammar crate directly; it can never block
`cargo test --workspace` because this crate isn't a workspace member. Both build steps need a
`tree-sitter` CLI on PATH (`cargo install tree-sitter-cli`, or your preferred method);
`build.rs` prints exactly that if it is absent.

## State in this branch

- `tree-sitter-lichen/grammar.js` is the source of truth (updated for the `==>` table
  separator, the `X::a` raw named read, and the full operator set — see
  [operators §9](operators.md#9-the-editor-grammar), which is where the `<`/`>`
  two-token decision and its two consequences are written down).
- `tree-sitter-lichen/queries/highlights.scm` colours every operator in that set,
  and is mirrored in `lichen-language-zed/languages/lichen/highlights.scm`.
- The generated `src/parser.c` etc. are **not** committed; they are regenerated on demand
  when the grammar is built.
- The default workspace build/test and the Zed WASM build have **no** grammar/CLI coupling.
