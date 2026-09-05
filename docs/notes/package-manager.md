# The package manager

> Status: current
> Points at: `crates/lichen-package` (the crate), `src/main.rs` (the `lichen`
> CLI), `src/project.rs` (`Project`), `src/git.rs` (git fetching into the
> lichen-home source cache), `src/toolchain.rs` (binary install), `src/plugin.rs`
> (compiler rebuild), and the isolated preprocessor crate `crates/lichen-preprocess`
> (the block grammar, `Depend`, the preprocessor import path), plus
> `crates/lichen-language/src/package.rs` (`PackageStore` `register_vendored` /
> `resolve_import`).

The compiler binary is `lichen-compiler` (in `crates/lichen-language`, formerly
the `lichen` binary).  The `lichen` name is now the **package manager**: a
separate crate, `crates/lichen-package`, that resolves git dependencies from a
file's own `@{…@}` block, fetches the toolchain binaries, and rebuilds the
compiler when a native plugin is imported.

There is **no project manifest**: dependencies are declared per file.

## Splitting the work

- `lichen-compiler` (crates/lichen-language) — the frontend, the package store,
  the persistent device cache, `run`/`build`/`cache gc`.  Consumes the `@{…@}`
  block grammar and the `Depend` type from the isolated
  [`lichen-preprocess`](../../crates/lichen-preprocess/) crate (which owns the
  block *syntax* and the preprocessor import path).
- `lichen` (crates/lichen-package) — the project workflow.  Owns the
  **preprocessor import path** (which dependency alias resolves to which file)
  and drives the compiler binary (or a plugin-built one) for a project.  It
  depends only on `lichen-preprocess`, not on the language/VM stack.

## Declaring dependencies

Each file opens its `@{…@}` block with `name = depend "url"`:

```lichen
@{
  math = depend "https://github.com/you/lic-math" rev = "abc123"
  gpu = depend "https://github.com/you/lic-gpu" plugin
  math = import "math"
@}
…
```

`name = depend "url"` binds the dependency under `name` (its import alias) and
takes `rev`/`branch`/`tag` (a pinned checkout), `package` (the Rust
crate name of a native plugin), `sub` (a subdirectory of the repo holding the
source, for a monorepo dependency), and `plugin` (a native plugin — importing
it requires the compiler to be rebuilt).  Mixing with `import "alias"` and
metadata entries is fine: the block is one statement set.

## Fetching from git into the lichen home

`lichen fetch <file|dir>` clones each `depend` into a **source cache under the
lichen home** — `$LICHEN_HOME` or `~/.lichen` (the same root as the compiler's
static-module cache), under `sources/<alias>/`.  The `git` CLI is used (no
libgit2 dependency); an existing source is `fetch`ed and checked out to the
pinned revision, so fetching is idempotent.  Paths handed to git are normalized
to strip the Windows `\\?\` extended-path prefix, which `std::fs::canonicalize`
introduces and git refuses as a clone destination.

## Owning the preprocessor import path

The block scanner and mini-frontend live in the isolated
[`lichen-preprocess`](../../crates/lichen-preprocess/) crate (the preprocessor
import path: [`lichendir`](../../crates/lichen-preprocess/src/lib.rs) /
[`sources_root`](../../crates/lichen-preprocess/src/lib.rs)).  The package
manager owns the **resolution seam**: before the compiler's `preprocess` runs,
[`Project::stage`](src/project.rs) fetches every `depend` into the source cache
and registers each alias with the shared store via
[`PackageStore::register_vendored`](../../crates/lichen-language/src/package.rs).
[`resolve_import`](../../crates/lichen-language/src/package.rs) then resolves
`import "alias"` to the dependency's entry package (`_.lichen`, then
`<alias>.lichen`, then the directory's sole `.lichen` file) and
`import "alias/sub.lichen"` relative to the vendored dir.  A file-like first
segment (`math.lichen`) never hits the alias map.

`liche-preprocess` only knows an [`ImportResolver`](../../crates/lichen-preprocess/src/lib.rs)
trait for import resolution — it never names a package store or a compile
vocabulary.  The language crate's `PackageStore` implements that trait (adapting
its `PackageHandle`/`Diag`), and the package manager drives the scanner through
`lichen-preprocess` directly.

## Toolchain binaries

`lichen install compiler|language-server|all` drives `cargo install` against
the repository (or a local checkout), fetching and building the
`lichen-compiler` and `lichen-language-server` binaries.  Distribution is via
Cargo; "download" is fetch-and-build.

## Native plugins: the compiler cache

A native plugin contributes vocabulary leaves to the `Program` marker at
compile time, so a compiler that knows a plugin must be built with it composed
in.  When a program declares a native plugin (`name = plug "url"`, or a
`name = depend "url" … plugin`), `lichen run`/`build` collect the program's
plugins, then ensure a compiler over them in a **cache under the lichen home**
(`<lichendir>/compilers/<key>/`), keyed by the lichen-library version and every
plugin's resolved version (its `HEAD` in the fetched source cache).  A cache
hit reuses the binary; a miss generates a compiler crate (composing the plugin
set via `liche_language::lang_compose_vocabulary!`) and runs `cargo build`,
then drives the produced `lichen-compiler-<name>` binary.  `lichen
rebuild-plugin [<file|dir>]` is the explicit form of the same build.

> **Status:** the *composition* is real.  The generated compiler's tooling (its
> package store, persist codec, CLI, and `run` path) is currently monomorphic
> over the shipped `LangProgram`, so a compiler built with an *additional*
> plugin cannot yet route through the language crate's store/run machinery.
> That generalization — making the language layer's tooling generic over the
> `Program` marker — is the tracked follow-up in
> [plugin-taxonomy](plugin-taxonomy.md).  A rebuild over the shipping plugin
> set produces a fully working compiler.

## CLI

`lichen fetch/run/build/clean/install/rebuild-plugin/cache`, plus `--version` /
`--help`.  `run`, `build`, `clean`, and `cache gc` fetch the file's
`depend`s/`plug`s into the source cache, then **spawn the compiler binary**
(`run`/`build` to compile & run the program — the plugin-built compiler from the
cache when the program imports a native plugin, else the shipped
`lichen-compiler`; `clean`/`cache gc` to reclaim device-cache artifacts) — the
package manager never compiles or GCs in-process, and never links the
language/VM stack.  A directory target processes every `.lichen` file in it,
each with its own dependencies.
