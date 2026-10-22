# A launch that is not one row: a proposal

> Status: **proposal, first slice landed.** Nothing here is implemented except §6's first
> slice, which is written up in §8 and carries the trace §7 left open. This is the measured case
> for the last
> wall named in [llm-operators](llm-operators.md) §"What attention needs, in order", and the
> standing rule for a slice this size is to measure and propose first.
> Points at: `crates/lichen-kernel-ir` (`KernelFragment`, `TargetFacts`, `Plan`, `ParallelBackend`),
> `crates/lichen-graph-ir` (`Count`, `KernelNode`, `Runner`, `Policy`, `GraphRefusal`),
> `crates/lichen-compute` (`parallel_roles`, `record_launch`, `run_parallel_kernel`,
> `run_parallel_range`, `build_graph`), `crates/lichen-compute-gpu` (`spirv.rs`, `dispatch.rs`),
> `crates/lichen-runtime` (`graph_pass.rs`, `automatic_tiling.rs`), `crates/lichen-fusion-bridge`
> (`tiled.rs`), `crates/lichen-compute/src/compute/body.rs` (`lower_index_function`).
> Related, not restated: [graph-fusion](graph-fusion.md) (the plan/pack schema),
> [code-audit](code-audit.md) (the decision record), [shared-memory-and-barriers](
> shared-memory-and-barriers.md) (the `Shared`/`Barrier` story and the tail-lane obligation),
> [compute-graph-jit](compute-graph-jit.md) (`Count::Device`), [compute-runtime-scalars](
> compute-runtime-scalars.md) (the leaf ABI a second extent would ride on).

**What is measured, what is inferred, what is unknown**, stated first because the separation
decides the size of the change:

- **Measured** (§2.1): a parallel kernel whose config declares **two** extents — `.n` and
  `.cols` — compiles and runs **correctly today, on both backends, bit for bit**. The
  two-dimensional *launch* is not unreachable; it is **unnamed**. The row/column split is a
  `Div` and a `Rem` the body already owns.
- **Measured** (§2.2): the **graph** path admits that second extent or refuses it, and **which
  one it does is decided by which input buffers the body reads** — an unrelated fact. When it
  admits, it **drops the extent silently** and the run then refuses naming the wrong thing.
- **Inferred from the code** (§1): of the sites this proposal found, **twelve are
  one-dimensional by construction** (§1.1) and **twenty-five are one-dimensional by convention**
  (§1.2). The by-construction set is short and it is all *types and emitted geometry* —
  `KernelShape`, the parallel `param_shape`, the two leaf-counting helpers, the role walk,
  `lower_index_function`, `Range`, and the SPIR-V `LocalSize` mode — which is what makes a
  **2-D launch with a 1-D body** a cheap slice and a **2-D workgroup** an expensive one.
- **Unknown** (§7): what a rectangular tail-lane obligation costs, whether the `vec3` the
  emitter already binds is enough for a real 2-D workgroup, and what a second dimension does to
  the artifact hash contracts.

## 1. Every site that assumes one dimension

The distinction the whole proposal turns on: **by construction** means the type or the emitted
module has nowhere to put a second dimension, so admitting one changes a type; **by
convention** means a single `usize`/`u32` stands where a pair could, and admitting a second
dimension is an additive field plus whoever reads that field.

### 1.1 By construction

| site | what it assumes |
|---|---|
| `lichen-kernel-ir/src/lib.rs:616-626` — `KernelShape` | "A kernel domain is only ever a scalar or a **tuple of scalars**". There is no shape that says "two extents"; a second dimension would have to be a new variant or a reserved leaf. |
| `lichen-compute/src/compute.rs:2449-2455` — the parallel fragment's `param_shape` | `KernelShape::Tuple(scalar_classes ++ [Scalar(Int)])` — "The parameter's leaves then the index; the buffers are bound, not passed". The **last** leaf is the index by position. |
| `lichen-kernel-ir/src/lib.rs:1052-1062` — `scalar_leaves()` | "**The index is the last leaf** and is not a launch value". `flat_arity() - 1`. |
| `lichen-kernel-ir/src/lib.rs:1064-1073` — `runtime_scalars()` | "its leaves but the extent and the index" — `scalar_leaves() - 1`. The ABI's scalar block is *all* remaining leaves. |
| `lichen-kernel-ir/src/lib.rs:995-998` and `:1010-1011` | "a parallel fragment's shape is `(config, index)` however many buffers it reads" — **the sentence that is the wall**, restated verbatim. |
| `lichen-graph-ir/src/refusal.rs:85-88` | the same sentence again, in the graph's `InputArity` invariant. |
| `lichen-compute/src/compute.rs:1893-1941` — `parallel_roles` | a field named `in` is `Input`, one named `out` is `Output`, **everything else is `LeafRole::Scalar`** (`:1932-1938`). There is no role for "the second extent". |
| `lichen-compute/src/compute.rs:1820-1862` — `scalar_leaf_classes` | "**The first leaf is the launch extent**, read as an `Int` ordinal whatever the parameter declares", refused by name at `:1856-1860`. Leaf **order** is the ABI. |
| `lichen-compute/src/compute/body.rs:231-236` — `lower_index_function` | `domain_arity(params)` block params, then **one** more: "The fragment's domain is the config's leaves plus the invocation index, which no shape contains". |
| `lichen-compute/src/compute/body.rs:767` — `ComputeOperator::Range` | lowers to the block param at `sum of flat arities` — **one scalar, always**, and the operator's own argument is never read. |
| `lichen-compute-gpu/src/spirv.rs:3130-3136` — `EXECUTION_MODE_LOCAL_SIZE` | `vec![ids.entry(), EXECUTION_MODE_LOCAL_SIZE, lane_width, **1, 1**]`. The two `1`s are the dead dimensions, and they are the module's own geometry. |
| `lichen-compute-gpu/src/dispatch.rs:332-340` | `workgroups = count.div_ceil(lane_width)`, and `StagedLaunchNotAWholeWorkgroup` (`dispatch.rs:58`) bounds it in **one** axis. |

