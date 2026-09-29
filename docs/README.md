# lichen-vm documentation

This folder is the project's **documentation map**. lichen-vm keeps its
documentation in exactly two places, on purpose:

- **In-code Rust doc comments** (`//!` and `///`) — the authoritative, always-current
  description of what each module and item does. They live next to the code they
  describe, so they cannot drift.
- **This folder** — human/agent-oriented *feature notes* plus the **language spec**.
  A note explains what a feature is, why it exists, and how to use it, then points at
  the modules that implement it. It never repeats the implementation detail the
  rustdoc already carries.

> **Rule: one doc per concern.** Each feature has exactly one note here, and each fact
> is stated in exactly one place — the note, the spec, or the rustdoc. If you are
> about to repeat a fact, *link to it* instead. A note's source of truth is the code;
> read the note, then read the modules it names.

## Read order

1. [README](../README.md) — what lichen is, the crate map, quick start.
2. [Architecture overview](notes/overview.md) — the core idea (`[value, type]`
   pairs, `Type : Type`) and the layering.
3. The feature note for the crate you are working in.

## Status legend

Every note opens with a `> Status:` line:

- `current` — describes shipped behaviour (matches today's code).
- `historical` — a past decision or investigation, kept for context; never the
  authority on current behaviour.
- `proposed` — a design/investigation for not-yet-implemented behaviour; the
  design rationale, not a description of what the code does today.

## Feature notes

| Note | Crate(s) | Status |
|---|---|---|
| [Architecture overview](notes/overview.md) | — | current |
| [Lowlevel VM](notes/lowlevel-vm.md) | `lichen-lowlevel` | current |
| [Static modules & registry](notes/static-modules.md) | `lichen-lowlevel`, `lichen-language` (`persist`) | current |
| [Optional static shape](notes/lichen-lowlevel-shape.md) | `lichen-lowlevel` (`LowShape`), `lichen-language` (`compute`/`persist`) | current |
| [Low types for a lowlevel-based JIT](notes/lowlevel-low-types.md) | `lichen-lowlevel` (`LowShape`), `lichen-compute` | proposed |
| [Type-system cleanup plan](notes/type-system-cleanup-plan.md) | `lichen-lowlevel`, `lichen-highlevel`, `lichen-compute` | current (Phases 0/1a/1b/1c/2/3a); proposed (the rest of Phase 3, Phase 4+) |
| [Extensible attributes](notes/attributes.md) | `lichen-highlevel` (`attr`/`shape`/`ir`/`checker`), `lichen-language` (`program`/`compile`) | current |
| [Doc attribute rework plan](notes/doc-attribute-rework-plan.md) | `lichen-language`, `lichen-highlevel` (`attr`), `lichen-language-server` (`analysis`) | current |
| [The compiler-plugin model](notes/compiler-plugin.md) | `lichen-utils` (`extend`), `lichen-lowlevel`, `lichen-highlevel` (`program`/`native`), `lichen-compute` | current |
| [The plugin taxonomy](notes/plugin-taxonomy.md) | `lichen-highlevel` (`plugin`), `lichen-compute`, `lichen-std-native`, `lichen-perspective`, `lichen-language` (`program`/`package`) | current |
| [Raw index `X<e>`](notes/raw-index.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language` (`compile`), `lichen-highlevel` (`ir`/`checker`), `lichen-render` | current |
| [Raw named read `X::a`](notes/raw-field.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language` (`compile`), `lichen-highlevel` (`ir`/`checker`) | current |
| [Placeholder `_` anywhere](notes/placeholder-anywhere.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language-server` (`analysis`) | current |
| [No type mode](notes/no-type-mode.md) | `lichen-language-parser` | current |
| [Record programs (modules)](notes/record-program.md) | `lichen-language-parser`, `lichen-language` (`compile`/`session`), `lichen-language-server` (`analysis`) | current |
| [Separating lexer & parser from the language](notes/frontend-syntax-separation.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language`, `lichen-highlevel` | current |
| [Incremental parse/compile](notes/incremental-parse-compile.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language` (`resolve`/`compile`/`session`), `lichen-highlevel` (`checker`) | current (T3/T4 proposed) |
| [Build performance](notes/build-performance.md) | `lichen-language-parser` | current |
| [Language toolchain](notes/language-toolchain.md) | `lichen-language`, `lichen-language-server`, `lichen-language-zed` | current |
| [Lichen Home for the LSP](notes/liche-lsp-home.md) | `lichen-language-server` (`home`), `lichen-language` (`package`/`persist`) | current |
| [Packages & import](notes/packages.md) | `lichen-language` (`preprocess`/`package`/`persist`/`run`), `lichen-preprocess` | current |
| [Package manager](notes/package-manager.md) | `lichen-package`, `lichen-preprocess` | current |
| [Isolating the preprocessor](notes/preprocessor-isolation.md) | `lichen-preprocess`, `lichen-span`, `lichen-language` (`preprocess` shim, `package`), `lichen-package` | current |
| [Cross-process artifact store](notes/artifact-cache.md) | `lichen-registry`, `lichen-language` (`persist`/`package`), `lichen-lowlevel` (`static_module`) | current |
| [Testing the package manager offline](notes/venv-test.md) | `lichen-package`, `scripts/venv-test.sh` | current |
| [README example sync](notes/readme-sync.md) | `lichen-language` (`readme`) | current |
| [lichen-compute: the JIT package](notes/lichen-compute.md) | `lichen-compute`, `lichen-language` (`program`/`package`), `lichen-highlevel` (`native`) | current |
| [Kernels as `.native`/`.sig` structs](notes/compute-kernel-struct.md) | `lichen-compute`, `lichen-language` (`program`) | current |
| [Parallel buffer map (`range`/`read`/`write`)](notes/compute-parallel-buffer-read-write.md) | `lichen-compute` | current |
| [Parallel primitives (`parallel`/`plrun`/`pget`/`pcollect`)](notes/lichen-compute-parallel.md) | `lichen-compute` | historical |
| [Zed extension: build & test workflow](notes/zed-extension-testing.md) | `lichen-language-zed`, `lichen-language-server`, `tree-sitter-lichen` | current |
| [Tree-sitter generated-files testing](notes/tree-sitter-generated-files.md) | `tree-sitter-lichen`, `lichen-language-zed` | current |

## The language spec

[language-spec.md](language-spec.md) is the single source of truth for the lichen
**source language**: syntax, grammar, semantics, name resolution, and diagnostics.
It is the one document from the project's early days that is retained as the language
reference, because it describes a stable, user-facing contract. Feature notes and
rustdoc refer to it for syntax and never restate it.

## Reference

- The attribute extension is *inspired by* a paper — see
  [Typed Perspectives](reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf).
