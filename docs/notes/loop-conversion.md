# Loop conversion: compiling a marked recursive function into a loop nest

> Status: **proposed.** Nothing here is implemented. This is the design the
> research settled on, the qualification rules it is gated by, and the order the
> work goes in — not a description of what the code does today.
>
> **Three decisions are closed** and are not to be re-opened without a new
> reason: the **surface** is a compile-time marker on the function, not a `loop`
> builtin; the **scope** is tail-recursive cycles only; a `compute.write` inside
> a converted loop is **allowed and unchecked** — see
> [§6](#6-what-the-raw-write-decision-costs).
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
loop as non-terminating ([`P1-40`](code-audit.md)), and a **hard stack overflow**
in the emitter's walk between 400 and 1000 levels — and an unrolled loop costs
about 29 µs of compile time per iteration.

**The missing half is a dynamic loop.** A loop reads its trip count from a
register, so it removes both ceilings by construction, and it is the only form
the device can actually run.

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

It runs **in `lichen-compute`, at the seam §1 names** — the `Apply` arm of
`emit_node` (`compute.rs:2547`) — because that is where a reduced same-module
call has already been consumed and an *unreduced* one is still a node. It is a
rewrite of the not-yet-emitted body, not an emitter-side inline pass: a cycle
cannot be recognised from a stack-machine walk, because the walk sees one call at
a time and has already lost the caller.

1. **Find the cycle.** Over the function values the body can reach, build the
   call graph and take its strongly connected components. Only a component of
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

## 6. What the raw write decision costs

**Decided:** a `compute.write` inside a converted loop is allowed, and lichen
makes **no claim** about its effects.

The claim it withdraws is stated as a theorem in
[compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md) — a
run is "bit-identical to the sequential loop's, for every `count` and whatever
the worker count" — and it is exactly what lets `dispatch` allocate output
buffers **without initialising them** ([dispatch.rs](../../crates/lichen-compute-gpu/src/dispatch.rs),
and the single-`OpLabel` invariant it is derived from in
[spirv.rs](../../crates/lichen-compute-gpu/src/spirv.rs)). A lane that never
reaches a `write` therefore leaves its slots at whatever a fresh allocation held,
and the host reads them back as results.

**This is the same hazard the ladder already records**, in the same vocabulary: a
read past the end of a four-element buffer answers `0` on CPU and `5` on the GPU,
because "the device's padding is the last value written"
([gpu-algorithms-ladder §6](gpu-algorithms-ladder.md)). So the loop case joins
that family rather than inventing one, and the note that must travel with it is:

> A body containing a loop makes the every-ordinal-written claim void; an
> unwritten output slot is a fresh allocation on one backend and device padding
> on the other.

**And the asymmetry is not a GPU rule — it is a rule we bought.** CUDA has no such
premise at all: `cudaMalloc` leaves memory undefined, a kernel that never writes
an element simply leaves it undefined, and every program is responsible for that
itself. What makes it a premise *here* is that `dispatch` skips the zero-fill, and
that skip is a lichen-specific optimisation that has no CUDA counterpart. So the
loop is unremarkable on the device and load-bearing here, and the fix — if one is
ever wanted — is to give the zero-fill back, not to constrain the loop.

**One consequence is worth stating plainly, because it is the price:** the
scattered-histogram shape becomes expressible and silently wrong — 64 elements into
3 buckets, last writer wins, on both backends, no diagnostic
([gpu-algorithms-ladder §5](gpu-algorithms-ladder.md)). That is accepted.

**And `CONDITIONAL_WRITE` is not merely left in place — Stage 1 should delete
it.** An earlier draft of this section framed the two as separate rules that a
reader might confuse. They are not separate, and the loop is not an exemption from
a rule the branch somehow escapes. A `write` in a branch is refused *today* only
because the flat IR forces both arms to be emitted, so the arm's `write` would run
on every lane and overwrite the selected arm's own write
([compute.rs](../../crates/lichen-compute/src/compute.rs) — the check is
`then_body.contains(&BufferWriteCall) || else_body.contains(&BufferWriteCall)`,
and it exists because a flat stream has nowhere to put a branch). Once the arm is
a real `CondBr`, the arm's `write` runs on exactly the lanes that took it, and
because a `write`'s buffer position is a compile-time constant while its index is
the lane's own `i` ([kernel-ir](../../crates/lichen-kernel-ir/src/lib.rs)), the
every-ordinal-written claim holds again. **A conditional write is not made legal by
a decision; it becomes correct by construction.** What survives is only the
zero-iteration case, which is the loop's own caveat above.

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
everything to get wrong later.**

This is the expensive half and it is not optional, so it goes first and alone,
with a single-loop shape to prove the IR and the two emitters before any analysis
exists on top of it. It also converts the emitter's **400-to-1000 hard overflow**
into a named refusal, which is worth having even if nothing else lands, and it is
what makes `CONDITIONAL_WRITE` deletable (§6).

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
@{ compute = import "compute.lichen" @}
sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + compute.read [buf, s(0) - 1])
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
