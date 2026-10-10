# The GPU algorithm roadmap: what a buffer map cannot express

> Status: **proposal.** Not a design of any one feature — a statement of what
> a `compute.parallel` kernel cannot say, split into the four axes those gaps
> fall on, and an order to take them in.
>
> The evidence is [gpu-algorithms-ladder](gpu-algorithms-ladder.md): eighteen
> programs written and run against both backends on a real device. Every
> "cannot" below was reached, not inferred.
>
> **Floating point is deliberately out of scope.** It is the gate on the
> *numeric* half of this and it is designed in
> [floating-point](floating-point.md); §6 says what that costs this roadmap
> and why the rest is still worth doing first.
>
> Points at: [lichen-compute-gpu](lichen-compute-gpu.md) (the backend and the
> numbers this re-reads), [compute-buffer-wrapper](compute-buffer-wrapper.md)
> (the parallel ABI, and the determinism invariant §4.4 would break),
> [attributes](attributes.md) (`Perspective`, checked and unread — §4.2's
> subject), [plugin-taxonomy](plugin-taxonomy.md) (the rule each axis is
> classified against), [floating-point](floating-point.md),
> [compute-jit-low-types](compute-jit-low-types.md) (the `i64` kernel ABI).

## 1. The shape of the problem, in one sentence

`compute.parallel` is a **buffer map**: one invocation per index, a
straight-line body, reads from bound buffers, blind writes to fresh output
buffers, on a single flat index space.

The ladder's successes are all one fact restated — a tree reduction is a chain
of kernels, a 2×2 matmul is a loop the author unrolled by hand at a size they
chose, a 16-link chain is one submission. Cross-index work is expressible; it is
expressible *by chaining and by unrolling*, which is the shape of the primitive
showing through. Everything that does not fit that shape falls into one of four
axes, and **the four are not variations of one change** — they differ in which
layer they touch, in what they cost, and in one case in whether they are a
capability or only a constant factor.

| axis | the question | today |
|---|---|---|
| **A. ingress** | can a program value become a buffer? | no; a fill kernel is the only source |
| **B. body** | can a body loop, or reach across indices? | no; straight-line, compile-time sizes only, and a decidable trip count is the ceiling |
| **C. width** | can one invocation own several elements? can invocations see each other? | no; one element per lane, no shared memory, no barrier |
| **D. slot** | can a lane read-modify-write a slot another lane may touch? | no, and it is silently wrong |
| **E. value** | is there anything but an integer? | no — deferred, §6 |

**And one thing that is none of these**: the **graph's shape** — how many
dispatches there are — is decided at build time today, and §4.5 argues it should
not be. It is not a fifth axis because it is not about a kernel body at all; it
is the level above, and it is what the other four are waiting on.

## 2. The order, and why it is that order

**A → B → C → D**, with E gating what any of them is *for*.

A comes first because it is the only axis whose absence stops a program before
it starts: there is no way to hand the device anything it did not itself
compute. It is also the only axis that is purely additive.

C before D because a lane group is what makes an atomic meaningful: an atomic
over one element per invocation is a global contention primitive, and an atomic
over a lane group's elements is a sub-group reduction, which is the thing
algorithms actually want. D before nothing in particular — it is last because it
is the only axis that **removes a stated invariant** (§4.4).

**B was split here into "expansion" and "a loop that runs", and only the first
existed** — the second was listed as not proposed on the reasoning that a device
loop has no wasm counterpart. §4.1 reverses that: a dynamic loop is the missing
primitive *between* "one element per lane" and "a workgroup that can do work",
it is additive (the decidable case needs no IR change), and **B is better stated
as one `loop` operator that either expands or lowers, than as two features.** So B
is a single item, and the thing that makes C reachable is B's dynamic case.

**And §4.5 sits above all four**, because the "compile-time constant" limit that
keeps recurring in this document is not a property of any one axis — it is what
fixing the shape at build time costs. A dynamic graph is the only item here that
removes it, and it is also the only one whose first user is *not* a fixed count.

## 3. What the order is worth

The ladder's timings, whole program, best of 20, against a `wasmi` CPU path
whose per-element cost is an interpreter's:

| count | 1 kernel, cpu | 1 kernel, gpu | 16-link chain, gpu |
|---|---|---|---|
| 65 536 | 108.5 ms | 5.49 ms | 14.98 ms |
| 262 144 | 401 ms | 4.61 ms | 13.46 ms |
| 1 048 576 | 2839 ms | 6.54 ms | 17.86 ms |

