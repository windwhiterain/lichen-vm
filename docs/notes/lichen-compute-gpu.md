# The GPU backend for the lowered-kernel IR

> Status: **implemented, first slice** (`feature/compute-gpu-backend`, not yet
> merged). One output space and the arithmetic/select subset run on a real
> device; a cross-kernel call is refused by name. What is *not* here is listed
> under [Not yet](#not-yet) rather than left implied.

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
same `Vec<KernelInstr>` lowers to wasm bytes and to SPIR-V, and the one place
the two disagree is the clearest evidence it is genuinely neutral:

| | wasm backend | this backend |
|---|---|---|
| buffer access | a host import, `call` a function | `OpAccessChain` + `OpLoad`/`OpStore` on a storage buffer |
| comparison result | an `i64`, narrowed to `i32` for `select` | a `bool` already |
| `KernelInstr::I32WrapI64` | **emitted** | **a no-op** |

That last row is the whole argument in one line. `I32WrapI64` exists *solely*
because the wasm MVP's `select` takes an `i32` condition; SPIR-V's
`OpSLessThanEqual` yields `OpTypeBool`, which is what `OpSelect` takes, so the
narrowing has nowhere to go. An IR that had encoded either target's choice
would have made the other backend wrong.

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
`ash` and the IR, nothing else.

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

`cargo test -p lichen-compute-gpu` on the target above: **7 passing** — 3 device
runs, 4 refusals. The refusal tests need no device, deliberately: a refusal that
only appeared once a GPU was present would be untestable on a machine without
one.

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

The seam between the two — a real installed backend serving a real lichen program
— is not covered by a single test, because doing so needs a crate that depends on
both, and the only such crate would be a test-only dependency cycle. That is the
next thing to close.

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

**The honest framing is that the GPU does not win here, even chained, and the
reason is measured rather than inferred.** From the single-dispatch table, a run
of **64** elements — which is an empty dispatch with the object churn and nothing
else — costs **0.46 ms**. That is the per-dispatch fixed cost, and it is already
half of what the CPU spends on a *complete* million-element loop (1.0 ms). A
chained run adds about 1.07 ms per link on the GPU and about 1.0 ms per link on
the CPU, so the two are within noise of each other and the GPU pays two transfers
on top:

| count | chain | GPU | sequential | ratio |
|---|---|---|---|---|
| 65 536 | 16 | 9.88 ms | 0.22 ms | 0.02× |
| 262 144 | 16 | 11.81 ms | 2.47 ms | 0.21× |
| 1 048 576 | 4 | 8.74 ms | 4.17 ms | 0.48× |
| 1 048 576 | 8 | 12.97 ms | 6.92 ms | 0.53× |
| 1 048 576 | 16 | 21.49 ms | 16.25 ms | **0.76×** |

The ratio is climbing toward 1 and would cross it somewhere past 16 links, so the
shape is right — but the fixed cost is the reason it does not cross yet, and that
is the number to attack. Where the 0.46 ms goes is countable: each dispatch
creates and destroys a descriptor set layout, a pipeline layout, a descriptor
pool, a descriptor set, a command pool, a command buffer and a fence, and
`run` re-runs `spirv::compile` even when the pipeline cache hits. Reusing the
pools, the fence and the compiled words is the whole of the remaining gap.

**The CPU side of the chain measurement swaps two buffers per link** rather than
allocating a fresh one, because an allocation per link is the allocator being
measured instead of the loop. Getting that wrong reported a crossover at four
links that does not exist.

There is deliberately **no** minimum-count gate. A threshold would be a number
invented to look careful: the measurement says the answer depends on the fixed
cost per dispatch, not on `count`.

## Not yet

Named rather than implied, because each is a decision not a gap:

- **Cross-kernel calls.** `SpirvRefusal::CrossKernelCall`. Needs several
  functions in one module and a call graph; the refusal names the callee and the
  instruction position.
- **More than one result.** A compute shader communicates through its storage
  buffers, so a fragment whose body leaves one value is required here
  (`ResultArity`).
- **A non-index parameter read.** A buffer is *bound*, not passed, so there is no
  value for it to hold (`NonIndexParameter`).
- **Wiring into `compute.plrun`.** Done, and narrowly: `parallel` names a
  backend and `plrun` dispatches to it, with the refusals above. What is *not*
  wired is a single end-to-end test across both crates (see
  [Where a run is *not* wired yet](#where-a-run-is-not-wired-yet)).
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
- **The per-dispatch fixed cost.** 0.46 ms, and the whole remaining gap to a
  crossover. See the performance section: pools, the fence and the compiled
  words are all rebuilt per dispatch, and `spirv::compile` runs even on a
  pipeline-cache hit.
