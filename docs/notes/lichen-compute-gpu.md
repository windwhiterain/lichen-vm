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

## The GPU backend is currently a *pessimisation*, and the language says so

Measured on the target (`cargo run --release --example crossover`), against a
plain sequential scalar loop standing in for the CPU thread pool's single-worker
path — a deliberately **conservative** comparison:

| count | GPU | sequential | ratio |
|---|---|---|---|
| 1 024 | 0.63 ms | 0.003 ms | 0.02× |
| 65 536 | 4.32 ms | 0.086 ms | 0.02× |
| 1 048 576 | 59.9 ms | 3.98 ms | **0.07×** |

The GPU does not win anywhere in that range, and at a million elements it is
about **15× slower**. The fixed cost is ~0.5 ms, and it is *per-run object churn*:
every run builds and destroys its buffers, a descriptor pool, a command pool and
a fence, and the buffers are host-visible, so every element crosses the bus.

So `"cpu"` is what programs should write today, and the honest framing is that
this backend is a **correct reference implementation, not a fast one**. What
would change that, in the order it pays:

1. Reuse the command pool, descriptor pool and buffers across runs instead of
   rebuilding them per dispatch. This is most of the 0.5 ms.
2. Device-local buffers with a staging upload/download, so the transfer is one
   contiguous copy per run instead of per element.
3. Then, and only then, a crossover worth a threshold constant.

There is deliberately **no** minimum-count gate today, because the measurement
says no honest gate exists yet: the GPU loses at every count tested. Adding a
threshold that "protects" small runs would be a number invented to look
careful.

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
  [Where a run is *not* wired yet](#where-a-run-is-not-wired-yet)), and the
  performance work that would make `"gpu"` worth writing.
- **Device-local memory and staging.** Buffers are host-visible and host-coherent,
  which is correct and simple, and is the right choice for a kernel whose working
  set is its input and output. Memory ordering is explicit: coherence is not
  ordering, so the command buffer carries a barrier on each side of the dispatch.
