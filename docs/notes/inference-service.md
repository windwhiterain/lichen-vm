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

So the cost model a service is held to is: **per step, one submission, one fence,
and as few crossings as the control logic allows.** Everything below is about
whether a step can be built to hit it.

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
  reach that refusal, and Vulkan accepts an indirect dispatch of zero workgroups.
  **This row is read off the code and not yet measured**; §8 names the probe.

## 5. Cross-step scheduling in the graph: three forms, and which is blocked

The question that prompted this round was whether a graph, which describes one
input to one output, can hold the loop a service actually is. Three forms, and they
are not variants of one another:

**A — unroll N steps into one DAG, run under `Policy::Batch`.** Works today, with
no IR change: one submission, one fence, and every per-step parameter a `Constant`
known at record time, which is exactly what §4's rule allows. Two bounds are real:
**64 dispatches per submission**, which caps N at `64 / dispatches-per-step`, and no
early exit, since the host-known zero extent is refused. The cap is a pool size
(`max_sets`), not an architecture, so it moves; the exit is §4's indirect row.

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
evidence favours**, and it is blocked not by the emitter but by §6.

The reason C beats B is the cost model, not elegance: B re-runs a subgraph per
step, so it pays a dispatch per step; C pays one dispatch for the whole chunk. At
the measured floor of ~0.036 ms that is the difference between a step budget and a
token budget.

## 6. The one real blocker: a buffer written in place

A KV cache is appended to. Appending needs a body that **reads and writes one
buffer**, and that is unspellable (§3): outputs are write-only, `.in` and `.out` are
disjoint, and U1 confirms it from the other side — the output slot is exactly
`count` long, so an offset write past it is **silently dropped**, with no
diagnostic. That silence is a landmine of its own and belongs beside the others in
[compute-graph-jit](compute-graph-jit.md).

Without it, a cache is one of two bad shapes: a fresh buffer per step, which is
allocation churn — the tree already measured a one-buffer-per-link chain **three
times slower** than not fusing at all, all of it in `vkAllocateMemory` — or a full
copy per step, which is O(n) traffic per token and O(n²) per sequence.

The cure is already designed and staged elsewhere: the **buffer-version spine** of
[shared-memory-and-barriers](shared-memory-and-barriers.md) §3, where `write`
returns a new version of the buffer it wrote and `read` consumes it through the
`.from` field it already has. That note defers the spine to its §9 step 4, landing
it with the `Workgroup` declaration, on the reasoning that *"nothing needs it until
shared memory exists"*. **Serving is a second reason, and it is worth recording
against that sentence**: a cache written in place across a loop's iterations needs
the readable output whether or not any slice is staged.

It also fixes the shape of the first real step. A page of K/V for the chunk being
generated is small enough for `Shared`, and the pages before it are read-only
inputs — so **chunked decode** (a page's worth of tokens per dispatch, the current
page staged in workgroup memory, earlier pages reached through a page table by U2's
indirect address) fits the capability the tree is already building, instead of
needing a new one.

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

1. **Measure the indirect zero extent.** A hand-built two-node graph whose first
   node writes `0` and whose second takes it as `Count::Device`: does it dispatch
   zero workgroups, or refuse? `device_extent.rs` holds the machinery. §4's row is
   a code reading until this runs.
2. **The buffer-version spine**, for the reason in §6. Until it lands, every cache
   is a copy or a churn.
3. **A surface for a kernel-level loop with carried state.** The IR expresses it and
   the emitter's module for it is validated
   ([compute-runtime-scalars](compute-runtime-scalars.md), open item 5); what is
   refused is the lowering's placement of a destructured state.
4. **The `ServiceImage` package**: a codec beside `write_fused_kernel` holding the
   kernels, the serialized graph, the residents with their initializers, and the
   typed coupling holes. `Graph` is already plain data — kernel ids, edge numbers
   and counts — so this is serialization, not a new representation.
5. **`MAX_DISPATCHES_PER_SUBMISSION`.** 64 is a `max_sets`; raising it is a pool
   sizing decision, and form A's cap on N moves with it.
6. **argmax's class.** The token comes out `Float` (U3), because one kernel cannot
   read `Float` and write `Int`: *"instruction 8 mixed an integer and a float in one
   operation"*. An `Int` min-fold works on its own. So a device-side embedding
   lookup needs either a fourth dispatch to convert, or `float2int` at the point of
   use; a host-side readback does not care.

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

## 9. Two stale claims this round corrected

Both were steering, and both are recorded so the next reader does not re-derive
them:

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
