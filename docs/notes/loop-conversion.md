# Loop conversion: compiling a marked recursive function into a loop nest

> Status: **proposed.** Nothing here is implemented. This is the design the
> research settled on, the qualification rules it is gated by, and the order the
> work goes in — not a description of what the code does today.
>
> **Four decisions are closed** and are not to be re-opened without a new
> reason: the **surface** is a `loop` keyword on the function; the **scope** is
> tail-recursive cycles only; a `compute.write` inside a converted loop is
> **refused**, on the same ground as `CONDITIONAL_WRITE` ([§6](#6-why-a-loop-body-may-not-write));
> and **the absence of the keyword still means unroll** ([§1.1](#11-unroll-is-the-default-and-the-keyword-is-the-override)).
>
> The third was first decided the other way and then reverted, and the reason is
> worth keeping because it is the reason the first decision was wrong: an earlier
> draft allowed the write and made no claim, on the argument that a device has no
> such rule. A device indeed has no such rule — but lichen's `dispatch` **skips
> zero-filling the output buffers on the strength of one**
> ([dispatch.rs](../../crates/lichen-compute-gpu/src/dispatch.rs)), and that skip
> is ours, not CUDA's. Allowing the write was therefore not "following the
> device", it was dropping a premise we had chosen to rely on.
>
> **Where the conversion happens was also decided the wrong way first.** An
> earlier draft put it in the emitter, at the `Style 1` seam in `emit_node`, as a
> pass over a not-yet-emitted body. It belongs in **lichen evaluation** — the
> same pass that expands an unmarked recursion today — so that one place decides
> per recursive call between expanding it and recording a loop, and the JIT reads
> a loop that is already there. See [§3](#3-the-shape-of-the-transformation).
>
> Points at: `crates/lichen-kernel-ir/src/lib.rs` (`KernelInstr`,
> `KernelFragment::body`), `crates/lichen-compute/src/compute.rs` (`emit_node`'s
> `Apply` arm, `lower_body`, `CONDITIONAL_WRITE`),
> `crates/lichen-compute-gpu/src/spirv.rs` (the single-`OpLabel` invariant) and
> `dispatch.rs` (the uninitialised-output-buffer claim it underwrites),
> `crates/lichen-lowlevel/src/lib.rs` (`LowShape`),
> `crates/lichen-language/examples/recursion.rs` (the probe).
> Decided elsewhere: [gpu-algorithm-roadmap §4.1](gpu-algorithm-roadmap.md) (the
> `loop`-operator argument, the two measured ceilings), its §7 (the fork this
> answers), [compute-jit-low-types](compute-jit-low-types.md) (kernels compile
> from a template, before any apply), [compute-graph-jit](compute-graph-jit.md)
> (the recursion probe), [code-audit](code-audit.md) `P1-33` and `P1-40`,
> [lowlevel-low-types](lowlevel-low-types.md) (`LowShape`).

## 1. The problem, as one fact

A kernel is compiled from a **template, before any apply**
([compute-jit-low-types](compute-jit-low-types.md)). So a binding the body would
fill in at run time is still empty when the body is lowered, and a **recursive
call whose trip count is a run-time value** cannot be expanded: nothing decided
how many times to inline the body. Today that call reaches the emitter and is
refused by name:

> `compute.rs:2557` — *"Style 1: a full lichen-function call (inline its body) —
> deferred"* → *"kernel body Apply is supported only for a cross-kernel
> (kernel-value) callee v1; inline lichen-function calls are not yet supported"*.

The existing half of the answer is **static expansion**: the deep pass reduces a
same-module call and unifies the substituted parameter with the argument before
the emitter walks the body, so `steps 4` and `square (i + 1)` already work
([gpu-algorithm-roadmap §4.1](gpu-algorithm-roadmap.md)). Static expansion has
two measured ceilings — the VM's 2000-apply budget, which reports a *terminating*
loop as non-terminating ([`P1-40`](code-audit.md)), and the emitter's own
`emit_node` walk, which used to be a **hard stack overflow** between 400 and
1000 levels and is now a **named refusal** at
[`MAX_KERNEL_BODY_DEPTH`](../../crates/lichen-compute/src/compute.rs) = 512
levels — a trip count in the low hundreds, since expansion nests every copy
inside the last one's else arm — while an unrolled loop costs about 29 µs of
compile time per iteration.

**The missing half is a dynamic loop.** A loop reads its trip count from a
register, so it removes both ceilings by construction, and it is the only form
the device can actually run.

### 1.1 Unroll is the default, and the keyword is the override

**This is the part that decides where the work goes.** A lichen function with no
`loop` keyword is **expanded during evaluation** — the same pass that already
reduces a same-module call, working at the point where the recursion is *walked*,
which is why `steps 4` and `square (i + 1)` need no separate machinery. That is
the default and it stays the default: expansion is what you want whenever it
terminates, because a straight line has no branch, no divergence, and a
per-element write that runs exactly once.

A `loop` keyword does not turn expansion off in general. It says **this recursion
may become a loop**, and the evaluator takes that option at the same place it
would otherwise have expanded:

| the function | the trip count | what evaluation produces |
|---|---|---|
| no `loop` | decidable | **expanded**, as today |
| no `loop` | not decidable | refused, as today |
| `loop` | decidable | **expanded** — the default still wins when it works |
| `loop` | not decidable | **a recorded loop**, which the JIT then emits natively |

So the keyword is not "compile this to a loop"; it is "this recursion is allowed
to need one". The last row is the only new capability, and it is the row the two
ceilings above live in.

**And this is why the conversion belongs in evaluation, not in the emitter.** An
earlier draft put it in `lichen-compute`, at the `Style 1` seam, as a separate
pass over a body the emitter was about to walk. That is two passes making the same
kind of decision at two different times, and the second one is too late to have
the information the first one had — the evaluator is what *is* the recursion, so
it is the only place that knows the cycle, the state, and the base test while
they are still the things it walked. Putting it there also means the emitter's
job shrinks to reading a structure that is already in the graph, which is exactly
what the two backends then need to be able to do.

## 2. What the research settled

**Tail calls are not enough.** Tail-call optimisation "effectively turns this
recursive function into a loop which uses 𝒪(1) stack space"
([V8, on WebAssembly tail calls](https://v8.dev/blog/wasm-tail-call)) — but
`return_call` is still a call: the engine unwinds the frame, moves the
parameters, and jumps. A device wants no call at all.

**The boundary is measured, not stylistic.** Haberman's account of rebuilding
protobuf's parser as a chain of `musttail`-linked blocks
([musttail-efficient-interpreters](https://blog.reverberate.org/2021/04/21/musttail-efficient-interpreters.html))
ends with the limitation that decides this whole design: a **single non-tail
call** anywhere in the chain forces a stack frame, "a lot of data spills to the
stack", and the register allocation of the *entire* chain collapses. That is why
the gate is tail position, and why failing the gate is a catastrophe rather than a
slow path.

**The target IR shape is settled.** A tail-recursive function is an `scf.while`
with loop-carried values: a `before` region that evaluates the condition and
forwards values through `scf.condition`, and an `after` region that feeds the
next iteration back with `scf.yield`
([MLIR `scf` dialect](https://mlir.llvm.org/docs/Dialects/SCFDialect/)), which in
turn lowers to `cf` and then to a final target such as LLVM or SPIR-V. Structured
control flow as an intermediate IR is the consensus, not an invention here.

**A cycle of recursive closures becomes a nest of loops, and that has a name.**
MLKit's defunctionalization work converts a graph of mutually recursive closures
into loops, one per closure, in the order they are first entered
([MLKit](https://elsman.com/mlkit/pdf/mlkit-4.7.16.pdf); the PDF is behind this
link, so the algorithm below is stated from the standard form rather than quoted).
SML/NJ and MLton each ship an independent loop-conversion pass. **The step tail
calls cannot give is the one that matters**: the recursive call's *"which function
do I become next"* question stops being a runtime comparison of function values
and becomes a **compile-time edge in the control-flow graph**.

**A JIT needs the loop to be a countable first-class construct.** Truffle's
`LoopNode` exists so the runtime "can optimize loops in a better way", and the
loop's trip count is separately profiled
([`LoopNode`](https://www.graalvm.org/truffle/javadoc/com/oracle/truffle/api/nodes/LoopNode.html));
the mirror-image rule is that a **recursive AST node is not allowed** in a
Truffle language at all. So a loop that is merely "the compiler silently unrolled
your recursion" is invisible to the JIT — which is an argument for the marker
below, not against it.

**The device constrains the shape.** SPIR-V needs `OpLoopMerge` + `OpBranch`,
which is a function body becoming basic blocks; and a WGSL uniformity analysis
governs what may sit inside a loop
([WGSL §15.2.10.4, "Uniformity in a Loop"](https://www.w3.org/TR/2025/CRD-WGSL-20251206/#25)),
which is what makes a barrier inside a data-dependent loop a separate question.

## 3. The shape of the transformation

**It runs in lichen evaluation, at the point where the recursion is walked** — the
same decision the unmarked case makes when it expands (§1.1) — and what it
produces is a **loop already recorded in the graph**, which the JIT reads and
emits. It is not a pass over an emitter's body and not an emitter-side inline: a
cycle cannot be recognised from a stack-machine walk of a finished body, because
that walk sees one call at a time and has already lost the caller. The evaluator
is what *is* the recursion.

Three steps, and the first two are the evaluator's:

1. **Find the cycle.** Over the functions a `loop`-marked binding can reach, build
   the call graph and take its strongly connected components. Only a component of
   size ≥ 2, or a self-loop, is recursion; a call that leaves the component stays
   a call.
2. **Defunctionalise the component.** Each `f_i` in the cycle becomes
   - a **state vector** of its scalar parameters,
   - a **base test** `base_i(state)`, the branch that holds no recursive call and
     whose arms are the function's own results,
   - a **transition** `step_i(state) → (f_j, state')`, the branch that does hold
     the recursive call, with its argument expressions evaluated in `state`,
   - an **environment** `env_i`, which must be loop-invariant.
3. **Build the nest.** Order the cycle by first entry: `f_1` is the outermost
   level. **The level count is `|component|` and is fixed at compile time.** Level
   `L_i` has a header that evaluates `base_i` — true, the whole nest yields that
   result; false, evaluate the recursive branch's selector and descend — and a
   latch that applies `step_i` and jumps to the level `(f_j, state')` names. Each
   level carries **only its own** state; an inner level's state is re-established
   on entry, which is what keeps the nest finite-depth and register-resident.
4. **Emit.** The nest is a `while`-shaped loop per level, structurally the
   `scf.while` above. In the GPU's terms: `OpLabel` per block, `OpLoopMerge` at
   each header, `OpBranch`/`OpBranchConditional` between them, and `OpPhi` at
   every merge point. **The carried state is what the `OpPhi` is for**, which is
   why [§8](#8-the-order-of-the-work) makes it a Stage 1 requirement rather than a
   Stage 2 one.

The partial-inlining the deep pass already did is not in the way: a cycle the
deep pass *could* reduce is not in the residual graph, and a cycle it could only
half-reduce arrives with some levels already collapsed — which the analysis must
simply read as a smaller component.

## 4. The qualification rules — what "conditionally" means

Every one of these is checked, and every failure is a **named refusal naming
which rule failed**, never a silent fallback. A silent fallback would resurrect
the two ceilings of §1 invisibly, which is the defect class
[code-audit](code-audit.md) exists to keep out.

| # | Rule | Why this one |
|---|---|---|
| 1 | Every recursive apply **in the cycle** is in **tail position** | a non-tail call forces a frame and poisons the whole nest's register allocation — measured, §2 |
| 2 | Every carried value is a **kernel scalar** — `LowShape::USize` or `Float` | the carried value must live in a native local; a heap node cannot be a loop register. [`LowShape`](../../crates/lichen-lowlevel/src/lib.rs) already exists for exactly this read. **This rule also discharges what a strictness rule would have said** — see §5 |
| 3 | The closure's **captured environment is loop-invariant**, and is hoisted out of the nest | otherwise the nest is not single-entry and the "outermost level" has no meaning |
| 4 | **Strict**: the header's test is a forced comparison, and no lazy `~` crosses an iteration | the deepest obstacle, and lichen-specific — see §5 |
| 5 | The component's **size is capped** (a compile-time constant) | a device wants statically shaped control flow; the depth is a property of the program, not of the data |
| 6 | Only the cycle is rewritten; a call leaving it stays a call | this is not full-function defunctionalization, and should not become one |

## 5. There is no strictness rule, and finding that out changed the estimate

An earlier draft of this note carried a **rule 4**: both arms of a converted
`Select` must be forced, because a thunk created in one iteration and forced in
another would read a register that has since moved. **That rule was an artifact of
the flat IR, not a property of lichen, and it is gone.**

The chain, in the order it had to be unwound:

- `if c then a else b` desugars to `Index([e, t], c)` — a *lazy branch index*
  ([the spec](../language-spec.md) §4) — and the spec is explicit about what that
  buys: "an integer index selects a branch, and **the untaken branch is never
  evaluated** (the lowlevel `Index` stays lazy on it)".
- **So the language's `if` is already a branch.** Only one arm is ever evaluated.
  This is laziness giving it for free.
- **The emitter is what destroys it.** A flat instruction stream has nowhere to
  put two arms, so `emit_node` emits *both* into the body and finishes with a
  `Select` to pick one ([compute.rs](../../crates/lichen-compute/src/compute.rs),
  the `if c then … else …` arm). A `compute.read` in the untaken arm is still
  emitted and still read.

**So adding `CondBr` does not add a semantics — it gives the source semantics
back.** And with it the obligation disappears: the untaken arm does not exist, so
there is nothing to force and no thunk to carry across an iteration boundary. The
residue is rule 2, which already says the carried values are kernel scalars: a
scalar on a register has no forcing time to get wrong.

The cost of this finding is small and the benefit is large: the conversion's
hardest-sounding rule was never a rule, and the flat IR — the thing Stage 1
replaces anyway — was the only thing standing in the way.

## 6. Why a loop body may not write

**Decided:** a `compute.write` inside a converted loop is **refused by name**, on
the same ground and with the same discipline as `CONDITIONAL_WRITE`.

The premise it would break is stated as a theorem in
[compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md) — a
run is "bit-identical to the sequential loop's, for every `count` and whatever
the worker count" — and it is exactly what lets `dispatch` allocate output
buffers **without initialising them** ([dispatch.rs](../../crates/lichen-compute-gpu/src/dispatch.rs),
and the single-`OpLabel` invariant it is derived from in
[spirv.rs](../../crates/lichen-compute-gpu/src/spirv.rs)). A loop is the first
construct that makes a lane's *reachability* of a `write` data-dependent: a
trip count of zero means the body never runs. The every-ordinal-written claim is
then void, and the host reads back whatever a fresh allocation held.

**The first draft of this note allowed it, and the argument it made was wrong in
an instructive way.** The argument was: *a device has no such rule — CUDA leaves
`cudaMalloc` memory undefined and every program is responsible for it itself, so
following the device means making no claim.* The premise is true and the
conclusion does not follow, because **the skip is ours.** `dispatch` is not
following CUDA by skipping the zero-fill; it is doing something CUDA does not do,
in exchange for being able to reason that every invocation reaches its write.
Allowing a write inside a loop was therefore not "relaxing to match the device" —
it was dropping a premise we had chosen to rely on and had documented as a
theorem.

**So the rule is a `write` is refused anywhere control flow can skip it**, and
that is one rule with two sites, not two rules:

| site | why it is refused today | what changes it |
|---|---|---|
| a `write` in a `Select` arm | the flat IR emits **both** arms, so the arm's `write` would run on every lane and overwrite the selected arm's own ([compute.rs](../../crates/lichen-compute/src/compute.rs) — the check is `then_body.contains(&BufferWriteCall) || else_body.contains(&BufferWriteCall)`) | a real `CondBr`: the arm's `write` runs on exactly the lanes that took it, and a `write`'s position is a compile-time constant while its index is the lane's own `i`, so every-ordinal-written **holds again** |
| a `write` inside a loop | **nothing changes it** — a zero trip count is a real possibility, not an artifact of the flat IR | either the trip count is uniform across the warp, or the zero-fill comes back |

**A conditional write is not made legal by a decision; it becomes correct by
construction.** A loop write has no such second route today, so it stays refused.
What is still true of it, and is recorded so the day there is a barrier: the
uniform count is not a correctness rule for a `write` (each lane writes its own
index) but it is what makes a non-zero-trip count *guaranteed*, and
[§9](#9-the-performance-model-a-converted-loop-runs-under) is why that is the
shape to reach for.

What is *not* withdrawn: rule 6 above, and the fact that a converted loop is
compiled against a **template** — so a body-local binding the loop would fill in
at run time is still empty, which is a separate and still-refused case.

## 7. Why a marker, and not a `loop` builtin

[gpu-algorithm-roadmap §4.1](gpu-algorithm-roadmap.md) argues — correctly — that
`loop` must be a **builtin operator** rather than a library function, because a
library `loop` expands at exactly the ceilings of §1 while a builtin reaches the
run-time count. **Everything in that argument is kept.** What is rejected is only
the *surface*, and for three reasons:

1. `loop f n : T -> T` is a strict special case of this design. A `loop` is a
   one-node cycle, so the conversion **subsumes** the builtin rather than being
   replaced by it. Building the operator first would build a special case of the
   thing being built.
2. It cannot express what the language already has: `mutual_recursion.lichen`'s
   `is_even`/`is_odd` pair is a two-node cycle with no count parameter, and the
   general case the roadmap wants — a natural recursive formulation — is not of
   the form `T -> T` repeated `n` times. Each such site would also need the
   type annotation [`P1-33`](code-audit.md) names.
3. A **value-level** operator needs a type of its own, and a loop has none that
   fits the language: it is not a value, it is a fact about the call graph. A
   marker says that; an operator has to pretend otherwise.

The marker's real payoff is **recoverability**: a program that says "this is a
loop" and gets refused learns *which of the six rules* it violated, and the fix is
a change to one program. A silent fallback cannot offer either.

## 8. The order of the work

> **HANDOFF — where this stands, and what the next person needs.** Written
> against `dev` at the merge of `fix/adds-cfg-pos`. Read this section first; the
> rest of the note is the design and is not a description of the code.

### 8.1 What is settled, and one thing that was settled wrong

The **four decisions** hold: `@loop` is the surface; scope is tail-recursive
cycles only; a `write` inside a loop body is refused; and the conversion runs in
**evaluation**, not in the emitter, so one place decides per recursive call.

**What the marker means had to be corrected, and the correction matters.** The
first reading — the one an earlier version of this section gave — was that a
`@loop` recursion whose count is *not* decidable should be **refused by name**,
and one whose count *is* decidable should expand. **Both halves are backwards.**
Being undecided is the loop's reason for existing; `while (n > 0)` with `n` read
from memory is the most ordinary loop there is, and what being undecided
disqualifies is *unrolling*, which is a different thing. So:

| | undecided count | decided count |
|---|---|---|
| **no marker** | refuse — today's behaviour, unchanged | **unroll** — today's behaviour |
| **`@loop`** | **loop** | **loop** |

`@loop` means "this is a loop", full stop. The only choice is unroll-or-loop, not
accept-or-refuse, and the marker is not permission to refuse. **The refusals that
survive are §4's shape rules** — tail position, kernel-scalar carried values,
loop-invariant environment, no write in the body, component cap, cycle-only.
"The count is unknown" is not on that list and must not be put back on it.

### 8.2 Landed, on `dev`

- **The IR** (`lichen-kernel-ir`) — `KernelBody` with structured transfers
  (`Return` / `If` / `While`) over the same pure stack machine, plus
  **`Flow::Seq`**: instructions then a *plain* transfer. `Seq` is what lets a loop
  body **compute** its carried values instead of forwarding the header's own —
  without it a reduction, the acceptance case below, has no representation at all
  (a bare `Jump` makes the header's `OpPhi` self-referential). `validate()` is the
  gate a backend calls first: a label defined twice, arrived at but never defined,
  or shared as two loops' exit is refused; a loop's declared header must be the
  entry of the block holding it, which is what makes a zero-trip loop correct.
- **The `@loop` keyword** — through lexer, parser, AST and frontend, with `@`
  reserved as the sigil. It reaches `ExprKind::Function::looping`.
- **The wasm backend** — walks the structure and emits `If` and `While`. An `if`
  frame *is* the join; a `while` is a `loop` wrapped in a `block` so its two exits
  agree; a carried value is a local, because a `br` to a loop label takes no
  operands. **Straight-line fragments are byte-identical to the pre-change
  emitter.**
- **The emitter depth ceiling** — `emit_node` is `#[stacksafe]` and budgeted at
  `MAX_KERNEL_BODY_DEPTH` = 512, so a body too deep to lower is refused by name
  instead of overflowing the stack.

### 8.3 Known broken, and by whom

1. **The wasm `While` never tests its condition.** `lower_terminator`'s `While`
   arm emits the enclosing block's instructions and *then* the `loop` opcode, so
   the condition is computed once before the loop and never re-tested: the loop
   only ends if its body branches out. `Instruction::BrIf` appears nowhere in
   `crates/lichen-compute/src/compute.rs`. This contradicts this note's contract
   ("the condition is re-evaluated at `header` on every entry including the
   first") **and the SPIR-V emitter**, which does it correctly — so the two
   backends currently give one fragment two meanings. It is latent only because
   nothing can yet *produce* a loop. Fixing it needs the emission **reordered**:
   the `Block`/`Loop` must open *before* the header's instructions, which means
   `lower_flow`'s `Block` arm has to recognise a `While` terminator rather than
   letting `lower_terminator` do it after the fact.
2. **`passed_out` is under-specified, and this blocks the reduction.** §3 says a
   zero condition leaves to `exit` "with `passed_out` values" — a bare count that
   never says **which** values. In wasm the `Block` needs concrete values at its
   `End`, and if the carried values live in locals the emitter must `local.get`
   specific ones; `passed_out: usize` does not name them. **The natural reading,
   which is not yet written into the IR**, is the discipline the IR already uses:
   entry takes the top `carried` values, so exit hands out the top `passed_out`
   values sitting beneath the condition. That makes a reduction work — the
   header's `local.get`s leave the accumulator on the stack with the condition
   above it. **Decide this before writing either emitter's fix**, or the emitters
   will disagree again.
3. **`sums()` in `graph_on_device.rs` cannot be dispatched on the GPU**, and that
   is `dispatch.rs`'s documented refusal (a dispatch pushes the launch extent
   alone; a runtime scalar needs the leaf list only the CPU path passes). The test
   expects the opposite, so it is a test to update, not a fragment to repair. Not
   this feature's work.

### 8.4 Unmerged branches, and exactly what each needs

- **`feature/spirv-loop-emitter`** (`a0bfa2c`) — a complete SPIR-V emitter for
  `If` and `While`, validated with a real `spirv-val` (which rejected four genuine
  bugs during development), straight-line output byte-identical at 185 words, and
  the `single-OpLabel` invariant **replaced** with a stated structural one that
  `dispatch.rs` now cites. It also refuses a write inside a loop body by name.
  **What it needs**: it was cut before `Flow::Seq` existed, so it must be rebased
  onto `dev` and taught `Seq`; and its own report says the IR could not express a
  loop that computes its carried value — which is *precisely* what `Seq` fixed, so
  the pass-through-block workaround it used can likely be deleted in favour of a
  straight `Seq`.
- **`feature/eval-loop-recording`** (`a709c2d`) — the marker reaching the
  evaluator, cycle detection, and the *entering-call* insight (the recursive
  call's own argument is the next state, undecided for every trip count, so only
  an entering call's argument is the count — and the curried chain has to be
  resolved to find it). All of that is **still right and still needed**.
  **What it needs**: the `value_decided` gate drives the refusal, and it must be
  **deleted and inverted** — a marked recursion becomes a loop, and the gate has
  no remaining consumer, because for the unmarked path "is it decidable" is
  already answered implicitly by whether the deep pass reduced the call.

### 8.5 The critical path to the acceptance case

The acceptance case is a **dynamic reduction** — the one shape with an
**accumulator** (so it exercises the carried value) and a **run-time trip count**
that is the buffer's length (so it exercises the whole point). It is named as the
missing operator in [gpu-algorithm-roadmap §4.1](gpu-algorithm-roadmap.md), and it
is the shape a `T -> T` `loop` cannot express at all — the second reason §7 does
not ship one.

```lichen
---
  compute = import "compute.lichen"
---
@loop sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + compute.read [buf, s(0) - 1])
p = compute.parallel (cfg => { ... sum_to (cfg(0), 0) ... }) "BACKEND"
```

In order, and the order is forced:

1. **Settle `passed_out`** (§8.3 item 2) in the IR's doc and, if it needs more
   than a count, in the type.
2. **Fix the wasm `While`** (§8.3 item 1) — the reorder above — and emit `Seq`.
3. **Rebase and extend `feature/spirv-loop-emitter`** for `Seq`.
4. **Delete `value_decided`** in `feature/eval-loop-recording` and make the
   evaluator *record* a loop rather than refuse. This is the biggest remaining
   piece and the one no branch has started: the recorded structure itself, the
   defunctionalisation §3 step 2, and §4's shape rules.
5. **Run the reduction on both backends**, past the 2000-apply budget and the 512
   level ceiling, at more than one length so the count is demonstrably not a
   compile-time constant.

**Do not start 3 before 1.** Doing SPIR-V first against a contract that is already
known to be wrong is how the `br_if` bug above came about, and it is the one
mistake this section exists to prevent.

**Stage 0 — the `loop` keyword and the evaluator's choice.** The surface lands
first, and it is the smallest thing that can be observed working: a `loop` keyword
on a binding, carried from the lexer to the highlevel IR, so the evaluator can
see it. The default is unchanged — an unmarked recursion still expands — and the
first observable behaviour is the **refusal** side: a `loop`-marked recursion
whose count is not decidable still says "this is a loop and I have no way to
record one yet", which is a diagnostic that names the missing piece instead of the
`NodeId` one names today.

**Stage 1 — the structured body, and it must carry values, not just control
flow.** `KernelFragment::body` becomes an arena of basic blocks with explicit
terminators, and both backends are taught it: wasm gains `block`/`loop`/`br_if` and
a label stack; the SPIR-V emitter gains one `OpLabel` per block and **replaces** the
single-`OpLabel` invariant with a structural one (one label per block, one
terminator per block, every merge block dominated by its header).

**The correction: a CFG alone is not enough, and a Stage 1 without it is a Stage 1
that cannot be used.** A loop needs two things and they are not the same thing:

- **control flow** — a backedge, which is a branch;
- **values that survive the backedge** — SPIR-V's `OpPhi` at the header, one
  `(value, predecessor-label)` pair per incoming edge; wasm's `local.set`/`local.get`.

An earlier draft of this section specified only the first, on the assumption that a
loop could borrow a Function-storage-class `OpVariable` for its carried value. **It
can, and it must not**: that is scratch memory rather than a register, and on a GPU
it is the difference between a loop that runs and a loop that is memory-bound. The
repository's own invariant already names both missing pieces in one sentence — "the
emitter emits **no branch and no phi**"
([spirv.rs](../../crates/lichen-compute-gpu/src/spirv.rs)) — and this stage has to
answer both, because **the `phi` is the half that costs nothing to get right and
everything to get wrong later.** The rule is not a preference: SPIR-V's validator
rejects a Function-storage-class `OpVariable` outside a function's first block, so
on the GPU target the scratch route is not merely slow, it is illegal.

**And the stage order was wrong when this section was first written.** It put
Stage 1 first "because it is expensive", on the reasoning that the emitters are the
big risk. The correction (§3) is that the emitters are **downstream** of the
evaluator: the evaluator records a loop, and the emitters read one. So the emitters
are necessary but they are not first, and starting them before Stage 0 is building
a reader for a document nothing writes yet. Stage 1a's part of this stage has
landed — the `KernelBody` type, its validator, and both backends refusing a
transfer by name — which is the right order for it: the shape is fixed and
enforced before anything emits one.

**The acceptance case is a dynamic reduction.** Everything above is argued; a
reduction is where it is either true or not, because it is the one shape that has
**an accumulator** — so it exercises the `phi` — and a trip count that is a
**buffer's length at run time** — so it exercises the whole point. It is named as
the missing operator in
[gpu-algorithm-roadmap §4.1](gpu-algorithm-roadmap.md) ("a reduction needs an
accumulator, and the one a GPU algorithm wants is a fold over a **buffer** whose
trip count is the buffer's length"), and it is the shape a `T -> T` `loop` cannot
express at all, which is the second reason §7 does not ship one.

The reduction to be written and run on **both** backends, against a buffer filled
by a seed kernel, at more than one length so that the trip count is demonstrably
not a compile-time constant:

```lichen
--- compute = import "compute.lichen" ---
loop sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + compute.read ((compute.Read _)(.from buf, .at s(0) - 1)))
p = compute.parallel (cfg => { ... sum_to (cfg(0), 0) ... }) "BACKEND"
```

Three things must be true of it, and each is a rule above being exercised: it runs
at a length **past the 2000-apply budget** (rule set by Stage 1 removing the
ceiling), its accumulator is a **carried value** (Stage 1's `phi`), and its
trip count is **per-lane** (the performance model of §9, not a correctness one).

**Stage 2 — the conversion.** Cycle extraction, defunctionalisation, nest
construction, and rules 1, 2, 3, 5, 6. The probe grows two cases:
`RECURSIVE_INLINE` and `LOOP_RUNTIME_COUNT` in
`crates/lichen-language/examples/recursion.rs` are the programs this has to move
from REFUSED to a number, on **both** backends.

**Stage 3 — the write rules.** With §6 settled, the only remaining work is the
documentation side and the measurement: `spirv-val` / `spirv-dis` on the emitted
module (the offline validation this project's SPIR-V emitter is already built
around), and the ceilings of §1 re-measured to show they are gone.

> **The emitter's ceiling is already a refusal, so Stage 3 measures one number
> fewer.** `emit_node` is `#[stacksafe]` and depth-budgeted at
> `MAX_KERNEL_BODY_DEPTH` = 512, and its refusal already names the fix this
> section describes: *"Mark the recursion `@loop` so it may become a dynamic loop
> instead of an expansion"* — with the doc reference this section owns
> ([§1.1](#11-unroll-is-the-default-and-the-keyword-is-the-override)). That
> matters for the staging: **the keyword lexes and parses already** (Stage 0a
> landed it), so a program refused at 512 levels is being sent to a surface that
> exists and is simply not honoured yet. Until the evaluator's half lands, the
> honest reading of a 512-level refusal is "this recursion needs a `@loop` the
> compiler can name but not yet record" — which is a Stage 0b dependency, not a
> Stage 1 one. The 400-to-1000 **crash** it replaced was the same symptom with no
> way to act on it at all.

**One thing this does not buy.** A workgroup that cannot talk to itself buys
throughput, not capability: scan, sort, tiled matmul and sub-group reduction need
**shared memory and a barrier** ([gpu-algorithm-roadmap §4.2](gpu-algorithm-roadmap.md)),
and no loop provides them. A loop is the missing primitive *between* "one element
per lane" and a workgroup; it is not a workgroup. §9 is why the reduction above is
an *acceptance case* and not the finish line.

## 9. The performance model a converted loop runs under

A loop inside a parallel kernel is not a loop on a host, and nothing about it is
neutral. Three facts, none of which is a correctness question:

1. **The trip count is per-lane, and that is the default rather than the
   exception.** A parallel kernel hands every lane one index
   (`compute.range n`), and the count a converted loop runs on comes from that
   lane's own value. So converted loops are **divergent** unless the author made
   them otherwise.
2. **Divergence costs `max`, not `sum`.** Under SIMT the warp runs until every
   lane in it has exited, masking off the ones that finished. A warp with one lane
   running 1 iteration and another running 1000 pays for 1000
   ([PTX ISA §9.5, "Divergence of Threads in Control
   Constructs"](https://docs.nvidia.com/cuda/parallel-thread-execution/index.html#divergence-of-threads-in-control-constructs)).
3. **So the two count shapes are worth telling apart**, and the difference is
   cheap to state: a count derived from a **uniform** value (`cfg(0)`, a buffer's
   length) is warp-uniform and costs nothing; a count derived from the **lane's own
   index** (a prefix sum's `N - i`) diverges and costs `max`.

This is also the reason the uniform-count requirement exists at all, for a barrier
inside such a loop: `bar.sync` / `__syncthreads()` requires every un-exited thread
in the CTA to arrive, so a non-uniform count under a barrier is a hang, which is
what the WGSL uniformity analysis is about
([WGSL §15.2.10.4, "Uniformity in a Loop"](https://www.w3.org/TR/2025/CRD-WGSL-20251206/#25)).
There is no barrier today, so this constrains nothing yet — it is recorded so that
the day there is one, the rule is written down rather than rediscovered.

> **Sources, honestly labelled.** Read this session: MLIR `scf`, V8 tail calls,
> the `musttail` measurement, Truffle `LoopNode`, and the PTX ISA table of
> contents (the section bodies were truncated by the fetch, so §9.1–3 above are
> from knowledge, not from that read). Not readable this session: the SPIR-V
> specification proper — `registry.khronos.org` answered 403 and the SPIRV-Registry
> `adoc` source 404'd — so the `OpLoopMerge` placement rule is asserted from
> knowledge and unchecked against the text.
