# Runtime scalars in a parallel kernel

> Status: current — the **CPU path is end to end**; the device path and a
> recorded body still carry the extent alone and are **refused by name** rather
> than mis-run.
>
> What this note is: how a parallel kernel takes a number the host fixes at
> launch (`k.alpha`) beside its count, and why that needs a per-leaf ABI. The
> named-struct parameter is the model
> ([compute-buffer-wrapper](compute-buffer-wrapper.md)); this note is the leaf
> ABI it exists for.
>
> Points at: `crates/lichen-compute/src/compute.rs` (`scalar_leaf_classes`,
> `parallel_roles`, `ComputeOperator::ParLaunch`, `run_parallel_range`),
> `crates/lichen-compute-gpu/src/dispatch.rs` (`stage_run`,
> `RunError::ScalarsNotPushed`), and `crates/lichen-language/tests/compute.rs`.

## What a runtime scalar is, and why it is the point

Moving a parallel kernel's parameter from the positional `cfg = (n, (buffers…))`
to the named struct `struct<.n Int, .in …, .out …>` was for two things: a read
that names its buffer, and **runtime scalars beside the count** — `k.alpha`, a
number the host fixes at launch. The tuple form has since retired.

The ABI's shape, from the launch side: the parameter's **scalar leaves in field
order**, then the **index**; the input buffers are bound rather than passed. The
**first scalar is the launch extent** (the retired tuple form's `cfg(0)`, now
`k.n`), which is why the host reads it as an ordinal.

## Each scalar leaf's class is its parameter field's

`compile_parallel_fragment` used to seed *every* leaf `LowShape::USize`, so a
parameter leaf read `Int` whatever it was annotated with. `scalar_leaf_classes`
now reads each leaf's class from the field the role names (`parallel_roles`'s
`scalars`, in signature order) through the `shape` field accessors, and seeds the
`cfg` slot with those shapes — so `param_shape` is the leaf list, then the index,
and a runtime `Float` scalar is an `f32` argument beside `i64` ordinals.

Two leaves stay `Int` whatever the parameter says, and the reason is the same for
both: they are not data. The **index** is a lane number, and the **first scalar**
is the launch extent — a `Float` one is refused **by name** where the message can
name the field, rather than left to surface as an apply-time class conflict with
no span.

A decided leaf of the **wrong** class is refused by name too, and the reason is
the one this channel exists for: staying lazy would mean the dispatch quietly
does not run and nothing says so.

## The host half

| Step | Where | State |
|---|---|---|
| Decode the leaf list out of the launch value, then the input group | `ComputeOperator::ParLaunch`'s arm | the leaves are the value's leading positions, the buffers follow them under `.I`, and how many leaves there are is the fragment's (`parallel_leaf_classes`) |
| Carry the leaf words and their classes on the state | `ParallelState` (`leaves`, `leaf_classes`) | landed |
| Pass them beside the index as the `main` arguments | `run_parallel_range` | the argument list is the leaves in field order with the index appended, and only the index moves per element |
| Assemble the fragment's signature | `compile_parallel_fragment`'s `param_shape` | landed with the leaf classes — the assembler derives `main`'s parameters from it |
| Push the scalar leaves to the shader | `lichen-compute-gpu/src/dispatch.rs`'s `stage_run` | **refused by name** (`RunError::ScalarsNotPushed`): a dispatch pushes the extent alone, so a fragment with a runtime scalar is declined rather than dispatched with a leaf missing |
| Carry a runtime scalar in a **recorded** body | `record_launch` | **refused by name**: a recording carries the extent alone, and a runtime scalar would have to be one of its edges |

The decode is **positional**, and that is the ABI's rule rather than a
convenience: the host struct is the parameter's scalars in field order, then
`.I`. A parameter that interleaves scalars with `.in` would need a role table on
the host side, which the fragment does not carry — so the lowering should refuse
an interleaved parameter by name rather than mis-decode it (not written either).

## How to use it

The kernel's signature is the function's own `f: I -> O` annotation, read back
through the kernel struct's `.I`/`.O`, so a runtime-scalar parameter launches end
to end through `compute.parallel` with nothing else stated. What the author still
spells by hand is the **host input** struct, because no shipped lambda builds the
runtime-scalar shape — shipping one in `compute.lichen` beside `A`/`P`/`S` is the
convention that remains open, because it would have to hardcode a scalar name.

Scratch file, not to be committed (the example harness runs every file in
`examples/`):

