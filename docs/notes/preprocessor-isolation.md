# Isolating the preprocessor

> Status: current
> Points at: `crates/lichen-preprocess` (the isolated preprocessor: the `---…---`
> block scanner, `lex.rs`/`parse.rs`, `Directive`, `Depend`, `Preprocessed`,
> `ResolvedImport`, `PreprocessDiag`, the `ImportResolver` trait, and the
> preprocessor import path `lichendir`/`sources_root`),
> `crates/lichen-span` (the shared `Span`/`line_starts`/`line_col` protocol),
> `crates/lichen-utils` (`hash` module: `Hash`, `sha256`, `hex`),
> `crates/lichen-language/src/preprocess/mod.rs` (a re-export shim),
> `crates/lichen-language/src/package.rs` (`PackageStore` implements the
> resolver), and `crates/lichen-package/src/preprocess.rs` (its re-export).

The preprocessor is its own crate so that the package manager
(`crates/lichen-package`) can depend **only** on the `---…---` block grammar, not
on the whole `lichen-language` crate (which drags in the
highlevel/lowlevel/VM/compute stack). Any type the preprocessor needs from a
heavier crate is either protocol-shared or abstracted behind a trait, so the
dependency graph stays acyclic and the package manager's own dependency set
stays small.

## What the crate owns

- **The block scanner and its mini-frontend**:
  `crates/lichen-preprocess/src/{lib,lex,parse}.rs` hold `Directive`, `Depend`,
  `Preprocessed`, `ResolvedImport`, the `block_*`/`split_block`/`depend_of`
  helpers, `preprocess`, `stage_depends`, and `scan_block`.
- **The source-position protocol**: `Span` / `line_starts` / `line_col` live in
  `crates/lichen-span` (a tiny dependency-free crate), re-exported by
  `lichen-language-lex` (`lichen_language_lex::Span` still resolves). The
  preprocessor needs the protocol but not the lexer, so it depends on
  `lichen-span` rather than `lichen-language-lex`.
- **The preprocessor import path**: `lichendir` / `sources_root` / `SOURCES_DIR`
  live in `lichen-preprocess`; `lichen-language::persist` re-exports them.
- **The cache-key hash**: `Hash` / `sha256` / `hex` live in
  `crates/lichen-utils::hash`, so the package manager (which keys its compiler
  cache by them, see `compiler_cache.rs`) and the language artifact cache share
  one implementation without depending on each other.
  `lichen-language::persist` re-exports them.

## The seams

The preprocessor never names a package store or a compile vocabulary.

- **Import resolution** is behind a small trait: `crate::ImportResolver<E>` with
  `resolve_import(base, path)` returning a vocabulary-agnostic
  `ResolvedPackage<E> { export, path, direct }`, plus
  `register_vendored(alias, dir)`. The language crate's `PackageStore`
  implements it for `E = StaticNodeId`, adapting its own `PackageHandle`/`Diag`.
- **Data types are generic over the export handle `E`** (`Preprocessed<'_, E>`,
  `ResolvedImport<E>`). The language crate pins `E = StaticNodeId` via type
  aliases in its shim, so `lichen_language::preprocess::{Preprocessed,
  ResolvedImport}` keep their old non-generic names.
- **Diagnostics are program-blind**: `PreprocessDiag { span, message }` (no
  checker payload, no program marker). The language crate widens it to its
  `Diag` via `Diag::from_preprocess` at `Stage::Preprocess`.

## The shim

`lichen-language/src/preprocess/mod.rs` is a `pub use` re-export of the pure
items plus two generic wrappers (`preprocess`, `stage_depends`) that keep their
old call shape — generic over one program type `P: LangProgramShape`, returning
`(Preprocessed, Vec<Diag<P>>)` and `Vec<Diag<P>>` respectively — so `package.rs`,
`run.rs`, `cli.rs`, and `lichen-language-server` call them unchanged.
`lichen-package::preprocess` is likewise a pure re-export.

## The package manager's dependency set

`crates/lichen-package` depends on `lichen-preprocess` (+ `lichen-utils` for the
cache-key hash) and the type-independent `lichen-registry` — **not**
`lichen-language`. Its `run`/`build` commands delegate the compilation to the
compiler binary, and `clean` opens each plugin-composed compiler slot's
[`DeviceRegistry`](artifact-cache.md) and calls `gc()` itself, so it never
constructs a `PackageStore` or names a `LangValue`. The plugin-built compiler
path (`plugin.rs`) only references `lichen-language` in the *generated* crate's
source, never as a compile dependency.

## Notes

- The cache key in `compiler_cache.rs` is versioned by
  `env!("CARGO_PKG_VERSION")` of the package manager (the toolchain version)
  rather than `lichen_language::VERSION`. The crates are released together, so
  this tracks the library version; it intentionally leaves the other core crates
  out.
- The vendored Zed grammar workspace (`lichen-language-zed/grammars/lichen/`) is
  a separate snapshot built on its own; it is not part of this crate's build.
