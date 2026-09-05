# Isolating the preprocessor into its own crate

> Status: current
> Points at: `crates/lichen-preprocess` (the isolated preprocessor: the `@{…@}`
> block scanner, `lex.rs`/`parse.rs`, `Directive`, `Depend`, `Preprocessed`,
> `ResolvedImport`, `PreprocessDiag`, the `ImportResolver` trait, and the
> preprocessor import path `lichendir`/`sources_root`),
> `crates/lichen-span` (the shared `Span`/`line_starts`/`line_col` protocol),
> `crates/lichen-utils` (`hash` module: `Hash`, `sha256`, `hex`),
> `crates/lichen-language/src/preprocess/mod.rs` (a re-export shim),
> `crates/lichen-language/src/package.rs` (`PackageStore` implements the
> resolver), and `crates/lichen-package/src/preprocess.rs` (its re-export).

## The goal

The package manager (`crates/lichen-package`) should depend **only** on the
preprocessor — not on the whole `lichen-language` crate (which drags in the
highlevel/lowlevel/VM/compute stack).  To do that the preprocessor must be
isolated to its own crate, and any type it needs from a heavier crate must be
either protocol-shared or abstracted behind a trait.

## What moved

- **`crates/lichen-language/src/preprocess/{mod,lex,parse}.rs`** →
  `crates/lichen-preprocess/src/{lib,lex,parse}.rs`.  The block scanner, its
  mini-frontend, `Directive`, `Depend`, `Preprocessed`, `ResolvedImport`, the
  `block_*`/`split_block`/`depend_of` helpers, `preprocess`, `stage_depends`,
  and `scan_block` all live here now.
- **`Span` / `line_starts` / `line_col`** → `crates/lichen-span` (a tiny
  dependency-free crate), re-exported by `lichen-language-lex`
  (`lichen_language_lex::Span` still resolves).  The preprocessor needs the
  source-position protocol but not the lexer, so it depends on `lichen-span`
  rather than `lichen-language-lex`.
- **`lichendir` / `sources_root` / `SOURCES_DIR`** → `crates/lichen-preprocess`
  (it owns the preprocessor import path); `lichen-language::persist` re-exports
  them.
- **`Hash` / `sha256` / `hex`** → `crates/lichen-utils::hash`, so the package
  manager (which keys its compiler cache by them, see `compiler_cache.rs`) and
  the language artifact cache share one implementation without depending on
  each other.  `lichen-language::persist` re-exports them.

## The seams

The preprocessor never names a package store or a compile vocabulary.

- **Import resolution** is behind a small trait:
  `crate::ImportResolver<E>` with `resolve_import(base, path)` returning a
  vocabulary-agnostic `ResolvedPackage<E> { export, path, direct }`, plus
  `register_vendored(alias, dir)`.  The language crate's `PackageStore`
  implements it for `E = StaticNodeId`, adapting its own `PackageHandle`/`Diag`.
- **Data types are generic over the export handle `E`** (`Preprocessed<'_ , E>`,
  `ResolvedImport<E>`).  The language crate pins `E = StaticNodeId` via type
  aliases in its shim, so `liche_language::preprocess::{Preprocessed,
  ResolvedImport}` keep their old non-generic names.
- **Diagnostics are program-blind**: `PreprocessDiag { span, message }` (no
  checker payload, no program marker).  The language crate widens it to its
  `Diag` via `Diag::from_preprocess` at `Stage::Preprocess`.

## The shim

`lichen-language/src/preprocess/mod.rs` is a `pub use` re-export of the pure
items plus two generic wrappers (`preprocess`, `stage_depends`) that keep their
old signatures — generic over `V`/`O`/`C` and returning `Diag<CompiledProgram<V,O>>`
— so `package.rs`, `run.rs`, `compile.rs`, `readme.rs`, `cli.rs`, and
`lichen-language-server` call them unchanged.  `liche-package::preprocess` is likewise
a pure re-export.

## The package manager now depends only on the preprocessor

`crates/lichen-package` depends on `lichen-preprocess` (+ `lichen-utils` for the
cache-key hash) — **not** `lichen-language`.  Its `clean`/`cache gc` commands
delegate to the compiler binary (`lichen-compiler cache gc`), like `run`/`build`
already delegate, so it never constructs a `PackageStore` or names a `LangValue`.
The plugin-built compiler path (`plugin.rs`) only references `liche-language` in
the *generated* crate's source, never as a compile dependency.

## Notes

- The cache key in `compiler_cache.rs` is now versioned by
  `env!("CARGO_PKG_VERSION")` of the package manager (the toolchain version)
  rather than `lichen_language::VERSION`.  The crates are released together, so
  this tracks the library version; it intentionally leaves the other core
  crates out, as the old key did.
- The vendored Zed grammar workspace (`lichen-language-zed/grammars/lichen/`)
  is a separate snapshot built on its own; it is not updated by this change.
