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
   every merge point.

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
| 2 | Every carried value is a **kernel scalar** — `LowShape::USize` or `Float` | the carried value must live in a native local; a heap node cannot be a loop register. [`LowShape`](../../crates/lichen-lowlevel/src/lib.rs) already exists for exactly this read |
| 3 | The closure's **captured environment is loop-invariant**, and is hoisted out of the nest | otherwise the nest is not single-entry and the "outermost level" has no meaning |
| 4 | **Strict**: the header's test is a forced comparison, and no lazy `~` crosses an iteration | the deepest obstacle, and lichen-specific — see §5 |
| 5 | The component's **size is capped** (a compile-time constant) | a device wants statically shaped control flow; the depth is a property of the program, not of the data |
| 6 | Only the cycle is rewritten; a call leaving it stays a call | this is not full-function defunctionalization, and should not become one |

## 5. Rule 4 is the hard one, and it is lichen's own

`if c then a else b` **desugars to `Index([e, t], c)`** — a lazy branch index, not
a control-flow branch ([the spec](../language-spec.md) §4). So the language's
conditional is a **value-level select**, and the emitter compiles it to a
branchless `Select` rather than a jump. Turning a select whose two arms are
recursive calls into a `CondBr` is the *whole* of the conversion, and it is valid
only where both arms are forced at that point.

That makes the strictness obligation concrete, and it is not one predicate:

- the header's base test must be a **comparison**, forced, never a branch index;
- neither arm of a converted select may leave an unforced thunk that is read on a
  later iteration — a thunk created in iteration *i* and forced in iteration
  *i + k* would read a register that has since moved;
- a `~` **inside** the body but strictly contained in it is fine, and a lazy
  value that leaves the nest as the result is fine; what is forbidden is a lazy
  value that **survives across an iteration boundary**.

Failing this must be a refusal, because a program that gets it wrong does not
compute the wrong number — it computes a number that depends on when forcing
happened, which is the hardest class of defect this project has to avoid.

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

> A body containing a loop or a branch makes the every-ordinal-written claim
> void; an unwritten output slot is a fresh allocation on one backend and device
> padding on the other.

Two consequences are worth stating plainly, because they are the price:

- **The scattered-histogram shape becomes expressible and silently wrong** — 64
  elements into 3 buckets, last writer wins, on both backends, no diagnostic
  ([gpu-algorithms-ladder §5](gpu-algorithms-ladder.md)). That is accepted.
- **`CONDITIONAL_WRITE` stays exactly as it is, outside the loop.** A `write` in a
  plain branch is still refused by name; only the loop is exempt. The two rules
  must not be confused, because a loop is "a branch that repeats" and a reader
  will assume the same rule covers it.

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

**Stage 1 — the structured body.** `KernelFragment::body` becomes an arena of
basic blocks with explicit terminators, and both backends are taught it: wasm
gains `block`/`loop`/`br_if` and a label stack; the SPIR-V emitter gains one
`OpLabel` per block and **replaces** the single-`OpLabel` invariant with a
structural one (one label per block, one terminator per block, every merge block
dominated by its header). This is the expensive half and it is not optional, so
it goes first, alone, with a single-loop shape to prove the IR and the two
emitters before any analysis exists on top of it. It also converts the emitter's
**400-to-1000 hard overflow** into a named refusal, which is worth having even if
nothing else lands.

**Stage 2 — the conversion.** Cycle extraction, defunctionalisation, nest
construction, and rules 1, 2, 3, 5, 6. Rule 4 comes with it, because a conversion
that silently changes when forcing happens is worse than a refusal. The probe
grows two cases: `RECURSIVE_INLINE` and `LOOP_RUNTIME_COUNT` in
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
per lane" and a workgroup; it is not a workgroup.
