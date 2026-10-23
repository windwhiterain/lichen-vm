# Kernels declare buffers; the pack places them

> Status: **current design**, being implemented on `feature/pure-kernel-io`.
> Supersedes the `.out`-as-a-parameter-group model in
> [compute-kernel-struct](compute-kernel-struct.md)'s "The parallel parameter struct".
> Points at: `crates/lichen-compute/src/compute.lichen` (`BufParam`, `Buf`, `buf`),
> `crates/lichen-compute/src/compute.rs` (`BufTypeOp`, `BufOp`, `parallel_roles`,
> `compile_parallel_fragment`, `build_outputs`, `run_parallel_range`),
> `crates/lichen-compute/src/compute/body.rs` (`Lower::write_call`,
> `Lower::version_identity`), `crates/lichen-kernel-ir/src/lib.rs`
> (`KernelInstr`, `KernelFragment.outputs`), and both backends.

## The ruling

- **A buffer's type carries its element type, its length and a native payload.**
  The declaration is a call that is **both a type and the value it creates**:
  `compute.buf ((compute.BufParam _)(.type Float, .length 192))`. `BufParam` is
  the declaration's operand — `struct{.type _, .length _}` — and `Buf` is a
  **value-indexed** type function: given a `BufParam` value `x` it states
  `.native` (the payload, type `$buf(x.type, x.length)`), `.type` (`x.type`) and
  `.length` (`x.length`). There is **no `.element`**. An undecided buffer is that
  instance with holes that are exprs whose attributes are all undecided (`_`).
  `$buf(T, L)` is a kind marker with the shape `[element type, length]`, the shape
  `array{T, n}` already has, so the decode is the same walk. Two native operators
  back it: `$buf_type(T, L)` builds the **type** term (decided where it is
  written) and `$buf(T, L)` is the **payload** value (kernel-only).
- **A kernel only declares its buffers.** It states each buffer's **element type**
  and **length**; it never states where the buffer lives.
- **The pack decides the space**: `global`, `shared`, or `register`, and — for
  `global` — which global-memory slot the buffer is.
- **A perspective constrains the pack's choice**: a buffer's declared perspective
  says how many lanes share it (`0` crosses workgroups, `1` is one lane, the warp
  width is one subgroup), and a placement the perspective does not admit is
  refused by name.
- **`compute.buf` is the declaration, and it is the only one.** It takes a
  `BufParam` value — the element type and the length — takes **no array**, and
  **initialises nothing** — the allocation holds whatever the allocator held, and
  filling it is the program's job. **Its value is undecided**: the payload is the
  `$buf` operator's, which never evaluates on the host, so the node stays the
  identity the walk traces.
- **A host builds an input buffer explicitly, with a kernel.** There is no
  array-to-buffer conversion: a kernel declares an uninitialised buffer, writes
  it, and returns it; the returned `Buf` is the input of the next launch.

## The surface

```lichen
BufParam = _x => struct{.type _, .length _}
Buf = x : (BufParam _) => struct{
  .native $buf_type(x.type, x.length),
  .type   x.type : Type,
  .length x.length : Int,
}
buf = (x : (BufParam _)) => (Buf x)(.native $buf(x.type, x.length), .type @any, .length @any)
```

A parameter and a body:

```lichen
L    = 3
In   = struct{.d (compute.Buf ((compute.BufParam _)(.type Float, .length L)))}
Out  = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length L)))}
Par  = struct{.n Int, .in In}
raw  = (c : Par) => {
  i  = compute.range c.n
  y  = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((Out)(.y (compute.write ((compute.Write _)(.to y, .at i, .value 1.0)))))
}
```

Reads, writes and `collect` keep `(compute.Read _)`-style instantiations: their
length is open (`_`), because a read or a write does not care how long the buffer
is. `Mma` states the cooperative matrix's fixed lengths (16, 32, 16).

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
  `(count, class)`; `roles.outputs` is the codomain `O`'s buffer fields, in order,
  and when the parameter names no `.out` group the codomain's field indices are
  the paths.
- **A buffer's element class is its type's first shape position.** The wrapper's
  `.native` field type *is* the `$buf(type, length)` term, so the role walk reads
  the class from `buffer_shape(native).0`; the `.type` field is a `Type` value and
  answers nothing.
- **A write's target is a declared buffer**, so its output ordinal is the
  declaration's index. `Lower::write_call` resolves a target that is a `Buf`
  instance through the registry (`Lower::declared_ordinal`), falling back to a
  parameter path; `Lower::version_identity` no longer needs a parameter slot for a
  declared version. The per-position check that used to demand "output k is a
  write to output k" is **gone** (`20d0c351`): the codomain's field order and the
  declaration order are the same list, so a write always fills the output it
  names and a crossing is not an error the compiler can see any more.
- **A declaration's identity survives a clone.** The body is walked from the
  template, and a declaration or write reached along the apply path is a clone of
  the template's node: `node_origin` answers the template, and the lowering
  follows it (`Lower::definition`), so one definition emits one instruction. The
  registry's key is the canonical node.
- **A declaration's length is read, not lowered.** A body states the launch's
  runtime count (`k.n`), which has no value in the frozen template: the walk reads
  a literal extent and leaves a runtime one to the launch count.
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

1. **The declaration and the pure signature** (done): the body declares with
   `compute.buf`, writes and returns; `P` loses `.out`; the fragment's outputs are
   the declarations; the runtime allocates by the declared count in `global`
   space; the perspective check refuses a placement it does not admit.
2. **The value-indexed surface and the migration** (done): `BufParam`/`Buf`/`buf`
   and the `$buf_type`/`$buf` operators replace the old `Buf`/`Slice` wrappers;
   the tests and examples move to the declared form and to a kernel-built input
   where they need data. **A fill kernel spells an arbitrary table as a sum of
   one-level `if`s** — `(if i == 0 then 2.0 else 0.0) + (if i == 1 then 4.0 else
   0.0) + …` — which the body lowers (measured, `consumer_guard`); a **nested**
   `if` chain is refused by the template blocker (measured,
   `zz_pure_kernel_probe`), so the summed spelling is the one to use. The outbound
   boundary is `compute.collect`: reading a buffer's old `.native` payload is
   refused (`frozen_buffer_reuse`).
3. **The pack**: the file format, its reader, the space/slot mapping, and the
   perspective constraint enforced against it.