### 1.2 By convention

| site | what it assumes |
|---|---|
| `lichen-compute/src/compute.lichen:13` | `range = x: Int => $range(x): Int`. The **argument is typed and then ignored** by the lowering. `compute.range k.cols` checks and answers the *invocation*. |
| `lichen-compute/src/compute.rs:6970-6976` — `RangeOp` | "a parallel fragment's **two** scalar parameters are both that class" — a sentence written for a config that has exactly two scalars, which a config with `(n, cols)` already has. |
| `lichen-compute/src/compute.rs:5749-5757` — `run_parallel_kernel` | "The ABI's first leaf is the launch extent, the one ordinal this side needs"; `count = leaves.first()`. |
| `lichen-compute/src/compute.rs:5623-5629` — `run_on_installed_backend` | `runtime_scalars = leaves[1..]` — **every** leaf after the first is a scalar, by `zip`, with no bound on how many. |
| `lichen-runtime/src/graph_pass.rs:16` — `LaunchExtent<'a>` | `dyn Fn(usize) -> Option<usize> + 'a` — **one number per node**. A pass cannot be asked for two. |
| `lichen-compute/src/compute.rs:5310-5319` — the pass's closure | `Count::Constant => Some(count)`, `Count::Value => …`, **`Count::Device => None`**: "The number is on the device, so this host has no answer to hand a pass." A device-held extent is invisible to *both* passes. |
| `lichen-graph-ir/src/graph.rs:244-260` — `Count` | three arms, all naming **one** number. `KernelNode.count` (`:231`) is one. |
| `lichen-graph-ir/src/run.rs:285-329` — `Count` → `BatchExtent` | one `BatchExtent` per node; `BatchExtent::{Host, Resident, Produced}`. |
| `lichen-graph-ir/src/run.rs:19-41`, `:109-112`, `:346+` — `Policy::{Serial, Async, Batch}` | every policy dispatches one `count` per node. `Serial` and `Async` differ in **when the host waits**, not in geometry, so a 2-D extent would ride through them unchanged. |
| `lichen-compute/src/compute.rs:4677-4736` — `record_launch`'s extent | reads `roles.scalars.first()` and nothing else. **This is the gate a second extent meets** (`:4738-4749`). |
| `lichen-compute/src/compute/graph.rs:215-219` vs `:375-383` | the graph's input **width** is `rposition(Option::is_some) + 1`, while the allocator's **value numbers** are dense over occupied slots. Two numbering rules over one table — see §2.3, where they disagree. |
| `lichen-compute/src/compute.rs:4844` — `build_graph`, `:5302-5324` — the run | the pass is consulted with the same one-`usize` closure. |
| `lichen-compute/src/compute.rs:5683-5691` — `MAX_PARALLEL_ELEMENTS` | `1 << 20` bounds **elements**, not `rows × cols`; a flattened launch pays it in full. |
| `lichen-compute/src/compute.rs:5832-5835` — `chunk_bounds` | "Contiguous chunk bounds, a function of the **count** and worker count alone" — a 1-D decomposition of `[0, count)`. A 2-D launch would *keep* it: `id = base + j`, and the body splits as in §2.1. |
| `lichen-kernel-ir/src/lib.rs:576-596` — `LaneWidth(u32)` | one number: "the lane count one workgroup runs". A 2-D workgroup needs a *pair*, and the pair is `TargetFacts`. |
| `lichen-kernel-ir/src/lib.rs:958-974` — `Slice { class, count, space }` | one array per space, counted in one dimension. |
| `lichen-kernel-ir/src/plan.rs:64`, `:110-137` — `Plan::staging[].group_width` | "the plan stages `{name}` in a group of {asked} lane(s) and **one workgroup runs {lane_width}**… a group narrower than the workgroup needs a barrier narrower than the block, which lichen does not have". **A 2-D workgroup falsifies this sentence.** |
| `lichen-fusion-bridge/src/tiled.rs:345-346`, `:380`, `:399` | "Each lane folds `[lane, lane + lane_width, …]` for `ceil(extent / lane_width)` steps"; `stride: lane_width`; `modulo: Some(lane_width)`. One axis, and it is the axis of the slice. |
| `lichen-runtime/src/automatic_tiling.rs:274-280` | the count a tile needs is derived from `target.lane_width` alone. |
| `lichen-compute-gpu/src/spirv.rs:179-186` — `LOCAL_SIZE_X` | "A run of `count` indices is `ceil(count / LOCAL_SIZE_X)` workgroups". |
| `lichen-compute-gpu/src/spirv.rs:1212-1215` | a fragment whose `lane_width` is not `LOCAL_SIZE_X` is refused (`UnsupportedLaneWidth`). |
| **`lichen-compute-gpu/src/spirv.rs:3190-3193` and `:1255-1278`** | **the emitter binds `SpvBuiltInGlobalInvocationId` — a `vec3<u32>` — and reads component `0`**. `LocalInvocationId` and `WorkgroupId` are not bound at all. The second and third components are already there. |
| `lichen-runtime/src/wasm/lower.rs:519-531` — `staging_refusal` | "`run_parallel_range` calls a kernel body **once per index**, never per workgroup**, so no barrier has a group to synchronise". A host-side 2-D launch would *not* change this sentence; a 2-D *workgroup* still would not exist on wasm. |
| `lichen-compute-gpu/src/dispatch.rs:841-855`, `:864+`; `lichen-kernel-ir/src/lib.rs:432-466` | `ParallelBackend::run` / `submit` / `run_batch` and `GpuContext::run` / `submit` / `run_over_device_extent` all take one `count: usize`. **There is no compute-side `spawn`** — these are the launch signatures. |

