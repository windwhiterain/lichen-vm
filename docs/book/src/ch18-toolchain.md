# The Toolchain

A language is more than its compiler. This chapter tours the programs and
crates that come with lichen: the compiler's own command line, the
language server and its editor integrations, the package manager, and the
shared cache that all of them use. It ends with a map of the repository
for a reader who wants to look under the hood.

## The compiler's command line

The compiler binary is `lichen-compiler` (Chapter 1). Its surface,
abridged, is:

```text
$ lichen-compiler [OPTIONS] [program] [COMMAND]

Commands:
  run    Compile & run a program (a file, or every `.lichen` file in a directory)
  build  Compile a program and print its exported type

Options:
      --pack <path>  A placement pack to install for this run; name it once per pack
  -h, --help         Print help
  -V, --version      Print version
```

A bare path runs the program; `run` says so explicitly; `build` prints the
type the file exports (Chapter 15). A directory path runs each `.lichen`
file directly inside it (Chapter 1). `--pack` installs a **placement pack**
for the run: a runner's guidance for how the fusion machinery places a
node or fuses a region, which never says what anything computes
(Chapter 17). It does not change the program's meaning.

The compiler writes the program's answer to **standard output**, and its
compile diagnostics there too, with a non-zero exit. Its **reports** — a
refused computation's reason, a pass's lines — go to standard error
(Chapter 14).

## The language server

`lichen-language-server` speaks the Language Server Protocol. Its
capabilities are:

- **diagnostics** — the same checks the compiler runs, published as you type;
- **hover** — a name's type, and a prelude name's carried type;
- **go to definition** — a name's binding;
- **completion** — the names in scope at the cursor, which is the same set
  an unresolved name's "did you mean" candidates come from;
- **semantic tokens** — syntax coloring, with or without the tree-sitter
  grammar.

A document is analyzed incrementally: the open buffer stays in the
server's process and is re-analyzed by a per-document session, while its
settled imports go through the persistent store (below), so an imported
package is compiled once and shared with the compiler across processes.
The design principle behind that is worth stating: **the frontend is the
artifact**. The server, the compiler, and any future tool consume the same
lexer, parser and checker; there is no second grammar to drift.

## Editors

The repository ships editor integrations:

- [`lichen-language-zed/`](https://github.com/windwhiterain/lichen-vm/tree/dev/lichen-language-zed) is a Zed extension:
  a manifest, the LSP command, and the tree-sitter grammar's query files.
- [`helix/`](https://github.com/windwhiterain/lichen-vm/tree/dev/helix) holds a Helix configuration that launches the
  server and the grammar.
- [`tree-sitter-lichen/`](https://github.com/windwhiterain/lichen-vm/tree/dev/tree-sitter-lichen) is the grammar
  itself, built from `grammar.js` and kept out of the workspace on purpose
  (the Zed extension builds it).

The server is editor-agnostic: any client that speaks LSP and can run a
command gets diagnostics, hover and completion.

## The package manager

The `lichen` binary (Chapter 15) reads the header's `depend` and `plug`
directives, fetches git dependencies into the lichen-home source cache,
stages them for imports, downloads the compiler and language-server
binaries a project pins, and rebuilds the compiler when a native plugin is
imported. The compiler itself is a pure function of the sources it is
given; the package manager's job is to make them present.

## The cache: Lichen Home

All the tools share one cache root, the **Lichen Home**, under the user's
home directory (`~/.lichen` on the machines where this book was written).
Inside it, each compiler vocabulary has a slot keyed by its plugin set:

```text
$LICHEN_HOME/
  compilers/<plugin-set-key>/
    artifacts/     the persistent artifact store
    builtin/       the materialized core prelude (core.lichen)
```

Two things follow. First, a build's compiled artifacts are **shared across
processes**: a second run of the compiler, or the language server, reads
what the first one stored, which is why a warm start is fast. Second, the
core prelude is a real file under `builtin/`, which is why a failure
inside it can name `…/builtin/core.lichen:3:39` (Chapter 11). The artifact
store's key is content, not time: an artifact is valid for the sources and
the vocabulary that produced it, and an edit that changes neither does not
invalidate it.

## The repository's map

The core of the tree is four layers, one per crate (the
[architecture overview](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/overview.md) is the long form):

```text
crates/lichen-language      text → lex → parse → resolve → highlevel IR → checked program
crates/lichen-highlevel     IR, checker, attribute extension point, module building
crates/lichen-lowlevel      the VM: values, lazy evaluation, unification, block GC
crates/lichen-utils         shared extension plumbing
```

Around them sit the tools and the compute stack:

```text
crates/lichen-compiler      the CLI binary (and lib) this book uses
crates/lichen-language-server  the LSP server
crates/lichen-package       the package manager binary
crates/lichen-kernel-ir     the kernel IR (Chapter 17)
crates/lichen-compute       the compute package, the JITs, buffers and the graph path
crates/lichen-compute-gpu   the SPIR-V emitter and device dispatch
crates/lichen-runtime       the recorded-graph and fusion artifacts
crates/lichen-registry      the persistent artifact store
crates/lichen-tools         repository tooling (the README sync)
crates/lichen-testkit       an IR writer/probe harness for the crate tests
```

The syntax crates (`lichen-span`, `lichen-language-lex`,
`lichen-language-parser`) sit under `lichen-language` and are re-exported
by it, so a tool that wants only tokens or the AST can depend on the small
crates.

## Building and testing

From a checkout:

```text
$ cargo build
$ cargo test -p lichen-compute --test lichen_compute every_example_runs_and_prints_what_it_declares
```

The first builds every binary and library. The second is the example
suite (Chapter 16), the test that every user-facing claim can be checked
against. The workspace's own tests are the crate-level ones; they are for
people changing the implementation, not for people writing programs.

## Summary

- `lichen-compiler` runs programs and directories, builds a module's
  exported type, and installs placement packs with `--pack`.
- The program's answer is on stdout; the compiler's reports are on stderr.
- `lichen-language-server` provides diagnostics, hover, definition,
  completion and semantic tokens, over an incrementally analyzed buffer and
  a shared persistent cache.
- Zed, Helix and tree-sitter integrations use the same server and grammar.
- The package manager fetches dependencies and plugins; the compiler never
  fetches.
- The Lichen Home cache stores compiled artifacts by content and materializes
  the core prelude as a real file.
- The repository's four layers are language, highlevel, lowlevel and utils,
  with the compiler, server, package manager and compute stack around them.

## Exercises

1. Run `lichen-compiler --help` and identify which subcommand compiles a
   module without running it.
2. Run the example suite test and read its `test result:` line.
3. Find the Lichen Home on your machine and locate the materialized
   `core.lichen`. Which line does the prelude's `add` contract live on?
4. Explain why the language server and the compiler cannot disagree about a
   program's syntax.
5. Name the crate that owns the compute package and the one that emits
   SPIR-V.
