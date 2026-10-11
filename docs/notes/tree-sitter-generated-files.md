# The tree-sitter generated-files testing method

> Status: current
> Points at: `tree-sitter-lichen/` (`grammar.js`, `bindings/rust/build.rs`,
> `queries/`) and `lichen-language-zed` (the opt-in `grammar-consistency`
> feature).

The generated parser files are **not committed**; they are regenerated from
`grammar.js` at build time.  The grammar tooling is **fully decoupled** from the
workspace: the default `cargo test --workspace` and the Zed WASM build never
build the grammar and never need the tree-sitter CLI.  Grammar changes are
tested locally by the developer who changes `grammar.js`.

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

- Regeneration needs a `tree-sitter` CLI on PATH (e.g. `cargo install tree-sitter-cli`, an npm
  `tree-sitter-cli` under `tree-sitter-lichen/node_modules` — which `build.rs` finds as a
  fallback — or any other way you prefer).  The version the generated output in this repository
  was last produced with is **v0.25.10**; because the generated `parser.c` is no longer
  committed, the **exact version no longer matters** for reviewability, and the only constraint
  is ABI compatibility with the `tree-sitter` *runtime* crate (0.25).  There is no Node/npm
  dependency in the repo or in CI; an npm install is a local, gitignored convenience only.
- **What a checkout without a CLI can and cannot do.**  Nothing in the default workspace build,
  the workspace tests, or the Zed WASM build touches the grammar, so those all work with no CLI
  at all.  What does need one is the grammar itself: `cargo test --manifest-path
  tree-sitter-lichen/Cargo.toml` and `cargo test -p lichen-language-zed --features
  grammar-consistency` both build `tree-sitter-lichen`, and its `build.rs` regenerates
  `src/parser.c` whenever it is missing or older than `grammar.js`.  On a fresh clone the file is
  absent (it is not committed), so both commands fail in `build.rs` with the install message —
  measured here by removing the generated `src/` from a tree with no CLI, which panics at
  `bindings/rust/build.rs:45`.  A tree that already has the generated files and an unchanged
  `grammar.js` builds and tests fine without a CLI, because `build.rs` finds `parser.c` newer
  than the grammar and does nothing.  So the cost of the decision is exactly this: the grammar
  can be *built and tested* without the generator only where someone has already generated it,
  and a grammar *change* always needs one.
- `bindings/rust/build.rs` regenerates the parser **when `src/parser.c` is missing or older
  than `grammar.js`**, by running `tree-sitter generate`.  It fails with a clear message to
  install a `tree-sitter` CLI on PATH if it is missing.
- It watches exactly two paths, `grammar.js` and `src/parser.c`.  Cargo treats a
  `rerun-if-changed` path that does not exist as changed, so naming `src/parser.c` is what
  re-runs the build script when the generated parser appears or disappears; naming the whole
  generated `src/` directory instead would re-run the script after every regeneration.

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

The grammar's own tests run by default when testing the grammar crate directly; they can never
block `cargo test --workspace` because this crate isn't a workspace member. Both build steps
need a `tree-sitter` CLI (on PATH, or an npm install under `tree-sitter-lichen/node_modules`,
which `build.rs` finds as a fallback); `build.rs` prints exactly that if it is absent.

## The grammar's state

- `tree-sitter-lichen/grammar.js` is the source of truth, and it models the
  language's current surface: the keyword-led braced types (`tuple{…}`,
  `struct{…}`, `array{T, n}`, `set{E}`, `table{K, V}`), the parenthesised
  `set(…)`/`table(…)` values, the glued postfix reads (`a(k)`, `a[i]`, `X{n}`,
  `X::a`), the infix `t <== k`, `@in`/`@any`/`@shallow`/`@loop`, and the prefix
  keyword operators.  The retired angle forms (`<…>`, `struct<…>`, `array<…, n>`,
  the raw index `X<e>`) no longer parse; the grammar's own
  `retired_spellings_have_error_nodes` test pins that.  See
  [operators §10](operators.md#10-the-editor-grammar) for the `<`/`>` history and
  the one reading the grammar still cannot take (the container of a glued `{n}`,
  which is a checker unify rather than a spelling).
- `tree-sitter-lichen/queries/highlights.scm` colours every operator and keyword
  in that set, and is mirrored in
  `lichen-language-zed/languages/lichen/highlights.scm`.
- The generated `src/parser.c` etc. are **not** committed; they are regenerated
  on demand when the grammar is built.
- The default workspace build/test and the Zed WASM build have **no**
  grammar/CLI coupling.
- The Zed extension pins the grammar by `rev` in
  `lichen-language-zed/extension.toml`, and its `grammar_rev_is_not_stale` guard
  fails whenever `rev` is behind the last commit touching the grammar or the
  queries — so a grammar change and its `rev` bump are one commit, and a
  long-lived branch has to re-bump `rev` after it lands on `dev`.
