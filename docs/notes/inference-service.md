# An inference service, and the layer it is missing

> Status: current — this is the design record for serving a model from lichen, and
> the M0 measurement round that it rests on. Nine probes were run and four more
> facts were read off the tree; every row below names the probe or the file that
> answers it. Nothing here is a plan of record for the kernel work: it says what
> the serving layer needs and what already exists to build it from.
>
> What this note is: what an inference service actually is (a schedule with
> coupling points, not a kernel), which of lichen's layers already hold the pieces,
> the one rule the pieces impose on a step's design, and the four bridges that are
> missing between the language surface and the IR underneath it.
>
> Points at: `crates/lichen-graph-ir/src/graph.rs` (`Scalar`, `Count`,
> `KernelNode`), `crates/lichen-graph-ir/src/run.rs` (`Policy`, `Planned`),
> `crates/lichen-compute-gpu/src/dispatch.rs` (`MAX_DISPATCHES_PER_SUBMISSION`,
> `RunError::EmptyRun`, the device-held extent), `crates/lichen-runtime/src/lib.rs`
> (the artifacts and passes), and the seven probes named in §3.
>
> Companions: [compute-graph-jit](compute-graph-jit.md) (the recording, the
> policies, the refusals), [compute-runtime-scalars](compute-runtime-scalars.md)
> (the leaf ABI, and the open item §5 corrects),
> [shared-memory-and-barriers](shared-memory-and-barriers.md) (the buffer-version
> spine §6 depends on), [lichen-compute-gpu](lichen-compute-gpu.md) (the measured
> costs), [operator-gaps](operator-gaps.md) (the operator inventory this joins).

## 1. What the service is, and what it is not

A decode step is not a kernel. It is a **schedule** with four kinds of work in it,
and the coupling is in the fourth:

| kind | lifetime | where it runs | example |
|---|---|---|---|
| resident | the process | device | the weights |
| per step, data parallel | one step | device | the forward pass |
| per step, control | one step | host | scheduling, paging, the stop test |
| **coupling** | one step | **both** | the token readback, the metadata upload |

Three numbers bound the design, and all three are already measured elsewhere in
this tree: a dispatch's device time has a **fixed floor of ~0.036 ms** against a
resident input ([lichen-compute-gpu](lichen-compute-gpu.md)); a whole graph costs
**one submission and one fence wait** under `Policy::Batch`, against `Serial`'s one
of each per node; and one submission holds **at most 64 dispatches**
(`MAX_DISPATCHES_PER_SUBMISSION`, which is the descriptor pool's `max_sets`).

So the cost model a service is held to is: **per step, one submission and one fence for
the dispatches, plus one of each for every crossing.** Measured on the device through
`crates/lichen-serve/tests/step_cost.rs`, where every count is a delta read from the
installed context and every program answers a hand-derived number:

| the program | submissions | fence waits |
|---|---|---|
| one dispatch, then a `collect` | 2 | 2 |
| three dispatches, then the same `collect` | 4 | 4 |
| one **recorded step** (three dispatches), then the same `collect` | 2 | 2 |
| three **recorded steps**, then the same `collect` | 4 | 4 |
| one **prepared step**, with the readback left to the caller | **1** | **1** |

So a dispatch is one submission and one fence; a **recorded step is one of each however many
dispatches it holds**; and **a readback is a submission and a fence of its own** — it does not
ride along in the graph's submission, `Batch` or not. A step that hands a token back therefore
costs **two**, and a generation of `N` tokens costs **`N + 1`** where the same work as a direct
chain costs **`N × dispatches-per-step + 1`**. A step that **holds its answer on the device**
costs **one**, and the readback becomes the caller's separate act — which is what the last row
is, and what `Session::prepare`/`Prepared::step`/`Session::fetch` exist for. Everything below is
about whether a step can be built to hit that.

**And the submissions are most of what the clock is made of at this size.** One recorded graph
of 16 dispatches, prepared once and stepped 200 times, run under two schedules — same graph,
same kernel, same extent, no compilation inside the clock
(`crates/lichen-serve/tests/step_cost.rs`):

| schedule | submissions per step | fences per step | per step |
|---|---|---|---|
| `Batch` | 1 | 1 | **1.55 ms** |
| `Async` | 16 | 30 | 2.66 ms |

So batching saves **1.11 ms of a 2.66 ms step — 41.7%** on identical work. The kernel does almost
nothing (three elements per dispatch), so this is the *overhead* fraction and not a claim about a
real layer's compute: as each dispatch grows, the fraction the schedule owns shrinks. `Async` also
waits **30** fences for its 16 submissions — it pipelines and then settles more than it
submitted — so the two schedules are not comparable by submission count alone.

**Batching is what that per-step cost is for.** One prepared step serves any batch size, because
the batch is the extent **argument** and not a graph constant — so a single `Prepared` ran 1, 4 and
16 sequences through the same graph (`crates/lichen-serve/tests/step_cost.rs`):

| sequences in one step | the batched step | as separate steps | saved |
|---|---|---|---|
| 1 | 0.350 ms | 0.350 ms | — |
| 4 | 0.324 ms | 1.399 ms | 1.075 ms (77%) |
| 16 | 0.333 ms | 5.597 ms | 5.263 ms (94%) |

The 16-sequence step costs what a 1-sequence step costs: the submission and the fence do not grow
with the batch, so the fixed cost measured above is exactly what batching buys. The rows double as
the correctness check — the first sequence's answer is the same at every batch size and the last
one's is not, so the batch changed the schedule and not the arithmetic.

Two shape facts came out of building it. **A step of two *identical* dispatches is one node**: the
recorder collapses them, so a step needs genuinely different dispatches before it can hand an answer
back. And **two buffers under `.in` is still not a working shape** — the program is refused with
*"no field with this name in the struct type"* — so the batch's per-sequence values ride at the end
of the data buffer, read at `n + sequence`, instead of in a second buffer.

**And the cache has to stay on the device, or every step is a memory transfer.** A 4 MiB cache
(1,048,576 `Float` lanes), same graph and same extent, one step per iteration
(`crates/lichen-serve/tests/step_cost.rs`):

| the cache handed over as | per step |
|---|---|
| host data, re-uploaded every step | 53.9 – 104.9 ms |
| the resident the step before wrote | 0.53 – 1.65 ms |

Those are two runs of the one test, and the ratio (**98–99%**) is the finding while the absolute
numbers move with the machine's load. **Nothing comes back to the host between steps**: the seed is
the only host data, and step `k` reads the resident step `k − 1` wrote — the channel
`RunArgument::Resident` carries and the language surface cannot express, since a program's buffers
are built inside its own source. `Session::release` is the other half of it: a resident a step
produced is the caller's to hand back, or a service that steps for hours leaks its device memory.

**The decode step's shape runs through the service path.** A graph parameter holding four buffers,
with each dispatch reading a pair of them through its own struct, is what a decode step looks like
(`decode_step_graph.rs`). A `Prepared` of that shape takes **five arguments in the parameter's
order** — one count and the four buffers — runs in **one submission**, and accepts a **resident**
among a later step's arguments. Both answers were hand-derived and matched (`[63, 96]`, then
`[497, 848]`), so the ABI a service docks onto is the one the language already records.

