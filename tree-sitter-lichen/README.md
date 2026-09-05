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
- models the `@{ ... @}` preprocessor block, Lichen's only "prose" home, as a
  node so doc strings can be highlighted;
- keeps a light operator-precedence ladder for readable trees.

## Usage

Regenerate the parser after editing `grammar.js`:

```sh
tree-sitter generate
```

Parse a file:

```sh
tree-sitter parse path/to/file.lichen
```

Inspect highlight captures:

```sh
tree-sitter query queries/highlights.scm path/to/file.lichen
```

## Layout

- `grammar.js` — the grammar definition (source of truth).
- `src/parser.c`, `src/tree_sitter/`, `src/node-types.json` — generated output.
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