**The short form.** By construction: `KernelShape`, the parallel `param_shape`, `scalar_leaves` /
`runtime_scalars`, the role walk, `scalar_leaf_classes`, `lower_index_function`, `Range`,
`EXECUTION_MODE_LOCAL_SIZE`, and the dispatch's workgroup arithmetic. By convention: every
`usize`/`u32` that stands for "how much", every `.first()` / `leaves[1..]`, `LaunchExtent`,
`Count`, `LaneWidth`, `Slice`, `Plan::group_width`, the tiler's stride and modulo, and — the
cheapest one of all — the SPIR-V emitter's **component `0`**.

## 2. What was measured

A probe was run against the RTX 3060 and the "cpu" backend and then removed; nothing in the
tree changed and no test file remains. Every claim below is its output.

### 2.1 A two-extent launch already runs, on both backends

The kernel — `Par = struct{.n Int, .cols Int, .in In, .out Out}`, launched over `n = 6`,
`cols = 3`, reading `in.a[r * 2 + c]` where `r = i / cols` and `c = i % cols`:

| backend | answer | expected |
|---|---|---|
| `"cpu"` (the interpreted path) | `[1, 2, 3, 3, 4, 5]` | `[1, 2, 3, 3, 4, 5]` |
| `"gpu"` (RTX 3060, SPIR-V) | `[1, 2, 3, 3, 4, 5]` | identical |

The transposed spelling — the same launch writing `out.z[c * 2 + r]` — answers
`[1, 4, 2, 5, 3, 6]` on the device. **The flattening is the body's own choice**, not an
artifact of the lowering: nothing above the body knows a second dimension exists.

This settles the question the ladder left open. **`compute.range` is one dimension and the
parallel fragment's shape is `(config, index)`, and yet a two-dimensional launch runs.** What is
missing is not the capability; it is a *name* for the split, so that the `Div` and the `Rem` are
spelled by the language rather than written by hand, and a place for the second extent that the
graph, the passes and the tiler can all read.

### 2.2 Where a two-entry config shape refuses — and it is not where the code says

Three graph-path probes over the *same* signature `Par = struct{.n Int, .cols Int, .in In,
.out Out}`:

| config / body | recording result |
|---|---|
| `(n, cols)`, `.in.a (compute.Buf Int)`, body **reads** `.in.a` | **no line at all** — admitted |
| `(n, cols)`, `.in.a (compute.Buf Int)`, body **ignores** `.in.a` | refused: *"this dispatch's kernel parameter declares 2 runtime scalars, and a recorded body carries the extent alone: a runtime scalar would have to be an edge of the recording, and that is not written yet"* (`compute.rs:4738-4749`) |
| `(n, cols, rows)` | refused with the same sentence, naming **3** |
| two input buffers `.in.a`, `.in.b`, body reads **both** | **no line** — admitted |
| two input buffers `.in.a`, `.in.b`, body reads **only `.in.b`** | **refused** — same sentence |

The last two rows are the control, and they are the finding: with the parameter type held
**identical**, whether the second extent is admitted is a function of **which input buffers the
body reads**. `record_launch` counts `roles.scalars.len() - 1`
(`compute.rs:4738`), and a buffer the body never reads is counted as a runtime scalar instead.
So the rule the recording actually enforces is
not "a config may carry one extent plus one scalar"; it is "a config may carry one extent, plus
whatever number of leaves the body's read pattern leaves over", which is not a stated rule and
cannot be intended.