**Read the shape, not the ratio.** The CPU column is linear in the count and the
GPU column is nearly flat across a sixteen-fold one, so the backend's advantage
is not arithmetic throughput — it is that the alternative is an interpreter.
That is a real and large advantage, and it is available *today*, to an integer
element-wise kernel, with no part of this roadmap.

**The GPU column is not monotonic, and that is the finding rather than noise.**
The 65 536 row is *slower* than the 262 144 row, and across three runs of the
same binary the 65 536 figure read 2.95, 3.90 and 5.49 ms — a spread wide enough
to invert a row. These are whole programs, so every figure includes parsing,
checking and evaluating the lichen source, and at these counts that host work
dominates a dispatch. The number to believe is the *slope*, which is consistent,
and not the level, which is not.

It also bounds what the roadmap is worth. The note's own dispatch-vs-loop
measurement found a single dispatch still behind at a million elements, and a
chain only ahead from about four links. So the argument for any axis here is
**not** "this makes the GPU faster at what it already does" — it is "this makes
a class of program expressible that is not expressible at all". Each axis below
is judged on that, and the two axes that only move a constant factor say so
about themselves.

## 4. The four axes, and the level above them

Each is classified against [plugin-taxonomy](plugin-taxonomy.md)'s one rule —
does the language layer have to change to accommodate it — because that rule is
what decides whether a package manager can pull the feature or the compiler has
to be written with it. §4.5 is not an axis: it is the **graph** rather than a
kernel body, and it is the level the other four are waiting on.

### 4.1 Axis B: **already in the language**, and what it does not reach

**Correction, made by writing the feature and finding it unnecessary.** This
section argued for a "static expansion" — a compiler pass that inlines a
recursive function to a compile-time depth — on the strength of a comment in
`emit_node`'s `Apply` arm reading *"Style 1: a full lichen-function call (inline
its body) — deferred"*. **That comment describes the unreduced case, and the
reduced case already works.** Measured, on unmodified `dev`:

```lichen
--- compute = import "compute.lichen" ---
square = x => x * x
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value square (i + 1)))
}) "gpu"
-- [1, 4, 9, 16]
```

and `square 3` gives `[9, 9, 9, 9]`, and a self-recursive `steps = k => if k == 0
then 0 else steps (k - 1) + 1` applied as `steps 4` gives `4 : Int` — **all
three before any change.** A same-module call in a kernel body is already
inlined, and recursion with a decidable trip count already works.

**The mechanism is the deep pass, not the emitter.** `emit_node`'s own comment
says it: the apply-clone *"unifies* a substituted parameter with the argument, so
a reduced same-module call's parameter reference resolves to this kernel's
parameter — emit a `local.get` for it instead of failing." The body is reduced
before it is lowered, so by the time the emitter walks it there is no call left
to expand. An emitter-side inlining pass is therefore not merely redundant, it
is **unreachable** — no program written for this reached it, and the attempt was
reverted rather than shipped as dead code.

**What the unreduced case actually is, and it is one fact.** A kernel is
compiled from a **template, before any apply**
([compute-jit-low-types](compute-jit-low-types.md)). So a binding the body would
fill in at run time is still empty when the kernel is compiled:

| program | result | why |
|---|---|---|
| `square (i + 1)`, `square` at module level | works | the callee is a module binding, already in the graph |
| `square v`, `v` a body-local alias fed by a read | refused | `v` is an empty cell in the unapplied template |
| a helper *defined in the body*, called there | refused | the callee binding is itself empty |
| a trip count the host computed at run time | refused | nothing decided it before the template was lowered |

**So there is no missing expansion pass. There is a missing *apply*.** The
enabling change is compiling a kernel against an **applied** body — which is
per-call-site specialisation, and [compute-jit-low-types](compute-jit-low-types.md)
already parks that as "a fact about the language, not a mechanism gap" rather
than a codegen one. That is the same fork §7 names, and it is a larger decision
than this roadmap should make.

#### The `loop` operator, and why it must be a builtin rather than a function

`loop f n : T -> T`, applying `f` `n` times. **It is the right surface** — one
expression, asked for rather than syntax the compiler invents — and it is the
answer this section's §7 fork wanted. But **which `loop` is being proposed
decides the whole thing**, and the two are not variants of one feature:

| | as a **library function** | as a **builtin operator** |
|---|---|---|
| decidable `n` | expands (works today) | expands (identical) |
| run-time `n` | **refused** — nothing to expand | **a real loop in the backend IR** |
| code size | O(n) | O(1) |
| the two ceilings of an expanded loop | present | **absent by construction** |
| surface cost | a type annotation, per `P1-33` | none |

