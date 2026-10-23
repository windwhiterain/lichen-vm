# Kernels declare buffers; the pack places them

> Status: **current design**, being implemented on `feature/pure-kernel-io`.
> Supersedes the `.out`-as-a-parameter-group model in
> [compute-kernel-struct](compute-kernel-struct.md)'s "The parallel parameter struct".
> Points at: `crates/lichen-compute/src/compute.lichen` (`P`, `Buffer`, `buffer`),
> `crates/lichen-compute/src/compute.rs` (`BufferOp`, `parallel_roles`,
> `compile_parallel_fragment`, `build_outputs`, `run_parallel_range`),
> `crates/lichen-compute/src/compute/body.rs` (`Lower::write_call`,
> `Lower::version_identity`), `crates/lichen-kernel-ir/src/lib.rs`
> (`KernelInstr`, `KernelFragment.outputs`), and both backends.

## The ruling

- **A kernel only declares its buffers.** It states each buffer's **element type**
  and **length**; it never states where the buffer lives.
- **The pack decides the space**: `global`, `shared`, or `register`, and — for
  `global` — which global-memory slot the buffer is.
- **A perspective constrains the pack's choice**: a buffer's declared perspective
  says how many lanes share it (`0` crosses workgroups, `1` is one lane, the warp
  width is one subgroup), and a placement the perspective does not admit is
  refused by name.
- **`compute.buffer` is the declaration, and it is the only one.** It takes the
  **length and the element type**, takes **no array**, and **initialises
  nothing** — the allocation holds whatever the allocator held, and filling it is
  the program's job.
- **A host builds an input buffer explicitly, with a kernel.** There is no
  array-to-buffer conversion: a kernel declares an uninitialised buffer, writes
  it, and returns it; the returned `Buf` is the input of the next launch.

## The surface

```lichen
P = T: KT _ => struct{.n Int, .in T.I}                 // no `.out`
S = T: KT _ => A T.I -> T.O                            // the function's signature, now literal
Buffer = T => struct{.count Int, .element T}
buffer = (x : (Buffer _)) => ($buffer(x.count, x.element) : (Buf _) # _)
```

A body:

```lichen
raw = (c : Par) => {
  i  = compute.range c.n
  y  = compute.buffer ((compute.Buffer _)(.count c.n, .element Float)) # warp
  compute.write ((compute.Write _)(.to y, .at i, .value (int2float s(0))))
  ((Out)(.y y))
}
```

The signature is `A I -> O`, the codomain `O` holds the declared buffers, and
the perspective is spelled at the declaration.

## The ABI consequences

- **The fragment's outputs are its body's declared buffers**, in declaration
  order. `KernelFragment.outputs` is their count and each carries its
  `(count, class)`; `roles.outputs` no longer comes from the parameter's `.out`
  paths — it is the codomain `O`'s buffer fields, in order, and the two lists
  must agree.
- **A `write`'s target is a declared buffer**, so its output ordinal is the
  declaration's index. `Lower::write_call` resolves a target that is a
  `compute.buffer` node to that ordinal instead of through a parameter path;
  `Lower::version_identity` no longer needs a parameter slot for a declared
  version.
- **The allocation is the declared length, uninitialised.** The runtime
  allocates each output with its `count` and element class, in the pack's space,
  and hands back the codomain struct (`build_outputs` by `O`'s fields).
- **`collect` still reads a buffer to a host array** — that is the outbound
  boundary, and it is unchanged.

## The pack

A kernel-pack names a kernel (its fragment hash, or its export name) and gives,
per declared buffer (by declaration order), the space and — for `global` — the
global-memory slot:

```text
kernel <hash> {
  buffer 0 { space = "global", slot = "y" }
  buffer 1 { space = "shared" }
}
```

Without a pack entry, the placement defaults to `global`; a perspective the
default does not admit is refused by name (the shape is then expressible only
with a pack that names its space). The pack's format and reader are the last
slice; the ruling above is what the earlier slices build toward.

## Slices

1. **The declaration and the pure signature** (this branch): `compute.buffer`
   takes `(count, element)`; `P` loses `.out`; the body declares, writes and
   returns; the fragment's outputs are the declarations; the runtime allocates by
   the declared count in `global` space; the perspective check refuses a
   placement it does not admit (with no pack, that means: a `# warp`-style
   declaration waits for a pack that names it). One vertical program runs on the
   cpu and compiles for the gpu; **every other program is red on the branch
   until slice 2**.
2. **The migration**: every `compute.P …(.I In, .O Out)` and every
   `compute.buffer (array)` in the tests and examples — 46 files — moves to the
   declared form and to a kernel-built input where it needs data.
3. **The pack**: the file format, its reader, the space/slot mapping, and the
   perspective constraint enforced against it.