**And the step that runs is the real one.** `examples/generate.lichen`'s three kernels — the score
GEMV, the greedy sampler and the cache append — recorded as one graph and prepared once, run twice
through `Prepared` at **one submission each**, with the key and the value cache handed from step to
step as **residents** (`crates/lichen-serve/tests/decode_step.rs`). Both answers are that file's own
hand derivation: step zero samples `2` and leaves the key cache `[1, 2, 2, -1, -1]`, and step one
samples `0` and leaves it `[1, 2, 2, 0, -1]`. So the cost model above is not a model of a trivial
kernel: it is the decode step's, and the cache never comes back to the host between tokens.

**And a step holds at most 64 dispatches, after which it refuses rather than splitting.** Measured on
a chain of one kernel (`crates/lichen-serve/tests/step_cost.rs`):

| dispatches in the step | submissions | per step |
|---|---|---|
| 2 | 1 | 0.39 ms |
| 16 | 1 | 1.43 ms |
| 64 | 1 | 5.33 ms |
| 65 | — | refused |

> *"a run of 65 dispatch(es) cannot be recorded in one submission: the range is 1 to 64, which is
> what the descriptor pool has sets for."*

The fit is **0.23 ms of fixed cost and 0.08 ms per dispatch**, so a step's clock is its dispatch count
and the submission is the small part of it. That is the throughput budget: a model whose token costs
`D` dispatches has a floor of `D × 0.08 ms` per token, and at `D = 864` — 32 layers of 27 dispatches —
that is 69 ms, or 14 tokens a second, **before any arithmetic**. Batching amortizes the 0.23 ms across
sequences and cannot touch the 0.08 ms per dispatch; only fewer dispatches can, which is what fusion
is for, and the fusion floor currently refuses both real shapes by name (`llm-operators.md`).

**The 64-dispatch cap is a bookkeeping cost, not a throughput one.** A token whose dispatches exceed
one submission runs as **scoped steps**: forty dispatches, then forty more, with the state crossing
as a **resident** and no host data between them (`crates/lichen-serve/tests/step_cost.rs` — eighty
doublings of `1, 2, 3` over two submissions, answering `2^80`, `2^81` and `3 × 2^80` exactly).
Measured: **64 dispatches in one submission 5.07 ms, 80 over two 6.07 ms**, so the second submission
and its sixteen extra dispatches cost **1.00 ms**. A model of `D` dispatches per token therefore runs
as `⌈D / 64⌉` scoped steps and pays about `0.23 × ⌈D / 64⌉` ms of fixed cost — at `D = 400–580` that
is 7 to 10 submissions and **1.6 to 2.3 ms out of a 34 to 46 ms token**. So the cap decides **how many
steps a service issues per token**, not how fast a token is: the dispatches are the cost, and fusion
is still the only lever on them.

**The crate is an application now, not only a library.** `cargo run -p lichen-serve` loads the toy
model's weights into a resident, admits two sequences, and generates four tokens each at **one
submission a step**, then retires a slot and admits a new sequence at position zero so the batch
adapting is visible. The model's source and its tables are the crate's own — `DECODE_STEP`,
`WEIGHTS`, `EMBEDDING` — so the demo and the tests serve **one** model rather than one copy each.
Measured run: the first step `2.77 ms` with the cold start, then `0.45` to `0.77 ms` a step, and the
step after the swap advanced **one** sequence because the other's cache was full. The toy model
scores only the key it is handed, so its tokens repeat; the demo is the serving mechanics and says
that in its own module doc.

**A layer of three kernels with three different argument shapes runs as one submission.** Scale,
summarise and add — a normalisation's two passes — over two sequences, with each dispatch taking **its
own** input struct (two buffers, one, then two again) while the graph's parameter holds two
(`crates/lichen-serve/tests/layer_step.rs`): `1..6` scaled by `2, 1, 1` gives `2, 2, 3` and `8, 5, 6`,
their sums are `7` and `19`, and adding each sum back to its own row is `9, 9, 10` and `27, 24, 25`.
So **a two-buffer graph parameter with per-dispatch structs records and runs**, which is what round
2's grey area left open; the warning recorded there still stands for the **one-buffer** case, which
nothing has re-tested. A layer is therefore expressible — what it costs is its dispatches.

