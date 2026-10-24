# The serving performance boundary, measured

This note is the measured edge of what the current stack does, and the framework capabilities each
edge asks for. [serving-frontier](serving-frontier.md) is the survey and the map; this is the ruler.

## The probes

- `token_cost` — one stream, the per-stage split, weights in host memory.
- `batch_cost` — eight rows at one position, the same split.
- `prefill_cost` — time to the first answer at 8, 32 and 128 prompt tokens.
- `resident_token` — the same stream with the weights and the head placed on the device.
- `boundary_probe` — a one-node step's floor, and the attention's growth with the cache.

On the RTX 3060 Ti, `Narrowed`, model from `LICHEN_MODEL_DIR`.

## What a token costs

Four tokens of a single stream, **weights in host memory** (the `hold_in_memory` path): stages
2.324 s of 2.402 s wall, a warm token **0.54 s**, and the split:

| stage | time | share |
|---|---|---|
| projection | 1.664 s | **69%** |
| head | 0.380 s | 16% |
| norm | 0.200 s | 8% |
| rotation | 0.035 s | 1.5% |
| attention | 0.029 s | 1.2% |
| product | 0.015 s | 0.6% |

The same stream with **all 28 layers and the head resident on the device** (`reside(28)` +
`place_head`, 3.4 GB of card): a warm token **0.136–0.152 s**, and:

| stage | time a token | share |
|---|---|---|
| projection | 0.0625 s | 43% |
| norm | 0.0285 s | 20% |
| rotation | 0.018 s | 12% |
| head | 0.011 s | 7.5% |
| attention | 0.0085 s | 6% |
| product | 0.002 s | 1.5% |

**The weights were being re-uploaded every step, and a bug was why.** `weight()` hands a layer's
tensor over as a host buffer unless `reside` has placed it, and `reside`/`place_head` compile an
`UPLOAD` step that was still written against the superseded kernel I/O surface — the same debt
`model.rs` had ([kernel-buffer-declarations](kernel-buffer-declarations.md)). Every projection was
paying a **14 MB PCIe upload a step**, 2.82 GB a token, at the ~8 GB/s the bus gives. Migrating
`UPLOAD` and calling `reside(28)` + `place_head` moved the token **0.54 s → 0.14 s (3.8×)**, the
projection **6.7×** and the head **8.6×**, with the same five tokens.

That is the boundary's largest single finding: **the framework already had the capability (a
resident, a relabel, a held model); the path to it was dead.** A boundary measured on the dead path
would have said the projection is 69% and the answer is a tiled kernel. Measured on the working
path, the projection is 43% and the answer is **fewer steps**.

## The class granularity is landed; composition does not pay, and the reason is the dispatch

The class requirement is now independent of plans and claims, and it is selectable **per node and
per input**: a pack names the node positions it applies to and the input slots with their classes
(`nodes` and `classes` in `requires`), and the fragment rewrites exactly those reads
(`states_input_class`). `narrowed_composed` builds such a pack, and `composed_probe` prepares the
eleven-node pre-attention graph with its three projection weights at bf16 and every activation at
Float — three returns of 1024, 1024 and 2048 elements.

Wired into the model, the composite answers the same five tokens at **396 submissions a token**
against 648, and is **still slower**: 0.155–0.168 s a warm token against 0.136–0.152 s. The classes
are not the cause — the wildcard run, with bf16 activations, cost the same 0.155 s — and neither is
the submission count. `dispatch_probe` prices the difference:

| eight dispatches | one step | eight steps |
|---|---|---|
| separate nodes | **0.066 ms** | 0.062 ms each |
| chained nodes | **1.428 ms** | — |

A dependency **inside** a step costs about **0.19 ms** — the barrier the graph runner inserts between
the producer and its reader — while a submission's fence costs a fraction of that. A composed layer
is a chain of eleven, so it pays ten of those; the staged path pays fences instead and wins.

