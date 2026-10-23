# A workgroup that is not one row: a proposal

> Status: **§6's first slice has landed** (commit `fd494671`, `TargetFacts { int_width, lane_width,
> lane_height }`, `FORMAT_VERSION` 2, `max_workgroup_lanes_y`, and a by-name refusal for a workgroup
> that is not `x × 1`). **§7 is the rest, and §8 is what is still unknown.** §4's probe was run and
> removed, and the measurements §9 records were taken on that slice and their probes removed too.
> Everything §1–§5 below is re-read against `af634c14`, and the three places it is wrong are corrected
> in §9.
> Points at: `crates/lichen-compute-gpu/src/spirv.rs` (`OpExecutionModeLocalSize`, the two
> `Input` built-ins, `slice_pointer`), `crates/lichen-compute-gpu/src/dispatch.rs`
> (`launch_workgroups`, `device_facts`, the launch signatures),
> `crates/lichen-kernel-ir/src/lib.rs` (`TargetFacts`, `Slice`, `fragment_digest`),
> `crates/lichen-kernel-ir/src/plan.rs` (`Staging::group_width`, `check_plan`),
> `crates/lichen-fusion-bridge/src/tiled.rs` (`Folding::tiled`, `Fold { stride, modulo }`),
> `crates/lichen-runtime/src/device.rs` (`DeviceFacts::check`),
> `crates/lichen-runtime/src/fused/schema.rs` (`Target`, `FORMAT_VERSION`),
> `crates/lichen-runtime/src/placement_pack.rs` (the pack text).
> Companion, and the state of the art for everything *above* this: [two-dimensional-launch](
> two-dimensional-launch.md) — its §1 is a 12-and-25 list this note re-derives and corrects,
> and its §8–§10 have landed. Related, not restated: [shared-memory-and-barriers](
> shared-memory-and-barriers.md) §8 (the tail-lane obligation), [graph-fusion](graph-fusion.md)
> §5 (the plan and the pack), [code-audit](code-audit.md) (the decision record — `wontfix:D<n>`
> is a contract), [artifact-cache](artifact-cache.md) (the version precedent),
> [taking-a-kernel-to-a-server](taking-a-kernel-to-a-server.md) (the load gates),
> [llm-operators](llm-operators.md) (the ladder this unblocks).

**What is measured, what is inferred, what is unknown**, stated first because the separation
reorders the plan:

- **Measured** (§4): a **rectangular workgroup compiles, validates and runs** on the RTX 3060
  with `OpExecutionModeLocalSize = (16, 4, 1)`. The module contract is not the wall.
- **Measured** (§4): with a rectangle and the backend's own `workgroups = ceil(count / 64)`, a
  staged `Shared` exchange at `count = 64` answers `136 = 1 + … + 16` and writes **nothing past
  index 15** — a wrong answer, a truncated write, and **no error, no refusal, no report line**.
  The covered range shrinks by exactly the factor `Y`.
- **Measured** (§4): **binding** a second `Input` built-in is free and changes no answer
  (`2080` at all 64 positions, bit for bit). **Reading** it faults the Vulkan driver on this
  device — reproducibly, in five configurations, including one where the value is dead.
- **Measured** (§5): adding one field to `TargetFacts`, carrying its default value, moved a
  byte-identical fragment's digest from `1708622714001280166` to `6121588463028441745`. The
  `# Invariant` is exact, and the cost is **74 construction sites across 12 crates** plus a
  format-version bump.
- **Inferred from the code** (§1): of the sites that remain, **six are one-dimensional by
  construction** and **nineteen by convention**. The construction set is short, and it is *not*
  the set the launch note listed — three of its twelve have stopped being walls since §8–§10
  landed.
- **Unknown** (§8): why the driver faults; whether `WorkgroupId` behaves alike; whether a
  different recipe avoids it; whether `Policy::Batch` and `Count::Device` can carry an expr.

## 1. Every site that is still one dimension, re-derived

The distinction is the one [two-dimensional-launch](two-dimensional-launch.md) §1 sets: **by
construction** means the type or the emitted module has nowhere to put a second dimension, so
admitting one changes a type; **by convention** means a single `usize`/`u32` stands where a
expr could. This list is re-derived against `991e59bb`, not inherited.

### 1.1 By construction — six

| site | what it assumes |
|---|---|
| `lichen-compute-gpu/src/spirv.rs:3241-3255`, `:3630-3657` | A `Shared` slice is `OpTypeArray(scalar, len)` with **one** length constant, and `slice_pointer` emits `OpAccessChain(ptr_elem, p, variable, index)` with **one** index. A row-shaped exchange needs a nested array (`OpTypeStruct` over `OpTypeRuntimeArray`) and a **two-index** access chain. **This is the core construction site**: `Slice { count }` is not a summary of a rectangle, it is the emitted array's length. |
| `lichen-compute/src/compute/body.rs:249-254` | `lower_index_function` takes `domain_arity(params)` block params "**plus the invocation index**, which no shape contains". Two indices are plus *two*, and `KernelBody`'s parameter list is the ABI every backend binds positionally. |
| `lichen-compute/src/compute/body.rs:767` | `ComputeOperator::Range => self.parameter(domain_arity(self.params))` — **one** scalar, always, and the operator's own argument is still never read. |
| `lichen-kernel-ir/src/lib.rs:1052-1061` | "**The index is the last leaf** and is not a launch value". `scalar_leaves() = flat_arity() - 1` is the count a launch supplies; with two indices the last-leaf rule is not "the index", it is "the indices", and `spirv.rs:819-822`'s `index_local` reads one number. |
| `lichen-kernel-ir/src/plan.rs:14-21, 110-137, 186-191` | `Staging { name, space, group_width: u32 }`, and `check_plan` refuses `entry.group_width != lane_width`. The refusal's own sentence — "a group narrower than the workgroup needs a barrier narrower than the block, which lichen does not have" — has **no word** for a group that is narrower in one axis and equal in the other. |
| `lichen-runtime/src/wasm/lower.rs:519-531` | "`run_parallel_range` calls a kernel body **once per index, never per workgroup**". The host has no workgroup at all, in any number of axes. Unchanged by a 2-D launch; still the reason a 2-D workgroup is device-only. |