```lichen
---
  compute = import "compute.lichen"
---
In0  = struct<.a Int>
Out0 = struct<.z (compute.Buf _)>
Par0 = compute.P (compute.KT _)(.I In0, .O Out0)
g = (k : Par0) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
kg = compute.parallel g "cpu"
inbuf = (compute.plrun kg ((compute.A In0)(.n 3, .I In0(.a 0))) : Out0)
In   = struct<.b (compute.Buf _)>
Out  = struct<.w (compute.Buf _)>
Par  = struct<.n Int, .alpha Float, .in In, .out Out>
Host = struct<.n Int, .alpha Float, .I In>
f = (k : Par) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value v + float2int k.alpha))
}
k = compute.parallel f "cpu"
out = (compute.plrun k (Host(.n 3, .alpha 2.0, .I In(.b inbuf.z))) : Out)
(compute.read ((compute.Read _)(.from out.w, .at 0)), compute.read ((compute.Read _)(.from out.w, .at 1)), compute.read ((compute.Read _)(.from out.w, .at 2)))
```

```bash
cargo run -q -p lichen-compiler -- <probe>.lichen
```

**Measured, and this is the acceptance**: the launch's `cfg` is the parameter's
scalars in field order and then the buffers — `Host(.n 3, .alpha 2.0, .I In(.b
inbuf.z))` — and the answer is `(12, 13, 14)`: three indices (the extent reached
`k.n`), `inbuf = [10, 11, 12]` (the first kernel), and each element plus
`float2int k.alpha = 2` (the runtime scalar).

The refusal path is measured too: `Host(.n 3, .alpha 2, .I In(.b inbuf.z))` — an
`Int` where the parameter declares `Float` — reports

```text
compute.parallel: a parallel parameter's scalar leaf is Float here and the launch
passes an Int for it: Int and Float do not convert
```

rather than running with the bits reinterpreted.

## What the write argument's shape costs

A `compute.write` argument is a **struct instance**, and that is why a struct
parameter can write a `Float` element at all. With the *positional* parameter
shape the count and index cells are undecided, so a `Float` value simply drags
the whole cell to `Float` and the program checks. An **annotated** parameter
decides those cells, and with an **array** argument the same program is refused:
an array literal's elements share one element-type cell
(`check_array_term`), so the index (`Int`) and the value (`Float`) cannot sit in
one, and `compute.write [k.out.z, i, int2float v + k.alpha]` reports `expected
Int, found Float` at the array element. A container whose elements have
*independent* types — a struct instance — has no such cell.

Two things had to land with the struct argument:

- **The ordinals are the language's `Int`, not the element's class.**
  `ReadOp`/`WriteOp` used to unify the index (and, for a write, the length) with
  the **element's** cell, so an ordinal was "the class the data is". The ABI never
  said that: the assembled closures declare the position and the index `i64` **in
  every class** and only the element follows the class. They are now unified with
  `Int`, which is what a length and a lane number are — so an `Int` count no
  longer drags the written value's cell with it.
- **A named field read on a concrete container folds to its constant position**
  (`Checker::check_named_field`), which is what a lowering needs to walk a struct
  argument's fields as positions. An undecided container keeps the lazy
  `TableGet`, which is the case the two-pass `param_path` exists for.

**The types must be lambdas, not type values.** `Read = struct<.from _, .at _>` is
a *value*: one occurrence, one pair of `_` cells, and the first instantiation
specializes them — the next one fails, and the frozen-module path panicked
outright. `KT = _x => struct<.I _, .O _>` is a *function*, so every `(KT _)`
clones its inner `_`. That is why the wrappers are written
`Read = T => struct<.from (Buf T), .at Int>` and call sites are spelled
`compute.read ((compute.Read _)(.from buf, .at i))`.

The `_` spelling at the instantiation site (`_(.from buf, .at i)`) is **not**
refused: a named instantiation through an unresolved callee defers, and the
lazy read is woken by the unification that binds the callee (`docs/language-spec.md`
§Named instantiation arguments). Whether it *resolves* depends on what binds the
placeholder; the wrappers keep the struct spelling because the field types are
independent, not because `_` is impossible.

## What is still open

1. **The device path.** `stage_run` refuses by name
   (`RunError::ScalarsNotPushed`); pushing the leaf list is the work.
2. **A recorded body.** A recording carries the extent alone, so a runtime scalar
   in a recorded body is refused rather than recorded as an edge.
3. **An interleaved parameter.** The host decode is positional and the fragment
   carries no role table, so a parameter that interleaves scalars with `.in`
   should be refused by name; that refusal is not written.
4. **The host input struct is the author's to spell.** No shipped lambda builds
   the runtime-scalar shape, because a shipped one would have to hardcode a scalar
   name.
5. **A `plrun` result's element class reaches the type graph through the read
   argument's shape rather than the result's own cell.** With the struct argument
   the old path is gone: `a_gpu_program_chains_two_kernels_on_a_device` saw its
   collected array's element type go from `Int` to undecided, and it now pins the
   **values** rather than the element type. The honest fix is
   [class-channel](class-channel.md) §2/§3 — one authority for a class's value,
   read by the result's own cell.
