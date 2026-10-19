# The package graph: `import`, `plug`, `link`, and the `depend` edge

> Status: **landed** — all three edges are recursive through `depend`, and each
> is measured end to end. [packages](packages.md) and
> [package-manager](package-manager.md) carry the mechanics; this note carries the
> ruling and the measurements.
> Points at: `crates/lichen-language/src/package.rs` (`PackageStore`),
> `crates/lichen-preprocess` (the block grammar), `crates/lichen-package` (the
> manager), `crates/lichen-language/src/run.rs` and the language server (the root
> paths), [native-link-library](native-link-library.md).

## The ruling

A package is a lichen source file. Its block may declare all four edges, and the
architecture is recursive through them:

```
package { import, plug, link } --depend--> package { import, plug, link }
```

- **`import`** binds a package's export. The compiler resolves it recursively
  (a package's own imports load first, cycles refused, artifacts frozen and
  cached).
- **`plug`** names a native plugin: a Rust crate that extends the compiler's
  value/operator vocabulary. A plug **monomorphizes the compiler**: the plugin
  set is composed into the compiler binary before it compiles the graph, so a
  different plugin set is a different compiler.
- **`link`** names a dynamic library. It is **not** a compiler specialization: the
  compiler stays the one it is, and the library is loaded when the package
  compiles (the manifest's wrapper source compiles in the package's module, the
  entries install in the process table). A linked library never changes the
  compiler's vocabulary.
- **`depend`** is the fetch edge under all three: it names the git source a
  package needs, and the package manager fetches it into the source cache so the
  compiler can resolve the edge by alias. The compiler does not fetch.

The two specializations are therefore distinct by construction: `plug` **rebuilds
the compiler**; `link` **loads a library**.

## What exists today (measured)

| edge | behavior |
|---|---|
| `import` | recursive and uniform; a package's own imports load first, cycles refused, artifacts content-cached |
| `depend` | every source stages its own: `build_package` registers a package's aliases before its block resolves, and the manager collects the graph's `depend`s by following imports (fetch `a`, then `d`) |
| `plug` | a compiler declares its plugin set once (`plugins::install`, written into a generated compiler's `main`); a `plug` outside that set is refused, and the manager collects the graph's plugs through imports and rebuilds the compiler over them |
| `link` | a package's `link` splices its wrapper and installs its entries; the library bytes are part of the package's artifact identity |

## The slices

1. **`link` in a package — landed.** `build_package` loads the block's
   libraries, splices the wrapper sources in front of the package's code,
   compiles with the link entries, and folds the library bytes into the
   package's artifact identity. Measured: a package declaring its own `link`,
   imported by a root program, answers `(1, 1452218): tuple{Int, Int}` — the
   entry runs with the package's own argument. A library built from another
   tree is refused by the build tag (§12 of the native-link note), which is why
   the first probe crashed: the dylib came from a different checkout.
2. **`depend` in a package — landed.** `build_package` stages the block's own
   `depend`/`plug` before its imports resolve; the manager's `fetch_reachable`
   walks the graph by `import`, deduplicating by `Depend::identity`, and fetches
   as the closure grows. Measured: a root depending on `a`, where `a/_.lichen`
   depends on `d` and imports it, prints `7: Int`.
3. **`plug` in a package — landed.** The store compares a file's `plug` against
   the binary's installed set (`crates/lichen-language/src/plugins.rs`), the
   generated compiler installs its set from its composition, and the manager
   collects the graph's plugins and rebuilds the compiler
   (`compiler_cache::ensure`). Measured: a root depending on `a`, where
   `a/_.lichen` plugs `lichen-std-native` and sorts an array, rebuilds a compiler
   over the plugin set and prints `[1, 2, 3]: array{Int, ?a}`; the stock compiler
   refuses the same file with "this file plugs 's'; rebuild the compiler with
   `lichen rebuild-plugin` to include it".
