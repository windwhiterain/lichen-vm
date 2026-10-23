# Half precision: what it costs to hold a model at its own width

> Status: **in progress.** The class is admitted by the IR; the mirrored enums and the
> emitter's conversions are the work. Nothing here is merged.

## Why it is the prerequisite

`ScalarClass` is `{Int, Float}`, so a buffer element is 8 or 4 bytes and **nothing in the
workspace can hold a bf16 tensor**. Measured on Qwen3.5-2B's own config: its weights are
4.24 GiB of bf16, and at `Float` that is **8.5 GiB** against a **6 GiB** laptop and an
**8 GiB** desktop card — the model does not fit at the width the language can compute. The
local-inference answer to the same problem is not to widen the weights: bf16 stays bf16 and a
quantised file goes to four or six bits, with per-block scales. Both need a class that is
narrower than `Float`.

## The design: bf16 stores, f32 computes

A brain float is **the top sixteen bits of the `Float` it rounds to**, so the conversion is a
shift and needs no vendor extension:

| direction | operation |
|---|---|
| read | `f32_bits = u16_bits << 16`, bitcast to `Float` |
| write | `u16_bits = f32_bits >> 16` |

An SPIR-V storage buffer therefore carries the half as an **array of `uint16`**, which plain
SPIR-V has, and every kernel body still computes in `Float`. `SPV_KHR_bfloat16` is only
needed for bf16 *arithmetic*, which this design does not ask for — the trade is that a bf16
multiply becomes a widening and a narrowing around an f32 multiply, so the win is memory and
bandwidth, not compute.

The storage and value domains are deliberately **two answers, not one**: `Ids::storage_type_of`
names the 16-bit element a buffer holds, while `Ids::type_of` keeps answering `Float` for a
value, because the body widens on the read and narrows on the write and never holds a half.

## What is measured, not assumed

Adding the variant and its three derived answers (`byte_width`, `ALL`, `index`) produced
**zero** errors in `lichen-kernel-ir` and `lichen-compute`, and this list everywhere else:

| site | why it was reached |
|---|---|
| `lichen-fusion-bridge/src/lower.rs:179` | `KernelInstr::Const/Bin/BufferReadCall` with a class |
| `lichen-fusion-bridge/src/tiled.rs:170` | a fold's identity, given the class it computes in |
| `lichen-graph-ir/src/value.rs:27,43` | the pack and unpack of host data, at the class's width |
| `lichen-runtime/src/fused/convert.rs:97` | `ir::ScalarClass` → `schema::ScalarClass` |

**`ScalarClass` is mirrored**: the schema layer carries its own copy, and the two convert into
each other, so one class is two enums. That is the shape of the work — every mirrored enum and
every exhaustive match over the class — not the 440 textual hits a count of `ScalarClass`
suggests.

The two pack/unpack sites are the conversion expr itself and are now the shift expr above; the
fold identity is exact at bf16 because a reduction states `0`, `1` or `-inf` and all three are
exact halves; the lowering refuses a bf16 instruction **by name**
(`BridgeRefusal::UnsupportedClass`) until the emitter can carry one.

## What the compiler named, once the mirrors were in

`cargo check --workspace --all-targets` after the mirrors: **37 arms across five library files and
four test/example files**, every one an exhaustive match over the class. Splitting them is the
useful part, because the two kinds need different work:

| kind | sites | what an arm does today |
|---|---|---|
| **storage and width** — element read/write, packing, a binding's size, a wire tag, a stride | most | the 2-byte element and the shift expr, mirroring `Float` |
| **arithmetic or a backend** — the wasm assembler and lowerer, the SPIR-V operator table, the interpreter's operator tables | 6 | refused by name (`SpirvRefusal::UnsupportedClass`, or the `Result<_, String>` the function already returns) |
| **placeholder** — an id or a type the emitter hands out where it has no 16-bit spelling | 6 | the existing `float`/`uint` id, each with an `# Invariant` saying it is a placeholder |

The tree is **green at the class**, not green at the feature: a bf16 *buffer* was emitted with a
2-byte stride and a 32-bit element type, which `spirv-val` would reject. Three gaps the pass surfaced,
recorded so they are not mistaken for done — **(1) is now closed; see below**:

1. ~~**`element_stride` and `type_of` disagree for a bf16 buffer.**~~ **Closed by the emitter
   slice**: the two are different questions and now have different answers.
2. **`ScalarData` has no bf16 variant**, so reading such a buffer to the host reports `Float`, and
   `fetch_resident` would refuse it as a class disagreement — a loud failure on a path that is
   unreachable until (1) is fixed.
3. **The wire tag is a third value** (`write_class`/`read_class` gained `2`), so a persisted buffer's
   class codec changed. Appending the tag keeps old payloads readable; a reader older than the tag
   would not read a bf16 one.

## The emitter slice, and the capability it measured

`lichen-compute-gpu/src/spirv.rs` now emits a bf16 buffer, and the emitted module passes
`spirv-val`. The one fact worth writing down is the capability, because it was a hypothesis:

| declared | result under `--target-env vulkan1.1` |
|---|---|
| `Shader` alone | **rejected** — "Using a 16-bit integer type requires the Int16 capability" |
| `Shader` + `Int16` | **accepted**, empty output, exit 0 |

So the 16-bit element needs **`Int16` (22) and nothing else**. `StorageBuffer16BitAccess` (4433)
is *not* a module capability here: it is the Vulkan **device feature**
`VkPhysicalDevice16BitStorageFeatures::storageBuffer16BitAccess`, which gates pipeline creation,
not module validation, and `spirv-val` does not ask for it. A device without 16-bit storage
support is therefore still a runtime gate — measured here as a capability, not as a feature.

`Int16` is declared **only when a `BFloat16` buffer is in use**, because declaring the type
unconditionally would put the capability on every module the emitter writes.

## What remains

1. `schema::ScalarClass`, and the conversions that mirror it.
2. The value model: a `ScalarData` variant so a fetched half round-trips.
3. `lichen-compute`: the reference interpreter's arithmetic at the new class.
4. The language surface, so a program can spell a half buffer.
5. Then the model: the tied 248320×2048 embedding, the 24 layers, and — for Qwen3.5-2B
   specifically — **the eighteen linear-attention layers**, which need a matrix-valued
   recurrent state and a conv1d along time that no kernel here expresses today.

## Where it runs

`A` (`PS2022JJWZLIXG`, `ssh a`) has an **RTX 3060 Ti**, `cargo` and `rustc`, **257 GiB free**,
and **no lichen checkout and no Vulkan SDK**. Its reported `AdapterRAM` is 4 GiB because CIM
caps; the card is 8 GiB, which is what the 4.24 GiB at half fits into and the 8.5 GiB at
`Float` does not.
