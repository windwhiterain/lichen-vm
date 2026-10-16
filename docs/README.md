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
- `in progress` — a feature being built; the note doubles as the coordination
  document for the branches building it.
- `historical` — a past decision or investigation, kept for context; never the
  authority on current behaviour.
- `proposed` — a design/investigation for not-yet-implemented behaviour; the
  design rationale, not a description of what the code does today.

A status may carry a short qualifier; the note's own status line is the
authority, and this table only names the state.

## Feature notes

| Note | Crate(s) | Status |
|---|---|---|
| [Architecture overview](notes/overview.md) | — | current |
| [Lowlevel VM](notes/lowlevel-vm.md) | `lichen-lowlevel` | current — the graph, lazy evaluation, the unify write rules, GC |
| [`@gc e`: an expression in a scope of its own](notes/gc-block.md) | `lichen-language-lex`/`-parser`, `lichen-highlevel` (`checker/gc`), `lichen-lowlevel` (`gc`/`evaluation`/`function`) | current |
| [Static modules & registry](notes/static-modules.md) | `lichen-lowlevel`, `lichen-language` (`persist`) | current |
| [Low types for a lowlevel-based JIT](notes/lowlevel-low-types.md) | `lichen-lowlevel` (`LowShape`), `lichen-highlevel` (`shape`), `lichen-compute` | current |
| [A call's clones belong to the enclosing template](notes/apply-clone-ownership.md) | `lichen-lowlevel` (`function`/`static_module`) | current |
| [A class carries the call, not only the fact](notes/class-carries-the-call.md) | `lichen-lowlevel` (`equality`), `lichen-compute` | current — measured: a declared type's class holds this call's cells too, so sharing a fact by merging classes shares them as well |
| [Non-termination is the program's own](notes/nontermination-is-the-programs.md) | `lichen-highlevel` (`checker`), `lichen-lowlevel` (`apply`) | current — the ruling, and what was measured before it |
| [The read-path collection is disabled](notes/gc-trigger-disabled.md) | `lichen-lowlevel` (`evaluation`, `gc`) | current — off until its trigger is repaired; the trace and the acceptance test are in the note |
| [Floating point](notes/floating-point.md) | `lichen-lowlevel`, `lichen-highlevel`, `lichen-render`, `lichen-kernel-ir`, `lichen-compute`, `lichen-compute-gpu` | current — both classes run kernels on both backends |
| ["Contains the universe" is not "is the universe"](notes/universe-containment.md) | `lichen-render`, `lichen-lowlevel` (`equality`) | current — the lowlevel frozen-kind half is open |
| [Evaluation runs before unification, and nothing wakes what it read](notes/eval-before-unify.md) | `lichen-lowlevel` (`evaluation`/`equality`), `lichen-highlevel` (`checker`) | current — the named reads' wakeup is open |
| [A stalled operator re-runs only when a value changed](notes/stalled-rerun-epoch.md) | `lichen-lowlevel` (`evaluation`/`equality`/`module`) | current — the epoch gate; the phase-ordering question is closed |
| [Unify without forcing](notes/unify-without-forcing.md) | `lichen-lowlevel` (`equality`) | historical — the machinery removal it proposed has landed |
| [Incremental evaluation](notes/incremental-evaluation.md) | `lichen-lowlevel`, `lichen-highlevel` (`checker`), `lichen-language` (`run`) | proposed |
| [Function types, and a struct type's nominal identity](notes/function-type-merge.md) | `lichen-highlevel` (`checker`/`lambda`/`shape`), `lichen-lowlevel` (`equality`/`static_module`), `lichen-render`, `lichen-language` | current |
| [A function unifies by its own cells](notes/function-unify.md) | `lichen-lowlevel` (`equality`/`function`/`apply`/`evaluation`), `lichen-highlevel` (`checker`) | in progress — the function arm, and why a function's value positions hold no `@any` |
| [A nested apply loses the parameter it consumed](notes/nested-apply-none.md) | `lichen-lowlevel` (`evaluation`/`apply`/`function`), `lichen-highlevel` (`checker`) | cause named — the checker evaluates every function's return once, so an apply-bodied function's body is run with the parameter unbound and the generation it mints there is pinned into the run while the answer reads it; two readings of the failure are exact over the corpus and select the same programs; repaired by running both of an apply's slots and by reading the instantiated return's pair after evaluating it — all twenty-two failing programs now answer their correct value and no other program's answer moved |
| [Compound element slots](notes/compound-element-slots.md) | `lichen-lowlevel` (`equality`/`evaluation`), `lichen-highlevel` (`checker`/`shape`), `lichen-compute`, `lichen-render`, `lichen-language` (lexer/parser) | in progress — every compound's element is a slot and the angle spellings are retired (`Any`, readers, printers, attribute/pin reconciliation, `@any`, `tuple{…}`/`struct{…}`/`array{T, n}`, no `ShapeElements`) |
| [Operator polymorphism](notes/operator-polymorphism.md) | `lichen-highlevel`, `lichen-lowlevel` (`assert`), `lichen-language`, `lichen-compute`, `lichen-std` | current — the refinement contract, the set value, the routing |
| [`core`: the built-in prelude](notes/core-prelude.md) | `lichen-language` (`core.lichen`), `lichen-registry` | current — a failure inside the prelude is unattributed (open) |
| [Extensible attributes](notes/attributes.md) | `lichen-highlevel` (`attr`), `lichen-language`, `lichen-doc` | current — includes the `?` doc attribute |
| [Checker encoding: unstable at the `lichen-compute` boundary](notes/checker-encoding-instability.md) | `lichen-highlevel` (`shape`), `lichen-compute` | current — defect narrowed to the body walk |
| [Block vs lambda struct-field inference](notes/block-vs-lambda-struct-field-inference.md) | `lichen-highlevel` (`checker`) | current — defect open, root cause known (§7) |
| [A type value's rendering depends on its form](notes/type-rendering-inconsistent.md) | `lichen-render`, `lichen-language` (`render`) | current — defect open |
| [The raw mark in the printer](notes/raw-rendering-mark.md) | `lichen-render` | current |
| [Tests do not assert what the printer prints](notes/tests-do-not-render.md) | every test that renders a printer result | current — a rule from the maintainer |
| [The computational operators](notes/operators.md) | `lichen-language-lex`/`-parser`/`-language`, `lichen-highlevel`, `lichen-kernel-ir`, `lichen-compute`, `lichen-compute-gpu` | current |
| [Separating lexer & parser from the language](notes/frontend-syntax-separation.md) | `lichen-span`, `lichen-language-lex`, `lichen-language-parser`, `lichen-language`, `lichen-highlevel` | current |
| [No type mode](notes/no-type-mode.md) | `lichen-language-parser` | current |
| [Placeholder `_` anywhere](notes/placeholder-anywhere.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language-server` | current |
| [Record programs (modules)](notes/record-program.md) | `lichen-language-parser`, `lichen-language`, `lichen-language-server` | current |
| [Type accesses `X{n}` and `X::a`](notes/type-access.md) | `lichen-language-lex`/`-parser`, `lichen-language`, `lichen-highlevel`, `lichen-render` | current |
| [`type_of` is a standard-library function](notes/type-of-in-std.md) | `lichen-language`, `lichen-highlevel`, `lichen-compute`, `lichen-std` | current |
| [Incremental parse/compile](notes/incremental-parse-compile.md) | `lichen-language-lex`/`-parser`, `lichen-language`, `lichen-highlevel` | current — T3/T4 proposed |
| [Incremental update: identity by path, retention by `cache`](notes/incremental-update.md) | `lichen-language-parser` (`path`), `lichen-language` (`cells`/`session`), `lichen-lowlevel` (`freeze`/`registry`), `lichen-compute` | current — the keystroke fine cut is open |
| [Build performance](notes/build-performance.md) | `lichen-language-parser` | current |
| [Parse performance: the clean probe](notes/parse-clean-probe.md) | `lichen-language-parser`, `lichen-language-lex` | current |
| [Language toolchain](notes/language-toolchain.md) | `lichen-span`/`-lex`/`-parser`/`-language`/`-server`, `lichen-language-zed` | current |
| [Lichen Home for the LSP](notes/liche-lsp-home.md) | `lichen-language-server` (`home`), `lichen-language` | current |
| [Packages & import](notes/packages.md) | `lichen-preprocess`, `lichen-language`, `lichen-compiler` | current |
| [Package manager](notes/package-manager.md) | `lichen-package`, `lichen-preprocess` | current |
| [Isolating the preprocessor](notes/preprocessor-isolation.md) | `lichen-preprocess`, `lichen-span`, `lichen-language`, `lichen-package` | current |
| [Cross-process artifact store](notes/artifact-cache.md) | `lichen-registry`, `lichen-language` (`persist`), `lichen-lowlevel` | current |
| [Testing the package manager offline](notes/venv-test.md) | `lichen-package`, `scripts/venv-test.sh` | current |
| [README example sync](notes/readme-sync.md) | `lichen-tools`, `lichen-compute` (`tests/lichen_compute/readme.rs`) | current |
| [The compiler-plugin model](notes/compiler-plugin.md) | `lichen-utils` (`extend`), `lichen-lowlevel`, `lichen-highlevel`, `lichen-compute` | current |
| [The plugin taxonomy](notes/plugin-taxonomy.md) | `lichen-highlevel` (`plugin`), `lichen-compute`, `lichen-std-native`, `lichen-perspective`, `lichen-language` | current |
| [lichen-compute: the JIT package](notes/lichen-compute.md) | `lichen-compute`, `lichen-highlevel` (`native`) | current |
| [Kernels as `.native`/`.I`/`.O` structs](notes/compute-kernel-struct.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-compute` (`language`) | current — includes the SSA body model |
| [A buffer is a struct wrapper, and a parallel result is the parameter's `.out`](notes/compute-buffer-wrapper.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-compute` (`tests`) | current — the host read's result class is open |
| [The compute JIT on low types](notes/compute-jit-low-types.md) | `lichen-compute` | current |
| [The kernel parameter's class](notes/kernel-parameter-class.md) | `lichen-highlevel`, `lichen-lowlevel`, `lichen-compute`, `lichen-render` | current — both original defects fixed; residue open |
| [Per-value class semantics](notes/kernel-class-crossing-fixes.md) | `lichen-compute`, `lichen-compute-gpu`, `lichen-compute` (`tests`) | current |
| [Runtime scalars in a parallel kernel](notes/compute-runtime-scalars.md) | `lichen-compute`, `lichen-compute-gpu` | current — the device path is refused until built |
| [Graph JIT: a chain of dispatches as one submission](notes/compute-graph-jit.md) | `lichen-graph-ir`, `lichen-compute` (`compute/graph.rs`) | current — a whole graph in one submission, per backend |
| [Taking a kernel to a server](notes/taking-a-kernel-to-a-server.md) | `lichen-compute` (the fused artifact), `lichen-registry` (the frozen one) | current — the deployment unit is partly unbuilt, and the two artifacts are told apart |
| [Specialize-then-normalize: a named access should become a tuple index](notes/specialize-normalize.md) | lichen-highlevel (the choice), lichen-compute (the stand-in) | proposal - nothing implements it |
| [A launch that is not one row](notes/two-dimensional-launch.md) | `lichen-kernel-ir`, `lichen-compute`, `lichen-runtime`, `lichen-compute-gpu` (the emitters) | proposal - measured, nothing implemented |
| [A workgroup that is not one row](notes/two-dimensional-workgroup.md) | `lichen-compute-gpu` (the emitters, the dispatch), `lichen-compute` (the lowering), `lichen-kernel-ir` (the plan, the digest) | proposal - measured, nothing implemented; the emitter slice needs `spirv-val` |
| [Which write fills a `Buf` element cell](notes/compile-order-class.md) | `lichen-compute` (the role table), `lichen-lowlevel` (the equality classes), `lichen-highlevel` (the shape cells) | **fixed** by `7eedb125` - the three pins are ordinary passing tests now |
| [Auditing the recorded walls](notes/wall-claims-audit.md) | `lichen-compute`, `lichen-lowlevel`, `lichen-language` (a recursion), `lichen-compute-gpu` (walls 4, 5, 8 unreached) | measurement - **wall 2 is not real**, walls 3 and 6 are weaker than recorded, wall 1's cause is narrower |
| [A diagnostic an agent can act on](notes/agent-friendly-diagnostics.md) | `lichen-language-parser` (the cascade, the dedup), `lichen-highlevel` (the checker's diagnostics) | property stated, one case fixed (`4cb42d3f`); the offender list under-covers the checker and module layers |
| [Transcendentals as operators](notes/transcendentals-as-operators.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-highlevel`, `lichen-kernel-ir`, `lichen-compute` (both lowerings), `lichen-compute-gpu`, `lichen-fusion-bridge`, `lichen-runtime` (the codec) | `ln` landed as a named operator and `sin`/`cos` switched onto it (`82640443`); `ln`'s GPU column and an emitted-vs-modelled test are unmeasured |
| [The `examples` panic, diagnosed](notes/examples-panic-diagnosis.md) | `lichen-tools` (the runner), `lichen-language` (the compile), `lichen-highlevel` (the checker), `lichen-lowlevel` (`@gc` and `evaluation.rs:517`) | measurement - a **stale key from `drop_block`**, 1 of 40 examples, a real defect; **the four drifts share one read, and three causes** |
| [An undecided type cell: the four `examples` drifts](notes/undecided-type-cell.md) | `lichen-render` (the read), `lichen-highlevel` (`check_app`), `lichen-language` (`routed_operator`) | diagnosis — CLI and suite agree; the comparison family traced to a missing join, the lazy and frozen families not traced |
| [LLM inference operators, and what the machinery says about them](notes/llm-operators.md) | `examples/rmsnorm.lichen`, `lichen-runtime` (the passes), `lichen-fusion-bridge` | current - three operators landed, and the friction is the roadmap |
| [The ladder's device leg, measured](notes/device-leg-of-the-ladder.md) | `examples/{sample,topk,kvcache,decode_loop,generate}.lichen`, `lichen-compute-gpu` (the dispatch) | measurement - the append and the row maximum run bit-exact, the scoring row refuses by name, and the sampler takes the driver down |
| [The GPU backend for the lowered-kernel IR](notes/lichen-compute-gpu.md) | `lichen-compute-gpu`, `lichen-kernel-ir` | current |
| [Loop conversion: recursion into a loop nest](notes/loop-conversion.md) | `lichen-language-lex`/`-parser`, `lichen-highlevel`, `lichen-kernel-ir`, `lichen-lowlevel`, `lichen-compute`, `lichen-compute-gpu` | in progress — §8 is the handoff |
| [The wasm backend: handoff](notes/wasm-backend-handoff.md) | `lichen-compute` (`compute/wasm/`), `lichen-kernel-ir` | in progress — read first for wasm code generation |
| [Lowering structured control flow to WebAssembly](notes/wasm-control-flow.md) | `lichen-compute` (`compute/wasm/`) | in progress — the straight-line half has landed |
| [One channel for a class's value and its low type](notes/class-channel.md) | `lichen-lowlevel` (`equality`), `lichen-highlevel` (`shape`), `lichen-compute` | proposed — §1.1's write-rule decision is landed |
| [The GPU algorithm roadmap](notes/gpu-algorithm-roadmap.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-compute-gpu` | proposed |
| [The GPU algorithm ladder](notes/gpu-algorithms-ladder.md) | `lichen-compute`, `lichen-compute-gpu` | historical — the exploration record the roadmap argues from |
| [Graph fusion: logic, interfaces, runner-installed packs](notes/graph-fusion.md) | `lichen-graph-ir`, `lichen-compute`, `lichen-kernel-ir`, `lichen-fusion`, `lichen-highlevel` (`attr`) | proposed — interface-derived fusion and runner-installed packs; §5 has a pack hand over **guidance, not an implementation**: the region's algebra, the directive that decides which operations share a kernel, and the **memory plan** for what the fusion stages — its interface stated there and its mechanism staged in [shared-memory-and-barriers](notes/shared-memory-and-barriers.md) §9 |
| [Shared memory, and where a barrier comes from](notes/shared-memory-and-barriers.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-lowlevel`, `lichen-compute-gpu`, `lichen-fusion-bridge`, `lichen-perspective` | in flux — part one landed as a `State` token and is **reverted**: §3 retracts the argument that chose it and [code-audit D17](notes/code-audit.md) gives the reason (a write is the value of an output position, not an effect). The spine is a buffer **version** and has landed on global buffers — a write's ordinal is the `.to` it names — while a read-back still has no data path, so it is refused by name and moves with the staged landing. §5 states the **selection** among the two spaces a block offers — the buffer's **type** carries the `DP` that scopes it — and §9 is restaged around a lane-group width the fragment declares. **The GPU emission has landed**: a `Workgroup` array per slot and one `OpControlBarrier` per recorded pair, reached through `from_flat` bodies; the surface and its lowering are what step 5 still owes |
| [Zed extension: build & test workflow](notes/zed-extension-testing.md) | `lichen-language-zed`, `lichen-language-server`, `tree-sitter-lichen` | current |
| [Tree-sitter generated-files testing](notes/tree-sitter-generated-files.md) | `tree-sitter-lichen`, `lichen-language-zed` | current |
| [A lichen program as a whiting scene](notes/whiting-scene-document.md) | `lichen-language`, `lichen-lowlevel`, external `whiting-definition` | proposed |
| [Code audit and remediation queue](notes/code-audit.md) | — (all crates) | current — the live open queue and the design decisions |
| [The comment policy](notes/comment-policy.md) | — (all crates) | current — the rule enforced in CI by `comments-check` |

## The language spec

[language-spec.md](language-spec.md) is the single source of truth for the lichen
**source language**: syntax, grammar, semantics, name resolution, and diagnostics.
It is the one document from the project's early days that is retained as the language
reference, because it describes a stable, user-facing contract. Feature notes and
rustdoc refer to it for syntax and never restate it.

## Reference

- The attribute extension is *inspired by* a paper — see
  [Typed Perspectives](reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf).
