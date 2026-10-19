# The cooperative matrix product, and what the backend has of it

This note records the first slice of the hardware MMA item: the device's own
cooperative-matrix facts, the SPIR-V module the backend can emit, and the two
readings that pin it. The step after this one — the op's surface and the CPU path —
is named at the end.

## 1. What the device advertises, measured

`VK_KHR_cooperative_matrix` is present on this target's discrete device and absent
from the integrated one and from every D3D12 device the loader lists:

| device | extension | `cooperativeMatrix` | `shaderInt8` | `storageBuffer8BitAccess` |
|---|---|---|---|---|
| NVIDIA GeForce RTX 3060 Laptop GPU | yes | true | true | true |
| Intel(R) Iris(R) Xe Graphics | no | — | — | — |
| Microsoft Direct3D12 (any) | no | — | — | — |

The advertised combinations on the NVIDIA device (all `SUBGROUP` scope) include
the floating shapes (`f16` in, `f16`/`f32` out, `M16`, `N16`/`N8`, `K16`/`K8`) and
the integer ones:

| A | B | C / result | M | N | K |
|---|---|---|---|---|---|
| `SINT8` | `SINT8` | `SINT32` | 16 | 16 | 32 |
| `UINT8` | `UINT8` | `UINT32` | 16 | 16 | 32 |
| `SINT8` | `SINT8` | `SINT32` | 16 | 8 | 32 |

The product this note implements is the first row.

## 2. Why integer, and why this shape

`C = A · B + C` with `s8` inputs and an `s32` accumulator is **exact** integer
arithmetic, so the device's internal accumulation order is not observable and the
CPU path can agree bit for bit. The floating shapes would not give that: the
hardware's reduction order over `K` is the hardware's, and a host's left fold is a
different rounding — the two backends could disagree in the last bit, which is the
one thing this backend's contract forbids.

The shape is the device's, not a constant chosen here: `M16 N16 K32` is what the
device advertises for `SINT8`. A device that advertises only the floating shapes has
no integer product to run, and the item's gating is described in §5.

## 3. The module

`crates/lichen-compute-gpu/src/cooperative_matrix.rs` emits the module: one entry
point at `LocalSize 32` (one subgroup, the scope every advertised combination uses)
that loads the three matrices, multiplies, adds and stores.

- **Bindings**: three storage buffers, `DescriptorSet`/`Binding` decorated, laid out
  as `struct{ runtime array }` with the element stride of the component — 1 byte for
  `A`/`B`, 4 for `C`.
- **Types**: `OpTypeCooperativeMatrixKHR` naming `s8`/`s32`, `Subgroup` scope, the
  shape constants and the use (`MatrixA`/`MatrixB`/`MatrixAccumulator`).
- **The product**: `OpCooperativeMatrixLoadKHR` for `A`, `B` and `C` (row-major,
  strides `K`, `N`, `N`), `OpCooperativeMatrixMulAddKHR` with the signed-component
  mask, `OpCooperativeMatrixStoreKHR` for the result.
- **Capabilities**: `Shader`, `Int8`, `StorageBuffer8BitAccess`,
  `CooperativeMatrixKHR`, and the `SPV_KHR_8bit_storage` and
  `SPV_KHR_cooperative_matrix` extension declarations.

The workgroup is one subgroup so the collective load/multiply/store happens once; a
wider workgroup would run the product once per subgroup and store `C` twice.

## 4. The two readings

Both live in `crates/lichen-compute-gpu/tests/compute_gpu/mma_shader.rs`:

- `the_cooperative_matrix_module_validates` runs the emitted words through
  `spirv-val` (the suite's own validator helper), so the module's shape is checked
  without a device.
- `the_cooperative_matrix_product_matches_the_host` creates the extension-enabled
  device, binds `A`/`B` into a `16×32` and `32×16` signed-byte buffer and `C` into a
  `16×16` signed word buffer, dispatches one workgroup, and compares the result with
  the host's exact integer product. Measured: equal, bit for bit. The test skips
  where no device advertises the extension, like the suite's other device tests.

## 5. What is not done yet

`GpuContext` already answers the device's own fact: `cooperative_matrix()` is the
advertised integer shape, and `Some` means the extension and its feature were enabled
at device creation. The module runs and the device facts are in place; what is missing
is the op around it.

1. **The op and its fragment.** The kernel IR's execution model is one invocation per
   element; a cooperative product is one *workgroup* per tile. The op needs either a
   new fragment kind or a dedicated dispatch path, and the `LocalSize` is one
   subgroup (32), not the backend's `LOCAL_SIZE_X` (64).
2. **The CPU path.** The compute plugin's fused/wasm engine needs the same product in
   integer arithmetic — exact, so it agrees by construction — before the op is
   reachable from the language.
3. **The surface.** A `compute.mma`-shaped wrapper binding the three buffers, and the
   perspective story from [perspective-scope-and-mma](perspective-scope-and-mma.md):
   the warp-uniform tile descriptors are `# 32` on the register axis, the fragments
   are lane-varying, and the subgroup is the dispatch's.