> **Corrected in §8, and the gate was not the cause.** The walk does not *move* an unread
> buffer — it reads a **different type term**. A body that reads nothing under `.in` leaves
> every cell in that group undecided, and the walk's `Unknown` arm read every one of them as a
> runtime scalar. A body that reads even one buffer there resolves the whole group, which is
> why the two rows above disagree with the parameter held identical.

**This is a defect worth its own line in `code-audit.md`**, independent of any two-dimensional
work: an admitted config silently loses a value.

### 2.3 What "admitted" costs — the silent drop

When the recording does admit `(n, cols)`, `record_launch` reads `roles.scalars.first()` as the
count and **never makes an edge for `cols`** (`compute.rs:4677-4736`). Meanwhile the graph's
argument slots are assigned by the parameter scan in field order, so the width counts `cols`
(`graph.rs:215-219`, `rposition(Option::is_some) + 1`) while the allocator numbers values
**densely over occupied slots** (`graph.rs:375-383`) and therefore skips it. The run then
refuses:

> `compute.graph: this graph could not be run: node 0 was given value 1 as a buffer, and that
> value is a number. A number is not a one-element buffer, and reading it as one would run the
> dispatch against data nobody wrote.`

Two things are wrong with that sentence, and both are load-bearing for this proposal. It is a
**run-time** refusal for what is a **build-time** mistake, and it names a **number read as a
buffer** when the value that vanished was the second extent. A 2-D extent slice that made this
path *more* reachable without fixing the recording would turn a loud-but-misleading refusal into
a schedule-dependent wrong number.

### 2.4 What a `Count::Device` extent does today

Read, not measured, and stated because it constrains the plan:

- `graph-ir/src/run.rs:303-328` resolves `Count::Device { value, index }` into
  `BatchExtent::Produced` or `BatchExtent::Resident`: the **buffer is the count**, `index` is
  the element to read, and outputs are allocated for `capacity` rather than for a number.
- `run.rs:341-345`: under `Policy::Async` a device-held extent **is refused** — it "has one
  entry in the contract and it waits", so it is not submitted as a stall wearing a submission's
  name.
- `kernel-ir/src/lib.rs:468-498`: the **default** `run_over_device_extent` **fetches the number
  to the host** and calls `run` — "Only an override pays nothing for it — the default's read is
  the stall this exists to avoid."
- `compute-gpu/src/dispatch.rs:799-813`: the GPU override; "the workgroup count is handed to
  the queue as a buffer offset".
- **`compute.rs:5310-5319`: the pass closure answers `None` for `Count::Device`.** A node whose
  count lives on the device is therefore invisible to `AutomaticTiling` and to `region_merge`
  alike. A two-dimensional extent on that path would be invisible to both.

## 3. The smallest change that makes a two-dimensional launch demonstrable

There are three candidates, and they differ by an order of magnitude.

**(a) A 2-D launch extent with a 1-D body — an outer/inner split the body sees as two indices.**
This **already works** (§2.1). What is missing is that the split is written by hand: `r = i /
cols; c = i % cols`, with `cols` as an ordinary runtime scalar. The minimum here is a *name* —
a `compute.row`/`compute.col` pair, or a second extent the language states — plus the recording
fix in §2.3 so the second extent survives the graph path. Evidence that it works: the probe in
§2.1, turned into an `examples/*.lichen` operator that **records and runs through
`compute.graphrun` on the device** and answers the CPU result element for element. Nothing
above the body moves: no `Slice`, no `Barrier`, no `Plan`, no `TargetFacts` field, no emitter
change.

**(b) A genuinely 2-D workgroup.** `EXECUTION_MODE_LOCAL_SIZE` (`:3134`) becomes a pair, a
second built-in (`LocalInvocationId`, `WorkgroupId`) is bound and read, `Slice` counts per lane
in two dimensions, `Plan::group_width` becomes a pair, the tiler's stride and modulo become
strides, and `TargetFacts` grows a field — which by `fragment_digest`'s own invariant
(`kernel-ir/lib.rs:1076-1097`, "Every field is hashed") changes **every kernel id in the tree**.
Evidence: the same operator answering on the device, plus a `spirv-val` run and a staged-tile
answer that the 1-D flattening cannot produce (a `Shared` exchange across a **row**, where the
row is `LocalInvocationId.y`).

**(c) Something else.** A `Count` carrying a pair, or an `Extent::Value` per axis. This is (b)
with the backend left behind, and it buys nothing until the emitter can use it.

**Recommendation: (a).** It is demonstrable on its own, it needs no new type in any backend, and
it is the only one of the three that does not stand on the tile-equivalence claim that
`llm-operators.md:182-184` says is bit-for-bit **only** for `max`/`min`.

## 4. What a full implementation would break