### 1.2 By convention — nineteen

| site | what it assumes |
|---|---|
| `lichen-compute-gpu/src/spirv.rs:1255-1278` | The invocation is `OpCompositeExtract` component **`0`** of the `vec3` already loaded at `:1261`. Components 1 and 2 are in the register; nothing reads them. The cheapest site in the tree. |
| `lichen-compute-gpu/src/spirv.rs:1212-1216` | `UnsupportedLaneWidth` fires on `lane_width != LOCAL_SIZE_X` — **one** number, and it compares the *total* against the *x axis*. A `64 × 4` workgroup declares `lane_width: 64` and sails through. A gap, not a bar: §4 measures the dispatch it lets past. |
| `lichen-compute-gpu/src/spirv.rs:131-132, 3190-3193, 3390-3393` | Only `SpvBuiltInGlobalInvocationId` is bound, decorated and declared. `LocalInvocationId` and `WorkgroupId` are not bound at all. **Measured** (§4): binding is free; reading faults. |
| `lichen-compute-gpu/src/spirv.rs:179-186` | `LOCAL_SIZE_X: u32 = 64`, with an `# Invariant` naming itself "the only width this target schedules". **Measured false** (§4). The name is also wrong under a rectangle: it is the `x` axis, not the workgroup. |
| `lichen-compute-gpu/src/dispatch.rs:340-350` | `launch_workgroups`: `count % lane_width` and `count.div_ceil(lane_width)`. **The site §4 indicts.** One `usize`, one axis. |
| `lichen-compute-gpu/src/dispatch.rs:1052, 1353` | `padded = workgroups * target.lane_width.lanes()` — the buffer rounding, one axis. |
| `lichen-compute-gpu/src/dispatch.rs:370-372` | `max_workgroup_lanes = max_compute_work_group_invocations.min(max_compute_work_group_size[0])` — **`[1]` and `[2]` are never read**, so a height has no ceiling at all. |
| `lichen-kernel-ir/src/lib.rs:583-595` | `LaneWidth(u32)`: "the lane count one workgroup runs". One number where an expr belongs. |
| `lichen-kernel-ir/src/lib.rs:963-973` | `Slice { class, count, space }` — "the bound and the declared array are one number". Dual: convention in the struct, construction in the emitted `OpTypeArray` (§1.1). |
| `lichen-compute/src/compute.rs:1953-1961` | `parallel_roles`: `in` → Input, `out` → Output, **everything else → `LeafRole::Scalar`**. A fourth role variant would carry an axis; there is none. |
| `lichen-compute/src/compute.rs:1874-1883` | "a parallel parameter's **first** scalar is the launch extent, and an extent is Int". The *number* of scalars became unbounded in §9 of the launch note; the *position* is still ordinal 0. |
| `lichen-compute/src/compute.rs:4825-4833` | `record_launch` reads `roles.scalars.first()` as the extent; every other leaf is carried. Convention throughout since §8–§10 landed. |
| `lichen-compute/src/compute/body.rs:769-770, 788-` | `Row = Div`, `Col = Rem` over **one** divisor (the config's second scalar leaf). A second axis is two more instructions on values already bound — this is the landed spelling working as intended. |
| `lichen-graph-ir/src/graph.rs:295-311` | `Count::{Constant(usize), Value(ValueId), Device { value, index }}` — three arms, one number. `KernelNode::count` (`:231`) is one. |
| `lichen-runtime/src/graph_pass.rs:16` | `LaunchExtent<'a> = dyn Fn(usize) -> Option<usize>` — **one number per node**. A pass cannot be asked for two. |
| `lichen-runtime/src/device.rs:231-237` | `lanes > max_workgroup_lanes` — one number, against a ceiling `dispatch.rs:370` built from `[0]` alone. |
| `lichen-runtime/src/device.rs:245-256` | The shared-memory ceiling is `Σ slice.count * byte_width`, which a rectangle satisfies unchanged — **the one site here a 2-D slice does not touch**, and worth knowing so it is not "fixed". |
| `lichen-runtime/src/automatic_tiling.rs:274-281, 291` | `lane_width = target.lane_width.lanes()`, `group_width = staging.group_width`, `count % group_width` — one axis, three times. |
| `lichen-kernel-ir/src/lib.rs:431-465` and `lichen-compute-gpu/src/dispatch.rs:799-860` | `ParallelBackend::{run, submit, run_batch}` and `GpuContext::{run, run_over_device_extent, submit}` each take **one** `count: usize`. There is no compute-side `spawn`; these *are* the launch signatures. |

### 1.3 Three sites the launch note listed that are no longer walls

Its §1.1 named `KernelShape` (`lib.rs:620-625`), the leaf-counting helpers
(`scalar_leaves`/`runtime_scalars`), and the role walk as by-construction blockers of a second
extent. Re-read against `991e59bb`, only the leaf-counting survive, and only for the *index*
(§1.1 row 4):

- **`KernelShape` never was the wall.** Its `# Invariant` is about the *parameter domain*
  ("a kernel domain is only ever a scalar or a tuple of scalars"), and a **tuple of scalars
  already holds any number of leaves** — which is exactly what §9 of the launch note landed,
  with `K` as a parameter rather than a literal. A second extent rides in a longer tuple today.
- **`scalar_leaf_classes` and `parallel_roles` are convention, not construction**, and both are
  already unbounded in count (§1.2 rows 10 and 11). What is left of "the first leaf is the
  extent" is an *ordinal*, not a dimension.
- **`lower_index_function` survives, but only as "one invocation index"**, which §1.1 row 2
  states. The config's leaves were never the problem.

The separation matters for the plan: **the by-construction set is six sites, and four of the six
are in two files** — `spirv.rs` and `body.rs`. Everything else is an additive field plus whoever
reads it.

## 2. The smallest honest step, what it proves alone, and what it leaves

The launch note's §3(b) listed four things a genuine 2-D workgroup needs. Re-read against the
tree, three of the four have changed status:

| the launch note said | now |
|---|---|
| "`EXECUTION_MODE_LOCAL_SIZE` becomes an expr" | **measured unnecessary**: the instruction takes three literals and `(16, 4, 1)` compiles, validates and runs (§4). |
| "a second built-in is bound and read" | **half false**: binding is free and free of effect; **reading it faults on this device** (§4), and the cause is unknown. |
| "`Slice` counts per lane in two dimensions" | **unchanged and true**, and it is the real one (§1.1 row 1). |
| "`Plan::group_width` becomes an expr" | **unchanged and true** (§1.1 row 5). |
| "`TargetFacts` grows a field — which changes every kernel id" | **unchanged, and now measured** (§5): 74 sites, one version bump. |

**The smallest honest step is therefore not the emitter.** It is `TargetFacts` growing a workgroup
*expr* — with **no emitter change, no launch change and no `Slice` change** — because that is the
only part of the work which is (a) irreversible, (b) not blocked by anything unknown, and (c)
**demonstrable on its own**: the tree still compiles, every digest-keyed test moves together, a
`v1` artifact is refused by name at the version gate, and the **first time any code checks a
second axis** — `DeviceFacts` gains `maxComputeWorkGroupSize[1]` and a fragment declaring
`64 × 16` is refused on a device whose ceiling is `64`, while `64 × 4` is accepted.

**What it proves on its own:** that the digest migration is a **version bump and 74 mechanical
sites**, not a rewrite — before any behaviour depends on it. §5 measured the first half; the
second is a `deny_unknown_fields` text and a `count()`.

**What it must leave for the next slice:** everything. No emitter change, no `LocalInvocationId`,
no `Slice`, no `Plan`, no tiler, no `launch_workgroups`. And it must **refuse** a workgroup that
is not `x × 1` until the rest exists — otherwise it lands a fact no code reads, which is the one
thing `DeviceFacts`' own `# Invariant` forbids ("a fact no fragment is refused for would be a
number this crate collects and never reads", `device.rs:8-12`).

## 3. What it breaks, named

- **Tests that pin one-dimensional arithmetic.** `LOCAL_SIZE_X + 5` at
  `crates/lichen-compute/tests/lichen_compute/compute.rs:1531`; `tiled_count`/`tail` at `tests/lichen_compute/tiled_reduction.rs:404`;
  `tests/lichen_compute/reduction_chain.rs:369-386`; `tests/lichen_compute/plan_staging.rs:200, 263, 307-337`;
  `tests/lichen_compute/pack_placement.rs:217-228, 382-384`; `crates/lichen-compute-gpu/tests/compute_gpu/warp_exchange.rs:249-276`;
  `tests/upload_width.rs`; `tests/lichen_compute/automatic_tiling.rs:27, 36, 503`; `tests/lichen_compute/tiling_visibility.rs:18`.
- **Pinned artifact text** — six literal strings in `crates/lichen-compute/tests/lichen_compute/fused_kernel.rs`
  (`:167, 260, 308, 372, 403, 466`) and one in `tests/lichen_compute/fused_landing.rs:46`, all
  `"target": { "int_width": "i64", "lane_width": 64 }`. They are **inputs to a `deny_unknown_fields`
  parse**, so a new field breaks them at the parse, not at the assertion.
- **Pinned pack text** — `crates/lichen-compiler/tests/pack_install.rs:104, 358, 390`, the
  `"group_width": 64` inside `"plan": { "staging": [ … ] }`.
- **The digest-identity tests** — `crates/lichen-kernel-ir/tests/kernel_ir/fragment_identity.rs` asserts
  which exprs of fragments are the *same* artifact. Adding a field keeps every such assertion
  true and moves every number.
- **The device gate** — `DeviceFacts` gains a field, `check` (`:223`) gains a comparison, and its
  only construction site outside the GPU crate is the test at
  `tests/compute_gpu/fused_landing_on_device.rs:402` (a hand-made `max_workgroup_lanes: 1024`).
- **`lichen-compute` has six pre-existing failing binaries** (`automatic_tiling`, `combine_fold`,
  `fold_datum`, `identity_from_body`, `pack_placement`, `examples`) from another session's
  `lowlevel` work. Any of the above must be read against that baseline, not against a clean tree.

**Contracts a full implementation would have to change**, quoted. **None is changed here, and no
`# Invariant` or `Safety` contract is proposed for change without permission.**

1. `spirv.rs:182-185` — *"A run of `count` indices is `ceil(count / LOCAL_SIZE_X)` workgroups,
   and the surplus lanes of the last one are a caller obligation, not the shader's. It is also
   the only width this target schedules, so a fragment declaring another is refused by name
   rather than having its `LocalSize` rounded to this."*
   **This one is already false, and §4 is the measurement that says so.** A module declaring
   `(16, 4, 1)` was scheduled; the refusal did not fire.
2. `dispatch.rs:53-57` — *"A dispatch rounds up to whole workgroups and runs the surplus lanes
   with zero-padded input, so a surplus lane of a staged body writes a value a real lane of its
   workgroup reads and no diagnostic sees it. Both numbers are named because the expr is what is
   wrong."* Under a rectangle the obligation is a **`rows × cols`** region and "both numbers"
   become four.
3. `plan.rs:186-191` — *"a group narrower than the workgroup needs a barrier narrower than the
   block, which lichen does not have."* A 2-D workgroup falsifies the shape of the claim.
4. `kernel-ir/lib.rs:1077-1081` — *"Every field is hashed, and a field left out is a way for two
   fragments that differ in it to share one identity."* Unchanged in wording; §5 measures what
   obeying it costs.
5. `kernel-ir/lib.rs:963-964` — *"The fragment derives the count from `target.lane_width` while
   the surface that names one is unbuilt, so the bound and the declared array are one number."*
6. `kernel-ir/lib.rs:1056-1058` — *"The index is the last leaf and is not a launch value — on a
   device it is the invocation a lane is, and on the host the worker's loop variable."*
7. `kernel-ir/lib.rs:994-996` — *"a parallel fragment's shape is `(config, index)` however many
   buffers it reads, because they are bound storage buffers."*
8. `body.rs:252-253` — *"The fragment's domain is the config's leaves plus the invocation index,
   which no shape contains."*
9. `device.rs:26-28` — *"A ceiling, not the width a dispatch uses: `TargetFacts::lane_width` is
   the host compiler's choice and a device that cannot schedule that many lanes has no way to
   honour it. Refused rather than rounded down, which would run a different program."* The
   `[0]`-only read at `dispatch.rs:372` is the gap; a `[1]` makes the sentence true in two axes.
10. `wasm/lower.rs:521-524` — *"calls a kernel body once per **index**, never per workgroup, so
    no barrier has a group to synchronise and a slice has no lifetime between indices."* Still
    true, and still the reason a 2-D workgroup is device-only.
11. `code-audit.md` **D17**'s scope paragraph (`:1084-1090`) — *"The scope of this decision is
    global buffers… Whoever lands step 4 has to decide it again for a `block[1]` scope, and the
    honest default there is the opposite one: a staged write is reachable from any lane of the
    workgroup, so reachability from the codomain is not the test."*
    **A hand-off, not a change this proposal may make.**

## 4. What was measured

A patch to `spirv.rs` alone — a second `Input` variable for `SpvBuiltInLocalInvocationId` taken
from the **same id cursor** as `gid`, decorated `BuiltIn 29` and listed on `OpEntryPoint`'s
interface; `OpExecutionModeLocalSize` changed from `(lane_width, 1, 1)` to `(PROBE_X, PROBE_Y, 1)`;
and the entry point's index binding optionally read from it. The fragment is
`warp_exchange::sums_a_staged_tile(Space::Shared, 64)` — `slice[i] = in[i % 64]`, barrier,
`out[lane] = Σ_j slice[j]` — so the right answer is `1 + … + 64 = 2080` at every position. On the
**unpatched** tree the baseline is `2080 × 64` and `a_warp_exchange_answers_what_the_shared_exchange_answers`
**passes**. The patch and the probe file were removed afterwards; nothing else in the tree moved.

`spirv-val` is **not installed** on this machine, so `tests/compute_gpu/spirv_validation.rs`
skipped and no offline validation ran. "Validates" below means the driver's own pipeline
creation accepted it.

| # | `lid` on the interface | `lid` read | `LocalSize` | index from | outcome |
|---|---|---|---|---|---|
| 1 | yes | **no** | `(64,1,1)` | `gid` | runs, **`2080 × 64`** — identical to baseline |
| 2 | yes | yes | `(64,1,1)` | `lid.x` | **`STATUS_ACCESS_VIOLATION`** |
| 3 | yes | yes, row-major `lx + 16·ly` | `(16,4,1)` | `lid` | **`STATUS_ACCESS_VIOLATION`** |
| 4 | yes | yes | `(16,4,1)` | `lid.x` | **`STATUS_ACCESS_VIOLATION`** |
| 5 | yes | yes, into a **dead value** (index still `gid`) | `(64,1,1)` | `gid` | **`STATUS_ACCESS_VIOLATION`** |
| 6 | **no** | yes | `(64,1,1)` | `lid.x` | runs, but the built-in is unpopulated: `out = [1, 0, 0, …]` |
| 7 | yes | **no** | `(16,4,1)` | `gid` | **runs, and answers wrong — below** |

Rows 1, 2 and 5 are the discriminator: **binding** the second built-in costs nothing and changes
no answer; **any** read of it faults, whether or not it is the index. Row 6 shows the interface
entry is a necessary condition for the read to be meaningful at all.

**Row 7, verbatim:**

```
PROBE count=64 expected=2080 wrote_at=[] contiguous=false
full=[136 × 16, 0 × 48]
```

`workgroups = ceil(64 / 64) = 1` — `dispatch.rs:349`, unchanged. One workgroup of `16 × 4`
invocations. `GlobalInvocationId.x = wgId.x · 16 + lx` therefore covers only `[0, 16)`, every
index is computed by 4 lanes, and `out[16..64]` is **never written**. `out[0..15] = 136 =
Σ_{k=1..16} k`, which is the sum over the 16 slots the body actually staged; the 48 slots no lane
wrote contributed `0` on this run, and that is a coincidence of an uninitialised `Workgroup`
variable, not a guarantee. The dispatch **returned success**.

**What this settles, and it is not what the launch note asked:**

- A rectangle is **not** a construction site. `OpExecutionModeLocalSize` takes three literals and
  `(16, 4, 1)` is a legal, runnable module. §1 of the launch note listed it as one of twelve
  by-construction blockers; **it is not**.
- The emitter's recipe for `GlobalInvocationId` (`OpLoad` the `vec3`, `OpCompositeExtract`
  component `0`) **does not transfer** to `LocalInvocationId` on this device. This is exactly the
  question `two-dimensional-launch.md` §7 left as unknown — *"Whether the `vec3` the emitter
  already binds is sufficient for a real 2-D workgroup… Whether the staging story survives on the
  global id is a measurement nobody has made."* **It does not survive**, and the failure mode is
  a process fault rather than a refusal, which is the worst of the three.
