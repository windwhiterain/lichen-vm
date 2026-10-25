# Merging the pre-attention's two norm chains: the cross-group read, and three paths

> Status: **an investigation, not a plan in flight.** The question: the pre-attention composite
> is 11 nodes, 10 after the sibling merge, and the goal is 7 — both remaining chain regions
> merged, soundly, with no wrong answers. This note records where the refusal comes from
> (measured, not read off prose), prices the three ways past it, and names the minimal slice.
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
- The whole-workgroup launch rule covers **slices only** (`launch_workgroups`,
  `dispatch.rs:389-399`: `stages = any(!fragment.slices.is_empty())`). A store-keeping merge
  records a **buffer** barrier, declares it through `holds_a_staged_store`
  (`spirv.rs:4291-4300`), and is not in that check. Since the entry point's body branches on
  `index < extent` before any instruction (`spirv.rs:273-276`), a staged merge at an extent
  below 64 would put its `OpControlBarrier` in the surplus-lane branch — non-uniform control
  flow, undefined. Latent today: the only landed staged merge runs at exactly 64
  (`fan_in_probe`, `a1b68c28`). It becomes **live the moment a narrow merge is attempted**,
  which is exactly what Path B does.

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
   narrow staged merge.
2. **Extend the whole-workgroup launch rule to buffer barriers** (`dispatch.rs:389-399`:
   `stages` should also be true of `holds_a_staged_store`). A soundness fix regardless of
   Path B: today a staged merge at extent < 64 would run a barrier part of the workgroup
   never reaches. Land this *before* elision enables narrow merges, or the gap ships.
3. **Extent-aware chain derivation.** The sibling rule already groups branches by extent
   (`schema.rs:877-878`); the ordinary rule keeps a whole component as one region, so after
   B2-with-a-wide-matvec the region `(7,8,9)` is `ExtentsDiffer` and the mergeable sub-chain
   `(8,9)` is never derived. Splitting a chain region where the extent changes is the same
   rule one level down, and it is what turns region 3's fix into region 4's partial one.

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