The next capability is not more granularity in the pack and not fewer submissions; it is **cheaper
synchronization inside a step**: a barrier scoped to the buffer that needs it, or a fused kernel
that removes the dependency altogether (the tiling/plan path exists for a literal extent). Until
then the staged resident path — 648 submissions, 0.136–0.152 s — is the floor.

## The fusion path is installed, and it divides the graph the way the claims do

The serve's `record` installed only the automatic tiling pass; the **region merge** — the pass that
fuses a producer and its consumer into one kernel — had never run in this path. It is installed now
(the floor is the merge, then the tiling). What it does here:

- a plain `copy -> copy` chain is **not a region**: the merge derives regions only from fragments
  with a claim, and these have none (`merge_probe`);
- with the scale of the pre-attention composite claimed (`claim:Elementwise(element)` plus the
  `~ "element"` mark), the derivation joins the claimed nodes **across** the unclaimed rotation, and
  the composite derives **one region** (nodes 0–4 and 6–9). The matvec claims the element role
  too, so the region **derives a schema**; the merge then refuses it with `NotAReductionChain`,
  because the norm feeds three projections and the floor's own proof holds a reduction chain only.
  A `fuse` pack naming the region's hash (`sha256:7c4649db…`, `fuse_probe`) licenses the merge, and
  the composition refuses it with `ShapesDiffer`: the composite's kernels declare a third scalar
  leaf, and a producer is composable only when its domain is exactly `(extent, index)`. The tiling
  pass, separately, refuses the Elementwise kernels by name (`UnknownInterface`).

The bridge is the limit behind the rotation: claiming it made the whole graph one region (nodes
0–10) and the schema derivation then refused at the rotation's element function —
`ProjectionNotBridged`: *selects between two values, and the symbolic language has no conditional*;
rewritten branch-free (half = j/64, sign = 2h − 1), the refusal moved to *it computes ValueId(19)
in the machine-integer class … `Int` wraps and `f64` does not hold every value* — the **integer
division**. The rotation's gather (`x[i ± 64]`) is the deeper reason: the bridge models a read as
the value at the element's own position, so a lane-dependent read has no port. That is the one
limit that is the language's rather than the derivation's.

## How a region is derived, and where the abstraction is coarser than the dataflow

The pipeline, in the order the code runs it:

1. **Marks** — a kernel's function parameter carries `~ "claim:Interface(role,…)"`
   (`compile_claim`), and the body's `~ "combine"` / `~ "element"` marks are published into the
   fragment's marks. A kernel with no claim is in no row of the registry join.
2. **Claim** — the compile stores the fragment and the claim together (`intern_kernel`), and the
   passes read the registry, never the kernel value.
3. **Regions** (`fusion_claim/schema.rs::region_positions`) — the graph is walked in evaluation
   order; the claimed nodes form the run and an unclaimed node belongs to no region and is **not**
   a separator. The run is split into weakly-connected components over the producer/consumer
   relation (`components`): a node joins the component of a producer it reads, and an extent value
   counts as a read. A claimed consumer therefore joins a claimed producer across an unclaimed
   node.
4. **Schema** (`derive_regions`) — each component becomes a `RegionSchema` (ports, roles) through
   the claims, or a refusal.
5. **Licence and merge** — a region needs a licence (a `fuse` pack naming its schema, or a
   reduction chain the pass can prove), then the merge's legality checks, then `compose`.

Two places were **coarser than the dataflow**; the first is fixed, the second is fixed too:

- **The run split (fixed).** It used to sever a claimed consumer from a claimed producer across an
  unclaimed node. `region_positions` no longer flushes at an unclaimed node, so the pre-attention
  graph now derives **one region** (nodes 0–4 and 6–9) with the rotation outside it, and
  `merge_probe` shows the other side: an all-unclaimed chain still derives no region.
