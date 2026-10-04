# Loop conversion: compiling a marked recursive function into a loop nest

> Status: **in progress.** The design below is settled, and the parts [§8.2](#82-landed-on-dev)
> lists are landed on `dev` — the `KernelBody` IR with `Flow::Seq`, the validator,
> the `@loop` keyword, the wasm emitter, and the depth refusal. **The conversion
> itself is not written.** Three of [§8.3](#83-known-broken-and-by-whom)'s items are now
> **closed** — the `passed_out` contract, a loop body that can compute its state and
> reach the backedge (`Terminator::Jump`, and a validator that lets a body name the
> loop's landmarks), and the graph-dispatch item — and the wasm `While` fix is
> **withdrawn**: serving a loop in wasm goes through **`waffle`**, which owns the
> slot-first pipeline ([wasm-control-flow](wasm-control-flow.md) §5), and until its
> control-flow step lands the backend refuses a loop **by name** rather than
> mis-compiling one. `feature/waffle-spike` has lowered every non-looping kernel
> body through `waffle` already; what is missing is `If`/`Jump`/`While`
> ([wasm-backend-handoff](wasm-backend-handoff.md) §3.2).
> [§8.4](#84-unmerged-branches-and-exactly-what-each-needs) is what the two unmerged
> branches need, and [§8.5](#85-the-critical-path-to-the-acceptance-case) is the
> critical path to the acceptance case.
> [§8.6](#86-nothing-produces-a-loop) is the finding that reorders the reader's
> expectations of step 4: **nothing in the tree produces a loop**, and the reason
> is not in the evaluator.
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
produces is a **description of the loop**, which the JIT's walk turns into a
loop and emits. It is not a pass over an emitter's body and not an emitter-side
inline: a cycle cannot be recognised from a stack-machine walk of a finished
body, because that walk sees one call at a time and has already lost the caller.
The evaluator is what *is* the recursion.

> **Corrected: the conversion is lowlevel's, not evaluation's.** This section
> originally said the conversion runs in lichen evaluation and produces "a loop
> already recorded in the graph". **Both halves are wrong**, and the reason is one
> sentence of its own argument read properly: the conversion cannot happen in a walk
> of a finished body because *the deep pass has already expanded it* — by then the
> graph holds N copies, not a cycle. So it runs where the cycle still exists, which
> is `lichen_lowlevel`'s own graph, in the window between the bodies being compiled
> and the deep pass expanding them, and it produces a **control-flow skeleton** that
> the JIT walks ([§8.6](#86-where-the-conversion-lives-lowlevel-and-the-jit-reads-it)).
>
> **The argument above still carries, and it is worth being exact about why**: it is
> about *what information is available where*, not about which crate. The caller is
> preserved — lowlevel holds the same templates the caller is an apply node in. What
> is given up is the location.
>
> **The first two steps below are therefore lowlevel's**, not the evaluator's —
> "find the cycle" and "defunctionalise" are both over `Module`'s functions and
> `NodeId`s. `evaluation.rs` may still decide *whether* a mark becomes a loop, but
> it does not build one.

Three steps, and the first two are **lowlevel's**:

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
- **The conversion itself** — `crates/lichen-lowlevel/src/loop_conversion.rs`:
  `Module::loop_conversion(function)` answers §4's shape rules over the marked
  template and, for a convertible recursion, returns **what it converts to** —
  the carried state (one path per slot), each base test with its two arms, each
  step's next-state computations, and each exit's result values, all as
  `NodeId`s. It is **not** a control-flow graph; see §8.6 for the line, and
  `resolve.rs` for why the graph cannot hold one.
  `Module::parameter_value_path` is its read-resolution query, and the refusals
  are `LoopRefusal::{NotRecursive, MutualComponent, NonTailCall, NoBaseCase,
  StateShape}`, each with the name a diagnostic carries. Verified against the
  checker's real templates by `loop_marker.rs` (a scalar state and a two-element
  tuple state both convert; a non-tail call and a mutual pair name their rules)
  and by hand-built shapes in `tests/basic/loop_conversion.rs`. The probe
  `examples/recursion.rs` prints one verdict per shape.
- **The marker reaches the graph through real source** — `compile.rs`'s
  block-wide binding arm re-stamps `ExprKind::Function::looping` when the
  binding is `@loop`. Without it the mark was dropped at the transplant, and the
  probe read "no looping function found" for a source that had written `@loop`
  (`@loop sum_to = …` is a binding whose value is the function, so the two nodes
  are one). **Found by the dump probe, not by a test.**
- **`resolve.rs`'s `selection_of` views the conditional correctly** — the
  container rule is now the emitter's own (`emit_node`'s `value_of` arm): an
  operator target at constant 0 is a `value_of` peel, and only a target that
  *holds an array value* is a container. The previous rule peeled the target with
  `pair_value_half` first, which misreads the conditional's **bare two-element
  arms array** as a `[value, type]` pair — so `selection_of` answered `None` for
  every `[else, then][selector]` the checker compiles, and `operands_of`'s `Index`
  arm peeled the operand array the same way. Both are fixed; the conditional
  resolves as `Selection::Computed`, `Index(apply, 0)` as `Views(apply)`.
- **The host loop** — `crates/lichen-lowlevel/src/loop_run.rs`: a marked,
  convertible recursion is **run** by the evaluator instead of expanded. One
  iteration is one `instantiate` (the same clone-and-unify the unroll uses, so
  the per-iteration parameter check, the per-iteration asserts and the class
  topology all come from the one implementation) plus the iteration's test
  resolved into that instantiation.
  Two things about the backedge and the exit were learned by running it, and
  both are now the design:
  - **The state crosses the backedge lazily, and the *exit* decides it.** The
    step's next-state nodes become the next iteration's parameter values through
    the same unify the unroll uses, so nothing is computed before the next test
    asks for it — but the loop's *result* is forced at the moment the base is
    chosen, **inside the loop**: a base's value is the state the loop
    accumulated, a chain one link per iteration, and forcing it is `count`
    applications deep. Forced outside the loop that chain is the *apply*
    budget's business, which refused counts the loop exists to afford.
  - **The result is built, not read off the template's return.** Evaluating the
    return produces the same `[value, type]` pair, but it leaves the selection's
    *untaken* step arm — an apply of the same function — in the graph, and the
    next deep pass walks it and unrolls one level, then that level's arm, and so
    on: the trip count the loop had just avoided paying was charged to the apply
    budget *after* the answer was already known (measured: a 1_000-count loop
    answered, then ~2_000 nested applications exhausted the budget). The loop
    therefore assembles the pair from the base's value half and the return's own
    type half, which is the whole of what an apply's result is.
  **The budget: an iteration is one application, and the saving is nesting.**
  The loop charges the *real* counters — one application per iteration, exactly
  as the unroll charges one per unwound level, plus whatever the body applies
  through the ordinary frame — so the cumulative bound means the same thing on
  both paths and a host that wants a large trip count raises *that* number.
  There is no loop-specific budget, and an earlier draft's was wrong twice: it
  was invented, and its "one unit per iteration plus one per call inside" made
  a loop's cost unreadable against an expansion's.
  What a loop never spends is **nesting**, and measuring where that binds was
  the other half of the work. It is *not* the lazy host path: a tail recursion's
  expansion keeps `apply_depth` flat (the apply returns its pair and the deep
  pass descends into it), so under one total bound the loop and the unroll
  afford the same count — measured, 1998 against 1999 for the reduction here.
  Nesting binds where it is real:
  - a **strict** recursion, where each level's result is forced (the lowlevel
    harness's countdown: the unroll meets `ApplyDepth`, the loop answers —
    `tests/basic/host_loop.rs`),
  - the **emitter's** expression-nesting ceiling (§8.5 item 1c's 512), which is
    what the kernel path pays for an expansion and a loop does not, and
  - the kernel path's whole definition-pass cost, which a converted loop does
    not pay at all.
  So the honest headline is narrower than "the loop affords a trip count the
  expansion cannot": **it affords the same work at a flat depth**, and a host
  loop is *currently* worth what that is worth on the host path — which is
  scope and stack, not count. The kernel side is where the count is free.
  **The cost, stated plainly**: each iteration still instantiates the body
  (~fifty nodes for the reduction here), so a host loop is linear in the trip
  count with a large constant — O(1) in *depth*, which is what removes the
  ceiling, but not yet cheap per iteration. The obvious next step is not a
  second design but the same one cheaper: walk the roles with a slot map over
  *one* instantiation instead of instantiating the body per iteration, which is
  also the shape the kernel reader wants.
  Evidence: `tests/basic/host_loop.rs` (the loop and the unroll agree; the loop
  spends exactly one application per iteration, pinned by the total bound; 50
  iterations answer under a depth bound of 8 where the unroll meets the nesting
  guard; an endless convertible loop is refused by the work budget; a marked but
  unconvertible recursion still expands), `lichen-highlevel`'s `loop_marker.rs`
  (a host that raises the work bound runs a 3_000-count loop) and
  `lichen-language/tests/loop_run.rs` on real programs (loop/unroll agreement at
  small counts, and both bounded by the same work budget at 3_000).
- **The wasm backend** — walks the structure and emits `If` and `While`. An `if`
  frame *is* the join; a `while` is a `loop` wrapped in a `block` so its two exits
  agree; a carried value is a local, because a `br` to a loop label takes no
  operands. **The reader exists; nothing writes the document it reads** — see
  [§8.6](#86-nothing-produces-a-loop). The hand-written emitter this bullet
  describes was withdrawn and replaced by **`waffle`**; straight-line fragments
  are not byte-identical to the pre-change emitter, which nothing asserts.
- **The emitter depth ceiling** — `emit_node` is `#[stacksafe]` and budgeted at
  `MAX_KERNEL_BODY_DEPTH` = 512, so a body too deep to lower is refused by name
  instead of overflowing the stack.

### 8.3 Known broken, and by whom

1. **The wasm `While` never tested its condition — NOT CLOSED, and the fix is
   withdrawn.** `lower_terminator`'s `While` arm used to emit the enclosing block's
   instructions and *then* the `loop` opcode, so the condition was computed once
   before the loop and never re-tested. Reordering it was attempted, hit four
   separate defects — all of them consequences of tracking the operand stack by hand
   — and was **withdrawn**. What replaced it is **`waffle`**
   ([wasm-control-flow](wasm-control-flow.md) §5), which owns the slot-first
   pipeline; the hand-written emitter is deleted and the straight-line path lowers
   through it ([wasm-backend-handoff](wasm-backend-handoff.md) §3.1). **The loop
   itself is §8.5 step 2a**, and what it still waits on is the JIT reader §8.6
   names — the conversion that answers "what is the loop" landed (§8.2), and
   nothing consumes it. The blocker is not anything in this emitter. The one
   independent bug the withdrawn attempt found —
   every block type being declared *after* the type section was serialized — is
   fixed.
2. **`passed_out` is under-specified — CLOSED.** The contract is now stated in the
   IR's own doc (`crates/lichen-kernel-ir/src/body.rs`, `Terminator::While`): the
   loop's whole state is the tuple of `carried` values the header's instructions
   start from, both counts are read off *that* stack after the condition is popped,
   and **the exit receives the header's own top `passed_out` values**. The exit's
   values are the header's rather than the body's because a zero trip count never
   runs the body, so a `passed_out` the body had to compute would have no source on
   that path; `passed_out ≤ carried` therefore holds, and `validate()` refuses a
   loop that breaks it by name. This is what the parked SPIR-V emitter already
   implements, so the fix was to the **documentation**, not to that emitter.
3. **`sums()` in `graph_on_device.rs` could not be dispatched on the GPU**, which
   is `dispatch.rs`'s documented refusal (a dispatch pushes the launch extent
   alone; a runtime scalar needs the leaf list only the CPU path passes). The test
   expected the opposite, so it was a test to update, not a fragment to repair —
   **done**: `sums()` declares the ABI's two leaves (extent and index) and the
   chain test runs, while a fragment that *does* declare a runtime scalar is
   refused by name in `a_parameter_with_a_runtime_scalar_is_refused_by_name`. Not
   this feature's work, and no longer open.
4. **A loop body could not compute its state and reach the backedge — CLOSED.**
   `Flow::Seq`'s terminator is a `Box<Terminator>`, and a plain transfer *was* only
   `Flow::Jump` — a variant of `Flow` — so the shape `Seq`'s own doc names could not
   be built, and with it **no terminating loop was representable at all**. Fixed by
   two changes: **`Terminator::Jump` now exists**, and **`validate_flow` lets a loop
   body name the loop's landmarks** — its header (the backedge) or its exit (leaving)
   — where the old rule required everything to reach the header.
   [loop-body-expressiveness](loop-body-expressiveness.md) has the analysis, what was
   unrepresentable, and the two changes.


### 8.4 Unmerged branches, and exactly what each needs

**Both are deliberately parked**, not dropped: this tree's clean-up pass does not
merge work that needs a rebase and a measurement of its own, and nothing about
them is lost — the commits are on the branches, and what each needs is below.
A successor should treat this section as the handoff.

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
- **`feature/eval-loop-recording`** (`ff0cbeb`, rebased onto `dev` and merged)
  — the marker reaching the evaluator, cycle detection, and the *entering-call*
  insight (the recursive call's own argument is the next state, undecided for
  every trip count, so only an entering call's argument is the count — and the
  curried chain has to be resolved to find it). All of that is **still right and
  still needed**. The rebased branch now computes real **strongly connected
  components** (§8.6's `B`, §3 step 1) rather than one cycle per marked binding.
  **What it needs**: the `value_decided` gate drives the refusal, and it must be
  **deleted and inverted** — a marked recursion becomes a loop, and the gate has
  no remaining consumer, because for the unmarked path "is it decidable" is
  already answered implicitly by whether the deep pass reduced the call.
  **Its probe claim survives the rebase, and only after a fix that had nothing to
  do with the marker.** Two constants were still on the pre-`dev`
  `compute.write [n, i, v]` spelling, which the surface has since replaced with
  the `.Write`/`.Read` struct form and which is refused as an unsupported index —
  `RECURSIVE_LITERAL` had been updated and `RECURSIVE_LITERAL_MARKED` had not,
  which reads exactly like the marker breaking a decided trip count. It does not:
  with the spelling fixed, `RECURSIVE_LITERAL_MARKED` answers `4: ?a`, which is
  what `a709c2d` said. **Stale probe programs, not a regression** — and the
  lesson is the probe's, not the branch's: a probe is evidence only where it is
  re-run, and two constants differing by one keyword is exactly the shape that
  fools you.
  `LOOP_RUNTIME_COUNT_MARKED_DECIDABLE` is still refused, by a **type** check —
  `expected Int -> Int, found Int -> Int` — and so is its unmarked twin
  `LOOP_IN_LICHEN`, which never had a marker. That is the curried
  `f => n => x => …` combinator no longer type-checking, which is a program to
  rewrite rather than a behaviour to explain.

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

In order, and the order is forced. **Item 1 is done; 1b is half; 2a has
cleared the CPU side but nothing turns on it yet, because 1c is not:**

1. ~~**Settle `passed_out`** (§8.3 item 2) in the IR's doc and, if it needs more than a
   count, in the type.~~ **Done** — the exit reads the header's own top `passed_out`
   values, it is stated in `body.rs`'s `Terminator::While`, and `validate()` refuses
   `passed_out > carried` by name.
1b. **Make a loop body expressible** (§8.3 item 4) — **half done**. The
   *transfer* is: `Terminator::Jump` exists and `validate_flow` lets a body name the
   loop's header or its exit. The *carried read* is not, and §2.1 of
   [loop-body-expressiveness](loop-body-expressiveness.md) is still true for that
   reason: a body can arrive at the header but cannot carry anything new.
1c. **Give the IR an instruction that reads the carried tuple.** `LocalGet` names a
   **parameter leaf**, and nothing names element `k` of the loop's current state —
   so a body can only forward the header's own values, and every loop the IR can
   build runs zero trips or forever. **This is what a lowering discovers on its way
   past**: `Flow::While` lowers through `waffle` and the thing it lowers cannot run,
   and `compute.rs`'s two loop fixtures are the forwarding shape, which is why they
   validate and cannot execute. The shape has one real question — a new
   `KernelInstr` beside `LocalGet`, or a second domain for `LocalGet` — and it
   belongs here, ahead of every remaining item, because nothing downstream can be
   demonstrated until a loop terminates.
2. ~~**Fix the wasm `While`** (§8.3 item 1).~~ **Done, by way of `waffle`.** Four
   defects, one cause (hand-tracked operand-stack height) took the hand-written
   reorder out of the picture; the backend lowers the whole body through `waffle`
   ([wasm-control-flow](wasm-control-flow.md) §5).
2a. ~~**Lower `If`/`Jump`/`While` through `waffle`.**~~ **Done — the CPU-side
    blocker is cleared.** The hand-written slot-based emitter this step called for
    was **not written** — `waffle` owns the slot-first pipeline
    ([wasm-control-flow](wasm-control-flow.md) §5), so the step was its
    control-flow mapping. All three transfers land in
    `crates/lichen-compute/src/compute/wasm/flow.rs`: the header's carried tuple as
    blockparams, `CondBr` at the test, `Br` at the backedge, and a selection's arms
    as blocks of their own. 62 kernel-execution tests green on all of it; the
    type-section ordering bug §2 found was fixed earlier. **What it does not clear
    is the backend's own gap**: no repository test builds a loop body, so the
    backedge is verified by structure rather than by a run, and the IR still has no
    instruction that reads the carried tuple into a body — so a loop may be
    *expressible* now without any loop that reaches the backend being able to
    terminate. That instruction is **item 1c**, and it is what this list turns on.
    See [wasm-backend-handoff](wasm-backend-handoff.md) §3.2.
3. **Rebase and extend `feature/spirv-loop-emitter`** for `Seq` — and it also
   inherits 1b: the refusal it wrote for a body that is "only a transfer" is the
   SPIR-V emitter saying there is no block for `OpLoopMerge`'s continue target, which
   `Terminator::Jump` now resolves.
4. **Make the JIT *emit* what the conversion returned.** The conversion half is
   no longer missing — it is `Module::loop_conversion` (§8.2) — and neither is
   its first consumer: the **host loop** (§8.2) runs a marked recursion in the
   evaluator, which is what makes the conversion observable today. What remains
   is the *kernel* reader: build the loop's `KernelBody` (SSA, `Flow::While`,
   one block per arm), bind the entering call's arguments to the state slots,
   and map each slot to a local — the same roles the host loop reads, emitted
   instead of interpreted. `value_decided` still decides whether a marked
   *kernel* site is refused: a host site the evaluator can run is answered, and
   a site whose entering state is a run-time value is still refused (now by
   name — the conversion's rule, or `LoopNotEmitted`), because no kernel loop
   exists to run it. **Read
   [§8.6](#86-where-the-conversion-lives-lowlevel-and-the-jit-reads-it) before
   sizing this.**
5. **Run the reduction on both backends**, past the 2000-apply budget and the 512
   level ceiling, at more than one length so the count is demonstrably not a
   compile-time constant.

**Do not start 3 before 1.** Doing SPIR-V first against a contract that is already
known to be wrong is how the `br_if` bug above came about, and it is the one
mistake this section exists to prevent.

### 8.6 Where the conversion lives: lowlevel, and the JIT reads it

**Step 4's "the recorded structure itself" is not a piece of evaluator work, and
§8.5 has been under-stating it.** Reading the tree rather than the plan: the chain
this feature is a chain of — *something turns a marked recursion into a control-flow
graph → the JIT reads it → a backend emits it* — has **no first link**, and the gap
is not in `feature/eval-loop-recording`. Four facts, each one a read of the code
rather than an inference:

1. **Nothing in production builds a non-straight-line `KernelBody`.**
   `KernelFragment::body` is reached through `From<Vec<KernelInstr>>`, which is
   `KernelBody::straight_line` — a `Flow::Block` with `entry: None` and a
   `Return`. The only `Flow::While` / `Terminator::While` constructions anywhere in
   the tree are in `lichen-compute`'s `kernel_intern_tests`: hand-built bodies
   that prove the IR can *express* a loop, not that anything **produces** one.
   `emit_node` refuses the recursion before it gets that far: its `Apply` arm's
   "Style 1 — a full lichen-function call (inline its body)" is *deferred*.
2. **The graph cannot express control flow.** `LowOperator` is exactly
   `Index | Apply | TableGet`; a `Node`'s `operation` is **one** operator and **one**
   operand array, defined once by `add_node` or `close_operation_cycle` and never
   replaced. There is no branch, no label, no merge, and no phi.
3. **`Block` in lowlevel is a garbage-collection unit** — an arena, with a `Bump`
   and a parent/child chain for collection. It is not a basic block, and
   `TraceContext::node_block` reads as though it were. The name collision is
   between `lichen_lowlevel::BlockId` (an arena) and `KernelBody`'s `BlockId` (a
   control-flow label), and the two are in the same feature.
4. `close_operation_cycle` is the nearest existing thing and it is **not** a
   control-flow cycle: it closes a *value* cycle, where a node's operand is only
   nameable after the node exists.

§8.2's "the wasm backend walks the structure and emits `If` and `While`" is
therefore true only of the reader: **the document exists and nobody writes one.**

#### Decided: the conversion is `lichen-lowlevel`'s, and the JIT reads it

**Lowlevel turns the marked recursion into a control-flow graph; `lichen-compute`
walks that graph instead of the nodes.** §3 is corrected above and stays corrected:
the conversion does **not** run in evaluation.

The argument that decides it is the one §3 already makes, read for what it is
rather than where it lives. *"A cycle cannot be recognised from a stack-machine
walk of a finished body, because that walk sees one call at a time and has already
lost the caller."* The load-bearing half is **the deep pass has already expanded
it**: by the time any consumer walks, the graph holds N copies, not a cycle. So the
conversion has to run where the cycle still exists — which is **lowlevel's own
graph, between the bodies being compiled and the deep pass expanding them** — and
its output has to be something a consumer can walk.

That gives three things, and they are the whole design:

| | what | where |
|---|---|---|
| the **mark** | [`Function::looping`](../../crates/lichen-lowlevel/src/lib.rs) | rides on the template, because the templates are the only place the recursion is still a cycle — every apply clones them away |
| the **analysis** | the strongly connected components of the marked call graph | same window, over the same templates the deep pass is about to walk |
| the **output** | **which node plays which role** — the carried state's paths, each base test, each step's next state, each exit's values — as [`LoopConversion`](../../crates/lichen-lowlevel/src/loop_conversion.rs) | read by the **host loop** (`loop_run.rs`, §8.2) and, when it lands, by the JIT |

**It has one reader today, and that is what makes the contract testable.** The
host loop walks the roles and *runs* them — one instantiation per iteration,
the same clone-and-unify the unroll uses — so "the loop and the unroll agree" is
a property that can be checked now, on real programs, before any backend emits a
loop (`tests/basic/host_loop.rs`, `lichen-language/tests/loop_run.rs`). The JIT
reader will walk the same roles and emit them; if the two disagree, the
conversion or the reader is wrong, and the host loop is the side that is already
running.

**The output is the recursion's own facts, not a control-flow graph, and that is a
correction this section needed.** The table used to say "a control-flow skeleton
over `NodeId`s — blocks, terminators, and which nodes each block evaluates", and
`resolve.rs` states why that cannot live in lowlevel: this graph is a **DAG of
values** — every node is computed once and `evaluate_node_deep` answers a value
for every node it reaches — while a CFG says *some of this is not computed once*
(a header is entered many times and its blockparams differ each time). That is a
**program** fact, and the program is `lichen-kernel-ir`'s `KernelBody`, which is
becoming SSA for exactly this reason. So the conversion names roles over the
value graph and the body is built where bodies live. What the earlier draft got
right is what survives: the conversion runs on the templates, before the deep
pass expands them, and a consumer reads its answer.

**The skeleton says *what runs when*; the JIT keeps saying *how to emit it*.** That
split is what makes this cheap: class tracking, `Positions`, the depth budget and
every refusal the emitter has stay in `lichen-compute`, and lowlevel grows no
knowledge of kernels.

**And it costs nothing I was wrong about in the A/B fork this section used to
carry.** A loop never has to be a *node*. Nodes stay values — so the GC roots,
`TraceContext`, and the deep pass's verdicts (`evaluated_deep`, `assumed_concrete`)
are untouched, which was the whole of the objection to a loop node — and the graph
gains one flag plus one derived structure. The fork I wrote framed the choice as
"the graph carries control flow" against "control flow is nowhere"; there was a
third answer and it is the right one.

#### The order the work goes in

Each step is landable and each is *used* by the one before it lands:

1. **The mark moves down.** `Function::looping`, stamped by the checker where the
   function's shell exists. **Done.**
2. **The analysis moves down** with it: the components over `Module`'s function
   graph, in the window between the statement pass and the deep pass. The checker
   keeps only stamping and recording sites. **Done, in `loop_conversion.rs`** —
   `loop_component` walks the marked call graph over the templates (nested
   closures included) and the checker only *asks*, per component entry, what the
   conversion says. The checker's own IR-level Tarjan run stays for what it alone
   can answer: which **call sites** enter a component and whether their argument
   is decided.
3. **The conversion facts**, and the JIT reads them for ordering and for the
   nest's shape. A body with no marked cycle is one block, which is exactly what
   the JIT emits today, so nothing else has to move. **Landed as
   `Module::loop_conversion`** (§8.2): a shape that converts, or a
   `LoopRefusal` naming the rule that refused it. **What remains is the reader** —
   no backend consumes `LoopConversion` yet, so a converted-and-undecided marked
   call still fails the build, now as `DiagKind::LoopNotEmitted` rather than as
   the generic refusal.
4. **The deep pass stops expanding a marked cycle** it cannot decide. Until this
   lands there is no graph with a live cycle in it for step 5 to read — and this is
   the step most likely to be underestimated: `function_apply` clones per
   application, and the budget refusal is the symptom.
5. **The nest**: defunctionalise, build the loops, and teach the JIT to emit
   `Flow::While` from them.

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