**An attention score reads the resident cache inside a recorded step.** Two sequences of three
positions of two columns, each query dotted with its own sequence's three slots — `1, 1, 2` for the
first and `2, 0, 1` for the second — at **one submission**, with the cache handed in as the resident
`upload` produced (`crates/lichen-serve/tests/attend_step.rs`). It is two kernels: one weighs every
cache word by its sequence's query, one lane per word, and the second folds over a slot's columns and
sums it, so every fold reads a single buffer. **Softmax and the value mix are not here**, and the
reason is structural rather than a gap in the language: each needs a **per-sequence reduction** that is
its own dispatch, so a full attention step is five kernels — weigh, sum, max, exponentiate-and-sum,
mix — where this is two.

**Softmax attention runs as seven kernels in one submission.** Weigh, sum, peak, exponentiate,
total, scale, sum again — over two sequences of three slots of two columns, with both caches handed in
as residents (`crates/lichen-serve/tests/softmax_step.rs`). Measured: the step answered
`3.7283485, 4.7283487, 1.1546986, 0.5794886` against the host's own softmax
`3.7283504, 4.7283506, 1.1546978, 0.5794875`, agreement to six significant digits at an `1e-4`
tolerance because `exp` is an approximation on both sides. The count is not free: **a fold's body
reads one buffer**, so weigh, exponentiate and scale each bind what they combine and write once, and
the scale writes transposed so the last fold reads a contiguous run. That rule is what made the
earlier two-kernel and one-kernel attempts fail.

**How many dispatches a real layer needs is a reading, not a measurement.** Counting the kernels the
examples already have — `rmsnorm` 2, `rope` 3, `attention` 2, `softmax` 2, `matmul` 1, `gemv` 1,
`silu` 2, `layernorm` 3 — a transformer block assembled from them is **13 to 18 dispatches**, so a
32-layer model is **400 to 580 dispatches per token**. At the measured 0.08 ms that is **32 to 46 ms
per token, before any arithmetic** — 22 to 31 tokens a second for one sequence, and that is the floor
a service is measured against, not an estimate of its throughput. The two levers are therefore
distinct and the note should not confuse them: **batching multiplies the sequences a step serves**
(measured: 16 sequences at one step's cost) while **fusion lowers the per-dispatch floor** (not
measured, and gated by the fusion floor refusing both real shapes).

**The cache layout is fixed slots, and the step's ABI does not change when it becomes a page table.**
A sequence owns `capacity × dim` slots of one flat resident, so a lane is one slot: its sequence is
`lane / (capacity × dim)`, its position the next division, and its column the remainder. One dispatch
then writes every sequence's cache without the host naming any of them, and the whole cache stays
resident (`crates/lichen-serve/tests/batch_step.rs`): three sequences of four positions of two
columns appended one token each at **one submission per step**, with each sequence's slots untouched
by the others and the positions no sequence reached still what they were. **A page table would change
the address arithmetic inside the kernel and nothing else** — the step would still take one extent
and one metadata buffer — so the simpler layout carries the proof and paging swaps in behind this
ABI rather than redesigning it.

**Per-sequence positions ride in one `Int` buffer, and an `Int` element is eight bytes.** A batch
whose sequences are at different points runs in one dispatch: a lane's position is the sequence's
word in the positions buffer, so three sequences at positions `2`, `0` and `1` append at three
different slots in one submission (`crates/lichen-serve/tests/positioned_step.rs`), and the next step
advances each of them independently. The landmine is the width — `ScalarClass::Int` is **8** bytes
while `Float` is 4 — so a positions buffer packed from `i32` words declares a third of the elements
it holds and **every read past the first answers `0`**. That reads as a kernel defect because
position `0` is a plausible answer: pack an `Int` buffer from `i64`.

**The service is `Service`: slots admitted and retired, the cache resident, one submission a step.**
`Service::open(session, source, sequences, capacity, dim)` prepares the step once and starts every
slot retired; `admit` hands out a free slot at position zero; `retire` sets its position to
`capacity`, a value no position inside the cache equals, so its lanes copy themselves and its memory
waits where it is; and `step(keys, values)` advances every live sequence one position at **one
submission** (`crates/lichen-serve/tests/service_step.rs` — three slots, one retired and then reused,
with the resident sequence still advancing from a cache it never uploaded). Two of its invariants are
the ones a reader must not break: a reused slot keeps the previous sequence's words past position
zero, which is safe only because a reader reads positions before its own; and `capacity` and `dim`
are the **source's own literals**, since the kernel divides a lane by their product, so a different
pair addresses the wrong slots and no run can detect it.

**A batched decode step runs, and a sequence's answer does not depend on its batch.** The whole step —
the score GEMV, the greedy sampler and the cache append — over three sequences at once, in **one
submission**, returning its tokens and both caches as a three-tuple
(`crates/lichen-serve/tests/batched_decode.rs`). The assertion is a differential rather than a hand
derivation: the first sequence's token and its whole twenty-word cache slot are **identical** to the
ones it got running alone through the same prepared graph, and its neighbours have distinct keys, so
a slot that read another sequence's work would have scored differently. Two mechanics made it fit:
**a step can hand back a tuple**, one value per element in the tuple's own order; and each dispatch's
extent is **one of the step's arguments**, because an extent cannot be computed — the score is
`sequences × rows` lanes, the sampler `sequences`, the append `sequences × capacity × columns`, so
the step takes three counts and the host passes all three.