- **`TargetFacts` and therefore every kernel id.** `TargetFacts { int_width, lane_width, lane_height }`
  (`kernel-ir/lib.rs:606-636`) is hashed whole, so the workgroup height is in every fragment's
  `fragment_digest`. **Landed**: the height is carried, `FORMAT_VERSION` went `1` → `2`, and
  `DeviceFacts` reads `maxComputeWorkGroupSize[1]` as `max_workgroup_lanes_y`
  ([two-dimensional-workgroup](two-dimensional-workgroup.md) §6). What is left of the bullet is the
  **digest**: every kernel id in the tree is new, so every cached artifact and every region hash
  moves. `lichen-runtime/src/fused/schema.rs:310-321` mirrors it as `Target` with
  `#[facet(deny_unknown_fields)]`, and the pack text `"target": { "int_width": "i64",
  "lane_width": 64, "lane_height": 1 }` is pinned by `crates/lichen-compute/tests/lichen_compute/fused_landing.rs:46`
  and `fused_kernel.rs:167`; `crates/lichen-compiler/tests/pack_install.rs:104` pins a
  `"group_width": 64`, which the landed slice deliberately leaves alone.
- **`Plan::staging[].group_width: u32`** (`kernel-ir/src/plan.rs:64`) is the pack schema's
  staging width, and `check_plan` (`plan.rs:110-137`) compares it to `target.lane_width`. A
  pack that states a group width would need a shape that can say "16 × 4". `pack_placement.rs:217`
  and `plan_staging.rs:200` construct it directly.
- **The launch signatures.** `ParallelBackend::{run, submit, run_batch}`
  (`kernel-ir/lib.rs:432-466`) and `GpuContext::{run, submit, run_over_device_extent}`
  (`dispatch.rs:790-855`) each take one `count: usize`. A 2-D extent is a second parameter or a
  pair, and `run_batch`'s `BatchRefusal::Cannot` (`lib.rs:464-466`) is the door every other
  backend would have to refuse by.
- **The tests that pin one-dimensional arithmetic.** `LOCAL_SIZE_X + 5` in
  `compute.rs:1531` (the surplus-lane test), `tiled_count` / `tail` in `tiled_reduction.rs:404`
  and `reduction_chain.rs:369`, `StagedLaunchNotAWholeWorkgroup` in
  `dispatch.rs:58,147-153,337`, `device_extent.rs`'s whole-file invariant (its count is
  produced on the device and never crosses the bus).
- **The examples' declared outputs.** No `examples/*.lichen` would fail — `(n)` stays legal — but
  a new `.rows`/`.cols` spelling moves every row of the generated README tables, and
  `cargo test -p lichen-compute --test lichen_compute examples` rewrites `README.md` as a side effect.

**Documented contracts to grep before changing any of this** — none of them forbids a 2-D
launch, and two of them constrain it:

- `code-audit.md` **`D17`** (`wontfix`, P1-42): "**a `compute.write` is not an effect, it is the
  value of an output position**". A 2-D launch does not touch this, but `code-audit.md:1084-1090`
  states that the decision's scope is **global buffers** and that "whoever lands step 4 has to
  decide it again for a `block[1]` scope". A 2-D workgroup that stages is that case, and this
  proposal must not be read as pre-empting it.
- `code-audit.md` **P1-39** (`todo`): "A `compute.call` inside a **parallel** kernel body is
  refused… so the device has no working call route". A 2-D body is still a parallel body, so
  **matmul stays blocked on this too** — the two are independent walls and §6 does not claim to
  clear both.
- The `# Invariant`s in the crates, named in §1.1: `KernelShape`, `scalar_leaves`,
  `runtime_scalars`, `scalar_leaf_classes`, `lower_index_function`, and `LocalInvocationId`'s
  absence. **Changing any of them needs permission**; none is changed here.

## 5. The contract changes a full implementation would need

Named one by one, with the sentence that would have to change. **None of them is made here.**

1. **`kernel-ir/src/lib.rs:995`** — "It is not in `param_shape` and cannot be: a parallel
   fragment's shape is `(config, index)` however many buffers it reads, because they are bound
   storage buffers." → must say what a second extent is.
2. **`kernel-ir/src/lib.rs:1010`** — "A parallel fragment's shape is `(config, index)` whatever
   its buffers' classes, so a class that lived only on the parameter leaves could say nothing
   about a buffer." → the same, one sentence over.
3. **`graph-ir/src/refusal.rs:86`** — "The count comes from `KernelFragment::inputs`, not
   `param_shape`, because a parallel fragment's shape is `(config, index)` however many buffers
   it reads". → restated a third time, in a fourth crate.
4. **`kernel-ir/src/lib.rs:1056-1059`** — "**The index is the last leaf** and is not a launch
   value… A backend that pushed a different number would place one leaf's value under another."
   → with two indices, the last-leaf rule is what has to go.
5. **`compute.rs:1823`** — "The first leaf is the launch extent, read as an `Int` ordinal
   whatever the parameter declares." → and `compute.rs:1856-1860`'s refusal sentence.
6. **`body.rs:234-235`** — "The fragment's domain is the config's leaves **plus the invocation
   index**, which no shape contains." → plus *two*.
7. **`spirv.rs:182-185`** — "A run of `count` indices is `ceil(count / LOCAL_SIZE_X)` workgroups,
   and the surplus lanes of the last one are a caller obligation, not the shader's. It is also
   the only width this target schedules." → two axes, and the refusal at `:1212-1215` with it.
