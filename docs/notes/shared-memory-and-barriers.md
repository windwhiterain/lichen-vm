# Shared memory, and where a barrier comes from

> Status: **proposal.** A design for the one capability the
> [roadmap](gpu-algorithm-roadmap.md) calls the largest single item in axis C —
> lanes that can see each other — and for the analysis that makes it sound.
> Nothing here is implemented. The three parts are one design and are not
> separable: a **write whose effect is a value**, a **DAG** built from it, and a
> **`DP`** judgement that decides where the barrier goes.
>
> Two of the three answers already exist and are named here rather than
> redesigned. The **`UP`** half of a perspective is live and checked
> ([attributes](attributes.md#perspective), `crates/lichen-perspective`); the
> **`DP`** half is not implemented, and the paper this project's attribute
> extension is inspired by supplies it
> ([Typed Perspectives](../reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf),
> read in Chinese in
> [the walkthrough](../reference/类型系统验证底层%20GPU%20语言：Perspective.md)).
>
> Points at: `crates/lichen-compute/src/compute.lichen` (the algebra),
> `crates/lichen-compute/src/compute.rs` (`Positions`, `write_value_node`,
> `parallel_output_nodes`), `crates/lichen-compute/src/compute/body.rs`
> (`Lower::buffer_write`), `crates/lichen-kernel-ir/src/lib.rs`
> (`KernelInstr::BufferWriteCall`), `crates/lichen-kernel-ir/src/body.rs`
> (the SSA body), `crates/lichen-compute-gpu/src/spirv.rs` (`LOCAL_SIZE_X`).
>
> Companions: [compute-buffer-wrapper](compute-buffer-wrapper.md) (the ABI this
> extends, and the determinism theorem it must not lose),
> [compute-kernel-struct](compute-kernel-struct.md) (the struct-wrapper model
> shared memory joins), [loop-conversion](loop-conversion.md) (the loop a
> shared-memory algorithm needs, and the `CONDITIONAL_WRITE` discipline a write
> inside one breaks), [lichen-compute-gpu](lichen-compute-gpu.md) (the tail-lane
> obligation, and why a write in a loop is refused today),
> [gpu-algorithms-ladder](gpu-algorithms-ladder.md) (the measurement that a
> contended write is silently wrong today — this design's sharpest evidence).

## 1. The claim, in one paragraph

lichen's kernel bodies are already pure, and the one impure thing in them is
`compute.write`. It is impure in the way that matters: **it returns nothing, so
it leaves no trace in the value graph.** A pure body is therefore a DAG over
*values*, and that DAG does not contain the one edge a barrier needs — the edge
from a write to the read that must observe it. So the work is not "make the body
pure"; it is **"give the effect a value"**, after which the barrier question
becomes a walk over a DAG. But a walk over the DAG answers only *which pairs to
consider*: whether a pair actually needs a barrier is decided by whether the two
lanes' addresses can overlap, and that is `DP` — the half of a typed perspective
this project never built.

## 2. What the write is today

Three facts, each of which the design keeps.

- **The write's "result" is already a value, and it is the wrong value.** The
  prelude's `write` is `(x : (Write _)) => $write(x.to, x.at, x.value)`
  (`compute.lichen`), so the node's value is a computed write, and the IR reads
  the *operand* for what was written: `write_value_node` takes
  `operand_items(…)[2]` — the `.value` field (`compute.rs:2282`). What the write
  does **not** produce is anything a later read can consume.
- **Every ordinal is a compile-time constant.** `write`'s first operand is the
  `.to` **buffer**, not a length — `body.rs`'s `buffer_write` reads item 1 as the
  index and item 2 as the value and ignores item 0 entirely, taking the ordinal
  from `tally.writes` (`body.rs:1533`, `KernelInstr::BufferWriteCall`'s doc
  names it "the write's *emission ordinal*"). So the write set is *already*
  available at compile time; it is simply not exposed.
- **The write set is a private tally.** `Positions { writes, reads,
  write_classes, read_classes, element_class }` (`compute.rs:2624`) counts them
  for the fragment's `outputs`/`output_classes`, and the return point recovers
  which nodes they are by walking the codomain (`parallel_output_nodes`,
  `compute.rs:2341`). Nothing outside the lowering can read it.

The body itself is SSA with one instruction per `ValueId`
([compute-kernel-struct](compute-kernel-struct.md), `lichen-kernel-ir/body.rs`),
so "a DAG" is not a rewrite the language needs — it is the shape the body is
already in.

## 3. The trap: a pure DAG is a value graph, and a barrier needs an order

This is the point worth being exact about, because "the body is pure, so it is a
DAG, so barrier analysis is easy" is *nearly* true and fails on one edge.

A pure node with no result — a `swrite` spelled `$swrite(buf, i, v)` returning
nothing — has **no consumers and no producer**. A later `sread buf i` shares only
the `buf` operand with it, and `buf` is a value: two reads of one value are not
ordered by the value graph, they are *unordered* by it. So the DAG's verdict on
the pair `(swrite buf i v, sread buf i)` is "independent", which is exactly
wrong.

The ordinary case shows how wrong. Today the plane is a **buffer map**: one lane
per index, each writing its own slot, and the design's whole determinism theorem
is that there is *no* order to depend on
([compute-buffer-wrapper](compute-buffer-wrapper.md): "There is no reduction, no
accumulation and no order to depend on"). A value DAG says that correctly. It is
only shared memory that makes order matter, and it makes it matter **across
lanes** — so the edge we need is not a value edge at all. It has to be created.

**Conclusion, and it is the whole of part one: a write must return something a
read consumes.** With that spine in place the pair is in the graph and the graph
can be walked.

### Why the spine should be a zero-width witness and not a new buffer

The proposal this note records first arrived as "each write returns a new
buffer, and the return point collects them". The spine is right; making it a
*buffer* buys nothing the spine does not already have, and costs three things
that a witness does not:

- **It writes a false dependency into the graph.** Two adjacent values of a
  scatter/gather are not ordered, and a buffer-valued write says they are. The
  false edge is not a correctness bug — it is a claim the optimizer would later
  have to be taught to disbelieve.
- **It removes the buffer from first-class position.** A closure that captures a
  buffer would have to name *which version* it captured; today it captures a
  handle whose writes are not versions.
- **It collides with the buffer's lifetime model.** A buffer's payload is arena
  data owned by its block and there is **no per-value destructor**
  (`ValueExt`/`ComputeValue::Buffer`, `compute.rs:775`) — one new buffer value
  per write is a new allocation per write.

A zero-width state value has none of the three, and gives the same spine. It is
a value in exactly the sense lichen already uses the word — it has a type, it can
be a struct field, it participates in the DAG — and it carries nothing: not an
element, not an address. What it *means* is "the effects up to here have
happened", which is a fact a later read can require.

## 4. What decides a barrier, and it is three facts

With the spine, the analysis is a scan. For every pair `(write j, read i)` in
program order, a barrier is needed iff **all three** hold:

1. **The addresses may overlap** — `slots(j) ∩ slots(i) ≠ ∅`.
2. **The writer and the reader are not provably the same lane.**
3. **No barrier already sits between them.**

Fact 2 is what makes the ordinary `swrite buf i v; sread buf i` cheap: both `i`s
are the same SSA value, hence the same lane, hence no barrier — one lane always
observes its own earlier write in program order. Fact 3 is a latch. Both are
free; the cost is fact 1.

**Fact 1 is not string equality, because an address is usually an interval.**
`swrite ws (i % 64) v` writes the *slot* `i % 64`, which is one concrete slot per
lane but a range `[0, 63]` at compile time; `sread ws ((i + 1) % 64)` reads a
range too. The question is therefore:

> do there exist two lanes `a ≠ b` with `index_of(a, j) == index_of(b, i)`?

which is a `may-alias` over two intervals. It is decidable and it is local —
the index expressions are affine over `%`, `/`, `+` and the lane count is the
fragment's `LOCAL_SIZE_X` (64, `spirv.rs`) — so it is O(1) integer arithmetic,
not a prover.

**And the value graph alone cannot answer it, which is why the type layer is
required.** The graph knows `k` is an `Int`. It does not know whether
`swrite ws k v` writes one slot per lane or the same slot for every lane — and
those two programs need opposite answers. That distinction is exactly a
perspective's level:

| index | writes | needs a barrier before a read of another lane's slot? |
|---|---|---|
| `0` | one slot, all 64 lanes colliding | no barrier helps — this is the **error** `DP` must refuse |
| `i % 64` | one slot, one per lane | depends on the reader's interval (fact 1) |
| `i` (a global address) | one slot, one per lane | no — the disjointness the buffer map already has |

The last row is also the **scope** of the analysis: a barrier is needed for
*shared* memory because one workgroup's lanes see each other, and a `Buf`'s
disjointness means a global write needs no barrier at all — cross-dispatch
ordering is the graph's job and the trailing `SHADER_WRITE → SHADER_READ |
TRANSFER_READ` barrier's ([lichen-compute-gpu](lichen-compute-gpu.md#invariants-a-graph-must-not-break)),
not a kernel's.

## 5. The division of labour

| | carrier | supplies |
|---|---|---|
| **order** | the witness spine in the body DAG | which `(write, read)` pairs to examine, and fact 3 |
| **addresses + level** | `DP`, at the type layer | fact 1, and the level fact 2 turns on |

Neither half is sufficient:

- the spine without `DP` inserts a barrier for every write/read pair — correct
  and slower than necessary, so it is the *conservative* fallback, not the
  design;
- `DP` without the spine has nothing to walk, and fact 3 has nowhere to live.

**So: `DAG` gives the order, `DP` gives the judgement, and the barrier is a scan
over the first using the second.** That is the sentence the whole note is for.

## 6. The shape of the two halves

### 6.1 `DP`: memory hierarchy as a type, which is where the paper already put it

The paper encodes the hierarchy in the data type and **derives** `DP` from it
(`Perspective.md` §"Data Memory Hierarchy"):

| hierarchy | type | `DP` |
|---|---|---|
| thread local | `int` | `thread[1]` |
| **shared** | **`shared(T, N)`** | **`block[1]`** |
| global | `ptr(T, N)` | `grid[1]` |
| view | `view(T, N, DP)` | `DP` |
| immutable | `const(T)` | `0` |

`DP` being a *type* fact and not an annotation is the load-bearing choice, and it
is why `shared` cannot be expressed today: `shared(T, N)`'s `DP` is `block[1]`,
which is a claim about *which* groups may touch the memory, and lichen's only
memory type (`Buf`) is silently `grid[1]`.

lichen already carries memory kinds as **struct wrappers** —
`Buf = T => struct<.native _, .element T>` and `K = _x => struct<.native _, .I _,
.O _>` ([compute-buffer-wrapper](compute-buffer-wrapper.md)) — so shared memory
needs no new channel. It is a second wrapper with the `DP` slot beside the
element slot:

```lichen
Shared = T => struct<.native _, .element T, .disjoint Int>
```

`.disjoint` is `block[1]` written as the level-and-count the `UP` lattice already
speaks — which is the other half of this section, because **`UP`'s lattice
currently has no level**. `# p` today is a bare non-negative integer whose order
is divisibility (`crates/lichen-perspective/src/perspective.rs`);
`thread(n)`, `block(n)` and `grid(n)` all collapse into that one number, which is
why the roadmap records the mismatch "divisibility admits a width no device has —
`6` is a legal perspective, and no lane group is six wide"
([roadmap §4.2](gpu-algorithm-roadmap.md#42-axis-c-lane-width-and-the-consumer-perspective-has-been-waiting-for)).
A level component repairs it: the bare number stays `lane(n)` (so every row of
[attributes.md](attributes.md)'s acceptance table keeps its meaning), and
`lane(n)`/`workgroup(n)`/`grid(n)` become distinguishable, with a componentwise
product lattice whose cross-level order is the containment the paper's `lower`
describes (`thread(32)` lowers to `thread(16)`; `grid(1)` lowers to `thread(32)`).

### 6.2 The write's value: what the spine carries, and what it deliberately does not

The spine is a chain of **state** values: a `swrite` takes the state it succeeds
and returns the state that includes it; a `sread` takes one and returns one; the
index function's return collects the final one. In an SSA body that is one value
threaded through the instruction list, which is a shape the IR already expresses.

**The state carries nothing.** It is not a new buffer, and it is not a summary of
what has been written — it is only "the effects up to here have happened". The
slot and the index are read off the *instruction that consumes the state*, which
is where the index expression already lives, and which is what `DP` judges:

```
t1 = swrite(t0, ws, i % 64, v)   -- state t1; the pair (64, i) is the instruction's
t2 = sread (t1, ws, (i + 1) % 64) -- consumes t1: ordered after it, and its own pair is (64, i + 1)
```

Encoding the touched slots *into* the state instead would make it a record of
index expressions rather than a token, would have to flow through every
intermediate value, and would duplicate the index that the instruction already
holds. So the split is clean: **the state orders, the instruction's operand
describes, and `DP` decides.**

Nothing about the buffer ABI changes either: the ordinal is still a
compile-time constant, the fragment still declares `outputs`, and the run still
binds inputs-then-outputs in one descriptor set. What changes is that the write
set becomes a **fragment field** rather than a private tally, and therefore
something `fragment_digest` must hash — two fragments that differ only in their
shared-memory layout are two modules, and a cache that conflated them would
serve one kernel's shader for another's.

## 7. What the loop rule does and does not change

The `WriteInsideLoop` refusal exists for one reason: `dispatch` allocates output
buffers without initialising them, so a lane that never runs a loop body leaves
its output element as whatever the allocation held — the refusal is the premise
the skip rests on ([lichen-compute-gpu](lichen-compute-gpu.md#the-tail-lane-obligation),
[loop-conversion](loop-conversion.md) §6).

**A shared slot is not an ABI output**, so the premise does not transfer: a
shared buffer is written before it is read by construction of the algorithm that
uses it, and its unwritten state is never a caller-visible element. So a write to
*shared* inside a loop is admissible where a write to an *output* is not — and it
has to be, because every algorithm that wants shared memory (a scan, a tiled
matmul, a block reduction) writes it from inside a loop.

That is a genuine relaxation of a stated invariant and is recorded as one, not as
an oversight. The guard it needs is the barrier one, not the reachability one: a
loop whose trip count differs per lane can reach a barrier on some lanes and not
others, which is a hang rather than a wrong number, and fact 3's scan must
therefore treat a barrier inside divergent control flow as a refusal
([§8](#8-what-is-open) item 2).

## 8. What is open

Each is a real fork, and each is cheaper to settle before the IR moves.

1. **Where the level lives.** Folding it into `Perspective`'s value — one
   attribute, the paper's own shape — touches four places that assume a `USize`:
   `AttrExt::combine` (which hard-codes `ctx.pair(gcd, ctx.int_type())`),
   the slot's type, the persist codec, and the renderer. A second attribute
   beside `#` touches none, but then a `view`'s `DP` refers to a level the
   `UP` attribute does not have, and lichen reconciles **one constraint slot per
   expression at apply time** — `Checker::check_ann` records a single
   `state[e].attr` and `check_app`/`check_lam` re-check exactly one marker, so a
   second side-carrying attribute is against that grain
   ([operator-polymorphism](operator-polymorphism.md) §6). Folding the level in
   keeps one perspective and one slot, which is why it is the option this note
   assumes despite its larger blast radius.
2. **Automatic insertion, or an explicit barrier with a checked placement.**
   The paper inserts synchronisation at the end of a `partition` scope, which
   needs a **scope** perspective; lichen has expression attributes only. So the
   first landing is an explicit `compute.barrier` whose *placement* is checked
   (convergent, not inside a selection or a divergent loop), and the analysis of
   §4 decides where it may legally go rather than emitting it. Automatic
   insertion is the second landing, and it is the larger one.
3. **What the spine is spelled as.** The conservative landing needs no new
   instruction at all — the scan treats "any write before any read" as fact 1
   true — so the spine can be introduced and validated before `DP` exists. What
   decides whether it is worth introducing early is whether the conservative
   answer is ever acceptable; §4's table says for `i % 64` it is not, which is
   the argument for doing the two halves together.
4. **`claim` and `partition`.** The paper's pair for *narrowing* a `DP` — the
   only way a shared slot becomes private to a lane group — is a second piece of
   surface (`partition(data, P, offset)` in, `claim(data, thread[1])` for the
   degenerate case), and `offset`'s disjointness is the user's to prove in the
   paper as well. Whether lichen needs both, or only the degenerate one, is not
   settled here.
5. **`DP` on `Buf`.** Encoding "a global buffer is `grid[1]`-disjoint" makes the
   buffer map's determinism theorem a *checked* fact rather than a documented
   one — which is worth having, and is also a change to what every existing
   parallel kernel's parameter type means. Whether that migration is additive
   (`Buf`'s `DP` is the top, and only `Shared` narrows it) is the safe answer and
   is what this note assumes; whether the top is the honest one is not.

## Recovered measurements

**Why this is not "just add a `__syncthreads`".** The ladder's histogram — 64
elements into 3 buckets, answered `(1, 1, 1)` on **both** backends with no
diagnostic and no refusal
([gpu-algorithms-ladder](gpu-algorithms-ladder.md#5-a-scatter-accumulate-is-silently-wrong))
— is a contended write that is *expressible today*: the same shape a naive
`swrite ws 0 v` has. That is the sharpest available evidence that the missing
half is a **judgement** and not an instruction: the program runs, both backends
agree, and the answer is wrong. A barrier would not have fixed it; `DP` is what
notices it.
