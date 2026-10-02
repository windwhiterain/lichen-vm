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

### 4.1 Axis B, in two parts: unrolling, and a loop that runs

These are commonly the same request. They are not the same feature, and the
difference is the whole of the value.

**B1 — the compiler unrolls.** A loop in the source whose trip count is a
compile-time constant resolves to straight-line code. The IR needs nothing: it
already has no loops, and unrolling produces no instruction a kernel does not
already have. The wasm and SPIR-V emitters are unchanged.

- **It is a native plugin.** No grammar production, no AST node, no IR form, no
  persist discriminator — unless the *surface* is a new form, which is the open
  question in §7.
- **What it buys**: the ladder's `dot4` and `mat2` become loops rather than
  hand-written unrollings, and — more usefully — a helper becomes usable at
  several sizes. A shared `dot k` for a compile-time `k` is a real library item;
  a shared `dot4` is four lines.
- **What it does not buy**: a sum over a *runtime* `N`. Unrolling is bounded by
  the code size the author accepts, and a reduction's cost is `O(N)` either way.
  **A ladder of unrolled reductions is still a ladder.** B1 makes the small
  cross-index algorithms writable; it does not make the large ones reachable.
- **Recommended, first, cheap** — and the honest framing is ergonomics, not
  capability.

**B2 — a loop that runs on the device.** A real loop in the shader, with a
runtime trip count. This is a new IR form, and it is not confined to the GPU:
the wasm emitter has exactly one branch (`Select`, a two-element conditional
with no jump), so a device loop has no wasm counterpart and the backend
contract would have to say what a target that cannot express one does.

- **Not recommended yet.** Nothing in the ladder needs it, a uniform-trip-count
  loop is a B1 special case, and a non-uniform one is a *workgroup* problem
  (C) wearing a loop's clothes.
- It is listed in §5 as explicitly not proposed.

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

**What the unroll's surface is.** A compile-time-constant loop is a native
plugin's work; whether the *syntax* for it is a new form or a re-use of an
existing one decides whether a package manager can pull the feature.

## 8. The two defects, fixed on the way

The ladder found two pre-existing silent-wrong-answer defects. Both are fixed on
`feature/gpu-algorithms`, each with a test that fails without the fix.

**The graph registry froze a backend.** `graph_digest` deliberately omits the
backend — it is a property of the *run*, not of what the graph computes — but
`intern` stored the backend beside the graph. The first build of a shape in a
process therefore decided the backend for every later build of that shape, and a
`"gpu"` program was handed a `"cpu"` entry and refused for a backend it never
named. The registries are process-global, so this crossed program boundaries.
The backend now rides on `ComputeValue::Graph`, exactly as it rides on
`ComputeValue::ParKernel`, which makes the digest's reasoning true instead of
worked around.

**A decided non-buffer was answered `parameterized`.** `compute.read`,
`compute.collect` and a launch's `cfg(1)` all took the lazy fallback for a value
that was not a buffer, so `compute.read [data, i]` on a plain array ran to
completion, printed `parameterized`, and still showed the type
`array<?a, ?b>`. The fallback is right for an *undecided* value and wrong for a
decided one; all three refuse by name now.

Neither is a gap in the primitive, but the second is the symptom of axis A, and
fixing it without axis A leaves a refusal where a user wants a feature.