**A model's weights become a resident through a load step, and the crate now has one.**
`graph::run` needs a `ResidentBuffer` for every buffer it reads from the device, and only
`RunResult::Resident` produces one: `GpuContext` exposes `fetch` and `release` but **no upload**. So
`Session::upload(&[f32])` prepares a built-in two-dispatch copying step and runs it once, and the
resident it answers is the weights' — after which the per-token path never uploads them, which the
residency measurement above says would otherwise be 99% of a step. The compile is paid per upload
rather than per token, which is what makes the shape right for a weight load, and
`crates/lichen-serve/tests/upload_step.rs` round-trips eight words through it.

**The service serves the decode step, with the weights resident and the tokens as its answer.**
`Service::open_decode(session, source, sequences, capacity, dim, rows, weights)` prepares the batched
decode step over weights the caller uploaded once, and `step` advances every live sequence at **one
submission**, answering with its tokens (`tokens()`) beside its key and value caches — all three
residents, so a caller fetches only what it asks for. The two step shapes are told apart by an enum
rather than a flag, because their argument lists differ in arity and order: the append step is
`count, k, v, keys, values, positions`, while the decode step is three counts, then the weights, then
the caches. Measured (`crates/lichen-serve/tests/service_decode.rs`): two sequences in one step at
`(1, 1)`, the first one's token identical to the one it samples alone, and its neighbour's different
because the two keys are.

**And a decode-shaped step takes a different argument list from the append-shaped one.** The append
step is `count, k, v, nk, nv, positions`, while the whole decode step is three counts and six
buffers, because its three dispatches have three different extents and the weights come before the
caches. So `Service` as written serves the append shape, and a decode-shaped service needs its own
open — with the weights resident from the load step — rather than a flag on the existing one.

## 2. The layers are already split — below the runtime is not where the gap is

The premise this round started from was that the runtime is not low-level enough
and wants another layer under it. Reading the tree says otherwise: the split
exists, in three places, and it is the right split.

| layer | crate | what it already holds |
|---|---|---|
| the backend contract | `lichen-kernel-ir` | `ParallelBackend` (`run`/`submit`/`run_batch`/`run_over_device_extent`/`fetch`/`release`), `ResidentId`, `BufferSlot`, `BatchExtent` with its `capacity`, `Slice` and `Space` |
| the schedule | `lichen-graph-ir` | `Graph` (one node kind), `Policy::{Serial,Async,Batch}`, `Planned` — *"a whole graph resolved before the first of it is recorded"* — `Value::{Pending,Device,Host,Int}`, `Count::{Constant,Value,Device}`, `Scalar::{Constant,Value}` |
| the artifacts | `lichen-runtime` | `FusedKernel` with `write`/`read_fused_kernel` and a `FORMAT_VERSION`, `kernel_registry`, `module_cache`, `PlacementPack`, `tiling_pass`, `region_merge_pass`, `DeviceFacts` — and **nothing here takes a `Module`** |

The JIT already emits **two** binaries: SPIR-V for the device and a wasm module for
the host path. So "let the JIT produce a binary" is not a missing capability but a
missing **package** — one artifact holding both, plus the resolved schedule and the
named coupling points. That is a codec beside `write_fused_kernel`, not a codegen,
and building a native CPU emitter instead would duplicate the wasm engine and lose
the reference implementation a codegen has to be differenced against.

## 3. What is expressible today, each row with the probe that printed it

Nine readings, taken on `dev@1a7c8a55` and `dev@e5b96493`. Every probe is
print-only and stays in the tree; the command that reads all of them at once is at
the end of §8.

| # | the question | the answer | the reading |
|---|---|---|---|
| U2 | an address that is itself a buffer read | **yes** | `zz_gather_address_probe`: `y[i] = x[tbl[i]]` over `x = [10…60]`, `tbl = [5,0,4,1,3,2]` answers `[60, 10, 50, 20, 40, 30]` |
| U2b | the same, one lane past the buffer's end | **answers `0`** | `tbl = [5,9,4,1,3,2]` answers `[60, 0, 50, 20, 40, 30]` |
| U7 | a fold's trip count read from a buffer | **yes** | `zz_dynamic_trip_probe`: one compiled kernel answers `7.0` at `lim = 3` and `31.0` at `lim = 5` |
| U8 | a fold recorded into a graph, its bound an earlier node's output | **yes** | `zz_graph_bound_probe`: `7.0`; moving one numeral in the producer (`+1` → `+10`) moves the consumer to `63.0` |
| U8b | an out-of-buffer read inside a fold | **answers `0`** | that `63.0` folds twelve terms over six elements: `1+2+4+8+16+32` |
| U8c | a one-node graph's own return | **refused** | `two answers met on one node: GraphValue(1) against GraphValue(0)` |
| U1 | a write at an offset into the output | **the offset applies; the slot is `count` long** | `zz_subrange_write_probe`: `off 2` answers `[0,0,1,2]`, `off 4` answers `[0,0,0,0]`, and a collect declared `array{Int,8}` still renders four |
| U1b | a runtime scalar inside a recorded graph | **yes — the open item is stale** | `zz_graph_scalar_probe`: `off 0 → [1,2,3,4]`, `off 2 → [0,0,1,2]`; and `Scalar::Value(ValueId)`, pinned by `graph_runtime_scalar.rs` |
| U3 | argmax as three single-class folds | **yes, all-`Float`** | `zz_argmax_probe`: max `9.0`, then `[…, 5.0, …]`, then token `5.0`; the mixed spelling is refused below |

