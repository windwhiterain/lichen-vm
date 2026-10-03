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
| [Floating point](notes/floating-point.md) | `lichen-lowlevel` (`LowValue`/`LowShape`/`codec`), `lichen-highlevel` (`shape`/`program`/`checker`), `lichen-language-lex`/`-parser`, `lichen-render`, `lichen-language` (`persist`), `lichen-kernel-ir`, `lichen-compute`, `lichen-compute-gpu` | phases 0–2 landed (`f32`, `==` is `value_eq`, a float kernel runs on both backends); the classes meet only at `int2float` / `float2int` — see [operators](notes/operators.md) §7 |
| [A lichen program as a whiting scene](notes/whiting-scene-document.md) | `lichen-language` (`session`/`compile`), `lichen-lowlevel`, external `whiting-definition` | proposed (the seam is one scene document; no compiler impl, no device) |
| [Static modules & registry](notes/static-modules.md) | `lichen-lowlevel`, `lichen-language` (`persist`) | current |
| [The computational operators](notes/operators.md) | `lichen-language-lex`/`-parser`/`-language`, `lichen-highlevel` (`ir`/`program`/`checker`), `lichen-kernel-ir`, `lichen-compute`, `lichen-compute-gpu` | current |
| [Operator polymorphism](notes/operator-polymorphism.md) | `lichen-highlevel` (`checker`/`attr`/`shape`/`program`), `lichen-lowlevel` (`assert`), `lichen-compute`, `lichen-std` | proposed — the contract is a **refinement** on the operand's value (an attribute holding one predicate function, applied and registered as an assert), separate from the implementation (a per-class leaf the dispatch selects), and the contract may be stricter |
| [The compute JIT on low types](notes/compute-jit-low-types.md) | `lichen-compute` | current |
| [Low types for a lowlevel-based JIT](notes/lowlevel-low-types.md) | `lichen-lowlevel` (`LowShape`), `lichen-highlevel` (`shape`), `lichen-compute` | implemented (Phases 3a–3c) |
| [Type-system cleanup plan](notes/type-system-cleanup-plan.md) | `lichen-lowlevel`, `lichen-highlevel`, `lichen-compute` | current (Phases 0–5 complete) |
| [Checker encoding: unstable at the `lichen-compute` boundary](notes/checker-encoding-instability.md) | `lichen-highlevel` (`shape`), `lichen-compute` | current (narrowed: the domain read is on low types; the body walk is the residue) |
| [Deferred unification does not recognise every form of a type value](notes/defer-pending-type-forms.md) | `lichen-lowlevel` (`equality`), `lichen-highlevel` (`shape`) | current — **defect open**; the split outcome is measured, the mechanism behind it is not isolated |
| [A type value's rendering depends on its form](notes/type-rendering-inconsistent.md) | `lichen-render` (`render/type_printer`), `lichen-language` (`render`) | current — **defect open**; the four-way split is measured, which path fires for each row is not traced |
| [An applied struct type expression is not a function](notes/applied-struct-nominal-id.md) | `lichen-highlevel` (`checker`/`ir`), `lichen-lowlevel` (`function`/`static_module`) | current — **fixed** on `feature/applied-struct-nominal-id`; the identity marker `[id, names]` is pinned at construction, so one written occurrence is one nominal type |
| ["Contains the universe" is not "is the universe"](notes/universe-containment.md) | `lichen-render` (`render`), `lichen-lowlevel` (`equality`) | **§2 landed; §3 open** — the printer half is fixed (`is_universe` tests the whole `[Type, ↺]` shape; the `type_of` render assertions run un-parked), the lowlevel half also merges two frozen kinds without comparing markers and is blocked on a compute-side question |
| [The raw mark in the printer](notes/raw-rendering-mark.md) | `lichen-render` (`render/type_printer`, `render/value_printer`) | current |
| [Extensible attributes](notes/attributes.md) | `lichen-highlevel` (`attr`/`shape`/`ir`/`checker`), `lichen-language` (`program`/`compile`) | current |
| [Doc attribute rework plan](notes/doc-attribute-rework-plan.md) | `lichen-language`, `lichen-highlevel` (`attr`), `lichen-language-server` (`analysis`) | current |
| [The compiler-plugin model](notes/compiler-plugin.md) | `lichen-utils` (`extend`), `lichen-lowlevel`, `lichen-highlevel` (`program`/`native`), `lichen-compute` | current |
| [The plugin taxonomy](notes/plugin-taxonomy.md) | `lichen-highlevel` (`plugin`), `lichen-compute`, `lichen-std-native`, `lichen-perspective`, `lichen-language` (`program`/`package`) | current |
| [Raw index `X<e>`](notes/raw-index.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language` (`compile`), `lichen-highlevel` (`ir`/`checker`), `lichen-render` | current |
| [Raw named read `X::a`](notes/raw-field.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language` (`compile`), `lichen-highlevel` (`ir`/`checker`) | current |
| [Placeholder `_` anywhere](notes/placeholder-anywhere.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language-server` (`analysis`) | current |
| [`type_of` is a standard-library function](notes/type-of-in-std.md) | `lichen-language-lex`/`-parser`, `lichen-language` (`compile`/`resolve`), `lichen-highlevel` (`ir`/`checker`), `lichen-compute`, `lichen-std` | current |
| [No type mode](notes/no-type-mode.md) | `lichen-language-parser` | current |
| [Record programs (modules)](notes/record-program.md) | `lichen-language-parser`, `lichen-language` (`compile`/`session`), `lichen-language-server` (`analysis`) | current |
| [Separating lexer & parser from the language](notes/frontend-syntax-separation.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language`, `lichen-highlevel` | current |
| [Incremental parse/compile](notes/incremental-parse-compile.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language` (`resolve`/`compile`/`session`), `lichen-highlevel` (`checker`) | current (T3/T4 proposed; unwired, `P2-1`) |
| [Incremental evaluation](notes/incremental-evaluation.md) | `lichen-lowlevel` (`evaluation`/`equality`/`function`/`gc`/`table`/`module`), `lichen-highlevel` (`checker`), `lichen-language` (`run`) | proposed (the within-build half; step 0 measured, the settled cut is not built) |
| [Incremental update: identity by path, retention by `cache`](notes/incremental-update.md) | `lichen-language-parser` (`path`), `lichen-language` (`cells`/`compile`/`dirty`/`session`), `lichen-lowlevel` (`freeze`/`registry`/`Release`), `lichen-compute` | mechanism complete and measured; the session consumes it, but the session itself has **no production caller** (`P2-1`); §12 is the handoff |
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
| [Loop conversion: recursion into a loop nest](notes/loop-conversion.md) | `lichen-language-lex`/`-parser`, `lichen-highlevel` (`ir`/`checker`), `lichen-kernel-ir` (`KernelBody`/`Flow`/`Terminator`), `lichen-compute` (`lower_body`), `lichen-compute-gpu` (`spirv.rs`/`dispatch.rs`) | **in progress** — four decisions closed (unroll by default, `@loop` opts in, tail-recursive cycles only, no write in a loop body); the conversion runs in **evaluation**. Landed: the `KernelBody` IR + validator, the wasm emitter, and the `@loop` keyword. The evaluator's loop recording and the SPIR-V emitter are not written |
| [The GPU backend for the lowered-kernel IR](notes/lichen-compute-gpu.md) | `lichen-compute-gpu`, `lichen-kernel-ir` | current |
| [Graph JIT: a chain of dispatches as one submission](notes/compute-graph-jit.md) | `lichen-graph-ir`, `lichen-compute` (`compute/graph.rs`) | current |
| [The compute JIT on low types](notes/compute-jit-low-types.md) | `lichen-compute` | current |
| [One query surface for types and classes](notes/type-query-api-proposal.md) | `lichen-highlevel` (`shape`), `lichen-compute`, `lichen-compute-gpu` | **superseded in diagnosis; §2's Level 1 landed in part** — the three conversion failures it blamed on a missing API had three other causes ([kernel-class-crossing-fixes](notes/kernel-class-crossing-fixes.md) §0); the `shape` accessors are built and consumed (§7), which also fixes a decided field read's lazy type, and `field_index`/`class_of`/Level 2 are not |
| [Finishing per-value class semantics](notes/kernel-class-crossing-fixes.md) | `lichen-compute` (`compute.rs`), `lichen-compute-gpu` (`spirv.rs`), `lichen-language` (`tests/compute.rs`) | **landed** — a `Bin`'s class is its operands', both element types are declared in every SPIR-V module, and a body may compute in one class and cross; §9 is what was measured |
| [Runtime scalars in a parallel kernel](notes/compute-runtime-scalars.md) | `lichen-compute`, `lichen-compute-gpu` | **in progress** — each scalar leaf's class is its parameter field's, and a buffer call's ordinals are `Int` as the ABI declares (both landed); the host half is unwritten, and the struct-argument API is a verified recipe (§4.3) whose one open item is the commit path §4.4 names |
| [One channel for a class's value and its low type](notes/class-channel.md) | `lichen-lowlevel` (`equality`), `lichen-highlevel` (`shape`), `lichen-compute` | **planned** — the lowlevel's class-routed value/low-type channel versus the highlevel's node-slotted second reading; §2 readers ask the class, §3 the decider writes through the choke-point, §4 the struct-argument migration that carries it, §5 the signature-before-`build` change that deletes the need |
| [The GPU algorithm roadmap](notes/gpu-algorithm-roadmap.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-compute-gpu` | **proposal** — the order the four axes go in |
| [The GPU algorithm ladder](notes/gpu-algorithms-ladder.md) | `lichen-compute`, `lichen-compute-gpu` | exploration record — the evidence the roadmap argues from |
| [Parallel primitives (`parallel`/`plrun`/`pget`/`pcollect`)](notes/lichen-compute-parallel.md) | `lichen-compute` | historical |
| [Zed extension: build & test workflow](notes/zed-extension-testing.md) | `lichen-language-zed`, `lichen-language-server`, `tree-sitter-lichen` | current |
| [Tree-sitter generated-files testing](notes/tree-sitter-generated-files.md) | `tree-sitter-lichen`, `lichen-language-zed` | current |
| [Code audit and remediation queue](notes/code-audit.md) | — (all crates) | current — the queue is the work list; each item's status is live |

## The language spec

[language-spec.md](language-spec.md) is the single source of truth for the lichen
**source language**: syntax, grammar, semantics, name resolution, and diagnostics.
It is the one document from the project's early days that is retained as the language
reference, because it describes a stable, user-facing contract. Feature notes and
rustdoc refer to it for syntax and never restate it.

## Reference

- The attribute extension is *inspired by* a paper — see
  [Typed Perspectives](reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf).
