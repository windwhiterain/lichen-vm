# Merging the pre-attention's two norm chains: the cross-group read, and three paths

> Status: **B2 has landed** (the composite's norm chains dispatch over heads, and the pre-attention
> composite is **8 nodes** under the sibling strategy — the ladder's 10 → 8 rung). What it measured
> is in **§7**, including the obstacle §3 did not foresee (an output buffer's length is not the
> launch count) and the limits the merge carries (a merged node is not bit-identical to the
> unmerged pair, and the report's extent numbers go stale after a rewrite). **§7.5 localises the
> first limit** — the composition had dropped the exactness the consumer's own body carries —
> **and fixes the cause**; what is left open is the *scope* of that protection.
> The rest of the note is the investigation as it was written: the question, the measured
> refusals, and the three paths.
> Written against `serve-frontier@1b93532d`; while it was being written the branch moved to
> `a1b68c28` (the fan-in's store-keeping path landed, `1fee3679`), which changes region 4's
> refusal — both states are recorded. The working tree carried the parallel session's
> uncommitted edits at writing time; nothing here is built on them.
>
> Points at: `crates/lichen-serve/src/model.rs` (`PRE_STEP`),
> `crates/lichen-kernel-ir/src/compose.rs` (the staged path), `.../fan_in.rs`
> (`compose_fan_in_staged`, at `a1b68c28`), `crates/lichen-runtime/src/region_merge.rs` (the
> group guard), `crates/lichen-compute-gpu/src/dispatch.rs` (`launch_workgroups`),
> `crates/lichen-compute-gpu/src/spirv.rs` (the extent branch), and
> [shared-memory-and-barriers](shared-memory-and-barriers.md) §9 (the axis-C staging this is a
> consumer of).

## 1. Where the cross-lane read comes from — measured

The two regions, from `PRE_STEP` (`model.rs:1118-1203`):

- **Region 3** (nodes 3, 4): `key2 = squares(.n 1024, .dim 128, .x key1.y)` →
  `key3 = scale(.n 1024, .dim 128, .s key2.s, …)` (`model.rs:1191-1192`).
- **Region 4** (nodes 7, 8, 9): `query1 = matvec(.n 2048, .dim 2048, …)` →
  `query2 = squares(.n 2048, .dim 128, .x query1.y)` →
  `query3 = scale(.n 2048, .dim 128, .s query2.s, .x query1.y, …)` (`model.rs:1195-1197`) —
  `scale` reads **two** producers inside the region, so the region is a fan-in, not a chain.

The lane map, from the kernel sources:

- `squares` (`model.rs:1131-1136`): `lane = compute.range k.n`; the fold reads
  `x[lane*dim + (s(0)-1)]`; the store is `write(.to b_s, .at lane, …)` — **lane = row (head)
  index, and the write is at the producer's own lane**.
- `scale` (`model.rs:1140-1151`): `i = compute.range k.n`; `head = i / k.dim`; the absorbed
  read is `read(.from k.in.s, .at head)` — **lane = element index, and the read is at
  `i / dim`, not `i`**.
- `matvec` (`model.rs:1155-1162`): writes `y` at `row`, its own lane.

So the read that blocks region 3 is `scale`'s `.s` read at `head = i/128`: consumer lane `i`
reads the slot **producer lane `i/128` wrote**. That is genuinely another lane — same lane
only for `i == 0` — not a same-lane read that the wide dispatch merely makes *look*
cross-group. And it is cross-**group**: one group is 64 lanes (`LOCAL_SIZE_X`,
`spirv.rs:277`), the meaningful writers are lanes `0..8` (heads = 1024/128), all in group 0,
so 15 of the 16 consumer groups read a store no barrier orders. Region 4 has three staged
edges: `matvec → scale` at `.at i` (**same-lane**), `squares → scale` at `.at head`
(cross-lane, cross-group, writers `0..16` in group 0 against 32 consumer groups), and
`matvec → squares`, a **gather**: squares lane `p` reads `y[p*128 .. p*128+128]`, written by
lanes of groups `2p` and `2p+1` — cross-group, and the read sits **inside the fold's loop**.

Verified by running, at `1b93532d` (`cargo run --example fuse_class_probe`, exit 0):

```text
region 3 (nodes 3, 4): refused: StagedReadIsNotOrdered: … the barrier that separates them
  orders the lanes of one group of 64 — but the merged node dispatches over 1024 element(s) …
region 4 (nodes 7, 8, 9): refused: ForwardedValueIsLaneDependent: … the cone holds the
  carried parameter ValueId(12), which is state a loop handed forward and no address
  re-derives …
answer 0/1/2: … equal: true          (the sibling merge alone; the differential is clean)
```

**Which read changed the answer in the earlier unguarded run.** Recorded in
[graph-fusion](graph-fusion.md) §5 (line ~880) and in `1b93532d`'s commit message: with the
guard lifted, the merged `squares → scale` over 1024 lanes printed `answer 0: … equal: false`
where the unmerged program and the merged one had agreed. The read responsible is the staged
one: the absorbed `.s` read, lowered to `KernelInstr::OutputReadCall` of the kept store at
address `head = i/128`. The writers of the meaningful slots sit in group 0; consumer lanes in
groups 1–15 read those slots in the **same dispatch**, and no `OpControlBarrier` orders
another group's lanes (execution scope is `Workgroup` or `Subgroup`,
shared-memory-and-barriers §9:1230-1237). What the read answers instead is whatever the
output allocation held — outputs are never initialised (`dispatch.rs:1651`: "Nothing is
written into the outputs here"). The `.x` and `.w` reads are surviving input ports and are
unaffected; only the staged store read is wrong. Hence the guard
(`region_merge.rs:884-892` at `1b93532d`; `:889-898` at `a1b68c28`, now keyed on the
composition's own `keeps_store` answer).

**Two latent facts the composite itself carries**, both by reading the code against the
dispatch:

- `squares` is dispatched over the **element** count while only lanes `< n/dim` are
  meaningful: key2's squares lane `p` reads `x[p*128 + j]` from a 1024-element buffer, so
  lanes `≥ 8` read past the allocation (padded only to a whole workgroup,
  `dispatch.rs:1619-1624`; no robust-access feature anywhere in `lichen-compute-gpu`). The
  answers stand only because `scale` never reads `s[8..]`. Path B below removes this as a
  side effect.
- The whole-workgroup launch rule covered **slices only** (`launch_workgroups`,
  `dispatch.rs:389-399`: `stages = any(!fragment.slices.is_empty())`). A store-keeping merge
  records a **buffer** barrier, declares it through `holds_a_staged_store`
  (`spirv.rs:4291-4300`), and was not in that check. Since the entry point's body branches on
  `index < extent` before any instruction (`spirv.rs:273-276`), a staged merge at an extent
  below 64 would put its `OpControlBarrier` in the surplus-lane branch — non-uniform control
  flow, undefined. Latent today: the only landed staged merge runs at exactly 64
  (`fan_in_probe`, `a1b68c28`). It becomes **live the moment a narrow merge is attempted**,
  which is exactly what Path B does. **The gap is closed** (`c6ac567d`): the launch rule now
  asks `holds_a_staged_store` too, so a launch that records a buffer barrier is refused by
  name unless the count is a whole number of workgroups.

## 2. Path A — cross-group reduction tiling (the axis-C answer)

Split the reduction so each consuming **group** derives the value it reads: per-group partial
sums staged in a slice, a barrier, a combine, and the consumer reads the combined value
inside its own group. The machinery exists as `Folding::tiled`
(`lichen-fusion-bridge/src/tiled.rs:354-434`): strided per-lane partials, one
`SliceWriteCall`, a recorded slice `Barrier`, and a per-lane fold of the slice.

What the IR/plan needs, beyond what exists:

- A **third `Forwarding` beside `Recomputed` and `Staged`** in `compose`
  (`compose.rs:149-172`): when the blocker is `NotIndexLocal::Carried` *and* the producer
  claims `Reduction` with an associative `combine`, re-tile the producer's fold per consuming
  workgroup instead of keeping its store. This is a **restructure, not a splice** — every
  composition today copies cones or keeps bodies whole — and it is the real cost.
- A slice `Slice { class: Float, count: 64, space: Shared }` (256 B of staging), the slice
  barrier (uniform: the tiled body places it at entry level), and the launch's
  whole-workgroup rule — already enforced for slices, and 1024/2048 are multiples of 64.
- The consumer's absorbed read becomes the per-lane slice fold (an SSA value in-lane), not an
  `OutputReadCall`.

Costs: **no extra dispatches** (one kernel, one barrier per group); region 3's each row sum
is recomputed by the two groups that read it (negligible); region 4's `matvec → squares`
gather forces each group to recompute the 128 rows of its head itself — **3× the
q-projection's flops**. And the decisive one for this goal: the fold is **re-associated**
(128 elements as 64 partials + a combine), so the answer is *not bit-identical* to the
unmerged step. The licence is the declared associativity (`BinaryOp::associative`,
shared-memory-and-barriers §9:1338-1343), but the acceptance harness reads `==` as
correctness (`fuse_class_probe`'s `equal: true`) — a tiled merge fails that check by
construction and needs a tolerance acceptance or a pack-level opt-in. The minimal sound slice
of Path A is the whole mechanism; there is no smaller cut of it.

## 3. Path B — lane-local composite (the composite is the test vehicle)

Redesign the chain's kernels so the consumer reads the producer's store at **its own lane**;
then no value crosses lanes at all, the barrier question never arises, and the staged path
merges at **any** width. Two variants:

- **B1, wide and replicated**: keep 1024/2048 lanes; `squares` writes the *replicated* sum —
  lane `i` folds head `i/128`'s row and writes `s[i]`; `scale` reads `s` at `i`. Same-lane,
  bit-identical (same per-row fold order). Cost: the sum is folded 128× redundantly
  (131 072 multiplies where 1 024 were meaningful), and the merge still needs the guard to
  learn fact 2 (below), because `keeps_a_store` fires on the output count alone and
  `StagedReadIsNotOrdered` would still refuse 1024 > 64. Region 4 is **not** helped: the
  `matvec → squares` gather is cross-group whatever `scale` does.
- **B2, per-head dispatch**: `squares` at `n = heads` (8/16 — also deleting the latent
  out-of-bounds reads above), `scale` at `n = heads` with a per-lane loop over the 128
  elements (writes in a loop are emitted today: the refusal was withdrawn,
  `spirv.rs:4-7` and `tests/compute_gpu/refusals.rs:501`). Extents 8 and 16 are one group, so
  the group guard passes; the `squares → scale` read is at the consumer's own lane, and the
  per-row fold order is unchanged, so the answers are **bit-identical**. Cost: the composite
  stops looking like a wide GPU program (a 16-lane `scale` writing 128 slots per lane), which
  is a real loss for a *test vehicle* — it then demonstrates the one-group staged path, and
  the wide-merge capability (axis C) stays open by construction.
  **Landed**: §7 measures it, and names the obstacle this bullet did not foresee
  (a buffer's length is not the launch count) and the limits the merge carries.

Both are composite-level changes plus one small framework move; neither touches the IR. Under
either, region 3 merges. **Region 4's last node is a different question** (§5).

## 4. Path C — what else the investigation found

Three small moves, each independent, each sound:

1. **Barrier elision for same-lane staged pairs.** `compose` records a `Barrier` for every
   absorbed read of a kept store (`compose.rs:420-428`; `fan_in.rs:464-487` at `a1b68c28`).
   §4's fact 2 — the two index operands being the same `ValueId`, and nothing else — applies
   here verbatim: when the read's address *is* the write's address (which is what B1/B2
   arrange), one lane observes its own store in program order and the barrier is dead weight.
   At extents below one workgroup it is worse than dead: it lands inside the
   `index < extent` branch and is **non-uniform** (§1's latent gap goes live). Elision is a
   same-`ValueId` comparison at recording time, and it is **required, not optional**, for any
   narrow staged merge. **Landed** (`f9d707f1`): the comparison is `same_lane`, in
   `lichen-kernel-ir/src/compose.rs`, and both the chain and the fan-in path use it. On the
   pre-attention's qnorm fan-in it removes the `matvec → scale` barrier (the read is at the
   consumer's own lane) and leaves the gather's — the one the reduction's own loop holds.
2. **Extend the whole-workgroup launch rule to buffer barriers** (`dispatch.rs:389-399`:
   `stages` should also be true of `holds_a_staged_store`). A soundness fix regardless of
   Path B: today a staged merge at extent < 64 would run a barrier part of the workgroup
   never reaches. Land this *before* elision enables narrow merges, or the gap ships.
   **Landed** (`c6ac567d`), ahead of the elision.
3. **Extent-aware chain derivation.** The sibling rule already groups branches by extent
   (`schema.rs:877-878`); the ordinary rule keeps a whole component as one region, so after
   B2-with-a-wide-matvec the region `(7,8,9)` is `ExtentsDiffer` and the mergeable sub-chain
   `(8,9)` is never derived. Splitting a chain region where the extent changes is the same
   rule one level down, and it is what turns region 3's fix into region 4's partial one.
   **Landed** (`72019d28`): the ordinary rule cuts a chain at a data edge whose ends dispatch
   over different extents, and the sibling rule's own component is the run's connectivity and
   not the ordinary rule's regions — cutting it would lose the sibling merge. The refusal
   `ExtentsDiffer` now names the region whose consumer's extent another node inside it
   computes.

A fourth candidate was examined and rejected: a **grid-scope synchronisation** (a device-wide
barrier or cooperative launch) would order the cross-group read directly — and no
`OpControlBarrier` offers it (`spirv.rs` emits Workgroup and Subgroup scopes only);
shared-memory-and-barriers §9:1235-1237 already records this. Not a path.

## 5. The minimal slice, and what stands between 10 and 7

The node arithmetic: region 3 merges −1, region 4 merges −2 (three nodes become one), so 7
requires **all of it**, including the `matvec → squares` gather edge. That edge is the wall:

- store-keeping it at 2048 lanes is unsound (cross-group gather; the guard is right to
  refuse);
- recomputing it is `ForwardedValueIsLaneDependent` — the accumulator is loop-carried
  (`body.rs:219-222`), and making it index-local by a literal trip count expands the fold
  into ~2 048 instructions **per absorbed read** — 128 reads in `squares`' gather is a
  ~262 000-instruction merged body — and folds the `~ "combine"` marks away, which is exactly
  the licence risk shared-memory-and-barriers §9:1345-1353 records ("the marked accumulation
  folds into the constant its base case returns");
- staging it at 16 lanes (B2 applied to `query1` too) is sound in fact — same-lane — but
  unprovable by the same-`ValueId` test (the write's `h*128 + r` and the read's `h*128 + j`
  are different values), so a barrier is recorded, the read sits **inside the fold's loop**,
  and `KernelBody::validate` refuses the merged body: this is region 4's *new* refusal at
  `a1b68c28`, `MergedBodyIsRefused` — "the barrier's read sits in the reduction's own loop"
  (`1fee3679`'s message). Placing the barrier soundly for a loop-resident read is the
  **post-dominator question §9 explicitly leaves unsettled** (§9:1200-1216): the concrete
  instance here is mild — the write's loop and the read's loop run in sequence, so the
  barrier belongs in the block between them, where every lane passes.

So the honest ladder:

| target | slice | framework work |
|---|---|---|
| 10 → 9 | B2 on region 3 alone | Path C.2, then C.1 |
| 10 → 8 | B2 on both norm chains, `query1` left wide | + Path C.3 (derive `(8,9)`) |
| 10 → 7 | B2 on the whole query chain (per-head `matvec`) | + the barrier-placement rule for a loop-resident staged read (the §9 post-dominator question, instantiated) |

**Recommended minimal slice for 10 → 7**: B2 across both chains (composite-only, answers
bit-identical, removes the out-of-bounds reads), preceded by C.2 (soundness) and C.1
(elision), and closed by the one real piece of design: place a staged pair's barrier whose
read sits in a loop in the loop's pre-header — after the producer's own loop — where it is
uniform and dominates every return. The fan-in's store-keeping itself **landed** while this
note was written (`1fee3679`, verified on the device at 64 lanes by `fan_in_probe`,
`a1b68c28`), so no composition machinery is missing. Verification is the one this branch
already uses: `fuse_class_probe`'s merged/unmerged differential (`equal: true` on all three
answers) plus the report lines showing regions 3 and 4 merged.

If the barrier-placement rule is judged out of scope, **10 → 8 is the sound floor** and the
last node is axis-C's, not this composite's.

**Landed**: the 10 → 8 rung, and the composite measures 8 nodes under the sibling
strategy (§7). The barrier-placement rule for a loop-resident staged read has since
landed (§5.1), and the 10 → 7 rung is still open: `query1` stays wide, and three
things stand between the composite and the merge (§5.2).

### 5.1 The barrier placement: the pre-header, and what it needs

**The rule, at the code** (`crates/lichen-kernel-ir/src/compose.rs`, `staged_barrier`): a staged
pair's barrier stands **where every lane passes**. A read whose block every lane reaches keeps the
barrier at the read. A read in a block a lane can skip — a read inside the consumer's own loop — is
ordered by a **pre-header observation**: an `OutputReadCall` of the producer's staged output in the
block the reading body landed on, which is after the producer's loop and before the consumer's, and
which every lane reaches. The observation's value is read by nobody; its position is what the
barrier is for. The same-lane elision (C.1) and the group-width guard (`StagedReadIsNotOrdered`) are
untouched, and `KernelBody::validate`'s uniform obligation still decides whether the block the
observation stands in is one every lane reaches.

**Measured** (`fan_in_probe`, at this slice's tip): the consumer's reads moved into its own loop, so
the barrier could not stand at the read. The fan-in merges into one node of three outputs, the merged
node runs on the device, and its answer is the unmerged program's and the host's own (`answer 0:
merged 64 element(s), plain 64 element(s), equal: true`, and the host's own answer: equal: true).
Before the rule, that body was refused by name (`MergedBodyIsRefused`: the barrier's read sits in a
block a lane can skip).

**And the wide query chain's refusal moves one step on** (measured with the query chain in the
pre-B2 shape §1 describes): region 4 — nodes 7, 8, 9 — composes, and the refusal is the group
width's own (`StagedReadIsNotOrdered`: the merged node dispatches over 2048 element(s), so the read
crosses a group and no barrier reaches it). The composition's own gate no longer fires first.

**The limit the rule carries**: the pre-header can order the producer's store only where the store is
written before that block. A producer that writes its store **inside its own loop** is still refused
by name, and the merged body's gate says so (§5.2's second fact).

### 5.2 What stands between the composite and the merge

Three facts; the ladder's row above names the first and the last:

1. **The region is cut before it is attempted.** B2 put the query's norm chains at 16 heads while
   `query1` stays at 2048, and the ordinary rule cuts a chain at a data edge whose ends dispatch
   over different extents (§4's C.3) — so region 4 is node 7 alone and region 5 is nodes 8 and 9,
   merged. The query *fan-in* is not attempted at all, and no barrier placement changes that.
2. **A per-head `query1` writes its store in a loop**, so the pre-header cannot order it: the
   store's write does not dominate the block after that loop, and `KernelBody::validate` refuses a
   barrier whose write does not precede its read. The CFG expresses the loop's completion — the
   loop's exit dominates the pre-header — but the IR cannot **name** the loop a write belongs to,
   the loop structure being implicit in the CFG, so the relation cannot be stated. **The minimal IR
   addition is that name**: the body must say which loop a write is in (or the barrier must name the
   loop's completion), so the gate can accept "the write is in the loop that completes before the
   observation". The other reading is the SPIR-V one — a barrier in a loop body is valid when the
   loop is dynamically uniform, which a runtime-scalar trip count is — and the IR states no loop's
   uniformity either.
3. **A merged node that records a buffer barrier must dispatch over whole workgroups** (C.2's launch
   rule): the surplus lanes leave the body before the barrier, so a barrier in a merged node of 16
   lanes is in divergent control flow. The query chain's per-head form would have to be one
   workgroup wide (64 heads of 32), not 16.

## 6. Risks, and what not to attempt

Risks of the recommended slice:

- **The vehicle stops measuring the frontier.** Per-head kernels at 8/16 lanes exercise the
  one-group staged path only; the wide-merge question (Path A) is deferred, not answered, and
  a future reader could mistake 7 nodes for the capability. Say so in whatever lands it.
- **C.1's elision is only as sound as fact 2's syntactic test** — the same `ValueId` and
  nothing else (shared-memory-and-barriers §4's table; the lane-invariance clause beside it
  is dead and unsound). Do not widen it to "equal-looking" expressions.
- **B2 depends on writes in a loop staying emitted** (`spirv.rs:4-7`) and on the surplus-lane
  branch never wrapping a recorded barrier — which is why C.2 comes first.
- **The pre-header placement reaches only the stores it can order**: a producer whose store is
  written inside its own loop is still refused by name, because the IR cannot name the loop a
  write belongs to (§5.1's limit, §5.2's second fact).
- Unverified here (no new probe was written): that a per-head `scale`/`matvec` rewrite
  composes through the staged path with no further refusal. The shapes are the ones
  `fan_in_probe` already runs at 64 lanes; the difference is extent 8/16, which is what C.1
  and C.2 are for.

Do **not** attempt:

- **Weakening or lifting `StagedReadIsNotOrdered` for the wide chains as they stand** — the
  wrong answer is measured (§1), and the guard's conservatism is not what blocks this
  composite: the read is genuinely another group lane's.
- **Store-keeping the `matvec → squares` gather at 2048 lanes**, in the fan-in or anywhere
  else — cross-group by construction; no barrier reaches it.
- **The recompute path for a loop-carried reduction via literal trip counts** — quadratic
  body growth (§5) and the marks, hence the floor licence, do not survive the expansion.
- **A grid-scope barrier or cooperative launch** — not in this target's vocabulary
  (§4, rejected candidate).
- **Paying Path A for this goal** — it is the axis-C roadmap item, it changes the numerics
  the acceptance harness reads as correctness, and its minimal cut is the whole mechanism.
  Build it when the wide merge is the goal, not when the node count is.

## 7. What B2 measured

**The slice landed on `serve-frontier`.** The pre-attention composite's norm chains dispatch over
**heads** now, and the composite is **8 nodes** under the sibling strategy — the ladder's 10 -> 8
rung. The measurements below come from `fuse_class_probe`, `fuse_probe` and `fusion_file_probe`
on the RTX 3060 Laptop GPU, at this slice's tip.

### 7.1 What the composite says now

`squares` and `scale` keep their shape and change their **lane map**: the squares kernel writes
one head's sum at its own lane, and the scale kernel reads it at **that same lane** and writes
its head's row in a loop of its own. The three chains are per-head: the hidden chain one head of
2048, the key chain eight heads of 128, the query chain sixteen heads of 128. `query1` stays wide
(2048 lanes), as the 10 -> 8 rung says.

**The latent out-of-bounds reads of section 1 are gone with it**: a squares kernel over heads
reads only its own heads' rows, so the key chain's eight lanes read `key1.y[0..1023]` of a
1024-element buffer and the query chain's sixteen read `query1.y[0..2047]` of a 2048-element one.
The hidden chain, which section 1 did not name, had the same defect (2048 lanes over a
2048-element buffer) and has it no more: its one lane reads `hidden[0..2047]`.

**The same-lane pair is what makes the merge sound.** The producer's write index and the
consumer's read index are the *same* `ValueId` in the merged body — both bodies' `compute.range`
lower to the merged body's own index parameter — so C.1's elision records **no barrier**, the
merged node stages nothing, and C.2's whole-workgroup rule has nothing to refuse.

### 7.2 The node arithmetic, measured

| | before | after |
|---|---|---|
| the sibling plan | 10 nodes | **8 nodes** |
| region 3, the key chain | refused: `StagedReadIsNotOrdered` | **merged**: nodes 3 and 4 into one node at 4 |
| region 4, the query chain | refused: `MergedBodyIsRefused` | cut at the extent change: region 4 is node 7 alone, and **region 5 is nodes 8 and 9, merged** |
| `query1`'s gather | refused | still refused: the matvec dispatches over 2048 and the squares over 16, so C.3 cuts the chain there |

`fuse_class_probe`'s differential: **equal: true** on all three answers (1024, 1024 and 2048
elements). The rewrite itself is bit-identical: the unmerged program of the new composite and the
unmerged program of the old one agree on **every element's bits**.

### 7.3 The obstacle this note did not foresee: a buffer's length is not the launch count

Section 3's B2 has the scale kernel write 128 slots per lane. A lane count and a buffer length
are two numbers, and the framework had only the first: an output buffer was allocated at the
launch count, so an 8-lane scale writing 1024 elements wrote past a 64-element allocation. Three
small moves carry the second number:

- `KernelRoles::output_extents` already said what it is — "the allocation the run owes that
  output" — but **nothing ever filled it for a source literal**: the lowering read the length's
  *compound* node, whose value is a one-element array, so the read answered `None` and every
  declaration fell back to the launch count. The lowering now reads the length at its element
  slot (`crates/lichen-compute/src/compute/body.rs`).
- the **GPU backend** allocated each output at the launch count; it allocates it at the declared
  extent when there is one (`crates/lichen-compute-gpu/src/dispatch.rs`).
- the **merged fragment** carried no output extents at all, so a merged node's own wide output
  would have been allocated at the launch count; the composition now carries the bodies' own
  declared extents, in the outputs' order (`crates/lichen-kernel-ir/src/compose.rs`).

The **value's count** followed: a count is what a fetch reads, so it is the buffer's own length
now, and not the number of lanes that wrote it (`crates/lichen-graph-ir`).

### 7.4 The limits, measured

- **A merged node is not bit-identical to the unmerged pair.** On a two-node program (eight heads
  of 128) the producer's sums are equal to the bit, but 100 of the 1024 elements of **one** head
  differ by one ULP from the unmerged program's — deterministically (three runs, the same 100
  elements). That head's divisor differs by one ULP; the sums do not. **The cause is localised in
  §7.5.** **The probe's differential cannot see it**: both of its preparations merge the same
  chains now (the `component` strategy folds the whole component, and the component's chains are
  mergeable), so it compares two merged programs.
- **The report's extent numbers go stale after a rewrite.** The launch-extent closure the
  automatic-tiling pass reads is built over the graph it was handed, so a merged node's "covers N
  element(s)" is the *old* graph's node at that position. The key chain's merged node (8 lanes) is
  reported as `MergedWorkgroupIsPartial` (the old graph's node 3 is the squares, 8 lanes), while
  the query chain's (16 lanes) passes the same check on the old graph's node 6 (1024 elements).
  That check is the report's own rule, not the launch rule: C.2's rule is enforced at the
  dispatch, where the merged node records no barrier and so is not refused.
- **The vehicle measures less.** The hidden chain's norm now runs on **one** lane (2048 elements
  written in a loop), and the key chain's scale writes 1024 elements into a buffer declared at
  2048 — the widest chain's length, so that one kernel serves all three. The wide-merge
  question is deferred, not answered.
- **The key chain's region hash changed** (the scale kernel is another kernel), so `fuse_probe`'s
  fuse pack names no region any more and the floor's licence merges the region instead. The
  **sibling** region's hash is unchanged, so `fuse_class_probe`'s classed pack still names it.

### 7.5 The cause: the composition dropped the exactness the consumer's body carries

**Measured, on the RTX 3060 Ti, with `crates/lichen-serve/examples/norm_ulp_probe.rs`** (a
two-node program — the squares kernel and the scale kernel — over eight heads of 128, read as
bits). Three runs, the same host inputs:

| run | the sums | the divisors | the scaled elements |
|---|---|---|---|
| the **wide** chain (the composite's old lane map), unmerged | — | — | **every bit equal** to the per-head chain's |
| the **per-head** chain, unmerged | the host's own plain sum, bit for bit | the IR's definition, bit for bit | — |
| the per-head chain, **merged** | the same bits | **one head one ULP away** | **100 of that head's 128 elements one ULP away** |

**So the per-head rewrite is bit-identical to the old lane map, and the difference is the
merge's.** Not the association: the fold's order is the same in both programs (the counter runs
from `DIM` down to one). Not the extent machinery: the sums are the same bits and the divisor's
input is the same sum. Not the kernel bodies: the two programs' bodies are the composite's own.

**The cause is the composition's own.** A body that inlined a transcendental's definition carries
`exact_float_arithmetic`, and the emitter writes one `NoContraction` per float result of such a
body ([contraction-control](contraction-control.md) §9.1). The composition copied the consumer's
*instructions* and not the consumer's *decision*: the merged body answered `false` where the
consumer's own kernel answered `true`, and the merged module carried **no** `NoContraction` where
the consumer's module carried **113**.

**What the missing decoration buys the driver.** The IR's square root is a Newton iteration whose
`reduced / root` is a division, and an undecorated division is the driver's reciprocal-multiply
rather than the division the IR wrote ([division-rounding-gap](division-rounding-gap.md) §4). So
the merged node's square root is not the definition the kernel computes, that head's divisor
moves by one ULP, and the head's elements move with it.

**Fixed**: a composed body is as exact as the bodies it composes
(`crates/lichen-kernel-ir/src/compose.rs`, `.../fan_in.rs`, `.../siblings.rs`). The merged body is
marked when any composed body was, the merged module carries the decorations the consumer's
kernel had (113 → 115), and the merged node's answers are the pair's again — **the divisors, the
sums and every scaled element agree on every bit** in the probe.

**What the fix does not settle, and what is left for the maintainer.** The protection is per body
(the emitter's whole-module rule is [contraction-control](contraction-control.md) §6's decision),
and the *producer's* own kernel is not marked: the driver is free to contract the producer's fold
(`acc + x*x` into an FMA), and the probe measures it doing so, while the merged body — marked
whole — computes the fold as the IR wrote it. So a merged node and its pair still differ where
that contraction bites, and there the **merged node is the one that answers the IR's arithmetic**.
Making the two agree is a question of *scope*, not of this defect: either the protection becomes
per value (a narrower set than §6's whole-module rule, which the note says "was not measured"), or
every float body is marked (§6's scope decision), or the difference is accepted and recorded.
Each of those is a change to a recorded decision, and the maintainer's call.

