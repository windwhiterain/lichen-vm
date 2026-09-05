# lichen-package

The **lichen package manager**, a binary named `lichen` (distributed via
Cargo: `cargo install --path crates/lichen-package`).  It owns the
package-manager workflow for a lichen project: resolve dependencies from git,
fetch the toolchain binaries, and rebuild the compiler when a native plugin is
imported.

It is the companion to the language compiler, now `lichen-compiler`
(`crates/lichen-language`, formerly the `lichen` binary).

## Commands

```text
lichen add <git-url> [--name <alias>] [--rev <rev>] [--branch <b>] [--tag <t>]
                    [--package <crate>] [--plugin]   add a git dependency
lichen rm <alias>                                     remove a dependency
lichen list                                           list dependencies
lichen fetch                                          clone/update all git deps
lichen run <file|dir>                                 fetch, then compile & run
lichen build <file>                                   fetch, then load & print type
lichen install <compiler|language-server|all>         fetch a toolchain binary
lichen rebuild-plugin [--name <n>] [--repo <url>]     rebuild the compiler for the
                                                      project's native plugins
lichen cache gc                                       reclaim device-cache artifacts
```

## Git dependencies

A project declares its dependencies in `.lichen/package.toml`:

```toml
[package]
name = "my-project"

[dependencies]
math = { git = "https://github.com/you/lic-math", rev = "abc123" }
gpu   = { git = "https://github.com/you/lic-gpu", plugin = true }
```

`lichen fetch` clones each dependency into `.lichen/deps/<alias>/` (existing
clones are `fetch`ed and checked out to the recorded revision).  `lichen run` /
`lichen build` fetch, then **stage** every dependency onto the preprocessor
import path: an `@import` directive of the form `import "alias"` or
`import "alias/sub.lichen"` resolves into the vendored clone.  The
preprocessor's scanner and mini-frontend live in the language crate; the
package manager owns the *import-path resolution* that turns a dependency
alias into a file.

## Native plugins and the compiler rebuild

A native plugin extends the compiler's value/operator vocabulary at *compile
time* (see `docs/notes/plugin-taxonomy.md`).  Mark a dependency
`plugin = true`; `lichen rebuild-plugin` then generates a compiler crate that
composes the plugin's vocabulary into the shipped `LangProgram` and runs
`cargo build`, producing a `lichen-compiler-<name>` binary.  The composition
scaffold is real; the language tooling's generalization to an arbitrary
program marker is the tracked follow-up (per `docs/notes/plugin-taxonomy.md`).

## Library

The crate is also a library: `lichen_package::{Project, Manifest, Dependency,
preprocess, git, toolchain, plugin}`.  [`Project`](src/project.rs) is the unit
the package manager operates on.
