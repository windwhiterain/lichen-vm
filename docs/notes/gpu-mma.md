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

## 5. The op, and what it does not cover

`GpuContext` answers the device's own fact: `cooperative_matrix()` is the advertised
integer shape, and `Some` means the extension and its feature were enabled at device
creation. On top of it:

1. **The surface.** `compute.lichen` binds one `Mma` struct argument, not a chain of
   curried lambdas:

   ```
   Mma = struct{.s string, .a (Buf Int), .b (Buf Int), .c (Buf Int)}
   mma = (x : Mma # warp) => ($mma(x.s, x.a, x.b, x.c) : (Buf Int) # 0)
   ```

   The backend is a required field, as `parallel`'s backend argument is: a program says
   where its product runs. The **perspective** reading is the paper's register-side one,
   applied to what a host buffer is: the three tiles are warp-uniform descriptors — each
   field states `# warp` — so a lane-varying operand is refused with `expected 32, found
   1`, and the result is certified `# 0` — a host buffer is uniform over every lane. The
   check reads the argument's provided perspective (`Checker::provided_slot`) against the
   declared field's attribute, which the type value carries in its field slot
   (`Checker::unify_value_field_attributes`); `Mma` is a type function so its fields'
   requirements travel in the frozen type value, as `Read`/`Write`'s do.
2. **The op.** `ComputeOperator::Mma` reads the three buffers, checks that their
   elements are the shape's (`16×32`, `32×16`, `16×16`) and inside the component ranges
   (`s8` for the operands, `s32` for the accumulator), and answers a `Buf` of the
   result. `"cpu"` computes the product on the host; `"gpu"` hands it to the installed
   backend through [`ParallelBackend::cooperative_matrix`], which refuses by name where
   the device advertises no shape.
3. **The integration.** The product is one workgroup against a fragment, not a kernel
   over an index range, so it rides the trait's own host-in/host-out method instead of
   the launch set. The backend stages the operands into device-local buffers, dispatches
   the module, and reads the result back; the host and the device answer the same words.

It does not cover:

- **resident operands.** An operand that is still on a device (a kernel's output) is
  refused by name — collect it first. Feeding one straight in would need the product's
  operands to be resident ids and a dispatch that binds them, which is the next slice.
- **other shapes.** The emitted composition is the device's `s8`→`s32` one; the
  floating shapes the extension also advertises are not emitted, because their
  accumulation order is the hardware's and the CPU path could not agree bit for bit.
- **the perspective link.** A buffer is a host value, so the product's operands carry
  no lane perspective; the register-side story is
  [perspective-scope-and-mma](perspective-scope-and-mma.md)'s: the warp-uniform tile
  descriptors are `# 32` on the register axis, the fragments are lane-varying, and the
  subgroup is the dispatch's.
