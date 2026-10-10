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
| [Operator polymorphism](notes/operator-polymorphism.md) | `lichen-highlevel` (`checker`/`attr`/`shape`/`program`/`set`), `lichen-lowlevel` (`assert`), `lichen-language` (`compile`/`render`), `lichen-compute`, `lichen-std` | Phases 0–3 landed — the contract is a **refinement** (an attribute holding one predicate function, applied and registered as an assert), separate from the implementation (a per-class leaf the dispatch selects), and the contract may be stricter; the domain it admits is a **set** value, `set{Int, Float}`; `@in`, the **class refinement** `x : (_ ! in_num)`, the `core` prelude, the static-signature instantiation and the **routing** (the surface operators lower onto the prelude's bindings) are landed, and the routing's three costs are measured, closed and pinned in §7.1 — no test on `dev` is red for any of them |
| [Operator polymorphism: handoff](notes/operator-polymorphism-handoff.md) | the same crates | **landed** on `dev`, with **no known reds** — the refinement mechanism, the refinement attribute, `!`/`@assert`, the set value, the static class-naming fix, `@in`, the class refinement and the routing are all in; the two kernel targets §5 used to list are green, and what a kernel body may not do is refused by name (§7.1 cost 1) |
| [`core`: the built-in prelude](notes/core-prelude.md) | `lichen-language` (`core.lichen`/`package`/`preprocess`/`resolve`/`persist`), `lichen-registry` (`device`) | current — every source is seeded with the built-in `core` module (`Num`, `in_num`, and one binding per polymorphic operator); the prelude is shadowable, and a failure inside it is unattributed (open) |
| [The compute JIT on low types](notes/compute-jit-low-types.md) | `lichen-compute` | current |
| [Low types for a lowlevel-based JIT](notes/lowlevel-low-types.md) | `lichen-lowlevel` (`LowShape`), `lichen-highlevel` (`shape`), `lichen-compute` | implemented (Phases 3a–3c) |
| [Type-system cleanup plan](notes/type-system-cleanup-plan.md) | `lichen-lowlevel`, `lichen-highlevel`, `lichen-compute` | current (Phases 0–5 complete) |
| [Checker encoding: unstable at the `lichen-compute` boundary](notes/checker-encoding-instability.md) | `lichen-highlevel` (`shape`), `lichen-compute` | current (narrowed: the domain read is on low types; the body walk is the residue) |
| [Deferred unification does not recognise every form of a type value](notes/defer-pending-type-forms.md) | `lichen-lowlevel` (`equality`), `lichen-highlevel` (`shape`) | current — **defect open**; the split outcome is measured, the mechanism behind it is not isolated |
| [Evaluation runs before unification, and nothing wakes what it read](notes/eval-before-unify.md) | `lichen-lowlevel` (`evaluation`/`equality`), `lichen-highlevel` (`checker`) | **§2.1 fixed, and both container reads with it** — the order-sensitivity is **measured** (a binop's result type and a container-kind guard both flip on statement order); a merge carries the class's decided value to the members it adds, so the binop row decides in both orders, and `a(k)` is now the *tuple* read whose check is a unify against a tuple type (with every struct field named), so both orders refuse it — while `a.name`/`X::a` state their requirement in both tiers (the decided one as a kind unify, the undecided one as a registered `IsStructType` condition re-checked per apply clone, which also closed `X::a`'s `TableGet` panic); the class-attached blocked list remains a sketch, and the wider question it belongs to is [unify-without-forcing](notes/unify-without-forcing.md); `feature/read-kind-unify` (three commits, left unmerged) is the measurement of the direct-pin route and is parked on the reason §6.2 records |
| [Unify without forcing](notes/unify-without-forcing.md) | `lichen-lowlevel` (`equality`/`evaluation`/`function`/`static_module`), `lichen-highlevel` (`shape`) | **proposed** — the design question only: what "unify regardless of whether a node carries an operation" would cost (the pure-cell guard, several operations in one class, an eager class value), the two probes that bound it, and the run-state half that did land instead |
| [A type value's rendering depends on its form](notes/type-rendering-inconsistent.md) | `lichen-render` (`render/type_printer`), `lichen-language` (`render`) | current — **defect open**; the four-way split is measured, which path fires for each row is not traced |
| [An applied struct type expression is not a function](notes/applied-struct-nominal-id.md) | `lichen-highlevel` (`checker`/`ir`), `lichen-lowlevel` (`function`/`static_module`) | current — **fixed** on `feature/applied-struct-nominal-id`; the identity marker `[payload, TypeStruct]` (payload `[id, names, names_in_order]`) is pinned at construction, so one written occurrence is one nominal type |
| ["Contains the universe" is not "is the universe"](notes/universe-containment.md) | `lichen-render` (`render`), `lichen-lowlevel` (`equality`) | **§2 landed; §3 open** — the printer half is fixed (`is_universe` tests the whole `[Type, ↺]` shape; the `type_of` render assertions run un-parked), the lowlevel half also merges two frozen kinds without comparing markers and is blocked on a compute-side question |
| [A function's type is the function itself](notes/function-type-as-function.md) | `lichen-highlevel` (`checker`/`shape`), `lichen-lowlevel` (`equality`/`function`/`static_module`/`lib`), `lichen-render`, `lichen-language` (`persist`) | **historical** — the `f : f` half is current and still true; the arrow half is superseded by the row below, and most of the symbols it points at were deleted.  `gcd` reporting `6: ?a` was its accepted state and no longer holds |
| [A function's type is the function, and its signature is two cells](notes/function-type-merge.md) | `lichen-highlevel` (`checker`/`lambda`/`shape`), `lichen-lowlevel` (`equality`/`static_module`), `lichen-render`, `lichen-language` | **current — and it did not fix what it was aimed at.**  A written arrow lowers to a *function* rather than to a type, so the two representations are one and `f : f` follows instead of being maintained; two functions unify like two arrays, with no host hook.  `gcd` is `6: Int`.  The frozen two-line reduction it left as the next step **no longer diverges from its local control** (`fb62d96`), and its "What goes" list is a plan rather than a report — see [the kernel parameter's class](notes/kernel-parameter-class.md) |
| [The raw mark in the printer](notes/raw-rendering-mark.md) | `lichen-render` (`render/type_printer`, `render/value_printer`) | current |
| [Tests do not assert what the printer prints](notes/tests-do-not-render.md) | every test that renders a printer result | current — a rule from the maintainer; the `lichen-render` tests and the examples' `output =` are the scope still to be settled, and the operator routing's three conversions (two semantic assertions, two examples re-pinned) are recorded as how the rule has been applied |
| [Extensible attributes](notes/attributes.md) | `lichen-highlevel` (`attr`/`shape`/`ir`/`checker`), `lichen-language` (`program`/`compile`) | current |
| [Doc attribute rework plan](notes/doc-attribute-rework-plan.md) | `lichen-language`, `lichen-highlevel` (`attr`), `lichen-language-server` (`analysis`) | current |
| [The compiler-plugin model](notes/compiler-plugin.md) | `lichen-utils` (`extend`), `lichen-lowlevel`, `lichen-highlevel` (`program`/`native`), `lichen-compute` | current |
| [The plugin taxonomy](notes/plugin-taxonomy.md) | `lichen-highlevel` (`plugin`), `lichen-compute`, `lichen-std-native`, `lichen-perspective`, `lichen-language` (`program`/`package`) | current |
| [Raw index `X<e>`](notes/raw-index.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language` (`compile`), `lichen-highlevel` (`ir`/`checker`), `lichen-render` | current |
| [Raw named read `X::a`](notes/raw-field.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language` (`compile`), `lichen-highlevel` (`ir`/`checker`) | current |
| [Placeholder `_` anywhere](notes/placeholder-anywhere.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language-server` (`analysis`) | current |
| [`type_of` is a standard-library function](notes/type-of-in-std.md) | `lichen-language-lex`/`-parser`, `lichen-language` (`compile`/`resolve`), `lichen-highlevel` (`ir`/`checker`), `lichen-compute`, `lichen-std` | current |
| [One `unify`, and the write rules are member-local](notes/unify-write-handoff.md) | `lichen-lowlevel` (`equality`), `lichen-utils` (`disjoint`) | **in progress** — `union_with_value` is gone; the write path is the open half, with the two failed attempts and their refutation recorded |
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
| [Kernels as `.native`/`.I`/`.O` structs](notes/compute-kernel-struct.md) | `lichen-compute`, `lichen-language` (`program`) | current |
| [A buffer is a struct wrapper, and a parallel result is the parameter's `.out`](notes/compute-buffer-wrapper.md) | `lichen-compute`, `lichen-kernel-ir` (`KernelRoles`), `lichen-language` (`tests`/`examples`) | current — the model the misnamed `.sig`/bare-buffer notes predate |
| [The kernel parameter's class: two measured losses](notes/kernel-parameter-class.md) | `lichen-highlevel` (`shape`/`checker`), `lichen-lowlevel` (`equality`), `lichen-compute` (`compile_fragment`/`kernel_domain`), `lichen-render` (`value_printer`) | current — the arrow node the decoder could not read is **fixed** (`7e795f9`, and the parked case un-parks); the frozen struct whose field-type cells are empty is **open**, with the decision it needs |
| [The named parameter's fields: the handoff, resolved](notes/compute-param-struct-handoff.md) | `lichen-compute`, `lichen-language` (`tests`/`examples`) | resolved — both blockers fixed and verified |
| [Parallel buffer map (`range`/`read`/`write`)](notes/compute-parallel-buffer-read-write.md) | `lichen-compute` | the design, with its parameter shape superseded by the wrapper note |
| [Loop conversion: recursion into a loop nest](notes/loop-conversion.md) | `lichen-language-lex`/`-parser`, `lichen-highlevel` (`ir`/`checker`), `lichen-kernel-ir` (`KernelBody`/`BasicBlock`), `lichen-lowlevel` (`resolve.rs`/`loop_conversion.rs`/`loop_run.rs`), `lichen-compute` (`compute/body.rs`, `compute/wasm/`), `lichen-compute-gpu` (`spirv.rs`/`dispatch.rs`) | **in progress — see §8 for the handoff.** Settled: unroll is the default, `@loop` means *loop* (not permission to refuse), scope is tail-recursive cycles, no write in a loop body, conversion runs in **`lichen-lowlevel`**, over the templates — and its output is the recursion's **roles**, not a CFG, because the value graph is a DAG and a body is `lichen-kernel-ir`'s fact. Landed on `dev`: the **SSA `KernelBody`** (`ValueId`s, `BasicBlock`s with `params`, `Br`/`CondBr`/`Return { values }` — so a loop header's carried state and a function's argument are one rule), the validator, the `@loop` keyword, the wasm backend lowered through `waffle`, the conversion itself (`Module::loop_conversion`: the carried state's paths, the base tests, the steps and the exits, or a `LoopRefusal` naming the shape rule), and **its first consumer — the host loop** (`loop_run.rs`): a marked recursion runs as a loop in the evaluator, so a trip count the expansion cannot afford is answered instead of refused, with the unroll's own values. **Closed since**: the `passed_out` contract, a loop body that can compute its state and reaches the backedge, the graph-dispatch item §8.3 carried, and (found on the way) the `selection_of`/`operands_of` mispeel that hid every conditional from the graph. **Open and blocking**: the conversion's **kernel** reader — a host site the evaluator can run is answered, but a marked site whose state is a run-time value is still refused as `LoopNotEmitted`, because no producer builds a loop's `KernelBody` (§8.5 step 4, §8.6); the SPIR-V emitter's `If`/`While` are a port of the parked branch onto the SSA body (§8.4), and it must **not** be ported by translating its old operand-stack walk |
| [The wasm backend: handoff](notes/wasm-backend-handoff.md) | `lichen-compute` (`compute/wasm/`), `lichen-kernel-ir` | **handoff, with the first migration step landed** — read first for anything touching wasm code generation: what is on `dev`, what is on `feature/waffle-spike`, the six settled questions and their evidence, and the migration in the order it should be done |
| [A loop body cannot compute its state and jump back](notes/loop-body-expressiveness.md) | `lichen-kernel-ir` (`body.rs`), `lichen-compute` | **closed** — `Terminator::Jump` exists and `validate_flow` lets a body name the loop's landmarks, so a body that computes its next state and reaches the backedge is expressible; §2 records what was unrepresentable and §4 the two changes |
| [Lowering structured control flow to WebAssembly](notes/wasm-control-flow.md) | `lichen-compute` (`compute/wasm/`) | **design, adopted, and the straight-line half has landed** — the four wasm rules a loop needs (§1), the four defects of the hand-written emitter that was withdrawn and their one cause (§2), the type-section ordering bug that was hiding under them (§3, fixed), §5: adopting `waffle` with the spike that proves it works, and §6: what the migration has landed and what §2 still says about it |
| [The GPU backend for the lowered-kernel IR](notes/lichen-compute-gpu.md) | `lichen-compute-gpu`, `lichen-kernel-ir` | current |
| [Graph JIT: a chain of dispatches as one submission](notes/compute-graph-jit.md) | `lichen-graph-ir`, `lichen-compute` (`compute/graph.rs`) | current |
| [The compute JIT on low types](notes/compute-jit-low-types.md) | `lichen-compute` | current |
| [One query surface for types and classes](notes/type-query-api-proposal.md) | `lichen-highlevel` (`shape`), `lichen-compute`, `lichen-compute-gpu` | **superseded in diagnosis; §2's Level 1 landed in part** — the three conversion failures it blamed on a missing API had three other causes ([kernel-class-crossing-fixes](notes/kernel-class-crossing-fixes.md) §0); the `shape` accessors are built and consumed (§7), which also fixes a decided field read's lazy type, and `field_index`/`class_of`/Level 2 are not |
| [Finishing per-value class semantics](notes/kernel-class-crossing-fixes.md) | `lichen-compute` (`compute.rs`), `lichen-compute-gpu` (`spirv.rs`), `lichen-language` (`tests/compute.rs`) | **landed** — a `Bin`'s class is its operands', both element types are declared in every SPIR-V module, and a body may compute in one class and cross; the GPU test targets are ported to the per-value IR and run; §9 is what was measured, §10 what the merge surfaced |
| [Runtime scalars in a parallel kernel](notes/compute-runtime-scalars.md) | `lichen-compute`, `lichen-compute-gpu` | **CPU path end to end** — the fragment types its scalar leaves from the parameter's own fields and the launch hands them to each worker beside the index; the device path and a recorded body still carry the extent alone, and both are refused rather than mis-run |
| [One channel for a class's value and its low type](notes/class-channel.md) | `lichen-lowlevel` (`equality`), `lichen-highlevel` (`shape`), `lichen-compute` | **planned** — the lowlevel's class-routed value/low-type channel versus the highlevel's node-slotted second reading; §2 readers ask the class, §3 the decider writes through the choke-point, §4 the struct-argument migration that carries it, §5 the signature-before-`build` change that deletes the need |
| [The GPU algorithm roadmap](notes/gpu-algorithm-roadmap.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-compute-gpu` | **proposal** — the order the four axes go in |
| [The GPU algorithm ladder](notes/gpu-algorithms-ladder.md) | `lichen-compute`, `lichen-compute-gpu` | exploration record — the evidence the roadmap argues from |
| [Parallel primitives (`parallel`/`plrun`/`pget`/`pcollect`)](notes/lichen-compute-parallel.md) | `lichen-compute` | historical |
| [Zed extension: build & test workflow](notes/zed-extension-testing.md) | `lichen-language-zed`, `lichen-language-server`, `tree-sitter-lichen` | current |
| [Tree-sitter generated-files testing](notes/tree-sitter-generated-files.md) | `tree-sitter-lichen`, `lichen-language-zed` | current |
| [Code audit and remediation queue](notes/code-audit.md) | — (all crates) | current — the queue is the work list; each item's status is live |
| [The comment policy](notes/comment-policy.md) | — (all crates) | current — the rule is enforced in CI by `comments-check`; the tree does not yet satisfy it, and the note carries the numbers |

## The language spec

[language-spec.md](language-spec.md) is the single source of truth for the lichen
**source language**: syntax, grammar, semantics, name resolution, and diagnostics.
It is the one document from the project's early days that is retained as the language
reference, because it describes a stable, user-facing contract. Feature notes and
rustdoc refer to it for syntax and never restate it.

## Reference

- The attribute extension is *inspired by* a paper — see
  [Typed Perspectives](reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf).