**So the builtin is not a refinement — it is the feature**, and a library function
is the fallback it already has. This corrects what an earlier draft of this
section concluded ("recommended: nothing here"): a library `loop` buys ergonomics
at a ceiling, and the thing a GPU kernel needs is the *unbounded* count. The two
ceilings are the argument rather than a side note, and they are
[`P1-40`](code-audit.md) with the measurement: **1024** is a matmul's inner loop
and **2²⁰** is a scan, and both are past the 2000-apply budget and the
512-level emitter budget — which is itself a named refusal rather than a crash,
since `P1-40`'s second half. A dynamic loop removes both by construction — which
is the strongest single reason to make it a builtin rather than a function.

> **Since this section: the surface argument held and the surface changed.** The
> *builtin rather than a library function* conclusion below is kept verbatim — it
> is the whole reason a dynamic loop exists. What is superseded is **which
> builtin**: a `loop f n : T -> T` operator is a strict special case of a general
> recursion-to-loop conversion, and the latter also reaches the natural
> formulations this section does not (`mutual_recursion.lichen`'s two-node cycle
> has no count parameter at all). The decided surface is a **compile-time marker
> on the function** — see [loop-conversion §7](loop-conversion.md) for the three
> reasons, and [§1](loop-conversion.md) for the `Style 1` seam the conversion
> runs at. The IR work this section predicts is unchanged and is staged as
> [§8](loop-conversion.md).

**And it is not free of `P1-33` on the host side.** The operator's own *declaration*
is the annotated one above, and that annotation is P1-33's escape, so a builtin
fixes the surface and leaves the underlying closure-instantiation defect for
whatever else hits it.

**What a builtin costs, and it is a real IR change.** The lowered body is a flat
stack machine with no branch but a two-element `select`; a loop makes it a **CFG**.
Concretely, `lichen-kernel-ir/src/lib.rs` gains a structured body, and every
backend consequence below is real work rather than a spelling change:

- **wasm** — `block` / `loop` / `br_if` and a label stack the emitter does not
  keep today.
- **SPIR-V** — `OpLoopMerge` + `OpBranch`, which means turning a function body into
  basic blocks. `spirv.rs` is hand-written and
  [`compute-graph-jit.md`](compute-graph-jit.md) names its **single-`OpLabel`
  invariant** as a soundness obligation; a loop breaks that invariant outright.
- **The emitter's per-slot type tracking** — the two facts it already records are
  "scalar or `bool`" and "was this a comparison". A loop introduces a merge point
  where two values meet, and neither machinery has an answer for it.
- **The every-ordinal-written invariant** — a `compute.write` inside a loop body
  would run once per iteration. That is compatible (a mapping kernel loops over an
  inner dimension, so each index still writes once) but it is a new interaction,
  and the guard is the existing `CONDITIONAL_WRITE` discipline applied to a new
  place.

**What is genuinely cheap, and it is the reason to do it as one operator:** the
decidable case needs **no IR change at all**. `loop f 3` expands exactly as a
library function does. So the builtin is purely additive — a new capability for
the run-time case, with the static case unchanged — which makes it a far smaller
decision than a loop *syntax* would be, and a smaller one than the roadmap's
original B2 framing implied.

**And it is the first thing that makes axis C reachable at all.** A workgroup
without a loop can only vectorise; every algorithm that needs a workgroup — a
scan, a tiled matmul, a sub-group reduction — needs an *inner* loop over something
the host cannot enumerate. **A dynamic loop is the missing primitive between
"one element per lane" and "a workgroup that can do work".**

**The shape is still not a reduction, and that is unchanged.** `T -> T` repeats a
step. A reduction needs an accumulator, and the one a GPU algorithm wants is a
fold over a **buffer** whose trip count is the buffer's length — so a `reduce`
over a buffer is a **separate operator**, and it is the one that would make `sum`,
`min` and `argmin` expressible. Naming it here rather than folding it into `loop`
is the decision; whether it is a second operator or a shape `loop` can also take
is not, and is cheaper to settle when the loop exists.

#### The ceilings, measured

An expanded loop is `n` copies of the step's body, and it runs into **two
independent limits**, both on the *host* side and neither on the device:

| limit | where | what it does |
|---|---|---|
| **2000 applies** | the VM's own budget | *"this binding never terminates — it applied a function more than 2000 times"*, at 4000 iterations that terminate in 62 ms |
| **512 levels of the emitter's walk** | `emit_node` (`MAX_KERNEL_BODY_DEPTH`) | a **named refusal** — *"a kernel body's expression nests more than 512 levels … Mark the recursion `@loop`"* — at a trip count in the low hundreds |