- **The first silent wrong answer a 2-D workgroup produces comes from the host, not the shader.**
  Not from `Slice`, not from `Plan`, not from the tiler: from `launch_workgroups`, because the
  covered index range is `X · ceil(count / X·Y)` rather than `count`.

## 5. The `TargetFacts` digest migration, and what it costs

**The hash.** `fragment_digest` (`lichen-kernel-ir/src/lib.rs:1082-1097`) hashes
`format!("{:?}", fragment.target)` at `:1093` — the **`Debug` string of the whole struct**, not a
field-by-field walk. `TargetFacts` is `lib.rs:605-610`, `#[derive(Debug, Clone, Copy, PartialEq,
Eq)]`, two `u32`-shaped fields.

**Measured.** A probe printed the `Debug` string and the digest of a fixed fragment on the
unpatched tree, then added `lane_height: LaneHeight(1)` — carrying its **default** — and
re-measured:

| | `Debug` | `fragment_digest` |
|---|---|---|
| `TargetFacts { int_width, lane_width }` | `TargetFacts { int_width: I64, lane_width: LaneWidth(64) }` | `1708622714001280166` |
| plus `lane_height: LaneHeight(1)` | `TargetFacts { int_width: I64, lane_width: LaneWidth(64), lane_height: LaneHeight(1) }` | `6121588463028441745` |

