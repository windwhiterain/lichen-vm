# The GPU backend for the lowered-kernel IR

> Status: **implemented and merged** to `dev`. One output space and the
> arithmetic/select subset run on a real device; a cross-kernel call is refused
> by name. What is *not* here is listed under [Not yet](#not-yet) rather than
> left implied. Work on launching a chain as one submission is designed and
> tracked in [compute-graph-jit.md](compute-graph-jit.md).
>
> **What the primitive cannot express**, as reached by writing algorithms
> against it rather than by reading this note, is
> [gpu-algorithm-roadmap.md](gpu-algorithm-roadmap.md); the evidence is
> [gpu-algorithms-ladder.md](gpu-algorithms-ladder.md). The short version: one
> element per lane, a straight-line body with no loop, no way to get data in,
> and no float.

This is the second consumer of the IR that
[lichen-compute.md §4](lichen-compute.md#4-codegen-bytecode-fragments-not-a-module)
split out. It exists because that split made a second backend *possible*, and
the point of this note is to record what a second backend actually cost, because
the answer was not the one first assumed.

## What the split bought, and what it did not

The split's claim was that the IR is target-neutral, so a backend other than the
wasm one could consume it without dragging in `wasmi`. That held, and it was
cheap: `lichen-compute-gpu` depends on `lichen-kernel-ir` and `ash` and nothing
else in the workspace, and nothing in the workspace depends on it. A build that
never dispatches a kernel to a device never links a Vulkan loader.

**The IR needed no change to be emitted for a completely different target.** The
same SSA `KernelBody` lowers to wasm bytes and to SPIR-V, and the one place the
two disagree is the clearest evidence it is genuinely neutral:

| | wasm backend | this backend |
|---|---|---|
| buffer access | a host import, `call` a function | `OpAccessChain` + `OpLoad`/`OpStore` on a storage buffer |
| comparison result | an `i64`, narrowed to `i32` for `select` | a `bool` already |
| `KernelInstr::I32WrapI64` | **emitted** | **a no-op** *when the condition is a comparison* |

**Neither backend tracks an operand stack**, and that is what made the table above
survive a rewrite that deleted `LocalGet`, `Flow` and `BlockId`. Both walk the
body's blocks; the wasm one maps a `ValueId` to a slot and a type, this one to a
`Slot`. `spirv.rs` still refuses a non-straight-line body — it cannot emit
`OpLoopMerge` yet — so it is the backend that is behind, not the IR. See
[loop-conversion](loop-conversion.md) §8.5 item 3.

That last row is the whole argument in one line. `I32WrapI64` exists *solely*
because the wasm MVP's `select` takes an `i32` condition; SPIR-V's
`OpULessThanEqual` yields `OpTypeBool`, which is what `OpSelect` takes, so the
narrowing has nowhere to go. An IR that had encoded either target's choice
would have made the other backend wrong.

### …and the two type facts the IR does not carry

The IR is untyped, and this target is not: `OpTypeBool` and `OpTypeInt` are
distinct, and a module that mixed them is rejected by the driver. Two facts
therefore live in the emitter rather than in the IR, and both are the *same*
fact — that the language says a comparison is the `0`/`1` scalar, which is not
what this target's comparisons produce:

- **A `bool` reaching a scalar position is materialised** (`as_scalar`:
  `OpSelect` over the i64 `1` and `0`), and a **scalar condition is converted**
  (`as_condition`: `OpINotEqual` against zero — non-zero is true, which is what
  wasm's `select` means by its `i32`). Each slot on the emitter's stack records
  which of the two it holds, because "was this a comparison" is knowledge only
  the walk has. The common shape — an `if` over a comparison — needs neither, and
  that is why `I32WrapI64` is a no-op there; the operators added in
  [operators](operators.md) are what made the other shapes reachable
  (`(a < b) & c`, a stored predicate, a comparison of a comparison).
- **The fragment's integer type is unsigned** — `OpTypeInt 64 0`. This is not a
  style choice: `OpUDiv`, `OpUMod` and `OpULessThan` require operands whose
  signedness is `0`, so a signed type with those opcodes is an invalid module
  rather than a wrong answer, and `spirv-val` names it (`Expected unsigned int
  scalar or vector type as Result Type: UDiv`). It is also simply true — an `Int`
  is a machine-sized unsigned integer, so `/`, `%` and the order comparisons are
  the unsigned ones, and declaring the type signed would have *required* the
  signed opcodes and their silent divergence above `2^63`.

## The corrections, because the first two answers were wrong

Two claims about GPU compute shaped this work, and **both were wrong at first**.
They are recorded because the wrong versions are the plausible ones.

**"A GPU backend needs the kernel body to be index-free."** False. A compute
shader indexes freely, and a dispatch giving each invocation one index maps
straight onto the existing index-based parallel kernel — one invocation per
index, the body unchanged. Being index-free is a precondition only for
*vectorising across a lane group*, which is a different and much larger win than
parallelism. See [attributes.md](attributes.md#nothing-consumes-it-yet).

**"The 64-bit integer width is the blocker."** Mostly false, in a way worth
being precise about. A buffer *index* needs no capability at all: SPIR-V's
addressing arithmetic is 32-bit by default with a 4 GB range, and 64-bit
indexing is an opt-in extension for buffers larger than that — while one `plrun`
is bounded at 8 MiB. Only the *data* width needs a device capability
(`shaderInt64` on Vulkan, MSL 2.3+ on Metal), which is why
[`IntWidth`](../crates/lichen-kernel-ir/src/lib.rs) is a *declared* fact with a
refusal behind it rather than an assumption: a device without `shaderInt64` gets
`RunError::MissingInt64`, not a silent narrowing.

## Measured on the target

RTX 3060 Laptop GPU, Vulkan 1.4.329, driver 596.36.0.0, `shaderInt64 = true`.
SPIR-V output validates clean under `spirv-val --target-env vulkan1.1` and
disassembles to:

```glsl
long index = (long)gl_GlobalInvocationID.x;   // zero-extend, then bitcast
out0[index] = index + 1;                       // access chain, store
```

## The emitter is hand-written, and that is a deliberate trade

Roughly twenty opcodes. A builder crate would remove the spelling mistakes but
**not** the failures that actually happen, which are all in the *module
contract*: the section order, the declared capabilities, the decorations, and
what the entry point's interface lists. A builder validates none of that.

What removes that risk is offline validation, and it is why `spirv-val` is the
development tool for this crate rather than a runtime hope. It earned its keep
immediately — five real errors were caught without a device in sight, each named
precisely:

- `OpConstant` emitted **inside** the function, where it is not module-scope.
- A `StorageBuffer` variable typed as a bare `OpTypeRuntimeArray` — Vulkan
  requires a struct (`VUID-StandaloneSpirv-Uniform-06807`).
- A runtime array instantiated directly rather than as a struct's final member.
- The struct holding the runtime array not decorated `Block`, and not laid out.
- `OpUConvert`'s result type signed, where it must be unsigned.

A sixth was caught by the device rather than the validator, and it is the one
worth remembering: **a `StorageBuffer` access chain needs three indices**, not
two — the buffer struct's member, the element, *and* the element struct's
member, because the element is itself a struct.

`spirv-val` and `spirv-dis` are a development aid only. The crate depends on
`ash` and the IR, nothing else — and that is still true, because
`tests/spirv_validation.rs` **runs** the validator rather than depending on it:
it emits a fragment, pipes the words to `spirv-val --target-env vulkan1.1 -`, and
asserts the module validates. That check needs no device, and it **skips loudly**
(rather than passing quietly) when the tool is not on `PATH`. So the manual step
above is also a checked one, and the two agree rather than drifting apart — with
the caveat that the fragment in the test is a copy of the one in
`examples/emit-spv.rs`, so a change to the example must be mirrored there.

The device-backed tests skip for the same reason and the same way:
`tests/common/mod.rs` holds the one helper, and every caller returns early and
prints the reason — which, note, is **any** failure to open a context, so a
genuinely broken backend on a GPU machine now skips rather than fails. A passing
run is therefore evidence that nothing crashed, not that a device agreed;
`--nocapture` is how a skip is read, because libtest captures stderr for a
passing test.

## Two position spaces, and the silent bug they caused

This is the one bug worth writing down, because **it produced no error at all**.

A read's position (`cfg_pos`) counts over the **inputs**. A write's position
(`out_pos`) counts over the **outputs** — the IR documents it as the write's
*emission ordinal*, the index function's codomain position. They are two
distinct spaces. An early version of the emitter collapsed them into one list of
buffers, so a write at position `0` reached **input 0**: every run wrote its
result into its own input, and the output came back as zeroes. No Vulkan error,
no validation message, no diagnostic — just a plausible-looking zero.

`tests/refusals.rs` pins the separation, including the negative case where a
write names an output that does not exist.

Related and worth knowing: a parallel fragment's `param_shape` is
`(config, index)` — **two leaves, however many inputs there are**. Extra inputs
are reached through a read's position, not through further parameters. So the
index is always parameter 1, and a fragment with three parameter leaves is not a
shape the compiler produces.

## The tail-lane obligation

A dispatch covers `LOCAL_SIZE_X` (64) invocations per workgroup, so the last
workgroup runs lanes past the end. The shader has **no bounds test**, and that is
deliberate: a bounds test is a branch every lane takes, on the hottest path, to
guard indices the host already knows.

Instead every buffer is allocated rounded up to a whole number of workgroups and
the padding is zeroed — inputs so the surplus lanes' reads are in bounds and
return `0`, outputs so their writes land in padding rather than past the end. Only
the first `count` elements are read back. Out-of-range access is impossible by
construction rather than by a runtime test, and the shader stays branch-free.

The tests pin **both** ends of that: counts that are not a multiple of 64 (100,
where the padding is exercised) and a count that is exactly one (128, where it
is not).

## Acceptance

The criterion is bit-for-bit agreement, checked three ways per run: against a
**hand-written expected vector**, and against an independent CPU reading of the
IR. The expected vector is the point — comparing the GPU only against another
implementation of the same reading would pass a fragment that both misread.

`cargo test -p lichen-compute-gpu` on the target above: **19 passing** — 12 device runs
(10 in `gpu_matches_cpu`, 2 graph runs in `graph_on_device`), 5 refusals, and 2
`spirv_validation` runs; those two shell out to the SDK's `spirv-val`, so they are the
only target that needs it installed. The refusal
tests need no device, deliberately: a refusal that only appeared once a GPU was present
would be untestable on a machine without one.

The three device tests the operator set added are the ones that pin the two type
facts above: `arithmetic` (a product, an unsigned division and remainder),
`predicates` (comparisons combined with `&`/`|` and stored — the materialisation),
and `unsigned_reading` (a `2^63` element, where the signed and unsigned readings
take different branches).

## The language selects the backend, on `parallel` only

`compute.parallel` takes the backend as a second argument, and there is **no
default and no `auto`**:

```
k = compute.parallel f "cpu"
out = compute.plrun k (4,)

k2 = compute.parallel g "gpu"
```

Two decisions carry most of the weight here.

**Only `parallel` takes one.** A `jit` kernel is launched a single invocation at
a time, and a device is for the thousands a dispatch runs at once, so there is
nothing for a backend to choose between. `ComputeValue::Kernel` therefore has no
backend field at all, and `launch` is untouched.

**No "try either" value.** A named backend that declines is a *named* failure.
There is no third value to fall back to, because overriding the author's choice
is the one place the language's explicit dataflow would quietly stop being
explicit. A program that said `"gpu"` and silently ran on the CPU would be a
program whose timing means nothing. The refusals, all by name:

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
`lichen-compute` still does not name the GPU crate.

### Where a run is *not* wired yet

The chain from source to a device is proved in two halves, on purpose:

- `lichen-compute`'s tests install a **stub** backend and pin the routing — that
  a `"gpu"` kernel reaches the installed backend, that its answer is the answer,
  and that a decline is reported rather than swallowed. No device needed, so a
  routing regression cannot be mistaken for a hardware question.
- `lichen-compute-gpu`'s tests run a real [`KernelFragment`] on the device and
  check it against a hand-written expectation.

The seam between the two **is** covered now. `lichen-language`'s test suite
already ran real lichen programs against the CPU backend, so the seam test lives
there and is the `"gpu"` twin of an existing chained program: a first kernel
writes `[10, 11, 12]`, a second reads that buffer and doubles it, and the whole
thing is checked against the rendered output. It runs on a real device, through
`install_default`.

It needed `lichen-compute-gpu` as a **dev-dependency** of `lichen-language`,
which is worth stating because an earlier version of this note claimed such a
thing would be a dependency cycle and therefore not possible. It is not:
`lichen-compute` does not depend on this crate and this crate does not depend on
`lichen-compute`, so bringing them together in a test is a plain
dev-dependency, and the library half of `lichen-language` still never links a GPU
loader.

What this closes, and what it does not: it proves a kernel compiled from a lichen
**function**, named `"gpu"`, reaches a device and comes back correct. It does not
prove the chain avoided the host — the values would be the same either way, since
a `plrun` that quietly fell back to the CPU would produce the same numbers. That
is what the routing tests in `lichen-compute` are for, and they are separate on
purpose.

## Staging must be cached, or the run is refused

The single biggest performance fact about this backend is not in the dispatch at
all — it is in which Vulkan memory type the staging lands in, and the rule is
easy to get wrong in a way that still produces correct results.

On the target machine, the memory types the RTX 3060 offers are:

| index | flags | heap | what it is |
|---|---|---|---|
| 1 | `0x0001` | VRAM 5.85 GiB | device-local, not host-visible |
| 3 | `0x0006` | system RAM | host-visible + coherent, **no cache** |
| 4 | `0x000e` | system RAM | host-visible + coherent + **cached** |
| 5 | `0x0007` | VRAM | BAR aperture, device-local + host-visible |

Selecting "the first type that is host-visible and host-coherent" picks **index
3**, which on a discrete GPU is system RAM reached over PCIe with nothing behind
it. Every host touch on it is a round trip, so a run pays two uncached transfers
per element and the whole backend lands ~15× behind a scalar loop — while
reporting success at every step. The fix is one flag (`HOST_CACHED`), and the
measured difference between index 3 and index 4 is the difference between 55 ms
and 4.8 ms at a million elements.

So `cached_memory_type` **requires** `HOST_CACHED` and refuses a device that has
none, rather than falling back. A fallback would be *correct* and would silently
put the backend back where it started, with nothing in the output to say so. The
refusal names the missing property instead. This is the same rule the rest of
the crate follows: no silent fallback onto a path that changes what the numbers
mean.

## What the GPU actually costs, and where the crossover is

Measured on the target (`cargo run --release --example crossover`), against a
plain sequential scalar loop standing in for the CPU thread pool's single-worker
path — a deliberately **conservative** comparison, because that path fans out
over threads above 4096 indices.

The table reports the dispatch and the fetch separately, because they are two
different costs: the dispatch uploads and runs, the fetch brings the answer home.
A program that chains kernels pays the first and not the second — that is the
entire reason `run` hands back an id instead of data.

| count | total | dispatch | fetch | sequential | ratio |
|---|---|---|---|---|---|
| 1 024 | 0.304 ms | 0.249 ms | 0.055 ms | 0.003 ms | 0.01× |
| 65 536 | 0.616 ms | 0.351 ms | 0.266 ms | 0.033 ms | 0.05× |
| 262 144 | 1.651 ms | 0.693 ms | 0.957 ms | 0.38 ms | 0.23× |
| 1 048 576 | 4.841 ms | 2.107 ms | 2.735 ms | 1.50 ms | 0.31× |

The GPU still loses, but it is no longer losing by 15× — it is within about
3× of a single-threaded scalar loop at a million elements, against 18× before
device-local memory. Two things moved it:

1. **Buffers are device-local, staged through `HOST_CACHED` memory.** This is
   where the order of magnitude was. The previous code asked for
   `HOST_VISIBLE | HOST_COHERENT` and took the *first* matching memory type,
   which on this machine is `memoryTypes[3]` — system RAM over PCIe with no
   cache behind it. Correct, and ruinous: every element cost two uncached
   round trips. See [Staging must be cached](#staging-must-be-cached-or-the-run-is-refused).
2. **Staging is reused.** One mapping per context rather than per run, which is
   why the fixed floor fell from ~0.54 ms to ~0.30 ms.

**The GPU wins on a chain, and the fixed cost is what decided it.** An **empty**
dispatch — one workgroup, so nothing but the dispatch itself — costs **0.043 ms
at best** over 200 runs, and the example prints the median beside it. That number
measured 0.046, 0.048 and 0.043 ms across three runs of the same binary on this
machine, so read it as 0.04–0.05 ms rather than to three places.

**There are two of these floors, and using the wrong one invents a result.** A
dispatch reading a **host** input is 0.043 ms; a dispatch reading a **resident**
buffer — which is every link of a chain after the first — is **0.036 ms**. The
difference is the upload: a `memcpy` into staging plus a device-side copy in the
same submission, worth about 0.007 ms. Neither figure is a constant: across runs
of the same binary on this machine the host floor has read 0.043–0.045 ms and the
resident one 0.036–0.041 ms, and the derived columns in the tables below come
from the 0.036 ms run specifically. The crossover example now measures both,
because dividing a chain's marginal link by the host-input floor credits the link
with an upload it never does — and at small counts that credit exceeds the cost of
the link, which is how a first run of the count sweep below reported links that
were 108% overhead. A fraction over 100% is a wrong divisor, not a noisy one.

The chain table shows what the floor buys:

| count | chain | GPU | sequential | ratio |
|---|---|---|---|---|
| 65 536 | 16 | 1.493 ms | 0.283 ms | 0.19× |
| 262 144 | 16 | 3.494 ms | 2.397 ms | 0.69× |
| 1 048 576 | 1 | 5.769 ms | 3.653 ms | 0.63× |
| 1 048 576 | 2 | 5.560 ms | 4.818 ms | 0.87× |
| 1 048 576 | 4 | 7.857 ms | 7.849 ms | **1.00×** |
| 1 048 576 | 8 | 7.315 ms | 11.434 ms | **1.56×** |
| 1 048 576 | 16 | 7.168 ms | 25.733 ms | **3.59×** |

A single cold dispatch still loses, and that is not a bug: the first upload is
8 MB across PCIe, which costs about what the scalar loop costs outright. The
cross-over is at **about four links** at a million elements, and it climbs from
there.

**Read this table loosely, and prefer the count sweep below for anything
per-link.** It is one sample per point and it is not monotonic in links — two
links came out *cheaper* than one, and four more expensive than eight. The
earlier run of it put the 1→16 slope at 0.15 ms per link and this one at
0.09 ms. The `sequential` column drifted further than the GPU column between
runs, so the **ratios** moved more than the GPU times did: the 16-link ratio went
2.72× → 3.59× while its GPU time went 7.79 → 7.168 ms. The ratio is the least
trustworthy number here, because its denominator is a stand-in for a thread pool
that is not being measured.

### What a link is actually made of

Everything above is one sample per point, and the question the graph design rests
on is not "how long is a chain" but "how much of a link is not the work". So: 16
links fixed, count swept, best of 20 per point, and the per-link cost taken as
`(16 links − 1 link) / 15` — a difference of two chain lengths, so that the upload
and the download cancel and what survives is the marginal cost of one link.

| count | 1 link | 16 links | per link | fusion share | overhead |
|---|---|---|---|---|---|
| 1 024 | 0.080 ms | 0.675 ms | 0.040 ms | **80.9%** | 91.9% |
| 4 096 | 0.086 ms | 0.681 ms | 0.040 ms | 80.2% | 91.7% |
| 16 384 | 0.111 ms | 0.731 ms | 0.041 ms | 74.7% | 88.1% |
| 65 536 | 0.214 ms | 0.846 ms | 0.042 ms | 64.6% | 86.4% |
| 262 144 | 0.814 ms | 1.555 ms | 0.049 ms | 35.1% | 73.7% |
| 1 048 576 | 3.298 ms | 5.096 ms | 0.120 ms | 10.7% | 30.4% |

*fusion share* is `15 × 0.036 ms / 16 links`; *overhead* is `0.036 ms / per link`.

The shape is the point: **the per-link cost barely moves with count** — 0.040 ms
to 0.120 ms across a 1024× range — because the round trip is fixed and the kernel
inside it is not. So the same 0.036 ms goes from 92% of a link to 30% of one, and
recording a chain as one submission is worth **65% to 81% of a 16-link chain at
counts up to 65 536**, against 10.7% at a million. Every estimate of this made
before the sweep used a million elements, which is the single row least favourable
to fusing.

Best of 20 is not a stylistic choice either. The single-sample table above reads
6.076 ms where this one reads 5.096 ms for the same point — 16% apart, wider than
most of the effects being measured.

### And the prediction, met

`run_chain` records a chain into one command buffer, submits once and waits once.
The *fusion share* column above is arithmetic done before that existed; this is
the feature answering it.

| count | unfused 16 links | fused 16 links | speed-up | predicted | actual |
|---|---|---|---|---|---|
| 1 024 | 0.646 ms | **0.113 ms** | **5.7×** | 86.3% | 82.5% |
| 4 096 | 0.653 ms | **0.120 ms** | 5.4× | 85.4% | 81.7% |
| 16 384 | 0.681 ms | **0.146 ms** | 4.7× | 82.0% | 78.5% |
| 65 536 | 0.801 ms | **0.266 ms** | 3.0× | 69.7% | 66.8% |
| 262 144 | 1.625 ms | **1.091 ms** | 1.5× | 34.3% | 32.8% |
| 1 048 576 | 5.752 ms | **4.669 ms** | 1.23× | 9.7% | **18.8%** |

*actual* is `(unfused − fused) / unfused`. Fusing is faster at every count, and
lands within a few points of the prediction — below it at the small counts, where
the fused path pays for a barrier and a descriptor set per link, and **above** it
at a million, where removing the host-side work per link shows up because the
0.036 ms floor never counted that work either. The model is conservative, which
is the direction to be wrong in.

**And the chain is checked, not assumed.** The example verifies a fused chain
against the kernel's closed form, `2^n * x + (2^n − 1)` — derived rather than
run, because a second CPU copy of the same loop would agree with a mis-ordered
chain just as cheerfully. Four shapes pass, the largest a 16-link chain.

**The version that was three times slower is worth recording too.** It allocated
one output buffer per link, reasoning that they are all live. They are not: a
link reads the buffer the link before it wrote, so the one before *that* is dead,
and two buffers ping-ponged round the chain are enough. Allocating per link was
not merely bigger — 2.226 ms against 0.679 ms unfused, **three times slower than
not fusing at all** — because a pool that has to hold a buffer per link is empty
at the start of every call, so every call allocates the lot and throws most of it
away. This is the same lesson as the largest win in the history above, arriving
from the opposite direction: the allocation is the cost, and a design that needs
more buffers than the pool holds is paying it on every single call.

**Where the fixed cost went, and the surprise in it.** Getting from 0.46 ms to
0.046 ms — the host-input floor, which is the larger of the two — took three
changes, and they were not equally important:

1. **Recycling released device buffers** — 0.20 ms → 0.046 ms, about **4×**, and
   by far the largest single win. `vkAllocateMemory` is a kernel-mode allocation
   and it was happening once per run, for an 8 MB buffer at a million elements.
2. **Reusing the per-dispatch objects** — command pool, command buffer, fence,
   descriptor pool and both layouts now belong to the context, and
   `spirv::compile` runs only on a pipeline-cache miss. That measured
   0.222 ms → 0.200 ms, about **10%**.
3. **Not initialising output buffers**, and staging through `HOST_CACHED` memory
   rather than the uncached host-visible type.

The surprise is that (2) — seven object create/destroy pairs removed from every
dispatch — was worth 10% while (1), reusing one allocation, was worth 4×. Pool
reuse looks like the optimisation and is not; the allocation is the cost. A
recycled buffer needs no clearing either, because every consumer writes all of
it: an output is written across `[0, padded)` by the shader's straight-line body,
and an input is uploaded across `[0, count)` with the tail cleared on the device.

**Two things about the measurement itself, because they changed the numbers.**
A single sample of an empty dispatch lands anywhere from 0.33 ms to 0.93 ms
depending on what else the machine is doing — a 3× spread, wide enough to hide
the effect of anything. The example therefore runs 200 and reports best and
median; the best is reproducible to within 0.006 ms across runs where a single
sample is not. And the CPU side of the chain measurement **swaps two buffers per
link** rather than allocating a fresh one, because an allocation per link is the
allocator being measured instead of the loop. Getting that wrong reported a
crossover at four links back when there was none.

There is deliberately **no** minimum-count gate. The measurement says the answer
is not a count: 65 536 elements loses at every chain length while 1 048 576 wins
from four links on. A count threshold would be a number invented to look careful
about a quantity that is not the one that decides it.

## Not yet

Named rather than implied, because each is a decision not a gap:

- **Launching a chain as one submission.** **Built at depth one, and measured.**
  `run_chain` records a chain into one command buffer, submits once and waits
  once, and it is faster than the unfused chain at every count measured — 5.7×
  at 1 024 elements, 3.0× at 65 536, 1.23× at a million. See
  [compute-graph-jit.md](compute-graph-jit.md). It is a sibling of `jit`, not a
  mode of `plrun`, and the two seams it needs in `lichen-lowlevel` have landed.
  What is left is not the pool, not the entry point, and not the measurement:
  `ParallelBackend::submit` hands a submission back unwaited and `Pending::wait`
  finishes it (checked on a real device by chaining two in-flight submissions),
  and the split is measured. **Async's saving is one submission's device time and
  no more** — `min(host work, device time)`, saturating at the device's own
  time, to within 0.05 ms in ten rows. So it earns nothing on a chain of pure
  kernels and up to a dispatch's worth per node on a graph with native nodes in
  it, and the *Batch* ceiling above is a different quantity entirely. The
  **executor** that decides when to submit and when to wait exists now, in
  `lichen-graph-ir`, written entirely against the contract in this file **with no
  change to it** — which is the strongest evidence yet that `submit` and
  `Pending::wait` are the right two seams. A graph of both kinds of node runs on
  a real device under this contract; see this crate's `tests/graph_on_device.rs`.
  `Batch` needs a fused-submission capability the contract does not have, and is
  refused by name until it does. What is left is the half that *builds* a graph
  rather than running one: the `compute.graph` operator and the lowering.
  `run_chain`'s own shape is a linear chain of one
  fragment, and a graph's fan-out is not that.
- **Cross-kernel calls.** `SpirvRefusal::CrossKernelCall`. Needs several
  functions in one module and a call graph; the refusal names the callee and the
  instruction position.
- **More than one result.** A compute shader communicates through its storage
  buffers, so a fragment whose body leaves one value is required here
  (`ResultArity`).
- **A non-index parameter read.** A buffer is *bound*, not passed, so there is no
  value for it to hold (`NonIndexParameter`).
- **Wiring into `compute.plrun`.** Done, and narrowly: `parallel` names a
  backend and `plrun` dispatches to it, with the refusals above. The seam test
  covers the whole path from a lichen function to a real device.
- **Holding a resident buffer across kernels.** **Done.** A `"gpu"` run produces
  a `DeviceBuffer` value rather than host data, `plrun` hands one straight to the
  next run as the id it already is, and `compute.collect` / `compute.read` are
  the only things that bring data back. A `"cpu"` run handed a resident input
  fetches it first, because it has no device to read from.
- **Freeing a resident buffer from the language.** There is **no per-value
  release**, and this is deliberate rather than an oversight: the value set has no
  per-value destructor, and a `Buffer`'s arena payload has no individual free
  either — so a device buffer is scoped exactly like a host one. The cost is real
  and worth stating plainly: **resident buffers accumulate for the life of the
  backend.** The backstops are `GpuContext::drop`, which reclaims everything, and
  a refused allocation once the device is full, which names itself. A program
  that runs many large kernels in one process will hit that backstop.
- **The per-dispatch submit and wait.** 0.036 ms best against a resident input,
  paid once per link, so a chain of N still does N submits and N fence waits
  where one would do — `run_chain` is what makes it one, and it is done. This is
  **not** worth a factor of N: the saving scales with the link count while the
  baseline scales with the *kernel* cost, and those are independent. What it is
  worth depends entirely on kernel size, and the count sweep answers that.
  Measured, at 16 links: **5.7× at 1 024 elements, 3.0× at 65 536, 1.23× at a
  million.** A link there is 88% overhead and here it is 29%.
- **A launch graph.** A `compute.graph` that JITs an ordinary lichen function
  into a graph IR — a DAG of kernels and the dataflow between them, which is the
  IR's natural shape rather than a special case to be detected — and optimises on
  that. What it buys is the one thing left: recording a whole chain into one
  command buffer and submitting it once. A single `plrun` cannot see the link
  after it, so this is not reachable from the per-run path at all. The
  `ResidentId` split is the prerequisite and is done; the buffer recycling above
  is the same discipline one level down, so a graph's arena is not a new idea so
  much as the same pool scoped to a graph instead of a run.
