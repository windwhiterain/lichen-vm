# lichen-package

The **lichen package manager**, a binary named `lichen` (distributed via
Cargo: `cargo install --path crates/lichen-package`).  It owns the
package-manager workflow for a lichen project: resolve the git dependencies
declared in each file's `@{…@}` block, fetch them into the lichen-home source
cache, fetch the toolchain binaries, and rebuild the compiler when a native
plugin is imported.

It is the companion to the language compiler, now `lichen-compiler`
(`crates/lichen-language`, formerly the `lichen` binary).  There is **no
project manifest** — dependencies are declared per file.

## Commands

```text
lichen fetch <file|dir>       fetch the git deps declared by the file(s)' `depend` block
lichen run <file|dir>         fetch, then compile & run
lichen build <file>           fetch, then load & print type
lichen install <compiler|language-server|all>   fetch a toolchain binary
lichen rebuild-plugin [<file|dir>] [--name <n>] [--repo <u>]
                              rebuild the compiler for the file(s)' native plugins
lichen cache gc               reclaim device-cache artifacts
```

## Declaring dependencies

Each file declares its dependencies in its `@{…@}` block:

```lichen
@{
  depend "https://github.com/you/lic-math" as math rev = "abc123"
  depend "https://github.com/you/lic-gpu" as gpu plugin
  math = import "math"
@}
…
```

`depend "url"` takes `as NAME` (the import alias), `rev`/`branch`/`tag` (a pinned
checkout), `package` (the Rust crate of a native plugin), `sub` (a subdirectory
of the repo holding the source, for a monorepo dependency), and `plugin`.

`lichen fetch` clones each dependency into a **source cache under the lichen
home** — `$LICHEN_HOME` or `~/.lichen` (the same root as the compiler's
static-module cache), under `sources/<alias>/`.  `lichen run` / `lichen build`
fetch, then **stage** every dependency onto the preprocessor import path: an
`import "alias"` or `import "alias/sub.lichen"` resolves into the fetched
clone.  The preprocessor's scanner and mini-frontend live in the language
crate; the package manager owns the *import-path resolution* that turns a
dependency alias into a file.

## Native plugins and the compiler rebuild

A native plugin extends the compiler's value/operator vocabulary at *compile
time* (see `docs/notes/plugin-taxonomy.md`).  Mark a dependency with `plugin`;
`lichen rebuild-plugin` then generates a compiler crate that composes the
plugin's vocabulary into the shipped `LangProgram` and runs `cargo build`,
producing a `lichen-compiler-<name>` binary.  The composition scaffold is real;
the language tooling's generalization to an arbitrary program marker is the
tracked follow-up (per `docs/notes/plugin-taxonomy.md`).

## Library

The crate is also a library: `lichen_package::{Project, Depend, preprocess,
git, toolchain, plugin}`.  [`Project`](src/project.rs) is the unit the package
manager operates on.