8. **`plan.rs:186-191`** — "the plan stages `{name}` in a group of {asked} lane(s) and one
   workgroup runs {lane_width}… a group narrower than the workgroup needs a barrier narrower
   than the block, which lichen does not have". → a group that can be *narrower in one axis and
   equal in the other* is a shape this sentence has no word for.
9. **`wasm/lower.rs:522-523`** — "`run_parallel_range` calls a kernel body once per **index**,
   never per workgroup, so no barrier has a group to synchronise and a slice has no lifetime
   between indices". → still true, and the reason a host-side 2-D launch is free while a 2-D
   *workgroup* is not.
10. **`kernel-ir/src/lib.rs:1079-1082`** — "Every field is hashed, and a field left out is a way
    for two fragments that differ in it to share one identity". → unchanged in wording, but a
    field added to `TargetFacts` **invalidates every id**, so this is a migration, not an edit.
11. **`code-audit.md`'s `D17` scope paragraph** (`:1084-1090`) — "Whoever lands step 4 has to
    decide it again for a `block[1]` scope, and the honest default there is the opposite one".
    → not a change this proposal may make; a **hand-off** to whoever does.
12. **The new sentence the graph path needs**, which does not exist today: what a graph records
    for a leaf that is neither the extent nor a buffer. `compute.rs:4738-4749`'s own text says
    it — "a runtime scalar would have to be an edge of the recording, and that is not written
    yet" — and §2 shows the gate is not where that text claims it is.

## 6. An ordered plan, and the one slice to take first

**First slice, and the only one I recommend taking before anything else: carry one runtime
scalar as a graph edge.** Concretely — `record_launch` makes an `Edge` for `roles.scalars[1]`,
`RecordedNode`/`KernelNode` carry the edge beside `count`, `Runner::plan` hands it to the backend
the way it hands `inputs`, and the admission rule at `compute.rs:4738` becomes *stated*: one
extent plus one scalar, or refuse.

**What it proves, on its own:**

- the probe program of §2.1 **recorded and run through `compute.graphrun` on the device**,
  answering the CPU result element for element — which is the evidence `llm-operators.md` asks
  for from every operator, and the first time a two-dimensional *shape* would have it;
- §2.2's accident becomes a rule: the admission stops depending on which buffers the body reads,
  and both rows of the control answer the same way;
- §2.3's silent drop is gone, because there is nothing left to drop.

**What it must leave for the next slice:** the workgroup geometry. No `TargetFacts` field, no
`Slice` change, no `Plan` change, no emitter change, no `LOCAL_SIZE_X` arithmetic. It moves
`Count`, `KernelNode`, `Runner::plan` and the recording — four places, all of which are
**one-dimensional by convention** — and none of the by-construction sites in §1.1. That is the
point: the next slice can then widen the *claim* (a real 2-D workgroup, with staging) on a
plumbing that is already measured, rather than widening both at once.

**After that**, in order, each independently demonstrable: (i) the language's spelling —
`compute.row` / `compute.col`, or a second extent the config states — with the existing
examples untouched; (ii) the emitter's second dimension, bound and read from the `vec3` already
at `spirv.rs:3192`, with `LocalSize` (x, y, 1); (iii) the **rectangular** tail-lane obligation,
which is `StagedLaunchNotAWholeWorkgroup` generalised from one axis to a rectangle
(`dispatch.rs:58,337`); (iv) `Slice`, `Plan::group_width` and the tiler's stride/modulo;
(v) only then `TargetFacts` and the digest migration.

## 7. The risks

**The silent wrong answer.** Two of them are already reachable, which is why the first slice is
the recording and not the geometry.

- §2.3: an admitted second extent **vanishes**. The run refuses, but late and naming the wrong
  thing. Any slice that widens the surface here without fixing the recording turns a refusal
  into a number.
- `compute.lichen:13` types `range`'s argument `Int` and `body.rs:767` **never reads it**.
  `compute.range k.cols` therefore checks and answers the *invocation*. A language that grows a
  second index makes that spelling natural to write and wrong to run.
- The strip-vs-transpose distinction of §2.1 is the hazard's shape in miniature: both spellings
  compile, both answer, and only the body knows which it meant.

**Widening a claim without proving it as tightly.** `tile ≡ body` is bit-for-bit only for
`max`/`min` (`llm-operators.md:182-184`). The tail-lane obligation is discharged today by
**padding the buffers** and is currently made unspellable by refusing a staged launch that is
not a whole number of workgroups (`dispatch.rs:58,147-153,337`;
`shared-memory-and-barriers.md:1083-1120`). With two dimensions that obligation is a
**rectangle**, and the existing refusal bounds one axis. Widening the launch without
generalising the refusal would make the surplus lane observable in a second direction — the exact
class that note says is currently unspellable rather than diagnosed after the fact.

**What I could not determine from the code, stated plainly:**

