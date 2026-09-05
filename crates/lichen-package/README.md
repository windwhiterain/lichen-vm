# lichen-package

The **lichen package manager**, a binary named `lichen` (distributed via
Cargo: `cargo install --path crates/lichen-package`).  It owns the
package-manager workflow for a lichen project: resolve the git dependencies
declared in each file's `@{…@}` block, fetch them into the lichen-home source
cache, fetch the toolchain binaries, and build a plugin-composed compiler (in
the lichen-home compiler cache) when a native plugin is imported.

It is the companion to the language compiler, now `lichen-compiler`
(`crates/lichen-language`, formerly the `lichen` binary).  There is **no
project manifest** — dependencies are declared per file.

## Commands

```text
lichen fetch <file|dir>                        fetch the git deps declared by the file(s)' `depend` block
lichen run <file|dir>                          fetch, then compile & run via the compiler binary
lichen build <file>                            fetch, then compile & print the type via the compiler binary
lichen clean                                   reclaim device-cache artifacts
lichen install <compiler|language-server|all>  fetch a toolchain binary
lichen rebuild-plugin [<file|dir>] [--repo <u>] build (or reuse) a cached compiler
                                                 over the project's native plugins
lichen cache gc                                reclaim device-cache artifacts
```

`lichen run`/`build` **delegate the actual compilation to the compiler
binary** (spawned as a subprocess), so a plugin-built compiler is the one that
actually runs a program — the package manager never compiles in-process.

## Declaring dependencies

Each file declares its dependencies in its `@{…@}` block:

```lichen
@{
  math = depend "https://github.com/you/lic-math" rev = "abc123"
  gpu  = plug "https://github.com/you/lic-gpu" package = "lic-gpu-crate"
  math = import "math"
@}
…
```

`name = depend "url"` binds a git dependency under its import alias `name` and
takes `rev`/`branch`/`tag` (a pinned checkout), `package` (the Rust crate of a
native plugin), `sub` (a subdirectory of the repo holding the source, for a
monorepo dependency), and `plugin`.

`name = plug "url"` binds a **native plugin**: a Rust crate dependency that
extends the compiler's value/operator vocabulary at compile time.  It is a
`depend` that is always a plugin (`plugin = true`), so it is fetched into the
source cache the same way.  A `plug` plugin has **no virtual import path** (it
is a Rust crate, not a `.lichen` module); the builtin `lichen-compute` plugin
is the exception — it keeps its `compute.lichen` virtual path served by the
shipping compiler.

`lichen fetch` clones each dependency into a **source cache under the lichen
home** — `$LICHEN_HOME` or `~/.lichen` (the same root as the compiler's
static-module cache), under `sources/<alias>/`.  The compiler resolves each
`depend`/`plug` against that cache (see
[`lichen_preprocess::stage_depends`]), so `import "alias"` or
`import "alias/sub.lichen"` resolves into the fetched clone.

## Native plugins and the compiler cache

A native plugin extends the compiler's vocabulary at *compile time* (see
`docs/notes/plugin-taxonomy.md`).  When a program imports one (a `plug`, or a
`depend … plugin`), `lichen run`/`build` gather the program's plugins, select
(or build) a compiler over them, and drive it.  The compiler is built into a
**cache under the lichen home** (`<lichendir>/compilers/<key>/`), keyed by the
toolchain version (the package manager's own `CARGO_PKG_VERSION`, which tracks
the core crates' release) and every plugin's resolved
version (its `HEAD` in the fetched source cache).  The same plugin set +
toolchain version reuses the cached binary; a change to any plugin (or the
toolchain) keys a new slot.  `lichen rebuild-plugin` is the explicit form of the
same build.

The composition scaffold is real; the language tooling's generalization to an
arbitrary program marker (composing a *new* plugin's leaves, and routing a
generated compiler's store/run machinery over the composed `Program`) is the
tracked follow-up (per `docs/notes/plugin-taxonomy.md`).

## Library

The crate is also a library: `lichen_package::{Project, Depend, preprocess,
git, toolchain, plugin, compiler_cache}`.  [`Project`](src/project.rs) is the
unit the package manager operates on.