Cost is linear at about **29 µs of compile time per iteration** — 100 iterations
4.4 ms, 400 iterations 11.6 ms, for a four-element kernel. So an expanded `loop`
is comfortable where the ladder could already write by hand (`dot4`, a 2×2 matmul,
`K ≤ 16`) and **unusable at what a data-parallel kernel wants**.

The second limit **used to be a hard stack overflow** and is now a refusal that
names itself. Measured first-hand on this machine: the overflow was in
`emit_node`, not in the backend, and the walk's depth *is* the trip count — an
unmarked recursion is expanded and every copy nests inside the last one's else
arm, so the graph is a chain (`depth ≈ 18 + 3.1 × trip`, measured on
`crates/lichen-language/examples/recursion.rs`). On a 1 MiB main thread of a
debug build it died at **level ~175**, which is why the same program was
"between 400 and 1000" on a thread with more stack. `emit_node` is now
`#[stacksafe]` — it was the one walk on that path with no annotation in front of
it, and the annotation is what lets a body of 100 trips compile at all — and the
512-level budget is what keeps the walk bounded. Before/after, both backends:

| trip | before | after (`cpu` / `gpu`) |
|---|---|---|
| 100 | `thread 'main' has overflowed its stack` | **answers** 103 — 25 ms / 55 ms |
| 400 | (never reached) | **refused by name** — the 512-level message |

The limit is a **constant** rather than a number derived from the thread's stack,
because a threshold that changes between a debug and a release build is not one
a program can be written against; the stack is handled on the other side of the
same change by the annotation. What a dynamic loop removes is both rows at once:
a loop reads its trip count from a register, so neither the apply budget nor the
walk's depth depends on it. See [code-audit `P1-40`](code-audit.md).

The first is [`P1-40`](code-audit.md) and its message is wrong in a way worth
fixing on its own: it reports a **terminating** loop as non-terminating, which
sends the reader to look for a fault that is not there. The second was the same
defect class one level out — a depth limit that crashes is the same thing as the
`NodeId` message — and is now the named refusal where the first is still a named
verdict.

### 4.2 Axis C: lane width, and the consumer `Perspective` has been waiting for

`Perspective` is live and fully checked — `#p` reaches the IR, it is in the
persist codec, it has an acceptance table — and **nothing in the runtime reads
it**. [attributes](attributes.md) says what it was built for: "the certificate
for which expressions are uniform across an aligned lane group", and what it
would buy: "hoisting a uniform (`#0`) subexpression out of a lane-varying body".

**The change**: the kernel emitter reads the perspective slot, and a value
declared uniform over an `n`-group is emitted once for the group rather than
once per lane — and, where the target has a vector width, emitted as one.

- **It is a compiler plugin**, and it is *already codesigned*. `Perspective` is
  the reference compiler plugin; the grammar production, AST fields, IR form
  and persist discriminator all exist. This axis adds no new codesign — which
  makes it the **cheapest capability in this roadmap by a wide margin.**