- ~~**Why** the role walk leaves an unread input buffer among the scalars.~~ **Traced; see
  §8.** The short form: the walk is not read-driven at all, but the *type term it reads* is
  only decided for the fields a body touches, and `walk_role`'s `Unknown` arm read an
  undecided leaf as a runtime scalar. A body that reads one buffer under `.in` resolves the
  whole group, so its unread siblings decode as buffers; a body that reads none leaves every
  cell open, so every one of them became a scalar.
- Whether the `vec3` the emitter already binds is **sufficient** for a real 2-D workgroup. It
  carries `x`/`y` of the *global* invocation id, which is what a flattened launch wants; a
  workgroup-shaped stage wants `LocalInvocationId` and `WorkgroupId`, which are not bound at
  all. Whether the staging story survives on the global id is a measurement nobody has made.
- What a second `TargetFacts` field does to the artifact-cache and pack-hash contracts
  (`artifact-cache.md`, `taking-a-kernel-to-a-server.md`). A field in `TargetFacts` is in every
  digest by `fragment_digest`'s own invariant, so the answer is "every id moves"; whether that
  is a migration or a break depends on a cache layout I did not read.
- Whether `Policy::Batch` (`graph-ir/src/run.rs:33-40`) can record two extents in one
  submission without the refusal at `run_batch`'s `BatchRefusal::Cannot`. Not probed.

## 8. §6's first slice, landed

**What it is.** One runtime scalar now travels as a graph edge. `record_launch` reads
`roles.scalars[1]` into an `Edge`, `KernelNode` carries it beside `count` as
`KernelNode::scalar: Option<Scalar>`, and `Runner::plan` hands it to the backend the way it
hands `inputs` — as a `ScalarLeaf` on the `LaunchSet` that already carried `inputs`, which is
what `dispatch.rs:1480` was already reading. **No backend signature changed**, because
`LaunchSet::with_leaves` and `KernelFragment::runtime_scalars` were already in the contract and
already one long.

**The §2.1 measurement, both backends.** The probe of §2.1 — `Par = struct{.n Int, .cols Int,
.in In, .out Out}` over `n = 6`, `cols = 3`, reading `in.a[r * 2 + c]` — answers
`[1, 2, 3, 3, 4, 5]` on the `"cpu"` backend, on the device through `compute.plrun`, **and now
through `compute.graphrun` on the device**, which is the first time a two-extent shape has been
recorded rather than merely run. A graph on the `"cpu"` backend is refused by the runner's own
rule (`graph.rs`), so the graph leg is device-only; the CPU leg is the direct path.

**The rule, stated.** `compute.rs`, at the gate: *the extent, then at most one runtime scalar,
then the buffers.* Two or more is refused by name; one is read at its own path and refused
there, by name, if it is not a number the build had or an argument.

**The silent drop, and one more.** §2.3's refusal is gone — the extent arrives and is used.
But the **allocator** was the deeper cause, and §2.3 did not name it: `Recording::inputs`
numbered a slot's value *as it was allocated*, so a parameter whose extents sit at slots a body
does not visit in order gave slot 1 a higher number than slot 2. The numbers are now assigned
once, in slot order, in `finish`. §2.2's control answered the same way in all three rows (a
body reading both buffers, only one, or neither), because the role table no longer depends on
the read pattern.

## 9. The config's leaves are carried for what they are (`c92b6471`)

**The rule §8 left behind is gone.** It read, at the gate: *the extent, then at most one runtime
scalar, then the buffers.* It is replaced by one statement, the `# Invariant` of `record_launch`
(`crates/lichen-compute/src/compute.rs`): *the first scalar leaf is the extent, and every other leaf
is carried for what it is. A leaf under `.in` or `.out` is a buffer; every other leaf is a launch
value, at whatever depth the role walk found it. The extent keeps its position — `scalar_leaf_classes`
reads the first scalar leaf as the count — so this bounds the kind of a leaf, never how many there
are.* Nothing else states it; `KernelNode::scalars`'s own invariant points at that one.

**Why the older contract survives.** The extent is still `roles.scalars.first()`, and `walk_role`
descends depth-first pushing as it goes, so nesting reorders nothing — `.meta.rows` is still leaf `0`.
`scalar_leaf_classes` is unchanged and still refuses a non-`Int` first leaf by name. What was a fact of
*the launch* ("the second leaf is the only runtime scalar") is gone; what was a fact of *the config*
("the first leaf is the extent") is untouched, so the count is now unbounded while the position is not.

**What a leaf becomes.** `record_launch` maps every scalar leaf after the first; `recorded_scalar`
reads each at its own path with its class from `leaf_classes()[ordinal]`, so a leaf is read at **its
own** class and not the extent's; `leaves_of` resolves the whole list onto the `LaunchSet`, whose ABI
was already one long — **no backend signature changed**. For
`{.meta (struct{.rows Int, .cols Float}), .in (struct{.a Buf, .b Buf}), .out (struct{.z Buf})}`:
`.meta.rows` is the count, `.meta.cols` is `scalars[0]` read as a `Float`, `.in.a`/`.in.b` are inputs
at depth 1, `.out.z` is the recorded output. Measured on the device: the nested config answers
`[105,109,113,117,121,125]` through `compute.graphrun`, **element for element** what the direct
`compute.plrun` leg answers, and a two-scalar `{.n, .K, .cols, …}` answers `[10,11,12,12,13,14]` the
same way.