The body, the parameter shape, the roles, the buffers and the lane width were **byte-identical**.
Every kernel id in the tree moves, exactly as the `# Invariant` at `:1077-1081` says it must.

**The mechanical cost, measured by doing it:** a scripted edit touched **74 construction sites
across 12 crates** (it also caught two struct *field* declarations named `lane_width: u32`, which
a real patch would not). Every one is `TargetFacts { int_width: …, lane_width: … }`.

> **Superseded — §9.** The 74 and the 12 crates are both wrong against the tree this landed on.
> The measured figure is **80 struct literals in 31 files across 6 crates**.

**Every consumer of the digest, so nothing is missed:**

| consumer | file:line | what a moved id costs it |
|---|---|---|
| the kernel registry's id allocation and intern index | `lichen-runtime/src/kernel_registry.rs:78` | every id is new; the **fallback on a genuine collision** (`code-audit.md` D15) is what keeps this a recompile and not a wrong kernel |
| the **pipeline cache key** | `lichen-compute-gpu/src/dispatch.rs:2017-2025` | a cold cache, per process — it is a `HashMap`, not a store |
| the claim `by_digest` index | `lichen-runtime/src/fusion_claim/schema.rs:1130, 1138, 1159` | a claim registered under the old digest stops being found |
| the automatic-tiling digest collapse | `lichen-runtime/src/automatic_tiling.rs:140, 456` | two equal fragments stop collapsing for one run |
| the wasm **module cache** | `lichen-runtime/src/module_cache.rs:69` (keyed on `KernelId`) | a cold cache; `code-audit.md` D15 records it as the reason content addressing exists |

