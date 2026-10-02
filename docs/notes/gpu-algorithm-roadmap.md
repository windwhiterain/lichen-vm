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
> numbers this re-reads), [compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md)
> (the primitive, and the determinism invariant §4.4 would break),
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
| **B. body** | can a body loop, or reach across indices? | no; straight-line, compile-time sizes only |
| **C. width** | can one invocation own several elements? can invocations see each other? | no; one element per lane, no shared memory, no barrier |
| **D. slot** | can a lane read-modify-write a slot another lane may touch? | no, and it is silently wrong |
| **E. value** | is there anything but an integer? | no — deferred, §6 |

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

B splits into two features that are not the same feature, and §4.1 says so.

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

## 4. The four axes

Each is classified against [plugin-taxonomy](plugin-taxonomy.md)'s one rule —
does the language layer have to change to accommodate it — because that rule is
what decides whether a package manager can pull the feature or the compiler has
to be written with it.

### 4.1 Axis B: **already in the language**, and what it does not reach

**Correction, made by writing the feature and finding it unnecessary.** This
section argued for a "static expansion" — a compiler pass that inlines a
recursive function to a compile-time depth — on the strength of a comment in
`emit_node`'s `Apply` arm reading *"Style 1: a full lichen-function call (inline
its body) — deferred"*. **That comment describes the unreduced case, and the
reduced case already works.** Measured, on unmodified `dev`:

```lichen
@{ compute = import "compute.lichen" @}
square = x => x * x
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, square (i + 1)]
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

**Recommended: nothing here.** Not a "small first win" — the feature exists. What
the work *did* leave is two refusals that were reporting a compiler-internal
`NodeId` and nothing else; both are named now, and §8 has the third.

#### A `loop` combinator, and the two ceilings it runs into

The natural next step is a `loop` operator — `loop f n : T -> T`, applying `f`
`n` times. **It is the right shape and it needs no compiler at all**, which is
the strongest argument for it: as a lichen function it is one line,

```lichen
loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)
```

using the recursion the interpreter already has. No IR form, no backend change,
no persist discriminator — a **native plugin** at worst, and a library function
at best. It is also the answer the roadmap's original §7 wanted: a loop that is
*asked for* rather than a syntax the compiler invents.

**The signature as proposed does not check, and the reason is worth stating
precisely**, because it is a type-system fact and not a codegen one. A recursive
binding cannot receive a function argument through a partial application: `loop
f` is a *function value*, and unifying it against the binding's own type cell
compares two function **kinds** — which lichen refuses, because a compound type
is typed by its kind and there is no subtyping. Measured:

| shape | result |
|---|---|
| `sum_to n x = … sum_to (n - 1) (x + 1)` — two-stage curried | `expected Int, found Int` |
| `loop f s = … loop f (… , f …)` — the step threaded through | `expected ?c -> Int, found ?c -> Int` |
| `sum_to s = … sum_to (s(0) - 1, s(1) + 1)` — **one tuple argument** | **`[3, 4, 5, 6]`** |

So the shape that works is a **specialised** loop: the step is baked into the
binding and the recursive call is a single application of one tuple. That is
exactly the `dot4`/`mat2` shape the ladder already wrote by hand — and it is one
definition per step, which is what the operator was meant to remove.

**The error messages are themselves a defect, and worth fixing whatever is
decided here.** `expected Int, found Int` renders two *incompatible* types
identically, so the reader is told nothing at all. Every other refusal in the
compute surface names its own cause; these two name a type the reader cannot
distinguish from the one it was expected to be. That is the same class as the
`NodeId` refusal §8 fixed.

**And the ceiling decides the usefulness, and it is small.** An unrolled loop is
`n` copies of the step's body, and it runs into **two independent limits**:

| limit | where | what it does |
|---|---|---|
| **2000 applies** | the VM's own budget | *"this binding never terminates — it applied a function more than 2000 times"*, at 4000 iterations that terminate in 62 ms |
| **stack, between 400 and 1000** | the emitter's walk | a **hard overflow**, not a diagnostic |

Cost is linear at about **29 µs of compile time per iteration** (400 iterations:
11.6 ms for a four-element kernel). So a statically expanded `loop` is
comfortable at the trip counts the ladder could already write by hand —
`dot4`, a 2×2 matmul, `K ≤ 16` — and **unusable at the ones a data-parallel
kernel actually wants**: 1024 is a matmul's inner loop and 2²⁰ is a scan.

**So the answer to "is it enough" is no, and the reason is sharper than "a device
loop would be better".** The two ceilings are the real content: one is a budget
whose *message conflates a long loop with a non-terminating one*, and the other
is a depth that should be a named refusal rather than a crash. Both are small,
bounded pieces of work, and both are prerequisites rather than the feature.

**What is not fixed by any of this is the shape.** `T -> T` covers *repeating a
step*, not *reducing a buffer*. A reduction needs an accumulator —
`f : S -> T -> S`, `loop f n : S -> [T] -> S` — and the reduction a GPU algorithm
wants is a fold over a **buffer**, whose trip count is the buffer's length: a
run-time value, which is the one thing already refused. **A loop that cannot
reduce is not the loop most GPU algorithms are missing.**

**What survives of the original B1/B2 split.** B1 is answered: recursion and
composition are both expressible, within the limit above. B2 — **a loop that runs
on the device, with a runtime trip count** — is unchanged and still not
recommended. It is a new IR form; the wasm emitter has exactly one branch
(`Select`, a two-element conditional with no jump), so a device loop has no wasm
counterpart and the backend contract would have to say what a target that cannot
express one does. Nothing in the ladder needs it, and it is the one case
§7's specialisation fork would *not* cover either, which is what keeps it open.
Listed in §5 as explicitly not proposed.
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
[compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md)
allows arbitrary element indices on the write too. What is missing is
**contention** — two lanes writing the same slot — which today means
last-writer-wins with no diagnostic, on both backends. The ladder's histogram of
64 elements into 3 buckets answers `(1, 1, 1)`.

**The cost here is semantic, and it is the reason this axis is last.**
[compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md)
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

## 5. Explicitly not proposed

Named so they are decisions rather than omissions.

- **A device-side loop** (§4.1, B2) — no wasm counterpart, no ladder evidence.
- **A multi-dimensional extent.** A 2-D dispatch is `i / w % h` over a flat
  index, and the ladder's 2×2 matmul already does exactly that. A real 2-D
  extent is ergonomics, not capability, and it would make the tail-lane
  obligation two-dimensional for no gain the flat form does not already give.
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
designed in [floating-point](floating-point.md), whose phase 0 is a kind marker,
a literal, a printer and a codec, and whose kernel widening is explicitly a
later, larger decision.

**The uncomfortable version, which this roadmap should not hide:** matrix
multiply, convolution, FFT, a physics step, a gradient — every algorithm people
picture when they say "GPU algorithm" is a float algorithm, and **not one of
them becomes reachable through any axis in §4**. The integer algorithms the
ladder could write are fill, axpy on integers, a hand-unrolled 2×2 integer
matmul and a tree reduction.

So float is not on the critical path *of this roadmap* — A through D are worth
doing without it, and the backend's advantage over an interpreter does not
depend on it. Float is the critical path *for the subject matter*. Both are
true, and the second is the one that decides when to start it.

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
fallback for a value that was not a buffer, so `compute.read [data, i]` on a
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