**One silent wrong answer closed.** A `Float` scalar leaf used to cross the argument's **integer bits**
as a float, silently. It is now refused **at the leaf**, naming its own parameter position: *"a dispatch's
scalar leaf at parameter position 0.1 is declared Float and its value is a number the graph's table
holds, which is an integer: a launch reads a leaf at its own class, so this one would read those bits as
a float. Give the leaf a literal, or declare it Int."* `GraphRefusal::ScalarArity` mirrors `InputArity`
so a hand-built node with the wrong leaf count is refused **at the plan** rather than by the backend's
`ScalarLeavesMismatch`, and `region_merge` refuses the whole rebuild rather than `.flatten()`-ing one
scalar away.

**What is still not carried.** A `Float` scalar leaf can only be a **literal**: a graph's value table
holds `i64` (`run.rs`) and `compute.graphrun` classifies a `Float` argument as *"neither a buffer nor a
number"*. The honest fix is a `Float` arm on `Value` and on `RunArgument` — **a second contract change,
not taken**, because it changes the sentence *"a graph's value table holds the caller's arguments"* and
how `graphrun` sorts its arguments. Until it lands, an operator that wants `eps` or a scale as a
parameter must write it as a literal. Also: a scalar leaf whose expression cannot evaluate never reaches
the recording, so the refusal that surfaces is about the *function's return* — a late refusal naming the
wrong thing; and a **literal** leaf in the middle of a shape leaves a hole in the graph's argument width
(`rposition(occupied) + 1`), so `graphrun` wants an argument nobody reads.

## 10. The value table carries a float (`e24d00fb`)

§9's last paragraph ended with the one thing standing between "arbitrary scalars" and true: a `Float`
scalar leaf could only be a **literal**, because a graph's value table held `i64` and `graphrun`
classified a `Float` argument as *"neither a buffer nor a number"*. That is done, and the user
authorised exactly this one change.

**The sentence, before and after** (`crates/lichen-compute/src/compute/graph.rs`, `place()`'s `other`
arm — before: *"…and a graph's value table holds only the caller's arguments and the values the graph's
own nodes produced"*; after: *"…holds only the caller's arguments — a buffer, or a number that is an
integer or a float — and the values the graph's own nodes produced"*.).

**The class is the arm, so nothing re-reads bits.** `Value::Float(f32)` sits beside `Int(i64)` in
`crates/lichen-graph-ir/src/value.rs` — **beside, not a flag on** — and `Value::bits_at(class)` is the
one place a leaf meets a value; `RunArgument::Float(f32)`, `run_graph`'s `LowValue::Float(n) =>
RunArgument::Float(n)`, and `leaves_of` reading `bits_at(class)` are the rest of it. The class-mismatch
refusal §9 added **still fires, in both directions**, and neither direction reinterprets bits: *"…a leaf
declared Float, and that value is a number of class Int. A launch reads a leaf at its own class, so this
one would read those bits as Float rather than Int. Pass a value of the leaf's own class, or declare the
leaf Int"*, and the same with the classes exchanged. Two additions were forced by the existing code and
are worth naming: `RunResult::Float(f32)` (so `returned_role` stays exhaustive — **it has no test**,
because it is the round trip of the same arm) and `GraphRefusal::ScalarClassMismatch` (the kept refusal
relocated, since `ScalarNotANumber`'s sentence is wrong for a number of the wrong class).

**The proof it was impossible before** is the refusal the previous slice earned on the pre-change tree,
quoted verbatim in §9; the proof it works now is a `Float` **parameter**: a config
`{.n Int, .cols Int, .scale Float, .in In1, .out Out1}` with `scale = 1.5` answers
`[0.0, 1.5, 1.5, 3.0, 3.0, 4.5]` on `"gpu"` through **both** the direct `plrun` leg and `graphrun`,
element for element, recorded as `Count::Value(0)`, `scalars [Value(1), Value(2)]`, `inputs [3]`.

**And an operator uses it.** `examples/layernorm.lichen`'s `eps` is now a **parameter** (`k.eps`,
passed as `1.0`) instead of a literal, on the direct `cpu` path, with its declared output unchanged —
measured here, which is also the half the float slice did not measure: a `Float` leaf through `plrun`
on `cpu`. Driving it *through a graph* would mean moving that example's kernels to `"gpu"`, because
`graphrun` refuses the `cpu` backend.

**Still not carried.** A literal leaf in the middle of a shape still leaves a hole in the argument width
(`rposition(occupied) + 1`); a `Float` **extent** is still refused (the first scalar leaf must be an
`Int`); a leaf **computed inside the source function** still never reaches the recording, and the
refusal there stays accurate because a computed node is not in the table; and `f64` is inexpressible —
`ScalarClass::Float` is 32-bit. A class mismatch is now a **run-time** refusal rather than a
compile-time one.