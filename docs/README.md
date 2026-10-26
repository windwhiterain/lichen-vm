# lichen-vm documentation

This folder is the project's **documentation map**. lichen-vm keeps its
documentation in exactly two places, on purpose:

- **In-code Rust doc comments** (`//!` and `///`) — the authoritative, always-current
  description of what each module and item does. They live next to the code they
  describe, so they cannot drift.
- **This folder** — human/agent-oriented *feature notes*, the **language spec**,
  the user-facing **book**, and reference material. A note explains what a feature
  is, why it exists, and how to use it, then points at the modules that implement
  it. It never repeats the implementation detail the rustdoc already carries; the
  book teaches the language in its own order and defers to the spec where a
  contract matters.

> **Rule: one doc per concern.** Each feature has exactly one note here, and each fact
> is stated in exactly one place — the note, the spec, or the rustdoc. If you are
> about to repeat a fact, *link to it* instead. A note's source of truth is the code;
> read the note, then read the modules it names.
>
> **Rule: an implementation invariant lives in the code.** A rule the code must
> keep is stated in a `# Invariant` section at the code it governs; a note may
> explain why it exists and record the measurement, but it is never the only
> place the rule is written. A note that states a rule links to the code section
> that owns it. The language contract is the exception: `language-spec.md` and
> the book state what a program means, and code cites them.
>
> **Rule: a dead path is named as dead.** A note may cite a path that no longer
> exists only when the note itself says what became of it — a probe reverted after its
> measurement, a module that moved, a deliberate "there is no such file" correction.
> A missing path with no such sentence is a stale note; fix the path or the sentence.
> A sweep of the notes found every missing path accounted for this way (removed
> probes, the `whiting` repository's own crates, and one path correction).

## Read order

1. [The lichen book](book/src/title-page.md) — the language, taught from scratch.
2. [README](../README.md) — what lichen is, the crate map, quick start.
3. [Architecture overview](notes/overview.md) — the core idea (`[value, type]`
   exprs, `Type : Type`) and the layering.
4. The feature note for the crate you are working in.

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
| [Lowlevel VM](notes/lowlevel-vm.md) | `lichen-lowlevel` | current |
| [`@gc e`: an expression in a scope of its own](notes/gc-block.md) | `lichen-language-lex`/`-parser`, `lichen-highlevel` (`checker/gc`), `lichen-lowlevel` (`gc`/`evaluation`/`function`), `lichen-language` (`block_boundary`), `lichen-lowlevel` (`tests/basic/gc_safety`) | current |
| [Static modules & registry](notes/static-modules.md) | `lichen-lowlevel`, `lichen-language` (`persist`) | current |
| [Low types for a lowlevel-based JIT](notes/lowlevel-low-types.md) | `lichen-lowlevel` (`LowShape`), `lichen-highlevel` (`shape`), `lichen-compute` | current |
| [A call's clones belong to the enclosing template](notes/apply-clone-ownership.md) | `lichen-lowlevel` (`function`/`static_module`) | current |
| [Normalization is a function's birth, not an application](notes/function-normalization.md) | `lichen-highlevel` (`checker`), `lichen-lowlevel` (`static_module`) | current |
| [The un-annotated recursive fold's type stays open](notes/recursive-fold-typing.md) | `lichen-language` (compile), `lichen-highlevel` (`checker`), `lichen-lowlevel` (`loop_run`, `function`) | current |
| [One test harness: write IR by name, ask the module about it by name](notes/test-harness.md) | `lichen-testkit`, `lichen-highlevel` (tests) | current |
| [The lowering map: which expression a node came from](notes/lowering-map.md) | `lichen-highlevel` (`lowering.rs`, `checker.rs`) | current |
| [The one-buffer gather: a wall, or an unnamed spelling?](notes/gather-spelling.md) | `lichen-compute` | current |
| [The recorded leg's count: the rule is principled, its own sentence is nearly unreachable](notes/recorded-extent-count.md) | `lichen-compute` (`compute.rs`, `compute/graph.rs`), `lichen-compute-gpu` | current |
| [A class carries the call, not only the fact](notes/class-carries-the-call.md) | `lichen-lowlevel` (`equality`), `lichen-compute` | current |
| [Non-termination is the program's own](notes/nontermination-is-the-programs.md) | `lichen-highlevel` (`checker`), `lichen-lowlevel` (`apply`) | current |
| [A colliding answer is a unify error](notes/answer-collision.md) | `lichen-lowlevel` (`equality`, `write_node_answer`), `lichen-compute` | current |
| [The read-path collection was disabled](notes/gc-trigger-disabled.md) | `lichen-lowlevel` (`evaluation`, `gc`) | historical |
| [The `@gc` trigger: what decides a scope is done](notes/gc-trigger-design.md) | `lichen-lowlevel` (`evaluation`, `gc`), `lichen-language` (tests) | current |
| [Floating point](notes/floating-point.md) | `lichen-lowlevel`, `lichen-highlevel`, `lichen-render`, `lichen-kernel-ir`, `lichen-compute`, `lichen-compute-gpu` | current |
| ["Contains the universe" is not "is the universe"](notes/universe-containment.md) | `lichen-render`, `lichen-highlevel` (`checker/structs`) | current |
| [The deferred instantiation checks the type only](notes/deferred-instantiation-checks.md) | `lichen-highlevel` (`checker`), `lichen-lowlevel` (`apply`), `lichen-perspective` | historical |
| [Evaluation runs before unification, and nothing wakes what it read](notes/eval-before-unify.md) | `lichen-lowlevel` (`evaluation`/`equality`), `lichen-highlevel` (`checker`) | historical |
| [The epoch run gate, retired](notes/stalled-rerun-epoch.md) | `lichen-lowlevel` (`evaluation`/`equality`/`module`), `lichen-compute` | historical |
| [Unify without forcing](notes/unify-without-forcing.md) | `lichen-lowlevel` (`equality`) | historical |
| [Incremental evaluation](notes/incremental-evaluation.md) | `lichen-lowlevel`, `lichen-highlevel` (`checker`), `lichen-language` (`run`) | proposed |
| [The editor checks without running](notes/editor-check-mode.md) | `lichen-language-server` (`analysis`), `lichen-highlevel` (`checker`) | current |
| [Late writes map](notes/late-writes-map.md) | `lichen-lowlevel` (evaluation), `lichen-compute` (`compute`) | current |
| [Graph recording and the re-drive: one graph is one walk](notes/graph-recording-rounds.md) | `lichen-compute` (the recording pass), `lichen-lowlevel` (`reopen_stalled`) | current |
| [Function types, and a struct type's nominal identity](notes/function-type-merge.md) | `lichen-highlevel` (`checker`/`lambda`/`shape`), `lichen-lowlevel` (`equality`/`static_module`), `lichen-render`, `lichen-language` | current |
| [A function unifies by its own cells](notes/function-unify.md) | `lichen-lowlevel` (`equality`/`function`/`apply`/`evaluation`), `lichen-highlevel` (`checker`) | current |
| [A nested apply loses the parameter it consumed](notes/nested-apply-none.md) | `lichen-lowlevel` (`evaluation`/`apply`/`function`), `lichen-highlevel` (`checker`) | current |
| [Array attribute view](notes/array-attribute-view.md) | `lichen-lowlevel` (`lib`/`module`/`codec`/`static_module`), `lichen-highlevel` (`shape`/`checker`) | current |
| [Compound element slots](notes/compound-element-slots.md) | `lichen-lowlevel` (`equality`/`evaluation`), `lichen-highlevel` (`checker`/`shape`), `lichen-compute`, `lichen-render`, `tree-sitter-lichen` | current |
| [Value templates](notes/value-templates.md) | `lichen-language-lex`/`-parser`, `lichen-language`, `lichen-highlevel`, `lichen-lowlevel` | in progress |
| [The template instantiation](notes/template-instantiation.md) | `lichen-lowlevel` (`template`) | current |
| [Operator polymorphism](notes/operator-polymorphism.md) | `lichen-highlevel`, `lichen-lowlevel` (`assert`), `lichen-language`, `lichen-compute`, `lichen-std` | current |
| [`core`: the built-in prelude](notes/core-prelude.md) | `lichen-language` (`core.lichen`), `lichen-registry` | current |
| [Extensible attributes](notes/attributes.md) | `lichen-highlevel` (`attr`), `lichen-language`, `lichen-doc` | current |
| [Checker encoding: unstable at the `lichen-compute` boundary](notes/checker-encoding-instability.md) | `lichen-highlevel` (`shape`), `lichen-compute` | current |
| [Block vs lambda struct-field inference](notes/block-vs-lambda-struct-field-inference.md) | `lichen-highlevel` (`checker`) | historical |
| [A type value's rendering depends on its form](notes/type-rendering-inconsistent.md) | `lichen-render`, `lichen-language` (`render`) | historical |
| [The raw mark in the printer](notes/raw-rendering-mark.md) | `lichen-render` | current |
| [Tests do not assert what the printer prints](notes/tests-do-not-render.md) | every test that renders a printer result | current |
| [The computational operators](notes/operators.md) | `lichen-language-lex`/`-parser`/`-language`, `lichen-highlevel`, `lichen-kernel-ir`, `lichen-compute`, `lichen-compute-gpu` | current |
| [Separating lexer & parser from the language](notes/frontend-syntax-separation.md) | `lichen-span`, `lichen-language-lex`, `lichen-language-parser`, `lichen-language`, `lichen-highlevel` | current |
| [No type mode](notes/no-type-mode.md) | `lichen-language-parser` | current |
| [Placeholder `_` anywhere](notes/placeholder-anywhere.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-language-server` | current |
| [Record programs (modules)](notes/record-program.md) | `lichen-language-parser`, `lichen-language`, `lichen-language-server` | current |
| [Type accesses `X{n}` and `X::a` (removed)](notes/type-access.md) | `lichen-language-lex`/`-parser`, `lichen-language`, `lichen-highlevel`, `lichen-language-server`, `tree-sitter-lichen` | historical |
| [`type_of` is a standard-library function](notes/type-of-in-std.md) | `lichen-language`, `lichen-highlevel`, `lichen-compute`, `lichen-std` | current |
| [Incremental parse/compile](notes/incremental-parse-compile.md) | `lichen-language-lex`/`-parser`, `lichen-language`, `lichen-highlevel` | current |
| [Incremental update: identity by path, retention by `@cache`](notes/incremental-update.md) | `lichen-language-parser` (`path`), `lichen-language` (`cells`/`session`), `lichen-lowlevel` (`freeze`/`registry`), `lichen-compute` | current |
| [Cross-edit cache: heavy values retained by key](notes/heavy-value-cache.md) | `lichen-language` (`cells`/`session`/`key`/`cellcache`), `lichen-lowlevel` (`static_module`), `lichen-compute` (buffer value codec) | current |
| [Build performance](notes/build-performance.md) | `lichen-language-parser` | current |
| [Parse performance: the clean probe](notes/parse-clean-probe.md) | `lichen-language-parser`, `lichen-language-lex` | current |
| [Language toolchain](notes/language-toolchain.md) | `lichen-span`/`-lex`/`-parser`/`-language`/`-server`, `lichen-language-zed` | current |
| [Lichen Home for the LSP](notes/liche-lsp-home.md) | `lichen-language-server` (`home`), `lichen-language` | current |
| [Packages & import](notes/packages.md) | `lichen-preprocess`, `lichen-language`, `lichen-compiler` | current |
| [Package manager](notes/package-manager.md) | `lichen-package`, `lichen-preprocess` | current |
| [Isolating the preprocessor](notes/preprocessor-isolation.md) | `lichen-preprocess`, `lichen-span`, `lichen-language`, `lichen-package` | current |
| [The assembled-kernel disk cache](notes/kernel-disk-cache.md) | `lichen-runtime` (`module_cache`), `lichen-compute` (`language`) | current |
| [Cross-process artifact store](notes/artifact-cache.md) | `lichen-registry`, `lichen-language` (`persist`), `lichen-lowlevel` | current |
| [Testing the package manager offline](notes/venv-test.md) | `lichen-package`, `scripts/venv-test.sh` | current |
| [README example sync](notes/readme-sync.md) | `lichen-tools`, `lichen-compute` (`tests/lichen_compute/readme.rs`) | current |
| [The compiler-plugin model](notes/compiler-plugin.md) | `lichen-utils` (`extend`), `lichen-lowlevel`, `lichen-highlevel`, `lichen-compute` | current |
| [The plugin taxonomy](notes/plugin-taxonomy.md) | `lichen-highlevel` (`plugin`), `lichen-compute`, `lichen-std-native`, `lichen-perspective`, `lichen-language` | current |
| [A native link library: a Rust generic behind a lichen generic](notes/native-link-library.md) | `lichen-lowlevel` (`External`, the entry table), `lichen-highlevel` (`native`, `TypeValue::name`), `lichen-language` (`link`: loader, recorder, builder, splice, check imports; the `link` directive), `lichen-language-server` (the editor check path), `lichen-preprocess` (`link`), `lichen-compute` (the kernel-body refusal) | proposed |
| [lichen-compute: the JIT package](notes/lichen-compute.md) | `lichen-compute`, `lichen-highlevel` (`native`) | current |
| [Kernels as `.native`/`.I`/`.O` structs](notes/compute-kernel-struct.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-compute` (`language`) | current |
| [Kernels declare buffers; the pack places them](notes/kernel-buffer-declarations.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-compute-gpu`, `lichen-runtime` | in progress |
| [A buffer is a struct wrapper, and a parallel result is the parameter's `.out`](notes/compute-buffer-wrapper.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-compute` (`tests`) | current |
| [The compute JIT on low types](notes/compute-jit-low-types.md) | `lichen-compute` | current |
| [The kernel parameter's class](notes/kernel-parameter-class.md) | `lichen-highlevel`, `lichen-lowlevel`, `lichen-compute`, `lichen-render` | current |
| [Per-value class semantics](notes/kernel-class-crossing-fixes.md) | `lichen-compute`, `lichen-compute-gpu`, `lichen-compute` (`tests`) | current |
| [Runtime scalars in a parallel kernel](notes/compute-runtime-scalars.md) | `lichen-compute`, `lichen-compute-gpu` | current |
| [Graph JIT: a chain of dispatches as one submission](notes/compute-graph-jit.md) | `lichen-graph-ir`, `lichen-compute` (`compute/graph.rs`) | current |
| [Taking a kernel to a server](notes/taking-a-kernel-to-a-server.md) | `lichen-compute` (the fused artifact), `lichen-registry` (the frozen one) | current |
| [Specialize-then-normalize: a named access should become a tuple index](notes/specialize-normalize.md) | `lichen-highlevel` (the choice), `lichen-compute` (the stand-in) | historical |
| [A launch that is not one row](notes/two-dimensional-launch.md) | `lichen-kernel-ir`, `lichen-compute`, `lichen-runtime`, `lichen-compute-gpu` (the emitters) | proposed |
| [A workgroup that is not one row](notes/two-dimensional-workgroup.md) | `lichen-compute-gpu` (the emitters, the dispatch), `lichen-compute` (the lowering), `lichen-kernel-ir` (the plan, the digest) | in progress |
| [Which write fills a `Buf` element cell](notes/compile-order-class.md) | `lichen-compute` (the role table), `lichen-lowlevel` (the equality classes), `lichen-highlevel` (the shape cells) | current |
| [Auditing the recorded walls](notes/wall-claims-audit.md) | `lichen-compute`, `lichen-lowlevel`, `lichen-language` (a recursion), `lichen-compute-gpu` (walls 4, 5, 8 unreached) | current |
| [A diagnostic an agent can act on](notes/agent-friendly-diagnostics.md) | `lichen-language-parser` (the cascade, the dedup), `lichen-highlevel` (the checker's diagnostics) | current |
| [Transcendentals as operators](notes/transcendentals-as-operators.md) | `lichen-language-lex`, `lichen-language-parser`, `lichen-highlevel`, `lichen-kernel-ir`, `lichen-compute` (both lowerings), `lichen-compute-gpu`, `lichen-fusion-bridge`, `lichen-runtime` (the codec) | current |
| [The `examples` panic, diagnosed](notes/examples-panic-diagnosis.md) | `lichen-tools` (the runner), `lichen-language` (the compile), `lichen-highlevel` (the checker), `lichen-lowlevel` (`@gc` and `evaluation.rs:517`) | historical |
| [An undecided type cell: the four `examples` drifts](notes/undecided-type-cell.md) | `lichen-lowlevel` (`equality`/`evaluation`), `lichen-highlevel` (`checker`) | current |
| [The last instance: is `arithmetic_class`'s open class the defect?](notes/arithmetic-class-stated.md) | `lichen-highlevel` (`arithmetic_class`, `check_index`), `lichen-lowlevel` (`check_asserts`) | current |
| [The index read that is never taken](notes/index-read-unify.md) | `lichen-highlevel` (`checker/structs.rs`'s `slot_read`, `checker/indexing.rs`'s `check_shallow_array_term`), `lichen-lowlevel` (`alias_read`, `apply_type`), `lichen-render` (the read) | current |
| [Does the tail closure reach `@shallow(n)`?](notes/shallow-depth-closure.md) | `lichen-highlevel` (`checker/indexing.rs`'s `check_shallow_array_term`/`wrap_shallow`/`close_shallow_tail`), `lichen-language` (`tests/language/pipeline.rs`, `src/resolve.rs`), `examples/` (the survey) | current |
| [What closing the depth mark's type cell actually does](notes/shallow-depth-mechanism.md) | `lichen-highlevel` (`checker/indexing.rs`'s registration, `checker.rs`'s walk), `lichen-lowlevel` (`shape`) | current |
| [What a depth-marked element holds, and where the arithmetic went](notes/shallow-depth-element-values.md) | `lichen-highlevel` (`checker/indexing.rs`'s `check_shallow_array_term`/`wrap_shallow`, `checker.rs`'s registration walk), `lichen-lowlevel` (`evaluation`'s `Index`), `lichen-language` (`testing`, `tests/language/pipeline.rs`) | current |
| [LLM inference operators, and what the machinery says about them](notes/llm-operators.md) | `examples/rmsnorm.lichen`, `lichen-runtime` (the passes), `lichen-fusion-bridge` | current |
| [The ladder's device leg, measured](notes/device-leg-of-the-ladder.md) | `examples/{sample,topk,kvcache,decode_loop,generate}.lichen`, `lichen-compute-gpu` (the dispatch) | current |
| [The sampler's crash, localised to one emitted id](notes/dispatch-crash.md) | `crates/lichen-compute-gpu/src/spirv.rs` (the emitter), `examples/sample.lichen` | current |
| [The operators' device legs, one example at a time](notes/device-operator-sweep.md) | `examples/{rope,silu,softmax,rmsnorm,layernorm,attention,gemv,matmul}.lichen`, `lichen-compute-gpu` (the backend and the validator) | current |
| [The operators' bodies, swept rather than sampled](notes/device-body-sweep.md) | `examples/{rope,silu,softmax,rmsnorm,layernorm,attention,gemv,matmul}.lichen`, `lichen-compute-gpu` (the backend and the validator) | current |
| [What the operator line cannot express, and who owns it](notes/operator-gaps.md) | `crates/lichen-compute` (the probes), `lichen-compute-gpu` (the fold template), the type system, fusion | current |
| [An inference service, and the layer it is missing](notes/inference-service.md) | `lichen-graph-ir` (`Scalar`, `Count`), `lichen-compute-gpu` (`dispatch.rs`), `lichen-runtime` (the artifacts), `crates/lichen-compute` (the seven probes) | current |
| [Writing a solver: the order that works](notes/writing-a-solver.md) | `examples/stencil.lichen`, `examples/torus.lichen`, `crates/lichen-compute` (the launcher and the leaves), `lichen-language` (the fold and the annotation rule) | current |
| [Physics operators: a stencil, and the boundary that is the padding](notes/physics-operators.md) | `examples/stencil.lichen`, `lichen-compute` (the host read rule), `lichen-compute-gpu` (the tail-lane guard) | current |
| [The transcendental gap, named](notes/transcendental-gap-mechanism.md) | `crates/lichen-kernel-ir/src/unary.rs`, `crates/lichen-compute-gpu/src/spirv.rs` | current |
| [The contraction can be forbidden](notes/contraction-control.md) | `crates/lichen-compute-gpu/src/spirv.rs`, `crates/lichen-kernel-ir/src/body.rs`, `crates/lichen-kernel-ir/src/unary.rs` | current |
| [Is the contraction fix durable?](notes/contraction-durability.md) | `crates/lichen-compute-gpu/tests/compute_gpu/spliced_transcendentals.rs`, `crates/lichen-compute-gpu/src/spirv.rs`, `dispatch.rs` | current |
| [The operator bodies, re-verified under the shipped emitter](notes/operator-reverification.md) | `examples/{rope,silu,softmax,rmsnorm,layernorm,attention,gemv,matmul}.lichen`, `crates/lichen-compute-gpu/tests/compute_gpu/spliced_transcendentals.rs` | current |
| [What a device's quotient is](notes/division-rounding-gap.md) | `crates/lichen-compute` (the probe), `crates/lichen-kernel-ir` (`ln_body`), `crates/lichen-render` (the printer) | current |
| [A self-referential block-wide binding: the crash, the ruling, and what is left open](notes/self-referential-binding-overflow.md) | `crates/lichen-language/src/resolve.rs`, `crates/lichen-compute/src/compute/body.rs`, `crates/lichen-highlevel/src/checker.rs` | current |
| [The fused multiply-add: what it is, what it would touch](notes/fused-multiply-add.md) | `crates/lichen-kernel-ir/src/lib.rs` (`KernelInstr::Fma`), `crates/lichen-compute-gpu/src/spirv.rs` (the emitter), `crates/lichen-runtime/src/wasm/lower.rs`, `crates/lichen-fusion-bridge/src/lower.rs`, `crates/lichen-runtime/src/fused/schema.rs` | current |
| [The GPU backend for the lowered-kernel IR](notes/lichen-compute-gpu.md) | `lichen-compute-gpu`, `lichen-kernel-ir` | current |
| [Loop conversion: recursion into a loop nest](notes/loop-conversion.md) | `lichen-language-lex`/`-parser`, `lichen-highlevel`, `lichen-kernel-ir`, `lichen-lowlevel`, `lichen-compute`, `lichen-compute-gpu` | in progress |
| [The wasm backend: handoff](notes/wasm-backend-handoff.md) | `lichen-compute` (`compute/wasm/`), `lichen-kernel-ir` | in progress |
| [Lowering structured control flow to WebAssembly](notes/wasm-control-flow.md) | `lichen-compute` (`compute/wasm/`) | in progress |
| [One channel for a class's value and its low type](notes/class-channel.md) | `lichen-lowlevel` (`equality`), `lichen-highlevel` (`shape`), `lichen-compute` | current |
| [The GPU algorithm roadmap](notes/gpu-algorithm-roadmap.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-compute-gpu` | proposed |
| [The GPU algorithm ladder](notes/gpu-algorithms-ladder.md) | `lichen-compute`, `lichen-compute-gpu` | historical |
| [Graph fusion: logic, interfaces, runner-installed packs](notes/graph-fusion.md) | `lichen-graph-ir`, `lichen-compute`, `lichen-kernel-ir`, `lichen-fusion`, `lichen-highlevel` (`attr`) | proposed |
| [Automatic DAG fusion: a heuristic strategy space](notes/automatic-fusion.md) | `lichen-runtime` (`fusion_claim`, `region_merge`), `lichen-compute` (`floor`, `placement_pack`), `lichen-fusion-bridge` | proposed |
| [Shared memory, and where a barrier comes from](notes/shared-memory-and-barriers.md) | `lichen-compute`, `lichen-kernel-ir`, `lichen-lowlevel`, `lichen-compute-gpu`, `lichen-fusion-bridge`, `lichen-perspective` | in progress |
| [Zed extension: build & test workflow](notes/zed-extension-testing.md) | `lichen-language-zed`, `lichen-language-server`, `tree-sitter-lichen` | current |
| [Tree-sitter generated-files testing](notes/tree-sitter-generated-files.md) | `tree-sitter-lichen`, `lichen-language-zed` | current |
| [A lichen program as a whiting scene](notes/whiting-scene-document.md) | `lichen-language`, `lichen-lowlevel`, external `whiting-definition` | proposed |
| [Code audit and remediation queue](notes/code-audit.md) | — (all crates) | current |
| [Open issues after the slot isomorphism](notes/open-issues.md) | `lichen-lowlevel` (`function.rs`'s clone rule), `lichen-compute` (the recording pass) | current |
| [The comment policy](notes/comment-policy.md) | — (all crates) | current |
| [One test binary per crate](notes/test-binary-layout.md) | — (all crates) | current |
| [The attribute isomorphism — handoff](notes/attribute-isomorphism.md) | `lichen-highlevel` (`checker`, `checker/annotations`, `checker/lambda`, `shape`), `lichen-lowlevel` (`apply`, `static_module/apply`), `lichen-compute` (`compile_fragment`) | current |
| [The apply's type side](notes/apply-type-side-check.md) | `lichen-highlevel`, `lichen-lowlevel` | current |
| [Who may start a backend](notes/backend-ownership.md) | — (all crates) | current |
| [Buffer precision](notes/buffer-precision.md) | `lichen-lowlevel`, `lichen-render` | current |
| [Evaluation performance](notes/evaluation-performance.md) | `lichen-lowlevel` | current |
| [The cooperative matrix product](notes/gpu-mma.md) | `lichen-compute-gpu` | in progress |
| [Half precision](notes/half-precision.md) | `lichen-kernel-ir`, `lichen-compute-gpu` | in progress |
| [Lazy plugin registration](notes/lazy-plugin-registration.md) | `lichen-language`, `lichen-package` | current |
| [The package graph](notes/package-graph.md) | `lichen-package`, `lichen-preprocess` | current |
| [Qwen3-1.7B on the device](notes/qwen-inference.md) | `lichen-serve`, `lichen-runtime` | current |
| [The serving frontier](notes/serving-frontier.md) | `lichen-serve`, `lichen-graph-ir`, `lichen-runtime`, `lichen-compute-gpu` | proposed |
| [The serving boundary](notes/serving-performance-boundary.md) | `lichen-serve` | current |
| [The register perspective the MMA needs](notes/perspective-scope-and-mma.md) | `lichen-highlevel`, `lichen-compute-gpu` | in progress |
| [Test time](notes/test-time.md) | — (the workspace) | current |
| [Undecided reads](notes/undecided-reads.md) | `lichen-highlevel` | current |

The serve crate's own README — Serving Qwen3-1.7B through an OpenAI surface — is [here](../crates/lichen-serve/README.md): how to build it, what it answers, and what it has not got.

## The book

[book/](book/) is *The lichen Programming Language*, the user-facing book. It is
written in the Rust book's idiom — one concept at a time, a complete program in
every chapter, and the compiler's real output — and its
[summary](book/src/SUMMARY.md) is the table of contents. A language change means
checking the chapters that state the behaviour it changed.

## The language spec

[language-spec.md](language-spec.md) is the single source of truth for the lichen
**source language**: syntax, grammar, semantics, name resolution, and diagnostics.
It is the one document from the project's early days that is retained as the language
reference, because it describes a stable, user-facing contract. Feature notes and
rustdoc refer to it for syntax and never restate it.

## Reference

- The attribute extension is *inspired by* a paper — see
  [Typed Perspectives](reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf).
- [Vertex Block Descent](reference/README.md) — a physics solver paper kept as
  reference material, in the [PDF](reference/Vertex%20Block%20Descent.pdf) and as
  [Markdown](reference/Vertex%20Block%20Descent.md).
