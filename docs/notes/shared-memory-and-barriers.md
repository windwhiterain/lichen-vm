# Shared memory, and where a barrier comes from

> Status: **in flux — part one landed as a token, is being reverted, and the
> replacement is restaged.**
> A design for the one capability the [roadmap](gpu-algorithm-roadmap.md) calls
> the largest single item in axis C — lanes that can see each other — and for the
> analysis that makes it sound. The three parts were written as inseparable; a
> code audit before implementation showed two of the three are **cheaper than
> this note first claimed**, and §9 records the staging that follows from it.
>
> Part one landed as a zero-width `State` token ([§6.2](#62-the-spine-what-landed)).
> **That choice is retracted and being reverted.** §3's argument for a witness over a
> buffer version rested on a runtime-value model that does not hold inside a kernel
> body, where a `Buf` is an undecided wrapper node and the JIT walks structure rather
> than tracing values — so all three of its objections fall, and the spine becomes a
> **buffer version**: `write` returns a new version of the buffer it wrote, and
> `read` consumes it through the `.from` field it already has. That is what
> *purifying* the write means, which a token never was, because a token is consumed
> and never read.
>
> A spike then measured the replacement and corrected this note twice more (§3). The
> read **does** grow an operand — a version needs it just as a token did — and reading
> back what a body wrote has **no data path in today's ABI**, so landing the
> annotation alone would turn a named refusal into a silent `0`. The spine therefore
> moves out of step 1 and lands **with** `Shared` (§9 step 4), where the slot space is
> readable by construction; step 1 is now the reversion itself.
>
> The spike also measured a case that looked like a silently wrong answer on `dev` —
> a write the codomain does not reach is never emitted and never diagnosed — and it
> is **not one**. Checking it against the type settled the design instead:
> [code-audit D17](code-audit.md), **a `compute.write` is not an effect, it is the
> value of an output position.** That is the sentence this retraction rests on, and it
> makes the token a category error rather than merely an expensive choice.
>
> The paper this project's attribute extension is inspired by supplies the `DP`
> half ([Typed Perspectives](../reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf),
> read in Chinese in
> [the walkthrough](../reference/类型系统验证底层%20GPU%20语言：Perspective.md)).
> [§6.1](#61-dp-deferred-the-nominal-wrapper-already-answers-it) records why
> lichen does not need to build it yet.
>
> Points at: `crates/lichen-compute/src/compute.lichen` (the algebra),
> `crates/lichen-compute/src/compute.rs` (`ComputeValue`, `ComputeOperator`,
> `Positions`, `write_value_node`, `parallel_output_nodes`,
> `run_parallel_range`), `crates/lichen-compute/src/compute/body.rs`
> (`Lower::buffer_write`), `crates/lichen-kernel-ir/src/lib.rs` (`ScalarClass`,
> `KernelInstr::BufferWriteCall`, `fragment_digest`),
> `crates/lichen-kernel-ir/src/body.rs` (the SSA body and `validate`),
> `crates/lichen-compute-gpu/src/spirv.rs` (`LOCAL_SIZE_X`),
> `crates/lichen-compute-gpu/src/dispatch.rs` (the padding that makes a barrier
> safe), `crates/lichen-perspective/src/perspective.rs` (the `UP` lattice §6.1
> defers changing).
>
> Companions: [compute-buffer-wrapper](compute-buffer-wrapper.md) (the ABI this
> extends, and the determinism theorem §3 scopes to `Buf`),
> [compute-kernel-struct](compute-kernel-struct.md) (the struct-wrapper model
> shared memory joins), [loop-conversion](loop-conversion.md) (the loop a
> shared-memory algorithm needs, and the `CONDITIONAL_WRITE` discipline a write
> inside one breaks), [lichen-compute-gpu](lichen-compute-gpu.md) (the tail-lane
> obligation, and why a write in a loop is refused today),
> [gpu-algorithms-ladder](gpu-algorithms-ladder.md) (the measurement that a
> contended write is silently wrong today — this design's sharpest evidence).

## 1. The claim, in one paragraph

lichen's kernel bodies are already pure, and the one impure thing in them is
`compute.write`. It is impure in the way that matters: **it produces no value a
later instruction can depend on, so the value graph has no edge from a write to
the read that must observe it** — and that edge is the one a barrier needs. So
the work is not "make the body pure"; it is **"give the effect a value"**, after
which the barrier question becomes a walk over a DAG. But a walk over the DAG
answers only *which pairs to consider*: whether a pair actually needs a barrier
is decided by whether the two lanes' addresses can overlap. This note first
called that judgement `DP`, the half of a typed perspective the project never
built. **The audit corrected it**: for the two memory kinds lichen will have, the
overlap question is answered by the *nominal wrapper* (`Buf` or `Shared`) plus a
def-cone query over the body, both of which exist or are local to
`lichen-kernel-ir`. `DP` is the **precision upgrade** on top, not the enabler
([§4](#4-what-decides-a-barrier-and-it-is-three-facts),
[§6.1](#61-dp-deferred-the-nominal-wrapper-already-answers-it)).

## 2. What the write is today

Four facts. The first two are corrections to what this note originally recorded;
each is cited so the next reader does not repeat the mistake.

- **At the language level the write is a lazy thunk, not "a computed write".**
  `ComputeOperator::Write`'s `run` arm returns `None` together with `Range`'s —
  the comment is "Kernel-only operators: `range`/`write` never reach the VM as a
  standalone apply — stay lazy" (`compute.rs:1487`). The node therefore has *no*
  value at all. What reads the written expression is `write_value_node`, which
  walks the **operand array** to `operand_items(…)[2]` — the `.value` field —
  rather than reading anything the write produced (`compute.rs:2282`). The
  consequence for the spine is the reason it is cheap at the surface and not
  cheap underneath: making `write` produce a value means making it **eager**,
  which is a change to `ComputeValue`, to the codec and to every match site, not
  a change to a signature.
- **At the IR level the write already has a node; what it lacks is a class.**
  `KernelBody::add_op` allocates a `ValueId` for every instruction
  (`lichen-kernel-ir/body.rs:114`), `BufferWriteCall` included. Its
  `ValueDef::Instr` carries `classes: vec![]` because `produces()` returns `0`
  (`lichen-kernel-ir/src/lib.rs:561`) and `validate` enforces the two agree
  (`body.rs:333`). So "it leaves no trace in the value graph" was too strong: the
  trace is a node with no result and no consumer.
- **Every ordinal is a compile-time constant.** `write`'s first operand is the
  `.to` **buffer**, not a length — `body.rs`'s `buffer_write` reads item 1 as the
  index and item 2 as the value and ignores item 0 entirely, taking the ordinal
  from `tally.writes` (`body.rs:1533`), which is what the emitter calls the write's
  *emission ordinal* (`spirv.rs:2772`). So the write set is *already* available at
  compile time; it is simply not exposed.
- **The write set is a private tally.** `Positions { writes, reads,
  write_classes, read_classes, element_class }` (`compute.rs:2624`) counts them
  for the fragment's `outputs`/`output_classes`, and the return point recovers
  which nodes they are by walking the codomain (`parallel_output_nodes`,
  `compute.rs:2341`). Nothing outside the lowering can read it.

The body itself is SSA with one instruction per `ValueId`
([compute-kernel-struct](compute-kernel-struct.md), `lichen-kernel-ir/body.rs`),
so "a DAG" is not a rewrite the language needs — it is the shape the body is
already in.

## 3. The trap: a value graph does not order two effects on one buffer

A pure node with no result — a `swrite` spelled `$swrite(buf, i, v)` returning
nothing — has **no consumers and no producer**. A later `sread buf i` shares only
the `buf` operand with it, and `buf` is a value: two reads of one value are not
ordered by the value graph, they are *unordered* by it. So the DAG's verdict on
the pair `(swrite buf i v, sread buf i)` is "independent", which is exactly
wrong.

The ordinary case shows how wrong. Today the plane is a **buffer map**: one lane
per index, each writing its own slot, and the design's whole determinism theorem
is that the split is unobservable
([compute-buffer-wrapper](compute-buffer-wrapper.md#the-parallel-abi): "the
**result is bit-identical to the sequential loop's** as long as no two indices
write the same output slot: the partition depends only on the count and the
worker count, never on a schedule or a timing"). A value DAG says that correctly,
and its proviso is exactly the theorem's: the order is unobservable *because* no
two lanes touch one slot. It is only shared memory that makes order matter, and
it makes it matter **across lanes** — so the edge we need is not a value edge at
all. It has to be created.

**Conclusion, and it is the whole of part one: a write must return something a
read consumes.** With that spine in place the pair is in the graph and the graph
can be walked.

### The audit's correction, and why the spine was built anyway

`BasicBlock.instrs` is an **ordered** `Vec<ValueId>` and the blocks form a CFG
(`lichen-kernel-ir/body.rs:39`, `:81`), so program order over a body's effects is
*already recoverable* without a spine, and no pass in the tree today reorders a
block's `instrs`. A scan that walks `instrs` in order and the CFG across blocks
would find every `(write, read)` pair §4 needs. The trap above is therefore a
fact about the **value** graph, not about what an analysis can see today.

The spine is being built regardless, for two reasons that are about the future
rather than the present, and they are recorded so the cost is not paid blind:

- **A positional order does not survive restructuring; a value edge does.** Two
  restructurings are already planned or live: [loop-conversion](loop-conversion.md)
  turns an expanded body into blocks with carried parameters, and the host
  backend's segment execution ([§7](#7-the-host-backend-is-the-expensive-half))
  splits a body at its barriers into separately callable pieces. Both move
  instructions between blocks. A state token is carried through a `Br`'s `args`
  by the same machinery that carries every other value, so the order survives
  both; an `instrs` index does not.
- **A token is what makes the edge checkable.** `validate` can refuse a
  `BufferWriteCall` whose state operand is not the previous effect's result. A
  positional scan has nothing to refuse against — its input is the thing it would
  have to trust.

What the spine must **not** be is the reason the analysis is deferred: §4's three
facts need no perspective attribute, so the spine lands alone and the scan lands
when shared memory does.

### Why the witness was the wrong answer, and what replaces it

**Retracted, with the evidence.** The subsection this replaces argued that a
buffer-valued write costs three things a zero-width token does not. All three were
written against a **runtime-value** model of what a buffer inside a kernel body is,
and that model is wrong. Inside a parallel index function a `Buf` is an **undecided
wrapper node** — "two items, the second an element *type*", with a placeholder in
`.native` ([compute-buffer-wrapper](compute-buffer-wrapper.md#a-placeholder-under-a-buf-wrapper))
— and a parallel kernel "is compiled by `$parallel`'s own run, and at that moment
the parameter's annotated type may still be an unresolved cell", which "leaves the
operator **undecided**" (same note, §"The kernel may compile before its
parameter's annotation resolves"). The JIT is a **structural** walk over nodes and
index paths — `Lower::value`, `write_value_node` over `operand_items`,
`parallel_buffer_pos` over `param_path`/`named_path` — and not a trace over values.
Each objection, against that:

- **"One new buffer value per write is a new allocation per write."** There is no
  buffer *value* in a kernel body to allocate; a version costs a **node**. And even
  where a payload-bearing `ComputeValue::Buffer` does exist, the freeze collects
  handles by `(ptr, len)` and dedups them (`static_module/freeze.rs:200`), so N
  nodes sharing one payload cost one arena entry, not N.
- **Two versions would be collapsed into one.** They would be, if they had decided
  values: `ValueExt::value_eq` compares handle payloads by pointer **and by content**
  (`lichen-lowlevel/src/lib.rs:346`), and the equality walk merges two nodes into one
  class when their values are `value_eq` (`equality.rs:744`). But that path is gated —
  "Undecided is not a fact to propagate, so **only a decided value reaches here**"
  (`equality.rs:292`), and `value_pair_equal` asks "whether two **decided** leaf
  values are one" (`:757`). An undecided wrapper is never merged by it.
- **"It removes the buffer from first-class position."** The one that survives, and
  it survives as a *question* rather than a refutation: a closure capturing a buffer
  would have to name which version it captured. Whether that is a cost or the honest
  semantics is [§10](#10-what-is-open) item 2's, not this section's.

What remains of the first objection — "two adjacent values of a scatter/gather are
not ordered, and a buffer-valued write says they are" — is not an objection to
versions at all. **Versions are the cure for it**: `w1 = write(b, …)` and
`w2 = write(b, …)`, both taken from `b`, are *not* ordered, which is the true
partial order, where a token threaded in emission position imposes a total one
([§10](#10-what-is-open) item 5).

**So the spine is a buffer version, not a token — but two rounds of measurement put
the price considerably higher than this section first assumed.** `write` returns a new
version of the buffer it wrote and `read` consumes it through the `.from` field it
already has; that much holds, and it needs no `ComputeValue` at all —
`ComputeOperator::Write` stays **lazy**, because a `: T` annotation writes the type
cell and neither the checker nor the lowering reads the operator's answer (`check_ann`,
`lichen-highlevel/src/checker/annotations.rs:274`; `NativeOp::build` for `Write`
already answers `decided: false`). This is also what *purifying* the write means — the
body becomes a function of buffer values — and a token was never that, because a token
is consumed and never read.

**The annotation is not the unlock, and that was measured rather than assumed.**
`: x.to` type-checks, and it changes **nothing observable**: `read (.from <a write's
result>, .at i)` compiles and lowers identically with and without it, because
`write`'s result type is *undecided* today and an undecided cell unifies with `Read`'s
`.from (Buf T)`. So the wiring the JIT reads is already there — which is exactly the
point of `$read`/`$write` being no-op placeholder operators whose only job is to be
recognised, and it is correct — and what is missing is not a type but everything
downstream of the wire.

**What the spike refutes is "no extra operand on the read"; what the probe then found
is worse.** The edge exists at the **lichen node** level — a read's `.from` operand
*is* the write's result node — but `parallel_buffer_pos` neither refuses it nor sees
through it: it resolves it to an **input position**, and the program silently reads the
wrong buffer. Measured on `dev@be3dcf8`, `"cpu"`, input seeded `[0, 1, 2, 3]`: a body
that writes `i + 7` to `k.out.z`, reads it back and writes `r + 1` answers
`[1, 2, 3, 4]` where it means `[8, 9, 10, 11]`, with no diagnostic at all. That is
[code-audit P1-43](code-audit.md), and it is high rather than medium precisely because
this spelling is what the spine is made of. A small structural walk — exactly
`write_value_node`'s shape — is what turns the mis-resolution into the right answer,
and until the ABI can read an output at all the honest interim is a named refusal.

The edge also does **not** exist in the lowered IR: `BufferReadCall`'s operands are
`vec![position, index]`, where `position` is a fresh `emit_const` ordinal
(`body.rs:1548`), so the write's `ValueId` has nowhere to travel. A version therefore
needs the same IR change a token needed — the read grows an operand — and step 3's
scan, which reads the *lowered* body, cannot see the edge without it. What a version
buys over a token is not the absence of that operand but its **content**: an operand
naming a buffer version lets the scan tell a read of `A` after a write to `B` from a
read of `A` after a write to `A`, which an opaque token
cannot.

**And behind that is a wall, which is the real reason to restage.** Reading back
what a body wrote has **no data path in today's ABI**: the host `read` import reads
`state.inputs` only (`compute.rs:5779`, `.unwrap_or(0)`), outputs are write-only, and
the fragment's `inputs`/`outputs` split has no room for "this input is really an
output of the same body". So landing the annotation and the position walk *alone*
would turn today's named refusal —

```text
compute.parallel: read's buffer argument is not an input buffer of the parallel
parameter (the parameter's inputs are [])
```

— into a program that compiles and **silently reads `0`**. Strictly worse. A global
`Buf` version is therefore not a type-level change but an **ABI** change, and nothing
needs it until shared memory exists — where the slot space is readable *by
construction*, because that is what `Workgroup` storage is for.

**Which moves the spine out of step 1 and into step 4.** [§9](#9-the-staging) now
lands it with `Shared`, `sread`/`swrite` and the `Workgroup` declaration as one
landing, so the edge, its operand and its data path arrive together and the scan has
something real to walk. The token is still reverted ([§6.2](#62-the-spine-what-landed)),
but nothing replaces it on global buffers, because on global buffers the read-back
the spine would order does not exist.

Two measurements outlive that decision and are recorded here rather than lost:

- **A write the codomain does not reach is not written, and that is correct.** The
  lowering is demand-driven from the codomain leaves, so a `write` bound to a
  statement the return value never reads is never emitted, never counted by
  `tally.writes` and never diagnosed: two writes to one output with only the second
  reachable answer with the second's value and no complaint. That looks exactly like
  the ladder's histogram until it is checked against the **type**: an index function
  is `A T.I -> T.O` over a parameter whose `.out` group *is* the outputs, so walking
  the return necessarily covers every declared output, the per-output
  `write_value_node` gate and the tally gate already say the two sets are the same,
  and an unreturned `write` is a dead expression — dropping it is what a
  pure-function semantics does. [code-audit D17](code-audit.md) records the decision
  and its scope: **it holds for global buffers only**, because §9 step 4 makes a
  `Shared` slot readable by another lane, and then an unreturned `swrite` has a
  reader and reachability from the codomain stops being the test. The token would not
  have caught it either, and could not — both writes hang off the same
  `Const(State, 0)` root, so the first is a value nothing consumes. **D17's sentence
  is what this whole section rests on**: a `compute.write` is not an effect, it is the
  value of an output position. So threading a state token through it was a category
  error and not merely an expensive choice, and returning a new version of the buffer
  is not a clever encoding but the honest type for it.
- **A data-flow chain of two writes is refused today.** Making the second write's
  `.at` close over the first's result trips the tally gate ("emitted 2 write(s) but
  its codomain names 1 output(s)"), because the gate counts *emissions* and a chain
  emits twice for one output. Under versioning that program is ordinary, so the gate
  must learn that writes to one buffer are one output — new work the replacement
  unlocks, not a reversion.

**The enforcement question is unanswered and is now the sharpest open one.** Nothing
in the tree forbids reading the original `b` after writing it, which is a silently
unordered read; `KernelBody::validate` has no read/write ordering check and cannot
have one, because it cannot tell an input buffer from an output. The token never
provided this either — `read` does not consume it. [§10](#10-what-is-open) item 3.

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
fragment's `LOCAL_SIZE_X` (64, `spirv.rs:150`) — so it is O(1) integer
arithmetic, not a prover.

### All three facts are a def-cone query in the IR, and none of them is `DP`

This is the audit's second correction, and it is what makes the staging of §9
possible. The value graph alone cannot answer fact 1 — it knows `k` is an `Int`
and not whether `swrite ws k v` writes one slot per lane or the same slot for
every lane. But the fact that decides it is **which memory the slot is**, and
that is a *nominal type* fact lichen already carries: `Buf` is grid-disjoint,
`Shared` is block-shared. Given that, all three facts plus the refusal the
"Recovered measurements" section wants are computable inside
`lichen-kernel-ir` from the body alone:

| the question | the query | cost |
|---|---|---|
| fact 2, same lane | the two index operands are the same `ValueId`, or neither's def cone contains the lane parameter | one memoised cone walk over the body |
| fact 1, may overlap | affine interval arithmetic over the two index expressions, modulo `LOCAL_SIZE_X` | O(1) integers |
| fact 3, already separated | a latch over the scan | free |
| the contended-write **refusal** | the write's index def cone excludes the lane parameter **and** the slot is `Shared` | the same cone walk |

The refusal is the interesting row, because it is the one this note originally
assigned to `DP`. "Excludes the lane parameter" is a **sound** test for
lane-invariance and therefore for a contended write; it is **incomplete** — it
misses `i % 1`, whose cone contains the lane and whose value is nonetheless
constant across lanes. `UP` is exactly what closes that gap: a value annotated
`# 0` is uniform over all threads, which is the precise form of "lane-invariant"
([attributes](attributes.md#perspective)). So the perspective attribute is a
**precision upgrade on a refusal that already works**, not the thing that makes
the refusal expressible. That is the whole of §6.1's deferral.

The table this note originally justified `DP` with, restated with what actually
decides each row:

| index | writes | needs a barrier before a read of another lane's slot? | decided by |
|---|---|---|---|
| `0` | one slot, all 64 lanes colliding | no barrier helps — this is the **error**, refused | the def cone (and `UP` for the incomplete cases) |
| `i % 64` | one slot, one per lane | depends on the reader's interval | fact 1's interval arithmetic |
| `i` (a global address) | one slot, one per lane | no — the disjointness the buffer map already has | the nominal wrapper: `Buf`, not `Shared` |

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
| **kind** | the nominal wrapper (`Buf` / `Shared`) | the scope: whether a pair is in the analysis at all |
| **address** | the def cone + interval arithmetic, in the IR | facts 1 and 2, and the contended-write refusal |
| **precision** | `UP` (`# p`), later `DP` | closing the refusal's incomplete cases; `claim`/`partition` narrowing |

The first three are sufficient and are what §9 stages. The fourth is an upgrade
on top of a working analysis, which is why it moved from "the missing half" to
"deferred":

- the spine without the scan is inert — correct and doing nothing, which is
  exactly what part one is meant to be on its own;
- the scan without the spine has a positional order to walk, but loses it the
  first time a pass restructures the body (§3);
- neither needs `DP` to be sound. Both get more precise with it.

**So: `DAG` gives the order, the wrapper gives the scope, the def cone gives the
judgement, and the barrier is a scan over the first three.** That is the sentence
the whole note is for; the original version of it named `DP` where the def cone
now sits.

## 6. The shape of the two halves

### 6.1 `DP`: deferred, the nominal wrapper already answers it

The paper encodes the hierarchy in the data type and **derives** `DP` from it
(`Perspective.md` §"Data Memory Hierarchy"):

| hierarchy | type | `DP` |
|---|---|---|
| thread local | `int` | `thread[1]` |
| **shared** | **`shared(T, N)`** | **`block[1]`** |
| global | `ptr(T, N)` | `grid[1]` |
| view | `view(T, N, DP)` | `DP` |
| immutable | `const(T)` | `0` |

`DP` being a *type* fact and not an annotation is the load-bearing choice, and
lichen already agrees with it without having built it: lichen carries memory
kinds as **struct wrappers** — `Buf = T => struct<.native _, .element T>` and
`K = _x => struct<.native _, .I _, .O _>`
([compute-buffer-wrapper](compute-buffer-wrapper.md)) — so shared memory is a
second wrapper, and **the wrapper is the `DP`**. `Buf` is `grid[1]`, `Shared` is
`block[1]`, and no attribute has to say so, because for these two the level is a
function of the type. Shared memory therefore needs no new channel:

```lichen
Shared = T => struct<.native _, .element T, .n Int>
```

with `.n` the slot count, not a `.disjoint` level.

**Why the level component is deferred rather than built.** The original §6.1
proposed growing `UP`'s lattice with a level, because `# p` today is a bare
non-negative integer whose order is divisibility
(`crates/lichen-perspective/src/perspective.rs`), so `thread(n)`, `block(n)` and
`grid(n)` all collapse into that one number — the mismatch the roadmap records as
"divisibility admits a width no device has — `6` is a legal perspective, and no
lane group is six wide"
([roadmap §4.2](gpu-algorithm-roadmap.md#42-axis-c-lane-width-and-the-consumer-perspective-has-been-waiting-for)).
That diagnosis stands. What the audit added is the price, and that the first
landing does not need to pay it:

- folding a level in touches `AttrExt::combine`, which hard-codes
  `ctx.pair(gcd, ctx.int_type())` (`perspective.rs:156`), the `GcdOp` codec *and*
  its `run`, `is_subtype`, `render`, `missing_value`, the persist codec, and
  every row of [attributes.md](attributes.md)'s acceptance table;
- a second attribute beside `#` touches none of those, but then a `view`'s `DP`
  refers to a level the `UP` attribute does not have, and lichen reconciles **one
  constraint slot per expression at apply time** — `Checker::check_ann` records a
  single `state[e].attr` and `check_app`/`check_lam` re-check exactly one marker,
  so a second side-carrying attribute is against that grain
  ([operator-polymorphism](operator-polymorphism.md) §6);
- and neither is needed while the only two memory kinds are `Buf` and `Shared`,
  whose levels the wrappers already state.

**The trigger that makes it necessary** is §8's open item: `claim`/`partition`
narrowing, or a subgroup-width reduction, is the first thing that puts a level in
a position the type cannot supply. When it arrives, folding it into `Perspective`
is still the option to prefer, for the one-slot reason above.

### 6.2 The spine: what landed

> **Reverted — read this as the record of what the token cost, not as the design.**
> [§3](#why-the-witness-was-the-wrong-answer-and-what-replaces-it) retracts the
> argument that chose it. Everything below stayed true of the landing while it was
> on `dev`, and the costs it records — a `ScalarClass` variant with `unreachable!`
> arms in four crates, a state carried through every loop nest, ~20 hand-written
> bodies reseeded, a refusal variant and a bridge arm that both exist only to reject
> the token — are exactly what the replacement does not pay.

Part one, and the only part built. **`compute.write` returns a zero-width
state**, and the lowered write threads it.

At the surface the change is two lines and one new type:

```lichen
State = struct<>
write = (x : (Write _)) => $write(x.to, x.at, x.value): State
```

`struct<>` is the zero-field struct form the parser already accepted, so the token
needed no new syntax. `Write` keeps its three fields and `$write` its three
operands — the state is threaded by the lowering, not spelled by the author (§3's
"the token threads through the *lowered* body, not through the codomain").
Underneath, `ComputeOperator::Write` stops being lazy (§2's first bullet) and
produces the token while `Range` stays lazy, so `ComputeValue` gains a `State`
variant and `ValueCodec::write_value` refuses to persist it the way it already
refuses the other runtime-only compute values.

In the IR the token is an operand and a result:
`KernelInstr::BufferWriteCall`'s `produces()` went `0 → 1` — now unconditional for
every instruction — and its `arity()` `3 → 4`, with the state **first** —
`(state, position, index, value)` — so the three existing operands keep their
relative order and a hand-written body is a four-operand push rather than a
reshuffle. `own_class` answers `State` for a write, not the element class in its
own form: that class is what was *written*, while what the instruction *leaves* is
the token, and declaring the element class would make `from_flat` state a result
class the write does not produce.

**`ScalarClass::State` is a third variant and deliberately not in
`ScalarClass::ALL`**, which stays the two *data* classes. That is what keeps the
change cheap: every fixed-size per-class table in the tree — an SPIR-V module's
element type and `ArrayStride` per class, the host's packing widths — is sized by
`ALL` and indexed by `ScalarClass::index`, and a state is no buffer's element, so a
table entry for it would be a chain nothing can bind. `byte_width` and `index`
answer `unreachable!` for it rather than a number, because a zero stride is a wrong
layout that compiles while a state reaching either is a bug in the compiler's own
construction. Where a state reaches a *data* position the emitter can see, it is a
named refusal: `SpirvRefusal::StateWhereAValueBelongs`.

The residual hazard is stated rather than fixed: **`KernelFragment` has no
`validate`**, so a fragment that *declares* `result_classes: [State]` reaches
`type_of`'s `unreachable!` and panics instead of being refused. The lowering cannot
produce one — a parallel fragment's result is the dummy constant in the first
write's value class — but a hand-written fixture can. §10 item 4 keeps the general
question open.

**The state is a carried value of every loop nest, and no source program can put a
write there yet.** `Lower` seeds the token with a `Const(State, 0)` in the entry
block, `buffer_write` consumes and replaces it, and `lower_loop` gives every nest
block one more parameter beside its slots, the backedge and the exit arm handing it
on. `spirv_validation`'s hand-built loops exercise the carry structurally; the
*source* path into it is closed, and naming the gates that close it matters because
[§8](#8-what-the-loop-rule-does-and-does-not-change) is exactly the relaxation that
opens the first.

A codomain position must reduce to a `compute.write`, and the one non-write leaf
that gate tolerates is a `Selection::Computed` conditional (`compute.rs:2316`) — a
`@loop` head is an `Apply`, so `@loop pass = s => if s(0) == 0 then write(…) else
pass(…)` *as* the codomain is refused before the lowering runs. With the loop inside
a codomain write's value instead, `buffer_write` takes its ordinal before it lowers
its operands (`body.rs:1574`), so the nest's own write bills a second ordinal and
the tally gate refuses it (`compute.rs:2335`). And there is no third route:
`lower_index_function` walks only the codomain leaves, so a `@loop` bound to a
statement the codomain never reads is not emitted at all, and `Lower::value` hoists
a node the active nest does not own to the nest's **entry** block rather than to an
arm (`body.rs:262`). A write reaches an arm's block only as a nest-owned node, and
both gates refuse one.

So the carry is built ahead of the relaxation that will need it, and §3's argument
for the spine rests on the restructurings that are *coming* — a converted loop's
nests, a shared write inside one, the host backend's segments — rather than on an
ordering a source program can observe today. **The end-to-end test of a write
inside a nest is therefore not writable yet either**; it becomes writable at §8,
which is the point to add it, and until then the carry's only coverage is the
structural one. One unit test did land with the spine, and it is not about the
carry: `static_function_compute_operator_skips_an_operand_less_operation`
(`lichen-lowlevel/tests/basic/static_module.rs`) pins the discriminator the
annotated `write` forced on `static_function_compute_operator` (§6.2's surface
change), which nothing else in the tree guards.

The two backends materialise the token for free, and neither changes an ABI:

| | the state's representation | what it costs |
|---|---|---|
| **wasm** | `i32`; the `call` is emitted over `[position, index, value]` with **no result**, and the token is a fresh `i32.const 0` | nothing — the host import stays `[i64, i64, value] -> []`, so `run_parallel_range` and the whole CPU path are untouched |
| **SPIR-V** | `Kind::State`, bound to the module-scope `OpConstant %uint 0` the emitter already declares unconditionally — for the write's result, for `Const(State, _)`, and for every incoming edge of a carried state's phi | no extra instruction, type or constant per write |

A hand-written `from_flat` body follows the same two rules: seed
`Const(State, 0)` as its **first** step, so it sits at the bottom of the value
stack while the other three operands are built above it, and end with a data
constant rather than with the write, because the return hands out the top of the
stack and a state is no fragment result. `FlatOp::Read(k)` is *not* how the seed is
named — that reads an entry-block parameter, so a state there would grow the ABI.

**The state carries nothing.** It is not a new buffer, and it is not a summary of
what has been written — it is only "the effects up to here have happened". The
slot and the index are read off the *instruction that consumes the state*, which
is where the index expression already lives:

```
t1 = swrite(t0, ws, i % 64, v)   -- state t1; the pair (64, i) is the instruction's
t2 = sread (t1, ws, (i + 1) % 64) -- consumes t1: ordered after it, and its own pair is (64, i + 1)
```

Encoding the touched slots *into* the state instead would make it a record of
index expressions rather than a token, would have to flow through every
intermediate value, and would duplicate the index that the instruction already
holds. So the split is clean: **the state orders, the instruction's operand
describes, and the def cone decides.**

The `swrite`/`sread` spelling above is the *shared-memory* landing's, not this
one's. Part one changed `write` only: `read` and `Read` are untouched, because a
global buffer read has no ordering requirement — that is the buffer map's
determinism theorem, and the spine must not quietly revoke it. Explicit threading
at the surface (a `.after` field on `Write`, an initial state to thread from) is
deferred with shared memory; until then `Lower` threads the token in emission
order, which is the order the body already had, so **no false dependency was
introduced** and no existing program changed meaning — `Positions`, the
`tally.writes == outputs.len()` check, and `write_value_node`, which reads the
*lichen-level* `operand_items[2]`, are all untouched.

Nothing about the buffer ABI changed either: the ordinal is still a compile-time
constant, the fragment still declares `outputs`, and the run still binds
inputs-then-outputs in one descriptor set. What changes later, when shared memory
lands, is that the write set becomes a **fragment field** rather than a private
tally, and therefore something `fragment_digest` must hash — two fragments that
differ only in their shared-memory layout are two modules, and a cache that
conflated them would serve one kernel's shader for another's. `fragment_digest`
hashes `format!("{:?}", fragment.body)` today (`lichen-kernel-ir/src/lib.rs:693`),
so the spine itself needed no new hashing; the shared-slot field will.

## 7. The host backend is the expensive half

This section did not exist in the original note, and it is the largest cost the
design carries.

[`run_parallel_range`](../../crates/lichen-compute/src/compute.rs) is a
**per-index call loop**: one worker owns a contiguous span of indices, instantiates
the wasm module once, and calls the kernel body once per index, with `read`/`write`
resolved through host imports. That is sound precisely because the body *is* a
function of one index.

**A barrier makes it not one.** `barrier` means "every lane of this workgroup has
arrived", so the host must run all 64 lanes of a group to the barrier and then all
64 from it — the body is a function of a *group*, evaluated in segments. Two ways
to get there:

| | what it is | what it costs | what it loses |
|---|---|---|---|
| **refuse** | `Shared` and `Barrier` are named refusals on the wasm/CPU path | one refusal and one diagnostic | `gpu_matches_cpu` cannot cover shared-memory algorithms |
| **segment** | split the body at its barriers, export one wasm function per segment, put the shared slots in wasm linear memory at `[slot_base + lane × len]`, and make the host loop per-group × per-segment | a second emitter shape, a new host loop, and a shared-scratch layout that must agree with SPIR-V's | nothing — the CPU/GPU agreement survives |

**Decision: refuse, GPU only.** The segment path is the right eventual answer and
is recorded here so it is not rediscovered, but the first landing takes the
refusal. The cost is stated plainly because this project's correctness evidence is
*cross-backend agreement* — the [ladder](gpu-algorithms-ladder.md) measures the
two backends against each other, and `gpu_matches_cpu` is the regression that
keeps them equal — and shared-memory algorithms are exactly the class that loses
it. Their evidence will have to be the SPIR-V structural tests plus on-device
runs, until segmentation lands.

## 8. What the loop rule does and does not change

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

**And the relaxation has three named sites, not one.** §6.2 records that no source
program can put a write inside a nest today, and the reason is *not*
`WriteInsideLoop` — that is the SPIR-V emitter's and it fires last. Two
`lichen-compute` front gates refuse first, both in `compile_parallel_fragment`:
the **codomain gate**, which requires every output position to reduce to a
`compute.write` and tolerates only a `Selection::Computed` conditional
(`compute.rs:2316`), and the **tally gate**, which requires exactly one write per
output and so bills a nest's own write as a second one (`compute.rs:2335`). A
shared write in a loop has to be admissible at both, *and* at `WriteInsideLoop`
for the output case that stays refused. That is the concrete meaning of "a
genuine relaxation": three refusals have to learn to tell a shared slot from an
ABI output, and the fact that lets them is the nominal wrapper (§6.1), not a new
flag.

That is a genuine relaxation of a stated invariant and is recorded as one, not as
an oversight. The guard it needs is the barrier one, not the reachability one: a
loop whose trip count differs per lane can reach a barrier on some lanes and not
others, which is a hang rather than a wrong number, and fact 3's scan must
therefore treat a barrier inside divergent control flow as a refusal. **That
refusal belongs in `KernelBody::validate`**, where the CFG lives, so one
implementation covers both backends and hand-written bodies too — not in
`spirv.rs` beside `WriteInsideLoop`, which is where the loop rule it resembles
happens to sit.

### The tail-lane obligation is already discharged, for free

The good news the audit found, and it removes what looked like the sharpest new
hang risk. `LOCAL_SIZE_X` is 64 and "a run of `count` indices is
`ceil(count / LOCAL_SIZE_X)` workgroups, and the surplus lanes of the last one are
a caller obligation, not the shader's" (`spirv.rs:145`) — and the caller discharges
it by **padding the buffers**, not by masking the lanes: "Round up so the last
workgroup's surplus lanes address padding rather than memory past the end"
(`dispatch.rs:624`). So every lane of every workgroup runs the whole shader.

Therefore **every lane reaches every barrier**, and a `count` that is not a
multiple of 64 does not hang. The padding that exists for an unrelated reason is
exactly what a convergent barrier needs. What remains is the divergence case
above, which is a refusal and not a padding question.

## 9. The staging

The order the audit's corrections make possible. Each step is separately
mergeable and leaves the tree green.

| | step | crate(s) | note |
|---|---|---|---|
| **1** | **revert the token**: `ScalarClass::State`, `ComputeValue::State`, `Lower.state` and the loop-nest carry, `StateWhereAValueBelongs`, `BridgeRefusal::StateClass`, the ~20 `Const(State, 0)` seeds, `State = struct<>`; `BufferWriteCall` back to `produces() == 0`, `arity() == 3`, and `Write`'s `run` arm back to lazy | `lichen-kernel-ir`, `lichen-compute`, `lichen-compute-gpu`, `lichen-graph-ir`, `lichen-fusion-bridge`, `lichen-language` (tests) | §6.2 is the inventory. Coordinates with `356fc4a`, which adapted the fusion bridge to the variant — and fixes a latent defect there while it does (`read_position` reads `args.first()`, which the token made the state). **Nothing replaces it on global buffers**: §3's ABI wall |
| **2** | IR vocabulary: `SharedReadCall`, `SharedWriteCall`, `Barrier` as *additive* variants; the shared-slot set as a fragment field, hashed in `fragment_digest`; `validate` refuses a barrier under divergent control flow | `lichen-kernel-ir` | **`SharedReadCall` carries the version operand here** — §3's "the read grows an operand" is paid once, at the point where a consumer exists. Additive variants, so the ~50 existing `BufferWriteCall` sites do not move |
| **3** | the scan: `insert_barriers(&mut KernelFragment)`, plus the contended-write refusal | `lichen-kernel-ir` | in the IR crate so `from_flat` bodies get it and it is testable without the language. Conservative first, then §4's interval refinement |
| **4** | **one landing**: the surface (`Shared`, `swrite` returning a new version, `sread` consuming it through `.from`), its lowering, and the GPU backend (`OpVariable` in `Workgroup`, `OpControlBarrier`, the slot layout) | `lichen-compute`, `lichen-compute-gpu` | §3's conclusion: the edge, its operand and its **data path** must arrive together, because a read-back with no data path compiles and silently answers `0`. The tally gate must also learn that writes to one buffer are one output. The descriptor set and binding layout do not change — shared memory is not bound |
| **5** | the host backend: the named refusal | `lichen-compute` | §7. Segmentation is a later, separate step |
| **6** | `UP`'s level component, then `DP` | `lichen-perspective` | **deferred until §6.1's trigger fires** |

Two facts about the starting point that the staging assumes, both verified:

- **The GPU backend is greenfield, not extended.** There is no `Workgroup`
  storage class and no `OpControlBarrier` anywhere in `crates/lichen-compute-gpu`;
  the only barriers in the tree are Vulkan **pipeline** barriers between
  dispatches (`dispatch.rs:1068` and neighbours), which order *dispatches* and have
  nothing to do with a workgroup's lanes.
- **Steps 2–4 are a strict prerequisite chain, and step 1 is not a prerequisite at
  all** — it is a reversion. The spine was originally step 1 precisely so that it
  could be validated before anything consumed it; §3 records why that turned out to
  be backwards, and why the version that does land lands with its first consumer.

## 10. What is open

Settled by the audit and the staging above, and recorded so they are not
re-litigated:

- **Where the level lives** — nowhere yet. §6.1: deferred, with the trigger named.
- **Automatic insertion, or an explicit barrier with a checked placement** —
  **automatic**. The original note preferred an explicit `compute.barrier` first,
  on the grounds that the paper's automatic insertion needs a **scope** perspective
  lichen does not have. That reasoning does not survive the audit: the scan's scope
  is the whole body, the one case a scope would have caught (divergent placement) is
  a `validate` refusal, and automatic insertion is *less* surface than an explicit
  operator plus a placement checker plus a sufficiency check on the author's
  placement.
- **What the spine is spelled as** — **a buffer version, not a token.** §3. The
  token landed first (`4a41a71`) and is reverted; §6.2 stays as the record of what
  it cost. `: x.to` is the annotation that expresses a version and it type-checks,
  but §3 measures that it changes **nothing observable**: `write`'s result type is
  undecided today and an undecided cell unifies with `Read`'s `.from (Buf T)`, so the
  wiring already exists and the annotation is not what unlocks it. It is worth having
  for honesty's sake; it is not step 4's blocker, and it must not land alone, because
  the thing downstream of the wire mis-resolves today
  ([code-audit P1-43](code-audit.md)).
- **Whether `write` must be eager** — **no.** A `: T` annotation writes the type
  cell, and neither the checker nor the lowering reads the operator's answer, so
  `ComputeOperator::Write` goes back to lazy and no `ComputeValue` variant is needed
  for a version. A token needed a *value*; a version needs only a *node*.
- **Where the spine lands** — **with `Shared`, not before it** (§9 step 4). §3's ABI
  wall: a read-back with no data path compiles and silently answers `0`, so the edge,
  its operand and its data path arrive together or not at all.
- **Whether the read grows an operand** — **yes, either way.** §3 measured that a
  version needs it just as a token did, because `BufferReadCall`'s operands are
  `[position, index]` and the write's `ValueId` has nowhere to travel. What a version
  buys is the operand's *content*, not its absence.
- **`DP` on `Buf`** — additive and deferred: `Buf` stays what it means today, and
  only `Shared` narrows it. Whether the top is the *honest* `DP` for a global
  buffer is still an open question, and it is now a question about a later landing
  rather than about this one.

Still open:

1. **`claim` and `partition`.** The paper's pair for *narrowing* a `DP` — the only
   way a shared slot becomes private to a lane group — is a second piece of surface
   (`partition(data, P, offset)` in, `claim(data, thread[1])` for the degenerate
   case), and `offset`'s disjointness is the user's to prove in the paper as well.
   Whether lichen needs both, or only the degenerate one, is not settled. **This is
   also §6.1's trigger**: it is the first thing that needs a level the nominal type
   cannot supply.
2. **Whether a closure must name the version it captures.** §3's one surviving
   objection to a buffer-valued write, and versioning makes it concrete rather than
   hypothetical: threading *is* the surface now (`read (.from w1, …)`), so a closure
   that captures a buffer captures some version, and the question is whether
   capturing a stale one is refused or silently unordered. Today a closure captures a
   handle whose writes are not versions, which is why this never arose.
3. **The enforcement question, and it is the sharpest one — and no longer
   hypothetical.** Nothing in the tree forbids reading a write's result, and what
   happens instead is measured: [code-audit P1-43](code-audit.md). The read does not
   merely go unordered, it **resolves to an input position** and silently answers with
   the wrong buffer. `KernelBody::validate` cannot host the check — it cannot tell an
   input buffer from an output — so it belongs where the roles are known:
   `parallel_buffer_pos`, which already has them, or a fragment-level validate. Until
   it exists, a version chain is an *opportunity* to be ordered rather than a
   *guarantee* that it is, and the token never provided one either, because `read` did
   not consume it. The interim refusal P1-43 asks for is the same check the spine
   needs, which is why it is worth landing before step 4 rather than inside it.
4. **Whether `KernelFragment` needs a `validate` at all.** The token made this
   concrete and then went away: `KernelFragment` has no validate, so a fragment
   declaring `result_classes: [State]` reached `type_of`'s `unreachable!` and panicked
   instead of being refused. The revert removes that instance, but item 3's check
   needs the same thing — fragment-level facts (`roles`, `inputs`, `outputs`) that
   `KernelBody::validate` cannot see. Step 2 adds a fragment field of its own, so the
   question should be settled there rather than after.
5. **The residual false edge.** Versioning makes the order partial and matches the
   program text, which cures the token's total order across unrelated buffers. It does
   not cure everything: a scatter writing *one* buffer at distinct indices still gets
   an edge between adjacent versions of that buffer. Breaking it needs the scan to
   prove two writes touch disjoint slots — fact 1's interval question on a
   write/write pair — and then to split the version chain, which means a body carries
   two versions at once and something must say which a later effect consumes. That is
   item 2 arriving by another route, so the two should be settled together, and
   neither is worth settling before a reordering pass exists.
6. **The refusal's diagnostic.** A contended write is today silently wrong on both
   backends (below). Making it a refusal is the fix, but the message has to name the
   index expression, the slot and the reason, and the reason differs between "the
   def cone excludes the lane" and "`UP` proves it uniform" — two causes that should
   not read as one.

## Recovered measurements

**Why this is not "just add a `__syncthreads`".** The ladder's histogram — 64
elements into 3 buckets, answered `(1, 1, 1)` on **both** backends with no
diagnostic and no refusal
([gpu-algorithms-ladder](gpu-algorithms-ladder.md#5-a-scatter-accumulate-is-silently-wrong))
— is a contended write that is *expressible today*: the same shape a naive
`swrite ws 0 v` has. That is the sharpest available evidence that the missing
half is a **judgement** and not an instruction: the program runs, both backends
agree, and the answer is wrong. A barrier would not have fixed it.

What notices it is the def-cone query of §4: `swrite ws 0 v`'s index has a cone
that excludes the lane parameter, so the write is the same slot in all 64 lanes,
so it is contended, so it is refused. **The original note credited `DP` with this,
and that was too strong** — `DP` would notice *more* of them (`i % 1`, and any
expression `UP` can prove uniform whose cone still contains the lane), which is
why §4 calls the perspective attribute a precision upgrade and §10 item 6 asks for
the two causes to be told apart in the diagnostic.
