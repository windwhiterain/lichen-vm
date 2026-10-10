# Shared memory, and where a barrier comes from

> Status: **in progress — part one has landed, the rest is settled and staged.**
> A design for the one capability the [roadmap](gpu-algorithm-roadmap.md) calls
> the largest single item in axis C — lanes that can see each other — and for the
> analysis that makes it sound. The three parts were written as inseparable; a
> code audit before implementation showed two of the three are **cheaper than
> this note first claimed**, and §9 records the staging that follows from it.
>
> What has landed is part one alone: **the write's effect as a value**
> ([§6.2](#62-the-spine-what-landed)) — `write` returns a zero-width `State` and
> the lowered body threads it, on both backends, with no ABI change and nothing
> new expressible. The `Shared` wrapper, the barrier, the scan and §7's host
> refusal are staged behind it and none of them is built.
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

### Why the spine is a zero-width witness and not a new buffer

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

The same argument decides a narrower question the spine's first landing hits:
**the token threads through the *lowered* body, not through the codomain.** A
parallel index function's return value *is* its output positions —
`parallel_output_nodes` takes one node per position and the lowering refuses
unless `tally.writes == outputs.len()` (`compute.rs:2128`, `:2189`). Collecting a
final state into the codomain, as this note originally proposed, would grow the
output arity by one and break that identity. It is also unnecessary: the codomain
already reaches every write, so the token only has to be threaded where the
effects are emitted, which is `Lower`.

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
write's value class — but a hand-written fixture can. §10 item 3 keeps this open.

**The state is a carried value of every loop nest, and that is §3's argument
tested rather than asserted.** `Lower` seeds the token with a `Const(State, 0)` in
the entry block, `buffer_write` consumes and replaces it, and a loop's nest blocks
take it as one more parameter beside their slots, with the backedge and the exit
arm handing it on. This is not a straight-line-only path: a write in a conversion's
exit or step arm — `@loop pass = s => if s(0) == 0 then write(…) else pass(…)` —
lands in a nest block, so the token crosses a backedge in the tree today. A
positional scan over `instrs` would have had nothing to carry.

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
| **1** | the spine: `write` returns a zero-width state, threaded through the lowered body | `lichen-compute`, `lichen-kernel-ir`, `lichen-lowlevel`, both backends | **landed.** §6.2 records what it cost and the one hazard it left open. Expresses nothing new on purpose: it is the edge, not the analysis |
| **2** | IR vocabulary: `SharedReadCall`, `SharedWriteCall`, `Barrier` as *additive* variants; the shared-slot set as a fragment field, hashed in `fragment_digest`; `validate` refuses a barrier under divergent control flow | `lichen-kernel-ir` | additive variants, so the ~50 existing `BufferWriteCall` sites do not move twice |
| **3** | the scan: `insert_barriers(&mut KernelFragment)`, plus the contended-write refusal | `lichen-kernel-ir` | in the IR crate so `from_flat` bodies get it and it is testable without the language. Conservative first, then §4's interval refinement |
| **4** | the surface: `Shared`, `sread`, `swrite`, and their lowering | `lichen-compute` | a second struct wrapper, per §6.1 |
| **5** | the GPU backend: `OpVariable` in `Workgroup`, `OpControlBarrier`, the shared-slot layout | `lichen-compute-gpu` | greenfield: see below. The descriptor set and binding layout do not change, because shared memory is not bound |
| **6** | the host backend: the named refusal | `lichen-compute` | §7. Segmentation is a later, separate step |
| **7** | `UP`'s level component, then `DP` | `lichen-perspective` | **deferred until §6.1's trigger fires** |

Two facts about the starting point that the staging assumes, both verified:

- **The GPU backend is greenfield, not extended.** There is no `Workgroup`
  storage class and no `OpControlBarrier` anywhere in `crates/lichen-compute-gpu`;
  the only barriers in the tree are Vulkan **pipeline** barriers between
  dispatches (`dispatch.rs:1068` and neighbours), which order *dispatches* and have
  nothing to do with a workgroup's lanes.
- **Steps 2–5 are a strict prerequisite chain, and step 1 is not.** Step 1 could
  have been deferred until step 3 needed it; §3 records why it was not.

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
- **What the spine is spelled as** — a zero-width state, at the language level,
  with the token threaded through the lowered body. §6.2.
- **The state's class in the IR** — a third `ScalarClass` variant, **outside
  `ALL`**, and not an `Int` alias. The cheaper alternative — materialise the token
  as a narrow integer and let the type system not know — was rejected for the reason
  §3 rejects a buffer-valued write: it is a claim a later optimizer would have to be
  taught to disbelieve. §6.2 records the diff it actually cost, which stayed inside
  the two backends. Its residual hazard is item 3 below.
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
2. **Whether the surface ever needs explicit threading.** §6.2 defers a `.after`
   field on `Write` and an initial state to thread from. If shared-memory algorithms
   turn out to need orderings the emission order does not give — two independent
   chains in one body, say — the field comes back, and with it a source-level
   change to every `Write` spelling in `examples/` and the tests.
3. **Whether the panic surface of `ScalarClass::State` is acceptable.** §6.2
   records what step 1 chose — `State` outside `ALL`, `unreachable!` in
   `byte_width`/`index`/`type_of`, `SpirvRefusal::StateWhereAValueBelongs` wherever
   the emitter can see a state in a data position — and the cost question is
   answered: the diff stayed inside the two backends and no fixed-size per-class
   table grew. What is still open is the one route by which a state reaches those
   `unreachable!`s from *outside* the compiler's own construction:
   **`KernelFragment` has no `validate`**, so a fragment declaring
   `result_classes: [State]` panics instead of being refused. The fix is a
   fragment-level check that every declared result, input and output class is a
   data class; whether it belongs beside `KernelBody::validate`, which cannot see
   the fragment, or beside `fragment_digest`, which can, is the question, and it
   should be settled before step 2 adds a fragment field of its own.
4. **The refusal's diagnostic.** A contended write is today silently wrong on both
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
why §4 calls the perspective attribute a precision upgrade and §10 item 4 asks for
the two causes to be told apart in the diagnostic.