**The stored artifact.** `TargetFacts` is mirrored as `schema::Target`
(`lichen-runtime/src/fused/schema.rs:301-310`) with `#[facet(deny_unknown_fields)]`, and the
conversion is a **two-way expr** at `lichen-runtime/src/fused/convert.rs:372-387`. `FORMAT_VERSION`
is `schema.rs:435`, currently `1`.

**The version gate is already in place and already measured.** `load_fused_kernel`
(`lichen-runtime/src/load.rs:113-137`) probes the version through `VersionProbe` — a struct whose
`# Invariant` is *"**Unknown fields are not denied here**, which is the whole point"* — and
refuses on a mismatch at `:122-127` **before** the strict read at `:128`. So an old artifact meets
a clean named refusal rather than a parse error naming a byte offset. That is recorded as measured
in [taking-a-kernel-to-a-server](taking-a-kernel-to-a-server.md) `:116-125`: *"a `"version": 2`
text is refused … a hand-made `subgroup_width: 16` refuses a 32-element `Warp` slice by name."*

**The three options, honestly costed.**

- **(a) Bump `FORMAT_VERSION`, no migration — recommended.** Cost: every stored **fused artifact**
  is retired; a recompile. `load.rs` already refuses it by name and in the right order, and the
  repo's own precedent for its *other* artifact family is exactly this — `artifact-cache.md:289-293`
  reads *"The reader accepts only the current value… there is no compatibility path"* — with cases
  `8` and `9` at `:299-308` recorded as **silent misreads** that are the reason a bump was not
  skipped. Six pinned artifact strings (§3) and the conversion expr are the edits. What it does
  **not** retire: there is no on-disk store of emitted modules at all
  (`taking-a-kernel-to-a-server.md:163-166`), so a bump costs a recompile and nothing else.
- **(b) Re-key the digest** — hash a named list of facts rather than the `Debug` string. This does
  **not** avoid the migration: the set of facts changed, so the ids must move regardless. What it
  buys is that the digest stops depending on a derive's output format, and the *next* field is a
  named line rather than an accident. **Do it in the same commit as (a), with the field**, so the
  tree pays one migration instead of two — and give it its own test proving the digest is a
  function of the named fields, or its own regression (an id that should *not* move, moving) will
  be invisible inside the move that is expected.
- **(c) Migrate the stored artifacts** — read a `v1` text and default `lane_height` to `1`. This is
  the *one* case where a default is defensible: a `v1` artifact's workgroup **was** one row, so
  `1` is the honest value rather than a guess. It costs a `v1` reader kept forever, and it fights
  the design already chosen — `deny_unknown_fields` plus a version-first gate is precisely what
  makes an old text a *refusal* rather than a reinterpretation. Rejected, and the rejection is
  `artifact-cache.md`'s own, not a new opinion.

