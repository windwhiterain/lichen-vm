# The package graph: `import`, `plug`, `link`, and the `depend` edge

> Status: ruling recorded; the refactor is in slices. The current state is measured
> in [packages](packages.md) and [package-manager](package-manager.md); this note
> fixes the target.
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

| edge | current behavior |
|---|---|
| `import` | recursive and uniform; a package's own imports load first, cycles refused, artifacts content-cached |
| `depend` | staged only for the **root** source; an imported package's own `depend` is parsed into `Preprocessed.depends` and dropped |
| `plug` | parsed and normalized into a plugin-typed `Depend`; the language layer has no consumer. The manager can fetch plugins and rebuild a compiler over them (`lichen rebuild-plugin`), but the compiler is still monomorphic over the shipped program, so the composed binary cannot route a program end to end |
| `link` | honored only for the **root** (splice + entries in `run`, check-imports in the editor); `build_package` ignores `Preprocessed.links`, and packages compile with `no_native_ops()`, so a package's linked surface is silently dropped |

## The slices

1. **`link` in a package — landed.** `build_package` loads the block's
   libraries, splices the wrapper sources in front of the package's code,
   compiles with the link entries, and folds the library bytes into the
   package's artifact identity. Measured: a package declaring its own `link`,
   imported by a root program, answers `(1, 1452218): tuple{Int, Int}` — the
   entry runs with the package's own argument. A library built from another
   tree is refused by the build tag (§12 of the native-link note), which is why
   the first probe crashed: the dylib came from a different checkout.
   dependencies are fetched.
3. **`plug` in a package.** The manager collects the graph's plugin set and
   rebuilds the compiler over it; the compiler records its own plugin set and
   refuses a graph whose set differs (the program-tag gate's shape).
