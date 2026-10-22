# tree-sitter-lichen

A [tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for the
[Lichen](https://github.com/windwhiterain/lichen-vm) programming language.

Lichen is a small, strongly-typed, dependent-ish functional language.  Its real
frontend (`crates/lichen-language`) is a strict, correctness-first lexer and
parser with whitespace-sensitive postfix forms.  This grammar is **deliberately
simple and permissive**: its job is syntax highlighting, outline and
bracket-matching, not semantic correctness.  It:

- models the current surface syntax — the keyword-led braced types (`tuple{…}`,
  `struct{…}`, `array{T, n}`, `set{E}`, `table{K, V}`), the parenthesised
  `set(…)`/`table(…)` values, the glued postfix reads (`a(k)`, `a[i]`), the infix
  `t <== k`, `@in`/`@any`/`@shallow`/`@loop`, and the prefix keyword operators.
  The retired angle forms (`<…>`, `struct<…>`, `array<…, n>`) do not parse;
  `tests/samples.rs` pins that;
- models the `--- ... ---` preprocessor block, Lichen's only "prose" home, as a
  node so doc strings can be highlighted;
- keeps a light operator-precedence ladder for readable trees, in the language's
  own order (see [docs/notes/operators.md](../docs/notes/operators.md) §3), with
  one rule per level;
- keeps the whitespace "Glue" distinction for the postfix forms that need it
  (`token.immediate`), so `a[0]` is an index and `a [0]` applies `a` to an array
  — the real language's own rule.  That model is written up in
  [operators §10](../docs/notes/operators.md#10-the-editor-grammar).

## Usage

The generated files (`src/parser.c`, `src/node-types.json`, `src/tree_sitter/`) are build
outputs of `grammar.js` and are **not committed**. Regenerate them after editing `grammar.js`:

```sh
tree-sitter generate
```

`bindings/rust/build.rs` also regenerates them automatically when they are missing or older
than `grammar.js`, so building the Rust crate works from a clean checkout. It needs a
`tree-sitter` CLI (on `PATH`, or an npm `tree-sitter-cli` under `node_modules/`, which
`build.rs` finds as a fallback); it prints that if it is missing.

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
repository = "https://github.com/windwhiterain/lichen-vm"
rev = "<commit>"
path = "tree-sitter-lichen"
```

Queries for Zed live in the extension's `languages/lichen/` directory (Zed reads
queries from the extension, not the grammar repo); keep them in sync with
`queries/`.

## License

Apache-2.0.  See `LICENSE`.