**And the whole step is one graph, measured** (`decode_step_graph.rs`, on the device):
`examples/generate.lichen`'s three kernels — the score GEMV, the greedy sampler whose
fold carries an `Int` slot beside two `Float` ones, and the cache append — record into
**one** graph and answer that file's hand derivation of step zero,
`[1.0, 2.0, 2.0, -1.0, -1.0]`; and a **second** `graphrun` reads the first's answer
through `first[0]` and answers step one, `[1.0, 2.0, 2.0, 0.0, -1.0]`. So a step is
**one submission**, and a loop over steps needs **no host round trip** — the shape this
note is built on is expressible today, in the language, with no copies.
[llm-operators](llm-operators.md) had measured the append-then-score **pair**; the
third kernel, and two graph runs chained, are what this adds.

**One defect rides with it, and it has a one-token workaround.** A graph run's answer
loses its element class — `raw[raw 1.0, …]: ?a` against the direct chain's
`array{Float, 5}` — which is
[compute-runtime-scalars](compute-runtime-scalars.md)'s open item 4
([class-channel](class-channel.md) §2/§3). An **annotation on the collect** restores
it, `compute.collect (…) : array{Float, 5}`, which is the spelling
`examples/generate.lichen` already uses to read its sampled token. So it costs a
spelling and not a capability.

Four more facts were read rather than measured, each with its own witness in the
tree:

- **A kernel-level loop with a carried accumulator runs on the device.**
  `a_hand_built_loop_carries_its_accumulator_on_the_device` builds `CondBr` plus a
  backedge carrying two block parameters and answers `3`. So the emitter is not the
  limit on a loop — the language surface is. This corrects the reading that a fold
  body's refusal of `if` was a backend limit: it is the lowering's.
- **A device-computed extent needs no host readback.** `Count::Device` plus
  `cmd_dispatch_indirect`, measured in [compute-graph-jit](compute-graph-jit.md):
  a count of 40 out of 128 ran **one** workgroup where `Constant(128)` ran two.
- **Outputs are write-only, and `.in`/`.out` are disjoint by construction**
  ([shared-memory-and-barriers](shared-memory-and-barriers.md) §3): no global buffer
  is both read and written by one body, and the unordered pair is *unspellable*
  rather than forbidden.
- **A launch that stages a slice must cover whole workgroups**
  (`RunError::StagedLaunchNotAWholeWorkgroup`), so a tiled kernel's extent is a
  multiple of 64 or it stages nothing.
- **The graph leg needs an installed backend, and an example cannot install one.**
  Measured through the CLI, which is the path the example suite takes:
  `compute.graph` refuses with *"this graph dispatches to a device, but no compute
  backend is installed: a host program has to install one before a run can be
  dispatched"*, and `readme::program_output` installs nothing. So
  `examples/generate.lichen` **stays host-only**, and the program that installs the
  backend is not an example — it is the service. That is a better reason for
  `lichen-serve` than convenience: without it the graph leg is unreachable from
  anything the tree runs.
- **A language-level run cannot report its own schedule counts.** `GpuContext` counts
  `submissions` and `fence_waits`, but `install_default()` deliberately does not hand
  the context back — *"a second handle would only be a second way to be stale"* — so the
  handle a language-level test can hold is a **second** context whose counters read
  zero. Per-step counts are therefore **derived from the policy's contract**, not
  counted: one submission and one fence per node under `Serial`, one of each per graph
  under `Batch`, both measured for two nodes in `graph_on_device.rs`. The cost
  consequence of the graph leg is two submissions and two fences per step, which at the
  measured 0.036 ms dispatch floor is **~0.07 ms per token** — small beside a real
  layer's compute, and binding only where a step's dispatch count approaches the
  64-per-submission cap.

## 4. The rule those readings impose

`Scalar` and `Count` look alike and are not. `Count` has three arms including
`Device { value, index }` — *"where the number is, not the number"*. `Scalar` has
two, and its invariant says why the third is absent:

> **There is no device arm, because a scalar is read when the node runs and not out
> of a buffer on the device.**

A runtime scalar travels in a **push-constant block**, which the host fills while
recording. So the design rule for a step falls straight out:

> **Anything the device must vary per step has to live in a buffer. A scalar is
> host-filled at record time and cannot be a device answer.**

That rule costs nothing, because the three things a decode step varies are exactly
the three U2, U7 and U8 prove a buffer can carry: the **address mapping** (a page
table, an embedding index), the **fold bound** (how much work a lane does), and a
**value an earlier node in the same submission produced**. The one thing it forbids
is a per-step scalar that the device computed — and §5 shows nothing in a decode
step needs one.

Two corollaries worth stating where they will be read:

- **Fixed capacity, data-driven shape.** Launch over the maximum extent and let the
  buffers decide how much of it means anything: an out-of-buffer read answers `0`
  (U2b, U8b), which is the mask a padded batch needs, and it works through an
  indirect address and inside a fold. This is why continuous batching needs no
  recompilation per batch size.
- **Zero extent is refused by name, and that is not the end of the stop test.**
  `zz_zero_extent_probe` prints, in both the plain and the recorded form: *"the run
  covers no indices, so there is no buffer for it to write: an empty dispatch is a
  program with nothing to do, not a smaller run."* But `dispatch.rs` computes a
  device-held extent's count as its **`capacity`**, not as `0`, and the dispatch it
  records is indirect — so a `Count::Device` reading a device-written `0` does not
  reach that refusal. **Measured** (`indirect_zero_extent.rs`): the run succeeds, the
  gated step dispatches nothing, and under `Policy::Batch` the graph still costs **one
  submission and one fence wait** — so a stop test driven by a device flag is
  expressible at the price the step already paid. **And a gated node's output is
  unspecified**: no invocation ran, so nothing wrote it, and the zeros a probe saw are
  an allocation's leftover rather than a computed answer. Never reading a gated step's
  output is the one rule this pattern owes.