- **The machinery is half-built.** The emitter's stack already records, per
  slot, whether a value is a scalar or a `bool`, because "was this a
  comparison" is knowledge only the walk has
  ([lichen-compute-gpu](lichen-compute-gpu.md#the-two-type-facts-the-ir-does-not-carry)).
  "Is this uniform" is the same kind of fact about the same stack.
- **What it does not buy**: it makes a body that is already correct run wider.
  It cannot express a scan, a sort, a reduction or a scatter. **Axis C is a
  constant factor, not a capability** — it multiplies the width of what a
  dispatch already computes, and a kernel that is one element per invocation
  stays one element per invocation for everything it cannot yet say. It is
  first because it is cheap, not because it is the prize.
- **Recommended, first.**

**The mismatch this exposes, which is a real finding and not a detail.** The
`Perspective` lattice is **divisibility** — `attributes.md` is explicit that
`2 ⊑ 4` and that `4` and `6` are incomparable. A hardware group is a small fixed
width: a sub-group is 8, 16 or 32 depending on the target, and this backend's
workgroup is 64. So the language can express a uniformity width that **no
device has**, and the emitter must either refuse it, round it down, or round it
to a power of two. Rounding silently would be the worst of the three: a body
proved uniform over 6 lanes is not automatically uniform over 4, and the program
would not know which answer it got. This is §7's first open question.

**And the part C does not cover.** A lane group that cannot talk to itself buys
throughput, not capability. Tiled matrix multiply, a scan and a sub-group
reduction all need **shared memory and a barrier** — lanes seeing each other's
intermediate results. That is a *third* thing, distinct from both the width and
the unroll, and it is the largest single item in this roadmap:

- **The surface is a native plugin** (a new operator, a new buffer kind bound at
  a group scope, a barrier op) — but the *IR* must grow, because a fragment has
  no notion of local memory or of a synchronisation point. So it is the first
  item here that is not confined to `lichen-compute`.
- **What it buys**: it is the axis that makes scan, sort, tiled matmul and
  sub-group reduction expressible at all. Every one of those is out of reach
  without it, and reachable with it.
- **What it does not buy**: anything on its own. A workgroup that cannot talk to
  itself is B1 in costume.
- **Recommended, and the one to argue about** — because it is the most expensive
  item and the one whose payoff depends on algorithms the language still cannot
  write, for want of E.

### 4.3 Axis A: a buffer from a program value

**The change**: a `compute.buffer` that turns an array into a `Buffer`, so a
kernel has something to read.

- **A native plugin.** One `ComputeOperator` leaf and one wrapper function —
  the shape `compute.parallel` and `compute.read` already establish.
- **It is the one place where the fresh-cell rule can be dropped, and that is the
  interesting part.** `plrun`'s result type is deliberately a fresh cell,
  because the arity that decides its shape is not knowable at check time. A
  buffer *is* knowable: the element type is the array's element type. So this
  is the one buffer-producing operation that can be **statically precise**, and
  it is the first thing a lichen program would get a decided `Buffer` type out
  of.
- **Cost**: the smallest on this list. **It also removes a hazard the ladder
  found**: a program that reaches for a plain array where a buffer belongs used
  to compute nothing and still print a type. That is now a refusal, but the
  refusal is only helpful if the thing the author wanted *exists*.
- **What it does not buy**: a view. A `Buffer` is a dense `[i64]`, so this is a
  *copy* of the array's elements, and a strided or batched view is a different
  type with a different lifetime story. §5.

### 4.4 Axis D: slot access, and the invariant it breaks

A *computed* index is already fine, on both sides: the ladder's gather reads at
`i % 4` and its stencil reads at a clamped neighbour, and
[compute-buffer-wrapper](compute-buffer-wrapper.md)
allows arbitrary element indices on the write too. What is missing is
**contention** — two lanes writing the same slot — which today means
last-writer-wins with no diagnostic, on both backends. The ladder's histogram of
64 elements into 3 buckets answers `(1, 1, 1)`.

**The cost here is semantic, and it is the reason this axis is last.**
[compute-buffer-wrapper](compute-buffer-wrapper.md)
states the invariant as a theorem: a run is "bit-identical to the sequential
loop's, for every `count` and whatever the worker count... There is no
reduction, no accumulation and no order to depend on." An atomic accumulate makes
the answer **order-dependent**, and the partition — which is a function of the
count and the worker count and nothing else — becomes what decides the winner.
The note already says this is reachable and already says what it costs; the
ladder confirms it is reachable from ordinary-looking code.

So axis D is not a feature request. It is a decision to **give up a property the
primitive currently has**, and the interesting question is not *whether* but
*how it is given up deliberately*.

**The language-shaped answer, which is the one worth building**: make
contention **declared rather than raced**. The program says which slots a body
may share — a second lattice, or a `#` requirement over a buffer — and then:

- a body declared **exclusive** is checked, and the backend may use a plain
  store;
- a body declared **shared** is permitted an atomic, and only there;
- a body that is **silent** keeps the current invariant, because nothing
  declared otherwise.

That turns today's silent race into a checkable distinction, and it means the
determinism invariant survives for every program that did not ask to lose it.
**It is also the one axis that `Perspective` is the natural home for**, which
is a second reason C precedes D: the lattice that would carry "these two lanes
contend" is the one that already exists and is unread.

- **Native plugin** for the atomic itself; a **compiler plugin** if the
  declaration is a new attribute or a new lattice order.
- **What it does not buy**: determinism. It buys a *refusal* where there was a
  race, and a sound atomic where the program asked for one.

### 4.5 A dynamic graph, which is not one of the four axes

The four axes are about what a **kernel body** may say. This is about the
**graph** — how many dispatches there are — and it is the proposal that most of
the others are downstream of, because it is the only one that removes the
"compile-time constant" limit that keeps recurring in this document.

**The shape of the idea.** The graph is built by recording an evaluation
([compute-graph-jit](compute-graph-jit.md)), and today the recording is of a
*fixed* number of dispatches: the node list is the topological order, and the
shape is decided when `compute.graph` runs. A **dynamic** graph decides the node
count at *run* time, and the mechanism is a recursion expanded over a grid — the
recursive function is the local pattern, instantiated once per grid point. A
`loop f n` at the graph level is the degenerate case: `n` nodes in one submission
rather than `n` submits.

**Why this is more than the loop of §4.1, and where it is not.** For a **fixed**
count, the kernel loop is strictly better and much cheaper: `loop step 1000` as a
shader loop is *one* dispatch, against 1000 graph nodes. §4.1 measured that a
16-link fused chain is worth 65–81% over unfused at counts up to 65 536 and about
10% at a million — so the *saving* from fusing 1000 links is real but shrinks
exactly where the algorithm stops being interesting. **The dynamic graph earns
its keep only where a loop cannot go at all**, and that is the honest case for
it:

| shape | loop | dynamic graph |
|---|---|---|
| `loop step 1000` (fixed count) | **one dispatch** | 1000 nodes |
| iterate until `err <= tol` | impossible — a trip count must bound the loop | **the shape is the answer** |
| one dispatch per nonzero of a sparse input | impossible — the count is data | **nodes emitted as found** |
| a recurrence whose step count is a `collect`ed length | needs the length first | the length is a value on an edge |

**So the first user is not the fixed-count case**, and that is what keeps this
from being §4.1 restated.

**The capability it requires already exists on the queue, refused by name.**
`Policy::Batch` in `lichen-graph-ir` is *"refused by name"* because
[compute-graph-jit](compute-graph-jit.md) states the reason exactly: *"a backend
can only fuse if it can be handed several dispatches to be put in one command
buffer, and the contract has no way to ask for that — `submit` records one run
and hands it over."* **A growing graph is that missing capability.** The design
here is therefore not a new mechanism but a **contract change**:
`ParallelBackend` gains something like *"hand me this segment, put it in one
submission, tell me when it is done"* — one method, and the two seams that must
be sound together (the segment boundary, and the demand point) are already named
in that note. That is a far smaller and better-specified piece of work than it
looks from the outside.

**The cost, and it is structural: "no dispatch while building" has to go.** That
property is currently a boast of the design — *"the graph is built by recording an
evaluation that has already happened"*, with every `ParLaunch` intercepted and a
placeholder substituted, so nothing runs. A dynamic graph must **let dispatches
actually run** in order to learn the shape, which makes the recording *interleaved
with execution*. The boundary — which dispatches run, and at what point the
recording stops and the running starts — is a new design decision and not an
implementation detail. There is also a soundness obligation already on file: *"if
a closure form ever appears that can capture a post-launch value, the scheduler
becomes unsound, silently."* A build that runs is exactly where that becomes
reachable.

**The "local pattern" needs a name in the IR.** If the grid is a graph, then the
pattern is a **subgraph template** — nodes and edges — instantiated at a
coordinate, with each instantiation's buffers and counts substituted. Today
`lichen-graph-ir`'s node is one kind (`Node::Kernel`), and a composite node
holding a sub-graph and a coordinate is a new concept, not a new leaf.

**And the grid framing reopens an extent question §5 closed.** A 2-D grid is a
stencil, and §5 declines a multi-dimensional extent on the grounds that `i / w % h`
over a flat index already gives what the ladder's 2×2 matmul needed. **A grid
framework either subsumes that decision or reinvents it**, and it has to answer it
explicitly, because a stencil is the case that most wants it. Worth noting the
analogy's limit too: **polyhedral compilation is the thing that makes a grid
*static but parameterised*** — symbolic index sets, symbolic trip counts, O(1)
code. "Polyhedral without the parallel optimization" therefore lands on §4.1's
*loop*, not on a run-time grid. The dynamic graph is the part polyhedral
compilation deliberately leaves out, which is a point in the proposal's favour and
a point against the analogy.

- **Where it sits in the order**: after B, because a dynamic graph is a way to
  express what a loop cannot, not a way to do loops.
- **What it needs first**: the `Batch` capability on the backend contract. That is
  a small, self-contained change and it is the honest place to start.

## 5. Explicitly not proposed

Named so they are decisions rather than omissions.

- **A loop *syntax*.** §4.1 argues for a `loop` **operator** instead, and the
  difference is the whole design: an operator is one expression the compiler
  either expands or lowers to a real loop, so the decidable and run-time cases are
  the *same* source program. A new grammar form would be a second thing to learn
  for the cases the operator already covers. This was the first answer here and it
  was the wrong one.
- **A multi-dimensional extent, as a *dispatch* feature.** §4.5 reopens this,
  because a stencil grid wants it: a 2-D extent is what a grid is, and declining
  it while proposing a grid is incoherent. What §4.5 does *not* support is the
  weaker claim it used to rest on — that a 2-D extent is "ergonomics, not
  capability", which was inferred from a 2×2 matmul that `i / w % h` already
  expresses. A stencil is the case that refutes that, so the item moves from
  *declined* to *deferred to §4.5*.
- **Buffer views and strides.** A `Buffer` is a dense `[i64]`; a view is a
  different type with its own lifetime story, and nothing in the ladder wanted
  one.
- **Multiple queues / stream overlap.** `Async` already removes the waits
  ([compute-graph-jit](compute-graph-jit.md)) and the measurement there says
  its ceiling is one submission's device time per node.
- **Releasing a resident buffer from the language.** The value set has no
  per-value destructor, so this is a real gap — but it is a *resource* change to
  the value vocabulary, not an algorithm capability, and it is better argued in
  its own note than smuggled in here.

## 6. What deferring float costs, stated plainly

There is no float. `1.5` does not lex; `LowValue`'s only number is `USize`;
`KernelBin` is integer-only; the fragment ABI is `i64` in and `i64` out. It is
designed in [floating-point](floating-point.md), whose status on `dev` is still
**"proposed. Nothing here is implemented"** — and re-read at merge time, with
**ten float worktrees in flight** (`float-lowlevel`, `float-marker`,
`float-lexer`, `float-literal`, `float-printer`, `float-shape-tag`,
`float-wiring`, `float-compute`, `float-integrate`), that is no longer "not
started". The note's *decisions* are taken where they were open before: `f32`
width, the relation to `Int`, and where a conversion is allowed to happen, plus
the lexing (leading digit required, overflow to infinity, no scientific
notation), the printer's round-trip (with the `1.0` → `1` trap recorded against
a real consumer, and `NaN`/`±inf` failing its document format silently), and
`Eq` coming off the token payload.

**So "deferred" here means *not this roadmap's*, not *not happening*.** Nothing in
§4 conflicts with it, and the two do not compete for the same sites — a float
reaches a kernel through the `i64` fragment ABI, which is
[compute-jit-low-types](compute-jit-low-types.md)'s business and larger than a
kind marker.

**The uncomfortable version, which this roadmap should not hide:** matrix
multiply, convolution, FFT, a physics step, a gradient — every algorithm people
picture when they say "GPU algorithm" is a float algorithm, and **not one of
them becomes reachable through any axis in §4**. The integer algorithms the
ladder could write are fill, axpy on integers, a hand-unrolled 2×2 integer
matmul and a tree reduction.

So float is not on the critical path *of this roadmap* — A through D are worth
doing without it, and the backend's advantage over an interpreter does not
depend on it. Float is the critical path *for the subject matter*. Both are
true, and the second is the one that decides when to start it — and with ten
worktrees already on it, that is now somebody else's sequencing decision rather
than this document's.

## 7. What this does not decide

Each is a genuine fork, and each is cheaper to settle before the IR moves than
after.

**Lane-group width, against a divisibility lattice.** `Perspective` admits `6`;
no device has 6 lanes. Refuse, round down, or round to a power of two — and
rounding silently is the one answer that makes a proved-uniformity claim false
without the program being able to tell. This is the first thing axis C needs.

**Where a workgroup's shared memory is a value.** A local buffer is a *third*
buffer kind beside `Buffer` and `DeviceBuffer`, with a lifetime shorter than
either. It could be a new `ComputeValue`, or a scope on the existing one. The
first is honest and grows the vocabulary; the second is smaller and makes
"which scope" a question every buffer read has to answer.

**Is a workgroup count a program value?** `n` is one, and a dispatch's extent
is a value on an edge. A workgroup count would be a second — and the two are
**not** interchangeable, because the tail-lane obligation makes `n` need not be a
multiple of the workgroup size while the workgroup count must be. Which of them
the launch allocates from is a decision with a wrong answer available.

**Is a shared slot a lattice, an operator, or a buffer property?** §4.4 argues
for declaring contention. Whether that is a second `Perspective` order, a new
`#` on the buffer, or a new `atomic` operator whose presence *is* the
declaration — three answers with different costs, and the second of them is a
codesign.

**Does the language ever compile a kernel against a call site? — This is the
fork, and [§4.1](#41-axis-b-already-in-the-language-and-what-it-does-not-reach)
is where it was found rather than first seen.** Same-module calls and recursion
are already inlined by the deep pass, so the question was never the unroll's
syntax; it is whether a kernel is ever compiled against an *applied* body, which
is what a run-time trip count and a body-local binding both need.
[compute-jit-low-types](compute-jit-low-types.md) calls per-call-site
specialisation "a fact about the language, not a mechanism gap", and that is
the honest classification: it is not a codegen task, so it does not belong in a
roadmap about what a kernel can express. It decides two of the four axes' limits
at once, which is why it is worth naming here even though it is not this
document's to decide.

**The next paragraph names the answer to the loop half, and the other half is
still open.** A run-time trip count does *not* need an applied body — it needs a
body that is left unreduced at exactly one call, and
[loop-conversion](loop-conversion.md) is that: it converts a marked recursive
function into a loop nest at the `Style 1` seam, so the count is a register the
emitter reads rather than a call it must expand. **A body-local binding still
needs the applied body**, so §4.5's narrowing and this are different answers to a
question this document had bundled into one.

**§4.5 may dissolve this fork rather than answer it**, and that is the one place
where the two interact. A kernel loop needs a run-time count, so it needs the
count to be a value the emitter can read — which is a smaller requirement than
compiling the whole body against a call site. A **dynamic graph** needs no
applied body at all, because the shape is decided by *running* the graph's head.
So if §4.5 is built, the specialisation question narrows to "which constructs
inside a single kernel body still need an applied body", rather than "does the
language ever specialise a kernel". Worth knowing before answering it, and worth
not assuming the loop and the graph need the same fix.

## 8. The defects, and which are fixed

The ladder found three pre-existing silent-wrong-answer defects. **Two are
fixed** on `feature/gpu-algorithms`, each with a test that fails without the fix.
**The third is not**, and it is the one on the critical path.

**The graph registry froze a backend — fixed.** `graph_digest` deliberately
omits the backend — it is a property of the *run*, not of what the graph
computes — but `intern` stored the backend beside the graph. The first build of
a shape in a process therefore decided the backend for every later build of
that shape, and a `"gpu"` program was handed a `"cpu"` entry and refused for a
backend it never named. The registries are process-global, so this crossed
program boundaries. The backend now rides on `ComputeValue::Graph`, exactly as
it rides on `ComputeValue::ParKernel`, which makes the digest's reasoning true
instead of worked around.

**A decided non-buffer was answered `parameterized` — fixed.**
`compute.read`, `compute.collect` and a launch's `cfg(1)` all took the lazy
fallback for a value that was not a buffer, so `compute.read ((compute.Read _)(.from data, .at i))` on a
plain array ran to completion, printed `parameterized`, and still showed the
type `array<?a, ?b>`. The fallback is right for an *undecided* value and wrong
for a decided one; all three refuse by name now.

Neither is a gap in the primitive, but the second is the symptom of axis A, and
fixing it without axis A leaves a refusal where a user wants a feature.

**An unresolvable node was reported as a `NodeId` — fixed, and the cause is
wider than the one case that was found.** A kernel body that reached a node with
neither a value nor an operation answered *"kernel body hits a node with neither
value nor operation (node=NodeId(394v1))"* — a compiler-internal number, and the
only refusal in the compute surface that named nothing.

The message now names the fact all of these share — *a kernel is compiled from a
template before any apply, so a binding the body would fill in at run time is
still empty* — and lists the three shapes that reach it, **without claiming
which one this is**, because a refusal that names the wrong cause sends the
reader to the wrong place. The three, all confirmed on both backends:

| program | result |
|---|---|
| a `compute.call` inside a **parallel** body (the identical call in a *scalar* body works) | refused |
| a module-level helper called with a **body-local alias** fed by a read | refused |
| a helper **defined in the body** and called there | refused |

The first is the one on the critical path, and the reason is unchanged by the
better message: **only `parallel` names a backend, so a device cross-kernel call
is reachable *only* from a parallel body**, and that path fails in the compiler
before any backend sees it. So `SpirvRefusal::CrossKernelCall`, which
[lichen-compute-gpu](lichen-compute-gpu.md#not-yet) documents as the reason
cross-kernel calls are out of scope there, **is a refusal no lichen program can
currently provoke**.

**What is left is not the message, it is the three cases.** All three are the
same missing *apply* §4.1 is about, and none is fixed by naming them.