**The pack is the gap, and it is loud rather than silent — checked, not assumed.**
`placement_pack.rs:367-382`'s `Pack` has **no `version` field**, and `read_placement_pack`
(`:279`) has no version gate. But it maps its parse error through `malformed`, and both
`Pack` and `Staging` carry `#[facet(deny_unknown_fields)]`. So a `v1` pack read by a `v2` reader
is a **named refusal that stops the run** (`:275-277`: *"a pack that steers neither a placement
nor a fusion is refused rather than installed inert"*), not a silent no-op. The `"group_width": 64`
fixtures at `pack_install.rs:104, 358, 390` move at that parse. The module doc's "inert" applies
to a hash or constraint mismatch, **not** to a parse failure — worth saying because the two read
alike from a distance.

**What this section does to the two notes.** `artifact-cache.md`'s `ARTIFACT_FORMAT_VERSION`
(`lichen-language/src/persist.rs:37` = `10`) is a **different artifact family** — the frozen
program, not the kernel — and nothing here touches it. `taking-a-kernel-to-a-server.md`'s
`fragment_digest` reference (`:18`) and its `target` row (`:142`) stay true; its
*"no versioned `read_fragment`/`write_fragment` expr"* (`:160-162`) is still missing, and a 2-D
workgroup makes that expr more valuable rather than less, because a server that ships fragments
across a driver boundary is exactly where `LocalInvocationId` earns or loses.

## 6. An ordered plan, and the one slice to take first

**First slice, and the only one I recommend taking before anything else: `TargetFacts` grows a
workgroup expr, and nothing else moves.**

Concretely — `TargetFacts { int_width, lane_width, lane_height }`, or a `LanePair { x, y }` in
place of the scalar; the digest re-keyed in the same commit; `schema::Target` and
`placement_pack`'s `Staging.group_width` widened; `FORMAT_VERSION` bumped `1 → 2`; `DeviceFacts`
gaining `max_workgroup_lanes_y` read from `max_compute_work_group_size[1]`; and `check` refusing a
workgroup that is not `x × 1` **by name**, because nothing downstream can honour one yet.

> **Two corrections, both measured in §9.** The digest was **not** re-keyed: it still hashes the
> `Debug` string of the whole struct, which is what §5 measured and what the `# Invariant` at
> `lib.rs:1109-1113` describes. And **`placement_pack`'s `Staging.group_width` was not widened**,
> because §2 above says a `Plan` is left for the next slice and §6 item 6 is that slice — the two
> halves of this note disagree, and §2 is the one that holds. A pack carries no `TargetFacts` at
> all (`placement_pack.rs`'s `Constraints` holds a scalar class and nothing else), so nothing in
> the pack path means something different at version 2.

**What it proves, on its own:**

- the digest migration is a **version bump and a counted edit** (§5 measured the digest; the edit
  was made and counted at 74 sites and then reverted — **and §9 recounts it at 80**), with every
  digest-keyed test moving together and none of them asserting a number;
- a `v1` artifact is refused **by name at the version gate**, before the parse, on the real
  loader — the migration's whole safety property, demonstrated without a single 2-D launch in the
  tree;
- **the second axis is checked by something for the first time.** A fragment declaring `64 × 16`
  is refused by name, on the real device, with no geometry change at all — **but not by the ceiling
  it guessed, and not only for `64 × 16`. §9 has both corrections.**

**What it must leave for the next slice:** everything else. No emitter change, no
`LocalInvocationId`, no `Slice`, no `Plan`, no tiler, no `launch_workgroups`, no 2-D launch, no
2-D extent in `Count`. It touches six construction-free sites and none of the six by-construction
sites in §1.1 — which is the point: the irreversible half lands while everything is still one
dimensional, and the next slice can then widen a *claim* on a migration already measured rather
than widening both in one step.

**After that, in order**, each independently demonstrable:

2. **Diagnose the fault** (§8). `spirv-val` on a toolchain that has it, then a second recipe
   (`OpAccessChain` into the built-in, or declaring the module at SPIR-V 1.4). Nothing in the
   emitter ships before this, because the failure is a fault rather than a refusal.
3. **The second built-in**, bound and read, with the existing device tests unchanged as the
   evidence that nothing moved.
4. **`launch_workgroups` generalised, and the refusal it does not have today.** §4 measured that a
   rectangle under the present arithmetic is a silent truncation; the slice must land a
   **rectangular** `StagedLaunchNotAWholeWorkgroup` (`count % (x·y) != 0` refused by name) *in the
   same commit as* the arithmetic that would otherwise allow it. Landing the arithmetic first is
   the tail-lane episode again.
5. **`Slice` as a rectangle** — nested `OpTypeArray`, two-index `OpAccessChain` (§1.1 row 1) — and
   `KernelBody::barrier_scan`'s index-escape check over a 2-D read set.
6. **`Plan::group_width` as an expr** and `check_plan`'s rectangle rule (§1.1 row 5), with the
   `D17` hand-off of §3 item 11 settled **first**, by whoever lands it.
7. **The tiler** — `Folding::tiled`'s domain `(extent, index)` and `Fold { stride, modulo }`
   (`tiled.rs:343-347, 380, 399`), with `tile ≡ body` pinned at the same tightness it has today
   (`llm-operators.md:216-217`: *"bit-for-bit only for `max`/`min`"*).
8. **The 2-D launch** — `Count` carrying an expr, `LaunchExtent` taking two, `run_batch` and
   `Count::Device` refusing or carrying one. Last, because everything above is a fact about the
   machine and this is a fact about the program.

## 7. The risks

**The silent wrong answer — and it is reachable today, measured.** §4 row 7 is not a hypothesis:
a rectangle under `launch_workgroups` shrinks the covered range by `Y` and answers wrong with **no
error, no refusal and no report line**. It is the tail-lane/upload episode's exact shape — *a
read set that is not a prefix silently reads nothing* — reached from the geometry side instead of
the upload side, and it is worse in one respect: the upload episode was found by a matmul
measurement, and this one is reachable by changing two literals in the emitter with no program
change at all. Slice 4 above exists for this reason and must not be reordered.

**A fault where a refusal belongs.** §4 rows 2–5: a shader module that reads `LocalInvocationId`
the way the global id is read **kills the process** on this device. That is not a wrong answer, so
the usual defences do not apply — there is no refusal to inspect and nothing in the run report.
It is also the reason slice 2 exists and the reason no slice may *assume* the local id is
readable. The honest state is: **the capability claim in `two-dimensional-launch.md` §1 about the
`vec3` being "already there" is true of the register and false of the recipe.**

**Widening a claim without proving it as tightly.** The sharpest instance is already in the tree:
`spirv.rs:182-185` claims this target "schedules" one width and refuses another by name, and
§4 measured a second width being scheduled with no refusal. A 2-D workgroup widens `Slice` from
one array to a nested one and `check_plan` from an equality to a rectangle, and neither is
checkable by reading the emitted module — the tail-lane obligation is discharged today by
**padding** and the staged launch is made unspellable by a **refusal**
(`dispatch.rs:51-58`), so the second axis's obligation has neither padding nor refusal behind it
until slice 4 lands. Widening `Slice` before that is precisely "widening what is claimed without
proving it as tightly as what is there now".

**`tile ≡ body` does not transfer.** §6 item 7 is last-but-one for a reason the note already
states: re-associating `+`/`*` in `f32` is an assumption, and only `max`/`min` are bit-exact
(`llm-operators.md:216-217`). A 2-D workgroup re-associates the **address arithmetic**, which is
`Int` and therefore exact — but it changes *which* lanes participate in the fold, which is the
variable that decides whether a boundary tile is the closed form or the per-lane one.

**The migration's own silent failure mode.** §5 checked the pack path rather than assuming it:
`read_placement_pack` refuses an unparseable pack **by name**, so both directions are loud. The
residual risk is not silence but **inertness by hash**: `placement_pack.rs:1-8` says a pack whose
hash or constraints do not match is inert and the engine's own choice answers, so a `v2` reader
matching a `v1` pack's *schema hash* while its `group_width` means something else would install
and do nothing. That is the one interaction the bump does not cover, and it belongs in slice 1's
test list.

## 8. What I could not determine, stated plainly

- **Why reading `LocalInvocationId` faults.** `spirv-val` is not installed on this machine, so
  `tests/compute_gpu/spirv_validation.rs` skipped and no module was validated offline; and a
  driver that faults on an invalid module cannot distinguish "invalid module" from "driver bug".
  Row 6 sharpens it — the module is *not* valid without the interface entry — so the two causes are
  entangled and I cannot separate them from the outside. **This is the single thing blocking the
  emitter slice, and it needs a toolchain that has `spirv-val`.**
- **Whether `WorkgroupId` behaves the same.** Untested; I stopped after `LocalInvocationId` had
  already faulted.
- **Whether a different recipe avoids the fault** — `OpAccessChain` into the built-in, or declaring
  the module at SPIR-V 1.4 where the interface rule is the one the emitter already assumes
  (`spirv.rs:3122-3123` writes a 1.4-style interface under `SPIRV_VERSION = 1.3`). Both untested,
  and the second is worth a look for its own sake: it is a **pre-existing** inconsistency the
  probe did not cause.
- **Whether `Policy::Batch` can carry two extents** in one submission without
  `run_batch`'s `BatchRefusal::Cannot` (`kernel-ir/lib.rs:463-465`). Carried forward unanswered
  from `two-dimensional-launch.md` §7; I did not probe it.
- **Whether `Count::Device` can carry an expr.** Its `# Invariant` (`graph.rs:303-310`) says the
  number is *where it is*, and an indirect dispatch hands the shader nothing — so a rectangle would
  need both axes read out of the buffer, and I did not read the dispatcher's indirect path.
- **How much a persisted fused artifact costs to retire**, if any are persisted anywhere. I read
  `artifact-cache.md`'s version section and
  `taking-a-kernel-to-a-server.md`'s load path; neither says a fused artifact is written to disk by
  anything in the tree, and `convert.rs:585-592` shows the write side is an `impl From<&FusedKernel>`.
  If a deployment outside this repository keeps them, option (a)'s cost is higher than I measured.
- **Whether a `Warp` slice survives a rectangle** at all. `device.rs:257-265` requires
  `slice.count == subgroup_width`, and under `(16,4,1)` the subgroup's lanes are a subset of the
  workgroup's in an order I did not measure. Untouched by any slice above, and it would be the
  first thing a 2-D workgroup silently broke.

## 9. §6's first slice, measured — and the three places this note was wrong

Commit `fd494671`, on `af634c14`. Two probes, both in `tests/` as files that print and assert
nothing, both **removed** afterwards: the tree carries no artefact of them. What they printed is
below, verbatim.

### 9.1 The migration moves 80 struct literals, not 74, in 6 crates, not 12

```
$ git -C <worktree> grep -c 'TargetFacts\s*{' -- 'crates/**/*.rs'   # 82 lines
```

82 lines match `TargetFacts {`. **Three of them are not constructions**, and the note's 74 counted
neither correctly: one is the definition (`lib.rs:606`), one is `fn target() -> TargetFacts {`
(`tests/compose.rs:21`), and the note's script also caught two struct *field* declarations named
`lane_width: u32`, which it says itself were false positives. **80 `TargetFacts { … }` struct
literals, in 31 files, across 6 crates** — `lichen-compute-gpu` 56, `lichen-compute` 12,
`lichen-fusion-bridge` 5, `lichen-kernel-ir` 5, `lichen-graph-ir` 1, `lichen-runtime` 1. Of the 80,
**6 are in `src/`** (`compute.rs` 5, `fused/convert.rs` 1) and 74 are in `tests/` and `examples/`.
The note's "74" is almost exactly the *test-side* count, which is a coincidence worth naming rather
than reading as a confirmation.

**What the migration did not touch is the larger half of the note's fear.** §5 and §6 predict a
digest migration that moves "every digest-keyed test". **No test in the tree asserts a digest
number.** All 77 `fragment_digest` uses are comparisons — `assert_eq!` between two digests, or a
digest stored and compared later — and no `assert_eq!` in any test compares anything against a
literal of 15 digits or more. So the digest moved and **zero assertions moved with it**: what the
80 sites cost is 80 lines and one version bump.

### 9.2 The digest, before and after, on one fragment

Same fragment throughout (`p0 + 1`, scalar shape, no buffers, no slices, no marks). The v1 number
is computed by hashing the same ten fields in the same order with the **two-field** `Debug` string
in the `target` slot, so the only difference between the two numbers is that one string:

```
PROBE target Debug (this build) = TargetFacts { int_width: I64, lane_width: LaneWidth(64), lane_height: LaneHeight(1) }
PROBE fragment_digest (lane_height: 1) = 14003084899050303553
PROBE fragment_digest (v1, no lane_height) = 9615707550460718134
PROBE fragment_digest (lane_height: 4) = 10033502527838007574
PROBE FORMAT_VERSION = 2
```

**That block is this slice's own build, and `FORMAT_VERSION` has since gone to `3`** — for a body
field added on a different branch, which is the same gate and the same required-field rule §5
describes. The three `fragment_digest` numbers above are **not** comparable with a digest taken
today, for a second reason as well as the one this section names: a `KernelBody` field has since
joined the body's `Debug` string, so today's build hashes a different string again. The claim §5
rests on — *a byte-identical fragment's digest moves because one `Debug` string grew a field* — is
what survives, and [contraction-control](contraction-control.md) §9.2 re-measures it on a body
field rather than a `target` one.

§5's mechanism claim is **confirmed**: a byte-identical fragment's digest moves because one
`Debug` string grew a field. §5's *numbers* are not comparable — they were a different fragment.

### 9.3 The refusal, measured — and `64 × 4` is **not** accepted

**Committed cover, added later (`0d120e4d`).** The four `PROBE` lines below were produced by probes that were removed; the two refusals they show are now pinned by tests that need no device — `crates/lichen-compute-gpu/tests/compute_gpu/workgroup_height.rs` (three cases: a single row accepted, a rectangle under the ceiling refused as `WorkgroupTallerThanOneRow`, a height past the ceiling refused as `LaneHeightTooWide`), so a regression that swapped the two rules would fail rather than pass silently. The suite went 78 → 81 passed for it.

Against a hand-made table answering `max_workgroup_lanes 1024`, `max_workgroup_lanes_y 64`:

```
PROBE device: max_workgroup_lanes = 1024, max_workgroup_lanes_y = 64
PROBE 64 x 1   accepted
PROBE 64 x 4   refused by name "WorkgroupTallerThanOneRow" = one workgroup runs 4 row(s) and this build schedules 1 row(s) per workgroup. A rectangle is a legal module — `OpExecutionModeLocalSize` takes three literals — but the dispatch rounds the count up by the `x` axis alone, so the rows no lane covers are indices nothing writes: no error, no refusal and no report line (`docs/notes/two-dimensional-workgroup.md` §4)
PROBE 64 x 16  refused by name "WorkgroupTallerThanOneRow" = one workgroup runs 16 row(s) and this build schedules 1 row(s) per workgroup. A rectangle is a legal module — …
PROBE 64 x 65  refused by name "LaneHeightTooWide" = one workgroup runs 65 row(s) and this device schedules at most 64, so the fragment's `LocalSize` is one no workgroup here can be: it is refused rather than rounded down, which would run a different program (`docs/notes/taking-a-kernel-to-a-server.md`)
```

**Correction 1: `64 × 4` is refused, not accepted.** §6's bullet is self-contradicting — §2 and §6's
"Concretely" both say `check` refuses anything that is not `x × 1`, and only the "what it proves"
bullet says `64 × 4` is accepted. The refusal is the right one: §4 measured that a rectangle is a
legal, runnable module, so a device ceiling cannot be the thing that catches it. Two refusals, and
both are reachable: `LaneHeightTooWide` when the height is past the device's, `WorkgroupTallerThanOneRow`
for every other second row.

**Correction 2: this machine's `maxComputeWorkGroupSize[1]` is 1024, not 64.** §6 says the
`64 × 16` refusal is "measured on the device" against "a device whose `maxComputeWorkGroupSize[1]`
is 64". Measured against the RTX 3060 actually present here:

```
PROBE int_bits              = 64
PROBE max_workgroup_lanes   = 1024
PROBE max_workgroup_lanes_y = 1024
PROBE subgroup_width        = Some(32)
PROBE 64 x 1     accepted
PROBE 64 x 4     refused by name "WorkgroupTallerThanOneRow" = …
PROBE 64 x 16    refused by name "WorkgroupTallerThanOneRow" = …
PROBE 64 x 64    refused by name "WorkgroupTallerThanOneRow" = …
PROBE 64 x 65    refused by name "WorkgroupTallerThanOneRow" = …
PROBE 64 x 1024  refused by name "WorkgroupTallerThanOneRow" = …
```

So on this device the ceiling is not what refuses a `64 × 16`; every rectangle is refused by the
build's own one-row rule, and `LaneHeightTooWide` needs a height past 1024. **The ceiling field is
still read** — that is what `DeviceFacts`' own `# Invariant` (`device.rs:8-13`) requires, and a
refusal nothing can reach would fail it — but its reach on this machine is narrow.

### 9.4 The version gate, on the real loader, both directions

A hand-written **version-1** text — the exact two-field `target` the tree carried before this
change — through `load_fused_kernel`:

```
PROBE read_fused_kernel(v2 text) ok? true
PROBE load_fused_kernel(v1 text) name    = VersionMismatch
PROBE load_fused_kernel(v1 text) message = the text is artifact format 1 and this build reads format 2, so the artifact is refused before any of it is parsed: reading another format with this one's fields is how a loader dispatches a kernel nobody wrote (`docs/notes/taking-a-kernel-to-a-server.md`)
PROBE read_fused_kernel(v1 text) without the gate = Some("the text is not a fused kernel: missing field `lane_height` in type `Target` (at `fragment.target`) (bytes 814..815)")
```

The last line is the whole point of the gate, side by side: **without** it the same text is a parse
error naming a byte offset in a field the reader has never heard of. §5's claim — "an old artifact
meets a clean named refusal rather than a parse error naming a byte offset" — is **confirmed**.

### 9.5 What §7's silent class looks like after this slice

§7's silent wrong answer is §4 row 7: a rectangle under the present `launch_workgroups` covers
`X · ceil(count / X·Y)` instead of `count`, answers wrong, and reports nothing. **It is not
reachable after this slice**, and the refusal above is the reason rather than a coincidence:
`DeviceFacts::check` runs before any dispatch (`load.rs:113-137`'s own ordering), every shape a
fragment can arrive in goes through it, and a `lane_height` other than 1 is refused by name. What
§4's probe reached required a patched emitter that no fragment could ask for.

**The hole this slice leaves is one step earlier and it is not closed**: `spirv.rs` still emits
`OpExecutionModeLocalSize (lane_width, 1, 1)` from `target.lane_width` alone, so a rectangle
declared in the IR would be refused at the device gate but a rectangle *written by hand into an
emitted module* still schedules, because nothing between the two reads `lane_height`. That is §6
item 4's slice, and it is unchanged.

### 9.6 Line numbers

§1–§5 were written against `991e59bb`; this landed on `af634c14`, 57 commits later. The claims
hold, the pointers drifted: `fragment_digest` is at `lib.rs:1114-1129` (not `1082-1097`), the digest
`# Invariant` at `1109-1113` (not `1077-1081`), `TargetFacts` at `606-636` (not `605-610`), and
`FORMAT_VERSION` at `schema.rs:446` (not `435`).