## 5. Cross-step scheduling in the graph: three forms, and which is blocked

The question that prompted this round was whether a graph, which describes one
input to one output, can hold the loop a service actually is. Three forms, and they
are not variants of one another:

**A — one step per graph run, the loop above it.** Works today, with no IR change, and
with the step itself measured (`decode_step_graph.rs`): a decode step's three kernels
are one submission and one fence, the loop over steps is the language's own recursion
or the host's, and `first[0]` carries the answer forward with no host round trip. Every
per-step parameter is a `Constant` or a graph argument, which is exactly what §4's rule
allows. Two bounds remain and neither is fatal: **64 dispatches per submission**, which
is a pool size (`max_sets`) rather than an architecture and caps how many steps may be
unrolled into one run; and an early exit, which §4's indirect row supplies — a step
gated by a device-written zero extent dispatches nothing at the schedule it already had.

**B — a loop node holding a subgraph.** The shape is forced by an invariant worth
protecting: `Graph`'s node list **is** the topological order, with no sort, no cycle
check and nothing at run time looking for one, because a cycle is *unwritable*. So
the loop must be **nesting**, never a back-edge — a node that holds a `Graph`, with
loop-carried values as an explicit binding table rather than as edges. What blocks
it is narrower than that: a graph-level loop needs its iteration index inside the
body, and there is no node that produces a number
([compute-graph-jit](compute-graph-jit.md), *Not yet*), so the index has nowhere to
come from. Form B is a real change and it is **not** the one to make first.

**C — the loop inside one kernel.** The emitter already carries values across a
backedge (§3's witness), and U7 proves the trip count can be a buffer read, so a
step loop needs no scalar channel at all: the position, the write offset and the
stop flag are block parameters, incremented on the device. **This is the form the
evidence favours**, and §6 records why nothing in the contract blocks it.

The reason C beats B is the cost model, not elegance: B re-runs a subgraph per
step, so it pays a dispatch per step; C pays one dispatch for the whole chunk. At
the measured floor of ~0.036 ms that is the difference between a step budget and a
token budget.

## 6. In-place is a backend decision, not a contract change

A KV cache is appended to, so one dispatch has to read and write one device buffer.
This note first filed that as unspellable and called for the buffer-version spine of
[shared-memory-and-barriers](shared-memory-and-barriers.md) §3. **Both halves of
that were wrong**, and the correction is worth writing out because it removes a
whole class of work:

- **`.in`/`.out` describes a parameter, not where a buffer lives.** The IR states
  how many outputs a fragment has (`KernelFragment::outputs`); it never states what
  device memory they occupy, and `BufferSlot` describes inputs only because an
  output has no id until it is produced. So the disjointness of the two role paths
  is a fact about the *payload the frontend describes* and says nothing about
  aliasing in memory.
- **The backend already decides placement, and already reuses.** `run_chain`
  allocates **two** buffers for a whole chain, ping-pongs them so link `i` reads what
  link `i + 2` wrote, and widens the trailing barrier to make that order sound — a
  placement decision taken in `lichen-compute-gpu` out of the chain's shape alone.

So **output aliasing** — one device buffer under both an input slot and an output
slot — is that same decision taken one step further, and it dissolves both halves of
the cache problem at once:

- there is no **copy**, because there is nothing to copy into;
- the **length** stops being the extent. A slot that aliases a resident is as long
  as that resident, because the backend is the one constructing the descriptor
  ranges. U1's *"the slot is exactly `count` long"* is a fact about the
  fresh-allocation path and not about slots at all — which is also why `plrun`'s
  allocation at `count` and `BatchExtent`'s separate `capacity` were two spellings
  of one gap: aliasing closes it without either of them moving.

### The rule that makes it sound

An alias lets one dispatch's write reach another of its own reads, so the safety
condition **is** the barrier question — and the analysis already exists, as
`barrier_scan` in `crates/lichen-kernel-ir/src/barrier.rs`:

> **A global input may be aliased with a global output iff every read of that input
> is by the lane that writes the same address.** The scan's fact 2 (the two index
> operands are the same `ValueId`) proves it, its conservative fact 1 (the address
> intervals are disjoint) proves the rest, and anything else is **refused**, not
> aliased.

Two more conditions, both cheap and both refused by name rather than assumed: the
**element class and width agree** — a slot is read as its class's elements, the same
reason `Value` carries a class — and the input has **no other live consumer among
the graph's later nodes**, so an alias cannot clobber a value somebody still reads.

The conservatism runs in the safe direction for exactly this use: the scan
over-approximates overlap, so "no barrier" is a proof of the aliasing condition
rather than a guess about it. The failure it must never have — an aliased pair
called safe that is not — is the shape this repository treats as worst, which is why
the rule is stated as a refusal first.

### Where the decision lives, and the part that is unmeasured

Three layers, split by what each can see:

| layer | what it contributes |
|---|---|
| `lichen-kernel-ir` | the **contract** gains an output placement: today nothing can even *ask* for an alias, because `BufferSlot` is inputs-only and an output is always freshly allocated |
| `lichen-graph-ir` | **whether** to alias: live ranges are a graph fact, and `Planned` already resolves the whole graph before the first of it is recorded |
| `lichen-compute-gpu` | **the binding**: one `vk::Buffer` under two descriptor bindings, and the aliased resident's own length as the slot's range |

