# The package manager

> Status: current
> Points at: `crates/lichen-package` (the crate), `src/main.rs` (the `lichen`
> CLI), `src/project.rs` (`Project`), `src/manifest.rs` (the `.lichen/package.toml`
> manifest), `src/git.rs` (git fetching), `src/toolchain.rs` (binary install),
> `src/plugin.rs` (compiler rebuild), `src/preprocess.rs` (the owned preprocessor
> import path), and `crates/lichen-language/src/package.rs` (`PackageStore`
> `register_vendored` / `resolve_import`).

The compiler binary is `lichen-compiler` (in `crates/lichen-language`, formerly
the `lichen` binary).  The `lichen` name is now the **package manager**: a
separate crate, `crates/lichen-package`, that resolves dependencies from git,
fetches the toolchain binaries, and rebuilds the compiler when a native plugin
is imported.

## Split of labour

- `lichen-compiler` (crates/lichen-language) — the frontend, the package store,
  the persistent device cache, `run`/`build`/`cache gc`.  Owns the `@{…@}`
  block *syntax* and its scanning/mini-frontend.
- `lichen` (crates/lichen-package) — the project workflow.  Owns the
  **preprocessor import path** (which dependency alias resolves to which file)
  and drives the language crate's block preprocessor for a project.

## The project and its manifest

A project is a directory with a `.lichen/package.toml` manifest (or none —
then it is a plain directory of programs, and the package manager is a no-op
wrapper over the compiler).

```toml
[package]
name = "my-project"

[dependencies]
math = { git = "https://github.com/you/lic-math", rev = "abc123" }
gpu   = { git = "https://github.com/you/lic-gpu", plugin = true }
```

`[dependencies]` maps an import alias to a git source (`git`, plus `rev` /
`branch` / `tag`).  A `plugin = true` dependency is a **native plugin**: its
import requires the compiler to be rebuilt (see below).

## Fetching from git

`lichen fetch` clones each dependency into `<project>/.lichen/deps/<alias>/`
via the `git` CLI (no libgit2 dependency); an existing clone is `fetch`ed and
checked out to the pinned revision.  Paths handed to git are normalized to
strip the Windows `\\?\` extended-path prefix, which `std::fs::canonicalize`
introduces and git refuses as a clone destination.

## Owning the preprocessor import path

The block scanner and mini-frontend stay in `crates/lichen-language`
(the language crate owns the grammar).  The package manager owns the
**resolution seam**: before the language crate's `preprocess` runs,
[`Project::stage`](src/project.rs) registers every fetched dependency's
vendored directory with the shared store via
[`PackageStore::register_vendored`](../../crates/lichen-language/src/package.rs).
[`resolve_import`](../../crates/lichen-language/src/package.rs) then resolves
`import "alias"` to the dependency's entry package (`lib.lichen`, then
`<alias>.lichen`, then the directory's sole `.lichen` file) and
`import "alias/sub.lichen"` relative to the vendored dir.  A file-like first
segment (`math.lichen`) never hits the alias map.

## Toolchain binaries

`lichen install compiler|language-server|all` drives `cargo install` against
the repository (or a local checkout), fetching and building the
`lichen-compiler` and `lichen-language-server` binaries.  Distribution is via
Cargo; "download" is fetch-and-build.

## Native plugins: rebuilding the compiler

A native plugin contributes vocabulary leaves to the `Program` marker at
compile time, so a compiler that knows a plugin must be built with it composed
in.  `lichen rebuild-plugin` generates a compiler crate under
`.lichen/compiler/<name>/` that composes the plugin's leaves with the shipped
set via `liche_language::lang_compose_vocabulary!` and runs `cargo build`,
producing a `lichen-compiler-<name>` binary.

> **Status:** the *composition* is real.  The generated compiler's tooling (its
> package store, persist codec, CLI, and `run` path) is currently monomorphic
> over the shipped `LangProgram`, so a compiler built with an *additional*
> plugin cannot yet route through the language crate's store/run machinery.
> That generalization — making the language layer's tooling generic over the
> `Program` marker — is the tracked follow-up in
> [plugin-taxonomy](plugin-taxonomy.md).  A rebuild over the shipping plugin
> set produces a fully working compiler.

## CLI

`lichen add/rm/list/fetch/run/build/install/rebuild-plugin/cache`, plus
`--version` / `--help`.  `run` and `build` fetch dependencies, stage them, and
then behave like `lichen-compiler`'s `run`/`build` (a project file's
`@{…@}` block resolves through the vendored aliases).
