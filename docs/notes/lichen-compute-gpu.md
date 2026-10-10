# The GPU backend for the lowered-kernel IR

> Status: current — the arithmetic/select subset of a kernel runs on a real
> device, and a `jit`/`plrun` chain can be **recorded and submitted once**
> (`run_chain`, and the `compute.graph`/`compute.graphrun` surface). What is
> *not* here is listed under [Not yet](#not-yet) rather than left implied.
>
> What this note is: the second consumer of the lowered-kernel IR. It exists
> because that IR is target-neutral, so a backend other than the wasm one can
> consume it without dragging in `wasmi` — and it records what that actually
> cost, because the answer was not the one first assumed.
>
> Points at: `crates/lichen-compute-gpu/src/spirv.rs` (the emitter and its
> refusals), `crates/lichen-compute-gpu/src/dispatch.rs` (the context, the pool
> of submission slots, staging), `crates/lichen-kernel-ir/src/lib.rs`
> (`ParallelBackend`, `Pending`, `KernelFragment`), and
> `crates/lichen-graph-ir/` (the executor).
>
> Companions: [compute-graph-jit](compute-graph-jit.md) (the recorded path and
> the scheduling model), [lichen-compute](lichen-compute.md) (the IR and the
> wasm backend), [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md) (what the
> primitive cannot express, reached by writing algorithms against it), and
> [gpu-algorithms-ladder](gpu-algorithms-ladder.md) (the evidence).

## What the split bought, and what it did not

`lichen-compute-gpu` depends on `lichen-kernel-ir` and `ash` and nothing else in
the workspace, and nothing in the workspace depends on it. A build that never
dispatches a kernel to a device never links a Vulkan loader.

**The IR needed no change to be emitted for a completely different target.** The
same SSA `KernelBody` lowers to wasm bytes and to SPIR-V, and the one place the
two disagree is the clearest evidence it is genuinely neutral:

| | wasm backend | this backend |
|---|---|---|
| buffer access | a host import, `call` a function | `OpAccessChain` + `OpLoad`/`OpStore` on a storage buffer |
| comparison result | an `i64`, narrowed to `i32` for `select` | a `bool` already |
| `KernelInstr::I32WrapI64` | **emitted** | **a no-op** *when the condition is a comparison* |

`I32WrapI64` exists *solely* because the wasm MVP's `select` takes an `i32`
condition; SPIR-V's comparison operations yield `OpTypeBool`, which is what
`OpSelect` takes, so the narrowing has nowhere to go. An IR that had encoded
either target's choice would have made the other backend wrong.

**Neither backend tracks an operand stack**, which is what let the table above
survive the SSA rewrite that deleted `LocalGet`, `Flow` and `BlockId`. Both walk
the body's blocks; the wasm one maps a `ValueId` to a slot and a type, this one
to a `Slot`.

**Structured control flow exists here too, and it is the SSA body's.** The emitter
plans blocks and emits `OpBranch`, `OpBranchConditional`, `OpLoopMerge`,
`OpSelectionMerge` and **one `OpPhi` per block parameter**, which is what a
block's `params` were always for. A loop header is recognised from the graph — a
block some block it transitively reaches branches back to — and its exit is the
arm that cannot reach it again; a header with no exit, or with both arms reaching
back, is a `ControlFlow` refusal naming which. What a loop changes for a
**dispatch** is the write rule (see the tail-lane obligation below), and that is
the only place this target is still narrower than the IR.

### The two type facts the IR does not carry

The IR is untyped, and this target is not: `OpTypeBool` and `OpTypeInt` are
distinct, and a module that mixed them is rejected by the driver. Two facts
therefore live in the emitter rather than in the IR, and both are the *same*
fact — that the language says a comparison is the `0`/`1` scalar, which is not
what this target's comparisons produce:

- **A `bool` reaching a scalar position is materialised** (`as_scalar`:
  `OpSelect` over the class's `1` and `0`), and a **scalar condition is
  converted** (`as_condition`: `OpINotEqual` against zero — non-zero is true,
  which is what wasm's `select` means by its `i32`). Each slot on the emitter's
  stack records which of the two it holds, because "was this a comparison" is
  knowledge only the walk has. The common shape — an `if` over a comparison —
  needs neither, and that is why `I32WrapI64` is a no-op there; the operators in
  [operators](operators.md) are what made the other shapes reachable
  (`(a < b) & c`, a stored predicate, a comparison of a comparison).
- **The fragment's integer type is unsigned** — `OpTypeInt 64 0`. This is not a
  style choice: `OpUDiv`, `OpUMod` and `OpULessThan` require operands whose
  signedness is `0`, so a signed type with those opcodes is an invalid module
  rather than a wrong answer. It is also simply true — an `Int` is a
  machine-sized unsigned integer, so `/`, `%` and the order comparisons are the
  unsigned ones, and declaring the type signed would have *required* the signed
  opcodes and their silent divergence above `2^63`.

A module's arithmetic class is baked in — it selects the opcodes a `Bin` reaches
and is the fallback for a slot its class lists do not reach — while a buffer's
element type, stride, block struct and variable type are read per buffer. Every
module declares both element types, so **a crossing always has the type it
needs**; the price is that a float module's `Int` data is 32-bit where the wasm
target's is 64-bit, so the two diverge past 2³². See
[kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) for the class rules
and the recorded truncation.

**`IntWidth` is a declared fact, not an assumption.** A buffer *index* needs no
capability — SPIR-V addressing is 32-bit by default with a 4 GB range, and one
`plrun` is bounded at 8 MiB. Only the *data* width needs a device capability
(`shaderInt64` on Vulkan), so a device without it gets `RunError::MissingInt64`
rather than a silent narrowing. `needs_int64` is what a caller checks before it
builds a pipeline.

## The language selects the backend, on `parallel` only

`compute.parallel` takes the backend as a second argument, and there is **no
default and no `auto`**:

```lichen
k = compute.parallel f "cpu"
out = compute.plrun k (4,)

k2 = compute.parallel g "gpu"
```

Two decisions carry most of the weight:

- **Only `parallel` takes one.** A `jit` kernel is launched a single invocation
  at a time, and a device is for the thousands a dispatch runs at once, so there
  is nothing for a backend to choose between. `ComputeValue::Kernel` therefore
  has no backend field at all, and `launch` is untouched.
- **No "try either" value.** A named backend that declines is a *named* failure.
  There is no third value to fall back to, because overriding the author's
  choice is the one place the language's explicit dataflow would quietly stop
  being explicit. The refusals, all by name:

  | refusal | when |
  |---|---|
  | `…is not a compute backend; name one of "cpu" or "gpu"` | a name that is neither |
  | `a compute backend must be the string "cpu" or "gpu", not …` | a non-string — caught statically, as a check error |
  | `this parallel kernel was compiled for the "gpu" backend, but no compute backend is installed…` | `"gpu"` with no backend installed |
  | `the "stub" backend declined this run: <its reason>` | a backend that declines |

The name lives on the **value**, not on the fragment, so it is *not* hashed by
`fragment_digest`. Two programs that compile the same body for different backends
share one fragment id, and the choice can never make a cache serve one backend's
module for another's.

A host program installs the backend with `lichen_compute_gpu::install_default()`.
That is the composition step, and it is the only place the two crates meet —
`lichen-compute` still does not name this crate. `install_default` is what
`lichen-language`'s tests and examples call; it is a **dev-dependency** of
`lichen-language`, so the library half still never links a GPU loader.

The chain from source to a device is proved in two halves, on purpose:
`lichen-compute`'s tests install a **stub** backend and pin the routing (that a
`"gpu"` kernel reaches the installed backend, that its answer is the answer, and
that a decline is reported rather than swallowed), while this crate's tests run a
real `KernelFragment` on the device against a hand-written expectation. The seam
test lives in `lichen-language`, because it is the one crate that can compile a
lichen program and install a backend: a first kernel writes `[10, 11, 12]`, a
second reads that buffer and doubles it, and the whole thing is checked against
the rendered output.

**A green device run is evidence that nothing crashed, not that a device
agreed.** Every device-backed test returns early and prints the reason when it
cannot open a context — and that is **any** failure to open one, so a genuinely
broken backend on a GPU machine skips rather than fails. `--nocapture` is how a
skip is read, because libtest captures stderr for a passing test.

## The emitter is hand-written, and that is a deliberate trade

Roughly twenty opcodes. A builder crate would remove the spelling mistakes but
**not** the failures that actually happen, which are all in the *module
contract*: the section order, the declared capabilities, the decorations, and
what the entry point's interface lists. A builder validates none of that.

What removes that risk is offline validation, and it is why `spirv-val` is the
development tool for this crate rather than a runtime hope. `tests/spirv_validation.rs`
**runs** the validator rather than depending on it: it emits a fragment, pipes the
words to `spirv-val --target-env vulkan1.1 -`, and asserts the module validates.
That check needs no device, and it **skips loudly** (rather than passing quietly)
when the tool is not on `PATH` — with the caveat that the fragment in the test is
a copy of the one in `examples/emit-spv.rs`, so a change to the example must be
mirrored there.

The errors that shape actually produced, each named by the validator or the
device, are the module contract in list form:

- `OpConstant` emitted **inside** the function, where it is not module-scope.
- A `StorageBuffer` variable typed as a bare `OpTypeRuntimeArray` — Vulkan
  requires a struct (`VUID-StandaloneSpirv-Uniform-06807`), so a runtime array is
  a struct's final member and the struct is decorated `Block` and laid out.
- `OpUConvert`'s result type signed, where it must be unsigned.
- **A `StorageBuffer` access chain needs three indices**, not two — the buffer
  struct's member, the element, *and* the element struct's member, because the
  element is itself a struct. This one the validator could not see; the device
  caught it.

## Two position spaces, and the silent bug they caused

A read's position (`cfg_pos`) counts over the **inputs**. A write's position
(`out_pos`) counts over the **outputs** — the IR documents it as the write's
*emission ordinal*, the index function's codomain position. They are two
distinct spaces, and collapsing them into one list of buffers is a bug that
produces **no error at all**: a write at position `0` reaches **input 0**, every
run writes its result into its own input, and the output comes back as
plausible-looking zeroes with no Vulkan error and no diagnostic. `tests/refusals.rs`
pins the separation, including the negative case where a write names an output
that does not exist.

Related and worth knowing: a parallel fragment's `param_shape` is
`(config, index)` — **two leaves, however many inputs there are**. Extra inputs
are reached through a read's position, not through further parameters, so the
index is always parameter 1, and a fragment with three parameter leaves is not a
shape the compiler produces. (This is the fact the graph runner's old
`flat_arity - 1` input check got wrong; see
[compute-graph-jit](compute-graph-jit.md).)

## The tail-lane obligation

A dispatch covers `LOCAL_SIZE_X` (64) invocations per workgroup, so the last
workgroup runs lanes past the end. The shader has **no bounds test**, and that is
deliberate: a bounds test is a branch every lane takes, on the hottest path, to
guard indices the host already knows.

Instead every buffer is allocated rounded up to a whole number of workgroups and
the padding is zeroed — inputs so the surplus lanes' reads are in bounds and
return `0`, outputs so their writes land in padding rather than past the end.
Only the first `count` elements are read back. Out-of-range access is impossible
by construction rather than by a runtime test, and the shader stays branch-free.
The tests pin **both** ends of that: a count that is not a multiple of 64 (so the
padding is exercised) and one that is exactly a multiple (so it is not).

**An uninitialised output buffer is safe only because a write is reached by every
invocation.** `dispatch` allocates output buffers and does not initialise them, so
a lane that never reaches its `BufferWriteCall` leaves a hole the reader would
take for data. A **selection** arm is fine: it runs on exactly the lanes that took
it, and a `select` is a value rather than a branch. **A loop body is not**, because
a zero-trip count is a lane that never runs it — so a write inside a loop is
refused by name (`SpirvRefusal::WriteInsideLoop`) rather than emitted, and the
straight-line body the invariant rests on is the one the subset produces.

## Acceptance

The criterion is bit-for-bit agreement, checked three ways per run: against a
**hand-written expected vector**, and against an independent CPU reading of the
IR. The expected vector is the point — comparing the GPU only against another
implementation of the same reading would pass a fragment that both misread.

The refusal tests need no device, deliberately: a refusal that only appeared once
a GPU was present would be untestable on a machine without one.

## Staging must be cached, or the run is refused

The single biggest performance fact about this backend is not in the dispatch at
all — it is in which Vulkan memory type the staging lands in, and the rule is
easy to get wrong in a way that still produces correct results.

On the development machine, the memory types on offer are:

| index | flags | heap | what it is |
|---|---|---|---|
| 1 | `0x0001` | VRAM | device-local, not host-visible |
| 3 | `0x0006` | system RAM | host-visible + coherent, **no cache** |
| 4 | `0x000e` | system RAM | host-visible + coherent + **cached** |
| 5 | `0x0007` | VRAM | BAR aperture, device-local + host-visible |

Selecting "the first type that is host-visible and host-coherent" picks **index
3**, which on a discrete GPU is system RAM reached over PCIe with nothing behind
it. Every host touch on it is a round trip, so a run pays two uncached transfers
per element and the whole backend lands ~15× behind a scalar loop — while
reporting success at every step. The fix is one flag (`HOST_CACHED`), and on the
development machine the difference is 55 ms against 4.8 ms at a million elements.

So `cached_memory_type` **requires** `HOST_CACHED` and refuses a device that has
none, rather than falling back. A fallback would be *correct* and would silently
put the backend back where it started, with nothing in the output to say so. This
is the same rule the rest of the crate follows: no silent fallback onto a path
that changes what the numbers mean.

## What the GPU actually costs, and where the crossover is

Two numbers decide the design, and both are measured on the development machine:

- **A dispatch's fixed cost is the round trip, and the kernel inside it is not.**
  An empty dispatch — one workgroup, nothing but the dispatch itself — reads
  **0.04–0.05 ms** across runs of the same binary. A dispatch reading a **host**
  input pays about 0.007 ms more than one reading a **resident** buffer, because
  the host input `memcpy`s into staging and the device copies it out in the same
  submission. Using the host floor on a chain link — which never uploads —
  credits the link with an upload it never did, and the resulting figure can
  exceed 100%, which is a wrong divisor rather than a noisy one.
- **The per-link cost barely moves with count** — roughly 0.04 ms to 0.12 ms
  across a 1024× range — because the round trip is fixed. So the same fixed cost
  is ~92% of a 16-link chain's link at a thousand elements and ~30% of one at a
  million: **recording a chain as one submission is worth 65–81% of a 16-link
  chain at counts up to 65 536, and ~10% at a million.** The single row every
  early estimate used — a million elements — is the one least favourable to
  fusing.

`run_chain` records a chain into one command buffer, submits once and waits once,
and it is faster than the unfused chain at every count measured: **5.7× at 1 024
elements, 5.4× at 4 096, 4.7× at 16 384, 3.0× at 65 536, 1.5× at 262 144, 1.23× at
1 048 576.** It is verified against the kernel's **closed form** (`2^n * x +
(2^n − 1)`), derived rather than run — a second CPU copy of the same loop would
agree with a mis-ordered chain just as cheerfully.

Three design facts the measurement bought, all of them load-bearing:

- **A recycled buffer needs no clearing**, because every consumer writes all of
  it: an output is written across `[0, padded)` by the shader's straight-line
  body, and an input is uploaded across `[0, count)` with the tail cleared on the
  device.
- **Allocating one output buffer per link is not merely bigger, it is three times
  slower than not fusing at all.** A link reads the buffer the link before it
  wrote, so two buffers ping-ponged round a chain are enough; a pool that has to
  hold a buffer per link is empty at the start of every call, so every call
  allocates the lot and discards most of it. The allocation is the cost, and the
  same lesson arrives from the other direction: recycling released device buffers
  was the largest single win in the fixed cost (about 4×), while reusing seven
  per-dispatch objects was about 10%.
- **There is deliberately no minimum-count gate.** The measurement says the
  answer is not a count: 65 536 elements loses at every chain length while
  1 048 576 wins from four links on. A count threshold would be a number invented
  to look careful about a quantity that is not the one that decides it.

The GPU still loses on a **single cold dispatch**, and that is not a bug: the
first upload is 8 MB across PCIe, which costs about what the scalar loop costs
outright. The crossover is at **about four links** at a million elements, and it
climbs from there. Read the ratio columns loosely — one sample per point is not
monotonic in links, and the CPU denominator stands in for a thread pool that is
not being measured.

## Two corrections, because the first answers were wrong

Two claims about GPU compute shaped this work, and **both were wrong at first**.
They are recorded because the wrong versions are the plausible ones.

**"A GPU backend needs the kernel body to be index-free."** False. A compute
shader indexes freely, and a dispatch giving each invocation one index maps
straight onto the existing index-based parallel kernel — one invocation per
index, the body unchanged. Being index-free is a precondition only for
*vectorising across a lane group*, which is a different and much larger win than
parallelism. See [attributes.md](attributes.md#nothing-consumes-it-yet).

**"The 64-bit integer width is the blocker."** Mostly false, in the way the
`IntWidth` rule above states: a buffer index needs no capability at all, and only
the data width does.

## Not yet

Named rather than implied, because each is a decision rather than a gap:

- **Cross-kernel calls.** `SpirvRefusal::CrossKernelCall`. Needs several
  functions in one module and a call graph; the refusal names the callee and the
  instruction position.
- **More than one result.** A compute shader communicates through its storage
  buffers, so a fragment whose body leaves one value is required here
  (`ResultArity`).
- **A non-index parameter read.** A buffer is *bound*, not passed, so there is no
  value for it to hold (`NonIndexParameter`).
- **A runtime scalar.** A dispatch pushes the extent alone, so a fragment with a
  runtime scalar is **refused by name** (`RunError::ScalarsNotPushed`) rather
  than dispatched with a leaf missing. The CPU path passes the leaf list; the
  device half is not written. See
  [compute-runtime-scalars](compute-runtime-scalars.md).
- **`Batch` as a submission policy.** A graph under `Batch` is refused by name,
  because a backend can only fuse if the contract can hand it several dispatches
  for one command buffer, and `submit` records one run. `run_chain` is that
  fusion at depth one; the general capability is not in the contract.
- **A general graph builder.** The `compute.graph` operator, the `Graph` value
  and the lowering exist; what remains is the shape the design records
  ([compute-graph-jit](compute-graph-jit.md) §"Not yet").
- **Freeing a resident buffer from the language.** There is **no per-value
  release**, and this is deliberate rather than an oversight: the value set has
  no per-value destructor, and a `Buffer`'s arena payload has no individual free
  either — so a device buffer is scoped exactly like a host one. The cost is real
  and worth stating plainly: **resident buffers accumulate for the life of the
  backend.** The backstops are `GpuContext::drop`, which reclaims everything, and
  a refused allocation once the device is full, which names itself.

## The pool of submission slots, and the pending submission

`ParallelBackend::submit` records a run, hands it to the queue, and returns a
`Box<dyn Pending>` **without waiting**. The ids it will produce are on
`Pending::outputs`, and their contents the device has not promised to have
written — they exist so the next node can be recorded against this one's
buffers, and that is the chaining the overlapping schedule needs. **A demand
point is `wait` followed by an ordinary fetch**, so ids a *host* ever sees have
been waited for while the ids the *executor* chains on have not.

**`submit` has a default that calls `run`.** A backend that cannot overlap
anything still satisfies the contract; it just never collects from it. Requiring
an implementation would mean every stub and every future backend wrote a method
whose only correct body is the one that does nothing, and a caller could not tell
"cannot overlap" from "has not implemented it yet". The GPU is the only backend
here that overrides it, and it can: it has a pool, so a submission handed back is
a **different slot** from the one the next submission takes.

`GpuConfig { slot_depth }` defaults to **2** and is configurable only from Rust:
`GpuContext::new()` takes the default and `GpuContext::with_config` takes the
rest. A depth of zero is refused by name, for the same reason a zero-link chain
is: it is not a smaller pool, it is none.

**One slot is a command buffer, a fence, a staging buffer and a descriptor pool,
and they live in one struct because they all die at the same moment.** That is
the whole design: a command buffer cannot be recorded while a previous recording
of it is running, a staging mapping cannot be written while a copy out of it is
in flight, and a descriptor pool cannot be reset while a dispatch bound to its
sets is executing. So "staging is per slot", "the descriptor pool is per slot and
reset only in `acquire`", and "`fetch` acquires a slot like everything else" are
properties of the type rather than rules someone has to remember.

**The claim is the lock.** `acquire` hands back a `Segment` carrying the pool's
`MutexGuard` for its whole life, so "two threads never record into one command
buffer" is something the borrow checker sees. `Segment` has three submit methods
that differ in what they leave the caller holding: `submit_and_wait` gives the
slot back, `submit` hands back a `Token` and keeps the slot claimed, and
`submit_and_read_back` reads its own staging *inside* the claim. `Token` is
consumed by the wait, so one submission is waited for exactly once and a second
wait is inexpressible.

What depth costs is VRAM and host RAM — each slot carries its own staging — not
round-robin time: at depth 2 with callers that all wait before returning (which
is every caller today), `acquire` never waits. `acquire`, `Segment` and `Token`
are private; the public entry point is `ParallelBackend::submit`, which returns a
`Pending` rather than ids, so the backend's representation never leaks into a
public signature.

## Invariants a graph must not break

Stated here because they are silent-wrong-answer rules rather than slow ones:

- **One descriptor set per dispatch, and no pool reset under a running command
  buffer.** A set is read when the submission *executes*, so one set cannot serve
  two dispatches, and a reset frees every set — including ones an earlier
  dispatch in the same command buffer is still bound to. A submission resets the
  pool once, at its own start. `MAX_DESCRIPTOR_BINDINGS` is a *per-dispatch*
  limit; the number of dispatches per submission is a second, separate limit.
- **The trailing barrier names both possible readers.** It was
  `SHADER_WRITE → TRANSFER_READ`, right only while every dispatch was followed by
  a `fetch` in another submission. A dispatch recorded after this one in the same
  command buffer reads those results as a *shader*, and a consumer outside the
  destination scope reads undefined data. The scope is therefore
  `SHADER_WRITE → SHADER_READ | TRANSFER_READ`. It over-covers the
  single-dispatch case, which is a cost — measured as not measurable, but not
  free.
- **A slot is not released until its fence signals.** A recorded-but-unsubmitted
  command buffer is clobbered by the next recording into it, and a command buffer
  whose submission is still in flight cannot be recorded into at all. This is the
  whole reason the pool exists.
- **`BufferSlot::Host` inputs are staged by `memcpy` before recording.** Reserve
  for the whole submission up front, and never share staging between slots: a
  submission that grows staging after recording has begun is a use-after-write on
  the mapping, and with several slots in flight a *different* slot growing or
  writing staging is the same hazard across slots.
- **A `DeviceBuffer` value dropped by `drop_block` never calls `release`.** Its
  memory lives until `GpuContext::drop`, the same rule as the deliberate
  no-per-value-release decision above.
