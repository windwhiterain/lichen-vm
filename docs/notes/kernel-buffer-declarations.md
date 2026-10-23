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
- **`compute.slice` is the declaration, and it is the only one.** It takes the
  **length and the element type**, takes **no array**, and **initialises
  nothing** — the allocation holds whatever the allocator held, and filling it is
  the program's job. **Its value is undecided**: the operation never evaluates to a
  `Buffer` payload, so the node stays the identity the walk traces. A declaration
  that really produced a physical buffer would be *decided*, and a decided value
  cannot be traced back to the output it names — that is why the operator is a
  slice and not a buffer.
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
  y  = compute.slice ((compute.Slice _)(.count c.n, .element Float))
  compute.write ((compute.Write _)(.to y, .at i, .value (int2float s(0))))
  ((Out)(.y y))
}
```

**`write` is itself a pure function, and its result is a buffer.** The wrapper
ascribes the result `(Buf _)`: a write returns the buffer's **new version**, so
the value that flows out *is* the write's result — `((Out)(.y (compute.write …)))`
— and no effect exists outside the value. Any buffer the value reaches is
accepted: a slice declaration, a write's version, a loop's result, a selection's
arm. A write whose value is discarded is an effect outside the value and is **not**
the model; the freeze drops it (a statement-form write is not reachable from the
return, measured — `docs/notes/kernel-buffer-declarations.md`).

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
  `compute.slice` node to that ordinal instead of through a parameter path;
  `Lower::version_identity` no longer needs a parameter slot for a declared
  version. The per-position check that used to demand "output k is a write to
  output k" is **gone** (`20d0c351`): the codomain's field order and the
  declaration order are the same list, so a write always fills the output it
  names and a crossing is not an error the compiler can see any more.
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

1. **The declaration and the pure signature** (this branch): `compute.slice`
   takes `(count, element)`; `P` loses `.out`; the body declares, writes and
   returns; the fragment's outputs are the declarations; the runtime allocates by
   the declared count in `global` space; the perspective check refuses a
   placement it does not admit (with no pack, that means: a `# warp`-style
   declaration waits for a pack that names it). One vertical program runs on the
   cpu and compiles for the gpu; **every other program is red on the branch
   until slice 2**.
2. **The migration**: every `compute.P …(.I In, .O Out)` and every
   `compute.buffer (array)` / `compute.buffer (count, element)` in the tests and examples — 46 files, 15 done — moves to the
   declared form and to a kernel-built input where it needs data. **A fill kernel
   spells an arbitrary table as a sum of one-level `if`s** —
   `(if i == 0 then 2.0 else 0.0) + (if i == 1 then 4.0 else 0.0) + …` — which the
   body lowers (measured, `consumer_guard`); a **nested** `if` chain is refused by
   the template blocker (measured, `zz_pure_kernel_probe`), so the summed spelling
   is the one to use. The outbound boundary is `compute.collect`: reading a
   buffer's old `.native` payload is refused on this branch
   (`frozen_buffer_reuse`).
3. **The pack**: the file format, its reader, the space/slot mapping, and the
   perspective constraint enforced against it.
