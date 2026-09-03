# Packages & import

> Status: current
> Points at: `crates/lichen-language/src/preprocess/` (the block scan + mini-frontend),
> `package.rs` (the store), `persist.rs` (the device cache), `run.rs`, `main.rs` (the
> CLI). The `@{…@}` block *syntax* is the spec's business: [language-spec.md §2.2](../language-spec.md).

A program may open with a single `@{ … }@` **preprocessor block**. Inside it,
`name = import "path"` loads a package bound to `name`, and `name = "value"` defines a
string metadata entry. The block is cut out by a pure byte scan (independent of the
lexer), so the language lexer/parser never see it; the code to compile is everything
after the block.

## Import

- The block syntax is the only import form: `@{ _p = import "geometry.lichen" @}`.
- A package is an ordinary lichen source file whose **final expression is the export** —
  a single frozen `[value, type]` ref.
- The preprocessor resolves each import through the shared store. A package's own
  imports resolve first (transitive dependencies load and freeze first); a cycle is a
  `cannot load package '…':` diagnostic anchored on the import that closes it.
- Diagnostics are re-anchored to the importing file's `@{…@}` line.

## The store

`PackageStore` is a shared registry plus a path cache:

- `new()` — in-memory (tests, tooling, embeddings).
- `with_cache_dir(dir)` — backed by the device's persistent store.
- `load_package(path)` / `resolve_import(base, import_path)` — load (or fetch) a package.
- `gc()` / `remove(path)` — explicitly reclaim / remove from the device cache.
- `packages` is public so a host or test can observe "the same package is frozen once".

A load compiles the package against the **shared registry**, then freezes the built
module. Dependency refs are key-carrying and verbatim, so every importer reads the
dependencies' shared payloads in place (see [static-modules](static-modules.md)).

## Persistent cache

With a cache directory, a load first runs the device's incremental verification over the
recorded dependency graph. When the whole graph is up to date, the artifact is
deserialized and registered under its device key and the compile is skipped; only the
changed chain recompiles, and each compiled package is serialized back.

## CLI (`lichen`)

- `lichen run <file|dir>` (or the bare `lichen <file|dir>`) — compile and run.
- `lichen build <file>` — load/freeze a package and print its exported type.
- `lichen cache gc` — reclaim unreachable artifacts from the device cache.
- `-h/--help`, `-V/--version`.

## Where the block metadata goes

`order` / `output` / prose in the block feed the README example sync
(`src/readme.rs`, `src/bin/sync-readme.rs`, enforced by `tests/readme.rs`).
