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
- **Wiring into `compute.plrun`.** Nothing calls this yet. The seam is a real
  design question — the GPU path owns its buffers, while the CPU path borrows
  `Vec<Vec<i64>>` and partitions it across workers, and the two do not share a
  shape. Extracting a trait around the CPU shape would be extracted around the
  wrong abstraction.
- **Device-local memory and staging.** Buffers are host-visible and host-coherent,
  which is correct and simple, and is the right choice for a kernel whose working
  set is its input and output. Memory ordering is explicit: coherence is not
  ordering, so the command buffer carries a barrier on each side of the dispatch.