**None of it is measured yet.** Two descriptor bindings naming one `VkBuffer` is
ordinary in Vulkan, but this emitter writes through an `OpAccessChain` on a
per-binding `OpVariable`, and whether such a module answers correctly when the two
bindings are one allocation is a claim to measure and not to assume. The probe needs
the contract's output placement to exist before it can be written at all, which is
why the contract is the first of the three.

And it fixes the shape of the first real step more cheaply than the spine would
have. A page of K/V for the chunk being generated can now simply **be** the cache
page, the pages before it are read-only inputs reached through a page table by U2's
indirect address, and so **chunked decode** — a page's worth of tokens per dispatch
— needs the aliasing rule and nothing else. `Shared` becomes an optimisation for the
current page instead of a requirement, which is the better place for it: a
requirement that only existed because the copy did.

## 7. The layering, and the decisions this round was given

| layer | owner | what it holds |
|---|---|---|
| the service shell | the host language, **not lichen** | HTTP, the scheduler, the tokenizer, the sampling policy, the page allocator |
| the session | **a new crate, `lichen-serve`** | the resident weights, the cache, the per-step metadata block, the loop over steps |
| the schedule | `lichen-graph-ir` | one submission per step, under `Policy::Batch` |
| the binaries | SPIR-V and wasm | already emitted, already versioned |

Three verdicts were given and are recorded here rather than left in a transcript:

1. **The session layer is a new crate, not a module of `lichen-runtime`.** The
   alternative was `lichen-runtime`, whose own invariant (*nothing here takes a
   `Module`*) the session would satisfy; the choice against it is that a session
   owns residency and lifetimes, which is a different subject from a pass over IR.
   **And §3's measured row is the stronger reason**: the graph leg refuses without an
   installed backend and an example cannot install one, so the session is the only
   place in this tree where a graph-leg step can run or be measured at all.
2. **The host side becomes a `ParallelBackend`.** The cpu path today is the wasm
   engine inside `lichen-compute`, which is *driven by a compiled module rather than
   by a submitted run*, so a `"cpu"` graph is refused by name. The way in is an
   **adapter** whose `submit` is synchronous and whose `Pending` is already settled —
   not a rewrite of the engine. Two consequences to design against: nodes must then
   carry a backend, which turns today's build-time refusal of a mixed graph into a
   routing decision and is **an invariant change, to be stated explicitly when it
   lands**; and a host node inside a batch is a **drain**, because *"a node that
   reads a dispatch's output has to wait for it and fetch it, and no arrangement of
   submissions removes that"*. So the budget is one host node per loop, ideally none.
3. **M0 before any of it** — measure what is expressible before designing against
   it. This note is that round's output, and two of its rows corrected claims this
   same session had already written down.

## 8. What is open, in the order the evidence puts it

1. **Output aliasing, in the three layers §6 names.** The `lichen-kernel-ir` entry
   first, because nothing can ask for an alias today; then the graph's liveness
   decision; then the backend's binding. The probe — one `VkBuffer` under two
   descriptor bindings — cannot be written before that entry exists, which is why
   §6 puts the contract first.
2. **A surface for a kernel-level loop with carried state.** The IR expresses it and
   the emitter's module for it is validated
   ([compute-runtime-scalars](compute-runtime-scalars.md), open item 5); what is
   refused is the lowering's placement of a destructured state.
3. **The `ServiceImage` package**: a codec beside `write_fused_kernel` holding the
   kernels, the serialized graph, the residents with their initializers, and the
   typed coupling holes. `Graph` is already plain data — kernel ids, edge numbers
   and counts — so this is serialization, not a new representation.
4. **`MAX_DISPATCHES_PER_SUBMISSION`.** 64 is a `max_sets`; raising it is a pool
   sizing decision, and form A's cap on N moves with it.
5. **argmax's class.** The token comes out `Float` (U3), because one kernel cannot
   read `Float` and write `Int`: *"instruction 8 mixed an integer and a float in one
   operation"*. An `Int` min-fold works on its own. So a device-side embedding
   lookup needs either a fourth dispatch to convert, or `float2int` at the point of
   use; a host-side readback does not care.
6. **The graph step's recipe — isolated this round, and it is two rules over one defect.**
   `compute.rs` numbers a recorded dispatch's input slots as the **rank of each role path in
   `scalars ++ inputs ++ outputs`, sorted by path**, over **the kernel's own parameter**, and
   `place` hands that number to the graph unchanged as a value id. So a step works only when
   its own parameter presents the same cells as the dispatch's, in the same order, and each
   is read. One rule is established by making the sentence disappear, and a second candidate is
   recorded beside it with the evidence against it:
   - **Read the extent argument (`s.n`).** A body that passes a literal `.n` never reads that
     cell, the step's cell list loses it, every buffer input shifts down one slot, and node 0
     is handed the extent — *"node 0 was given value 0 as a buffer, and that value is a
     number"*.
   - **The graph must register the cells the dispatch names.** A dispatch whose argument was a
     *different* struct still failed in the probe with `s.n` read — but
     `decode_step_graph.rs`'s real three-kernel step, whose graph parameter holds **four**
     buffers and whose three dispatches each read a subset through **its own** struct, records
     and runs. So this second rule is **narrower than the evidence**: what is established is the
     first rule and the node-count one below, and the exact cell-matching condition is still to
     be pinned down. It is recorded here as a warning rather than as a rule, because a reader who
     took it as one would exclude the shape that already works.
   A **one-node** step fails separately, with *"two answers met on one node"*, so a step needs
   at least two nodes. With the first rule obeyed `zz_graph_step_shape_probe.rs` prints
   `[4.0, 8.0, 12.0]` for a two-node step. **The defect to file is the conflation itself**: a
   kernel's parameter cell is not a graph input slot, and the recording treats one as the
   other; the fix belongs there and not in the backend.