- **The shape set (fixed).** The classifier admitted a pair, a chain, a fan-out or a fan-in —
  shallow shapes, depth at most two — and the composition had one `compose` per shape. The merge
  now **folds over the region's DAG**: a node reading the body before it is a chain step, a node
  reading several producers is a fan-in over the branch's own bodies, and a node several consumers
  read is a fan-out over the branches it splits the region into. A fan-out **of chains** (the norm
  feeding three projections, each followed by a norm) is one of those folds.

The claim itself is not the coarse part: membership through a claim is the right predicate (a body
no claim describes has no algebra to read), and the merge's fan-out composition exists. What
remains is the **licence policy** and the **kernels' own domains**: the pre-attention kernels
declare a third scalar leaf beside the extent and the index, and `spliced_scalars` admits a
producer only when its domain is exactly `(extent, index)`, so the fold refuses the region with
`ShapesDiffer` before it composes a body. The rotation's lane-dependent gather is the one limit
that is genuinely in the language, because the bridge models a read as the value at the element's
own position.

## The edges now

1. **Submissions are the floor.** A resident token is 648 submissions (420 stage steps, the rest
   fetches) at **~0.2 ms each**. A one-node step's floor measures **0.366 ms** with a 4 KB buffer,
   and the projection probe reads 0.3 ms at 8 rows — the same order. At 648 submissions the token
   *is* its submissions: 648 × 0.2 ms = 0.13 s. The capability is a **step that holds a whole
   layer** (the language and recorder already build one: `LAYER_STEP`, `BLOCK`); with the class rule
   fixed, what remains is the serving-side composition. Per-node packs are still needed if the
   composed step wants bf16 weights beside f32 intermediates; an all-float program with bf16 weights
   converted at binding works today.
2. **Prefill is serial per position.** Time to the first answer is **4.3 s at 8 tokens, 15.3 s at
   32, 60.0 s at 128** on the host-held path — 0.47 s a position, the layer pass repeated. The
   capability is a **layer pass over a block of positions** with a causal attention; the projection
   already takes rows, the attention takes one query row and does not take a block.
3. **The KV cache is host memory, re-uploaded every layer every token.** `boundary_probe`: an
   attention step is **0.593 ms at 64 positions and 1.389 ms at 256**, the 1.6 MB of extra cache
   accounting for ~0.8 ms of it. At 2 048 positions a layer's cache is 8 MB a row, 64 MB a token a
   layer at eight rows. The capability is a **resident or paged cache**, written by the append and
   read in place.
4. **Load time is a compile a tensor.** `place` prepares its `UPLOAD` step on every call, so
   `reside(28)` + `place_head` is 309 compilations, and they dominate the load. The capability is a
   **prepare cache** keyed by source and pack, which a server that loads once and serves for hours
   wants anyway.
5. **The head is now small.** Resident, it is 0.011 s a token; a two-stage head (a top-k over
   blocks, then the survivors' logits) would matter only after the step count does.

## The list, ranked by measured share

| # | capability | edge it moves | measured now |
|---|---|---|---|
| 1 | a cheaper intra-step barrier, or a fused kernel | the 0.19 ms a dependency costs | chained 8: 1.428 ms vs 0.066 ms |
| 1a | the symbolic bridge needs a conditional or integer division (or a real-only rotation) | fusion that fires | one region, `ProjectionNotBridged` |
| 2 | a layer pass over a prompt block + causal attention | prefill | 0.47 s a position |
| 3 | a resident or paged KV cache | long context | 0.593 → 1.389 ms at 64 → 256 |
| 4 | a prepare cache | load time | 309 compiles a model |

The class granularity and the per-node pack are **landed**; they are no longer an edge.

## Already at its floor

The dispatch count, the readbacks and the narrow/wide class question: the rotation and attention
batching cut 64% of the submissions for 15% of the clock; the readbacks are 3%; the per-buffer class
conversion is landed ([serving-frontier](serving-frontier.md)). What is left is the **submission
count** and the **bytes a step must read**, and residency settled the second: a resident projection
reads 2.82 GB a token in 0.0625 s, ~45 GB/s of effective read.
