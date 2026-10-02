# tree-sitter-lichen

A [tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for the
[Lichen](https://github.com/lichen-lang/lichen-vm) programming language.

Lichen is a small, strongly-typed, dependent-ish functional language.  Its real
frontend (`crates/lichen-language`) is a strict, correctness-first lexer and
parser with whitespace-sensitive postfix forms.  This grammar is **deliberately
simple and permissive**: its job is syntax highlighting, outline and
bracket-matching, not semantic correctness.  It:

- accepts more than the strict parser (the whitespace "Glue" distinction is
  glossed over — the same delimiter may be read as postfix or as a fresh atom);
- models the `--- ... ---` preprocessor block, Lichen's only "prose" home, as a
  node so doc strings can be highlighted;
- keeps a light operator-precedence ladder for readable trees, in the language's
  own order (see [docs/notes/operators.md](../docs/notes/operators.md) §3), with
  one rule per level;
- keeps `<` and `>` as **two** tokens — a glued one that opens an angle form and
  a spaced one that is the order comparison — because the real language's rule
  for telling them apart is the adjacency, and an LR table has no lookahead to
  spend. That one rule is written up in
  [operators §9](../docs/notes/operators.md#9-the-editor-grammar).

## Usage

The generated files (`src/parser.c`, `src/node-types.json`, `src/tree_sitter/`) are build
outputs of `grammar.js` and are **not committed**. Regenerate them after editing `grammar.js`:

```sh
tree-sitter generate
```

`bindings/rust/build.rs` also regenerates them automatically when they are missing or older
than `grammar.js`, so building the Rust crate works from a clean checkout. It needs a
`tree-sitter` CLI on PATH (`cargo install tree-sitter-cli`, or your preferred method); it
prints that if it is missing.

Parse a file:

```sh
tree-sitter parse path/to/file.lichen
```

Inspect highlight captures:

```sh
tree-sitter query queries/highlights.scm path/to/file.lichen
```

## Layout

- `grammar.js` — the grammar definition (source of truth, committed).
- `src/parser.c`, `src/tree_sitter/`, `src/node-types.json` — generated output (gitignored; regenerated from `grammar.js` on demand).
- `queries/` — canonical tree-sitter queries (highlighting etc.).
- `bindings/rust/` — Rust crate (`tree-sitter-lichen`), compiled from `src/parser.c`.
- `package.json`, `tree-sitter.json` — grammar package config.

## Zed

This grammar lives in the `lichen-vm` monorepo so the Zed extension
(`lichen-language-zed`) can reference it as a sub-directory grammar via
the `path` field:

```toml
[grammars.lichen]
repository = "https://github.com/lichen-lang/lichen-vm"
rev = "<commit>"
path = "tree-sitter-lichen"
```

Queries for Zed live in the extension's `languages/lichen/` directory (Zed reads
queries from the extension, not the grammar repo); keep them in sync with
`queries/`.

## License

Apache-2.0.  See `LICENSE`.