7. **`compute.graphrun` reaches `Async` and never `Batch`, and `Async` saves nothing.**
   `compute.rs`'s `GRAPH_POLICY` defaults to `Policy::Async`, and its own comment says *"`Batch`
   needs a host that asks for it"* — `set_graph_policy` is that ask. Measured on the same three
   dispatches: through the language default the run costs **4 submissions and 5 fences**, and
   through a session that sets `Batch`, **2 and 2**. So the language surface cannot reach a
   one-submission step at all; the host that can is `lichen-serve`, and that is now the crate's
   measured reason to exist rather than an anticipated one. **What stays open is whether the
   language surface should be able to ask for `Batch` at all** — today only a Rust host can,
   which means an example that used it would be unrunnable by the example suite. Keeping the
   probe is deliberate: the failing shapes are the evidence for the defect, and the defect is
   not fixed.
8. **Compile once, step many — and the step is already a public call.** Every
   `lichen_language` entry that reaches a program compiles and runs in one call
   (`evaluate_raw`, `run_at`, `evaluate`), and `compile_with_imports_at` hands back a `Report`
   whose module runs its root once and answers the cached value afterwards. So
   `Session::run(source)` recompiles per request: it is a measurement harness, not a service
   loop. **The step a service repeats is `lichen_compute::compute::graph::run`**, and it is
   public:
   > `pub fn run(graph: &Graph, backend_name: Backend, arguments: Vec<RunArgument>, policy: Policy)`
   - the **graph is the caller's** — looked up once, held, run many times;
   - the **policy is an argument**, so `Batch` needs no thread-local;
   - the arguments are `RunArgument`s, and **`RunArgument::Resident { … }` means weights and
     the cache stay resident across steps** with no re-upload — the channel the language
     surface does not have, since a program's buffers are built inside its own source;
   - the result **stays resident**: *"a value the device wrote stays a resident id and the
     caller downloads it when it asks"* — which is exactly the one-readback-per-step the cost
     table above measures.
   Its own doc states the caller's obligation too: the graph handed in is *"the one a pass
   handed back"*, so a session that never consults a pass runs without one. That is the
   default and worth saying rather than assuming.
   **The crate now does that**: `Session::prepare(source)` holds the graph,
   `Prepared::step(arguments)` runs it, and `Session::fetch(resident)` is the readback —
   measured at **one submission and one fence per step**, the compile paid once at load. Two
   gaps are named rather than hidden: preparing a **source** compiles it twice, because the
   refusal is asked for first (`run_at` panics on a refused program) and `line_starts` is not
   public, so the one-pass form is unreachable; and `run(source)` stays as the harness the
   cost table above is measured with.

The seven probes, in one command:

```bash
cargo test -p lichen-compute --test lichen_compute -j 3 -- --exact \
  lichen_compute::zz_gather_address_probe::probe_the_gather_address \
  lichen_compute::zz_dynamic_trip_probe::probe_the_buffer_read_trip_count \
  lichen_compute::zz_graph_bound_probe::probe_the_bound_a_producer_wrote \
  lichen_compute::zz_subrange_write_probe::probe_the_subrange_write \
  lichen_compute::zz_graph_scalar_probe::probe_the_graph_scalar \
  lichen_compute::zz_argmax_probe::probe_the_argmax_folds \
  lichen_compute::zz_zero_extent_probe::probe_the_zero_extent --nocapture
```

## 9. Three claims this round corrected

All three were steering, and all three are recorded so the next reader does not
re-derive them. The first two were stale in the tree; the third was this note's own
first position, retracted by the reading that `.in`/`.out` is frontend payload.

- **[compute-runtime-scalars](compute-runtime-scalars.md)'s open item 1 is stale.**
  It says a `KernelNode` *"carries its extent and its buffers and nothing else"* and
  that *"the graph IR has no value to put in"* the scalar block. `KernelNode.scalars`
  is a `Vec<Scalar>`, `Scalar::Value(ValueId)` is an edge, `Runner` resolves the list
  against the value table and refuses a node whose list is the wrong length, and
  `graph_runtime_scalar.rs` pins that both scalars *"cross as edges of their own"*.
  What remains true is narrower and is §4's rule: there is no `Scalar::Device`.
- **`lichen-graph-ir`'s own module docs name a node kind that does not exist.**
  `lib.rs` says *"two node kinds and no third: a kernel node … and a native node
  making host data"* and `graph.rs` calls `Graph` *"a DAG of kernel launches and
  host calls"*, while `Node` has one variant and
  [compute-graph-jit](compute-graph-jit.md) records the host-call node's removal and
  the reason. Both comments were corrected with this round.
- **This note's own §6 called in-place unspellable and called for the spine.**
  Retracted where it stood: an output's placement is not in the IR, `run_chain`
  already decides and reuses placement, and the safety condition is
  `barrier_scan`'s — so aliasing is a backend decision and neither the contract's
  role paths nor the spine has to move. The sentence this note had drafted *against*
  [shared-memory-and-barriers](shared-memory-and-barriers.md) — that serving is a
  second reason for the spine — is withdrawn with it, and that note was never
  edited, so nothing there needs undoing.
