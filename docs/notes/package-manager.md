# The package manager

> Status: current — the composition is real, and the language layer's tooling is
> generic over the composed program (`LangProgramShape`); the composed compiler
> binary itself is still monomorphic over the shipped `LangProgram`, so a
> compiler built with an *additional* plugin cannot yet route through the
> language crate's store/run machinery end to end (see
> [plugin-taxonomy](plugin-taxonomy.md)).
> Points at: `crates/lichen-package` (the crate), `src/main.rs` (the `lichen`
> CLI), `src/project.rs` (`Project`), `src/git.rs` (git fetching into the
> lichen-home source cache), `src/toolchain.rs` (binary install), `src/plugin.rs`
> (compiler rebuild), and the isolated preprocessor crate `crates/lichen-preprocess`
> (the block grammar, `Depend`, the preprocessor import path), plus
> `crates/lichen-language/src/package.rs` (`PackageStore` `register_vendored` /
> `resolve_import`).

The compiler binary is `lichen-compiler` (in `crates/lichen-compiler`, formerly
the `lichen` binary).  The `lichen` name is now the **package manager**: a
separate crate, `crates/lichen-package`, that resolves git dependencies from a
file's own `---…---` block, fetches the toolchain binaries, and rebuilds the
compiler when a native plugin is imported.

There is **no project manifest**: dependencies are declared per file.

## Splitting the work

- `lichen-compiler` (crates/lichen-compiler) — the command-line surface and the
  binary; the frontend, the package store, the persistent device cache and
  `run`/`build` live in the compiler library, `crates/lichen-language`.
  Consumes the `---…---` block grammar and the `Depend` type from the isolated
  [`lichen-preprocess`](../../crates/lichen-preprocess/) crate (which owns the
  block *syntax* and the preprocessor import path).
- `lichen` (crates/lichen-package) — the project workflow.  Owns the
  **preprocessor import path** (which dependency alias resolves to which file)
  and drives the compiler binary (or a plugin-built one) for a project.  It
  depends only on `lichen-preprocess` (+ the type-independent
  `lichen-registry`), not on the language/VM stack.

## Declaring dependencies

Each file opens its `---…---` block with `name = depend "url"`:

```lichen
---
  math = depend "https://github.com/you/lic-math" rev = "abc123"
  gpu = depend "https://github.com/you/lic-gpu" plugin
  math = import "math"
---
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
[`Project::stage`](../../crates/lichen-package/src/project.rs) fetches every `depend` into the source cache
and registers each alias with the shared store via
[`PackageStore::register_vendored`](../../crates/lichen-language/src/package.rs).
[`resolve_import`](../../crates/lichen-language/src/package.rs) then resolves
`import "alias"` to the dependency's entry package (`_.lichen`, then
`<alias>.lichen`, then the directory's sole `.lichen` file) and
`import "alias/sub.lichen"` relative to the vendored dir.  A file-like first
segment (`math.lichen`) never hits the alias map.

`lichen-preprocess` only knows an [`ImportResolver`](../../crates/lichen-preprocess/src/lib.rs)
trait for import resolution — it never names a package store or a compile
vocabulary.  The language crate's `PackageStore` implements that trait (adapting
its `PackageHandle`/`Diag`), and the package manager drives the scanner through
`lichen-preprocess` directly.

## Toolchain binaries

Every binary is a **prebuilt release asset**, fetched with `curl` and never built on the
user's machine. The two classes live in different places:
**plugin-sensitive** tools (compiler, language server) are composed per plugin set and resolve
under `<lichendir>/compilers/<plugin-set-key>/`, with the shipping (no-extra-plugin) binaries
in the base plugin-set slot, while **non-plugin-sensitive** ones (formatter, the package
manager itself) are one fixed binary at `<lichendir>/tools/<name>`.

`lichen install` addresses the release tagged with the binary's own commit. `build.rs` runs
`git rev-parse HEAD` from the package directory and emits the result as
`LICHEN_BUILD_COMMIT`; the release tag is that commit's first 12 hex characters, never the
raw 40-hex SHA, which GitHub rejects as a tag. Built outside a git checkout — the ordinary
case for a published package, whose `.git` cargo strips — the value is empty, and `lichen
install` **refuses** rather than chasing the repository tip, telling the caller to run
`lichen update`.

`lichen update` instead takes the newest **published** release from GitHub's newest-first
release list (pre-releases included), deliberately decoupled from the repository tip: the
maintainer publishes manually, so the tip may have no release at all. It writes
`$LICHEN_HOME/tools/lichen`, the copy the editor extension and the CLI resolve, and leaves a
copy on `$PATH` for the user to refresh; it reports "already current" when the tag derived
from its own commit is the newest.

A download lands through a unique temp sibling (pid plus a per-process counter, so nothing
can pre-create or replace the slot) and is `fsync`ed before the rename, so a crash cannot
install a truncated binary. The tag pins *which* revision was asked for, never *what*
arrived — audit item `D4` in [code-audit](code-audit.md).

The compiler is also installable from source for a developer with no published asset —
`cargo install --git <repo-url> lichen-compiler`, or `--path crates/lichen-compiler` — and
the package manager (`crates/lichen-package`, binary `lichen`) is the tool that fetches and
drives the binary.

## Native plugins: the compiler cache

A native plugin contributes vocabulary leaves to the `Program` marker at
compile time, so a compiler that knows a plugin must be built with it composed
in.  When a program declares a native plugin (`name = plug "url"`, or a
`name = depend "url" … plugin`), `lichen run`/`build` collect the program's
plugins, then ensure a compiler over them in a **cache under the lichen home**
(`<lichendir>/compilers/<key>/`), keyed by the lichen-library version and every
plugin's resolved version (its `HEAD` in the fetched source cache).  A cache
hit reuses the binary; a miss generates a compiler crate (composing the plugin
set via `lichen_language::lang_compose_vocabulary!`) and runs `cargo build`,
then drives the produced `lichen-compiler-<name>` binary.  `lichen
rebuild-plugin [<file|dir>]` is the explicit form of the same build.

The composition is real, and the generic library is available to the generated
compiler: the language layer's tooling is generic over the program shape
(`LangProgramShape`).  The composed compiler binary itself is still monomorphic
over the shipped `LangProgram`, so the tracked follow-up — driving an
additional-plugin compiler through the language layer's store/run path end to
end — is in [plugin-taxonomy](plugin-taxonomy.md).

The generated binary reports its own name: the compiler CLI's command name is overridden at
runtime from `argv[0]`, so a `lichen-compiler-<name>` prints itself in usage and help.

## CLI

The `lichen` command surface is declared with **clap** (derive) in
`crates/lichen-package/src/main.rs`: `fetch/run/build/clean/install/update/path/
rebuild-plugin`, plus `--version` / `--help`.  `run` and `build` fetch the
file's `depend`s/`plug`s into the source cache, then **spawn the compiler
binary** (the plugin-built compiler from the cache when the program imports a
native plugin, else the shipped `lichen-compiler`) — the package manager never
compiles in-process.  `clean` is the exception: it owns the device cache,
opening the shipping compiler's base cache root (`lichendir()`) and each
plugin-composed compiler cache slot's registry (`<lichendir>/compilers/<key>`,
a `lichen_registry::DeviceRegistry`) and calling `gc()` directly, so no
compiler subprocess and no language/VM dependency — the registry layer is
type-independent, in `crates/lichen-registry`.  A directory target processes
every `.lichen` file in it, each with its own dependencies.
