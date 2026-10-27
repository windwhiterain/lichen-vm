# Kernels and the Compute Layer

lichen is not only a language for pure expressions. The shipping compiler
ships a second, embedded language for writing **kernels** — functions that
run once per element of a buffer — and a layer that dispatches them to the
interpreter, to a CPU JIT, or to a GPU. This chapter is a tour of that
layer: one kernel from scratch, the scalar JIT path, and what the compute
package is for.

The compute package is a native package the compiler provides, so
`import "compute.lichen"` needs no file: the compiler registers the module
and its operators itself.

## A first kernel

The program fills a buffer with three floats, doubles each one, and reads
the result back:

```lichen
---
  compute = import "compute.lichen"
---
In  := _x => {.x (compute.Buf (compute.BufParam{.type Float, .length 3}))}
Out := _x => {.y (compute.Buf (compute.BufParam{.type Float, .length 3}))}
Par := _x => {.n @any : Int, .in In}
double_raw := (k <: Par) => {
  i := compute.range k.n
  x := (compute.read (compute.Read{.from k.in.x, .at i}) : Float)
  y := compute.buf (compute.BufParam{.type Float, .length k.n})
  (Out{.y (compute.write (compute.Write{.to y, .at i, .value (x * 2.0 ~ "element")}))})
}
double := compute.parallel (double_raw ~ "claim:Elementwise(element)") "cpu" Out
Fill := _x => {.n @any : Int}
fill_raw := (c <: Fill) => {
  i := compute.range c.n
  d := compute.buf (compute.BufParam{.type Float, .length 3})
  v := 1.0 + int2float i
  (In{.x (compute.write (compute.Write{.to d, .at i, .value v}))})
}
fill := compute.parallel fill_raw "cpu" In
b := (compute.plrun fill (Fill{.n 3}) : Struct).x
out := (compute.plrun double (compute.A In{.n 3, .I (In{.x b})}) : Struct)
compute.collect out.y : Array
```

There is a lot in that program, so let us take it apart.

**The data is buffers, and the shape is a struct constructor.** A buffer's
type states its element type and its length —
`compute.Buf (compute.BufParam{.type Float, .length 3})` is a buffer
of three floats. The kernel's input and output are struct **constructors**
— `In`, `Out` — and a kernel's parameter is a constructor the kernel is
matched against, `Par := _x => {.n @any : Int, .in In}`. There is no output
group in the parameter: a kernel's **result** is its codomain, `Out`, and
the body hands that result back (below).

**The body runs once per lane.** `(k <: Par)` is the kernel's parameter,
matched against the constructor: the match applies `Par` once and unifies
the parameter with the requirement it returns. The first line, `i :=
compute.range k.n`, is the lane's own index — the kernel is launched over
`n` lanes, and `compute.range` answers which one this invocation is. The
rest of the body reads its input at `i`, computes, and writes its output
at `i`. That shape — one lane per element, read at the lane index, write
at the lane index — is the compute language's model.

**Reads and writes name their buffer and position.** `compute.read` takes
a `Read` value built with `{.from buf, .at i}`, and the call states its
element type — `(compute.read … : Float)` — because the buffer's element
type is a value in the buffer's struct and the read's result is the type
the caller states, not one read off the buffer's value (Chapter 4's rule:
a value whose declared type is the element class does not carry a type for
the read to take). `compute.write` takes a `Write` built with
`{.to buf, .at i, .value v}`. The `_` placeholders are the constructors'
own type parameters (Chapter 5's `_`), which the checker decides.

**A kernel declares the buffers it writes.** The body names the buffer it
writes — `y := compute.buf (compute.BufParam{.type Float, .length
k.n})` — and a write's **result** is that buffer's new version. So the
body ends by building the result struct, `(Out{.y (compute.write …)})`:
the value that flows out *is* the write's result, and there is no effect
outside the value.

**The host builds its input with a kernel.** There is no way to turn an
array into a buffer, so the program runs a second, tiny kernel — the
`fill` kernel — whose body writes the values `1, 2, 3` at its lane's
index, and whose own result is the buffer the first kernel reads. The
fill kernel is an ordinary kernel: it declares a buffer, writes at its
lane's index, and returns it. A sequence that is not a plain sum is
spelled as a sum of one-level `if`s, one term per element — `(if i == 0
then 2.0 else 0.0) + (if i == 1 then 4.0 else 0.0) + …` — which is what
[the buffer-declaration note](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/kernel-buffer-declarations.md)
records.

**Marks label the computation.** The body is marked
`~ "claim:Elementwise(element)"` before `compute.parallel` builds the
kernel from it, and the written value is marked `~ "element"`. Marks are
compile-time metadata (Chapter 13); the compute layer's fusion machinery
reads them to know what shape a kernel has. A claim that does not match
the body is refused by the fusion pass, not by the language.

**The launch is typed.** `compute.plrun kernel argument : Struct` runs
the kernel once; the argument is built with `compute.A In` and carries the
extent (`.n 3`) and the input struct (`.I (In{.x b})`). `compute.collect
out.y : Array` reads a buffer back as an ordinary array — the only way a
value leaves the compute layer — and the annotation is what decides the
resulting array's type.

**The backend is a string.** `compute.parallel … "cpu" Out` binds the
kernel to the CPU path and names its output struct constructor. The output
type is passed rather than read off the kernel's fields: those fields hold
type values, and a read takes its type from a value that has one (Chapter
4's rule), so the caller states it. The other backend is `"gpu"`; the same
kernel program, dispatched to the device backend, runs the kernel's
compiled form on a GPU. The example suite's kernels run on `"cpu"` so the
suite is device-independent; the device legs are measured separately
(below).

### The parallel path is under repair

**This program does not run today.** The compute layer is mid-migration: the
constructor form above is the shape the language states, but the compute
package's own surface has not caught up with it. The kernel body reads
`k.in.x`, and `k.in` is the constructor `In`, not an instance of it, so the
field read has no struct to read:

```text
error: expected Struct, found ?a => Struct
  --> 9:42
   |
 9 |   x := (compute.read (compute.Read{.from k.in.x, .at i}) : Float)
   |                                          ^
```

The same refusal reaches every read through a constructor-valued field, and
the launch's argument then reports `no field named n` and `no field named I`
on `Struct`. The repository's buffer-valued operator examples are at different
stages of the same migration; the measured failures are recorded in
[the constructor note](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/value-templates.md).

What the program *means* is nevertheless stated by its own text, and it
is the shape every kernel in the repository has: the fill kernel writes
`1.0`, `2.0`, `3.0` at lanes `0`, `1`, `2`, and the double kernel reads
each lane's own element, multiplies it by `2.0`, and writes it back at the
same lane. The value that leaves the layer is the collection of those
three elements. The book does not print an answer for it, because the
program does not produce one yet.

## The scalar JIT path

Not every kernel needs a buffer. A scalar function can be compiled and
launched on its own:

```lichen
---
  compute = import "compute.lichen"
---
k_double := compute.jit (y : Int => y + y)
k_outer  := compute.jit (x : Int => compute.launch k_double x)
compute.launch k_outer 3
```

```text
6: Int
```

`compute.jit f` compiles the function `f` into the compute language's
kernel form; `compute.launch k x` applies one. A jitted function may
launch another, as `k_outer` does, and the launch is typed: the result of
`compute.launch k_outer 3` is `6 : Int`. This is the narrowest useful
entry into the layer: no buffers, no lanes, just a function compiled and
run.

## The graph path

The operator examples use the **graph** path: a chain of kernels is
recorded as one graph and submitted together, so the fusion pass can
decide that two kernels are one region and run them as one dispatch. The surface spells it with `compute.graphrun`, and the fusion
claims are what the pass reads. The whole path is documented in the
[graph JIT note](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/compute-graph-jit.md) and
[graph fusion](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/graph-fusion.md); the language-level chapters
only need to know three positions:

- the **interface claim** — the `~ "claim:…"` mark on a kernel — states the
  kernel's shape: `Elementwise(element)`, `Reduction(combine,identity,
  element)`, and similar;
- the **element marks** inside a body label the values a claim refers to;
- the **fusion floor** (`install_floor`) is what reads the claims, decides
  regions, and refuses a claim that does not match the body. Its refusal
  is by name, with the claim and the reason.

The examples that exercise the path are the LLM operators — `matmul`,
`softmax`, `rmsnorm`, `layernorm`, `attention`, and the rest — and their
notes record the ladder, the walls, and the measured device legs.

## The two backends

A kernel can run on `"cpu"` or `"gpu"`. The CPU backend lowers the kernel
to the compute language's own IR and runs it; the GPU backend compiles it
to SPIR-V and dispatches it. Both read the same kernel definition, so a
program's *answer* should be the same on both — and measuring that is the
device work the notes record:

- the emitted SPIR-V is validated with `spirv-val` (`--target-env
  vulkan1.1`);
- the device legs are measured operator by operator
  ([device operator sweep](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/device-operator-sweep.md));
- the transcendental functions are defined in the kernel IR, and the
  emitter forbids the device from contracting or reassociating them
  ([contraction control](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/contraction-control.md));
- a device's division is not correctly rounded, so a kernel with a
  division can differ in the last bit from the `cpu` path
  ([division rounding](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/division-rounding-gap.md)).

The point for a language reader is that the compute layer is **part of the
language's answer**: the same source, run on either backend, is the same
program, and the notes record where the backends may differ and why.

## What the surface can and cannot say

The compute language is deliberately narrow, and the walls are measured.
A few that shape what a kernel can look like:

- **No two-dimensional arrays.** A matrix is a flat array with index
  arithmetic in the body; the examples spell `A[r * k + c]` by hand.
- **No runtime-length arrays.** An array's length is in its **requirement**
  (Chapter 7); a buffer's length is the launch's extent, and the host
  writes it with a kernel.
- **One lane per element.** A kernel writes one position per lane; a
  reduction folds inside its own lane and writes its result there.
- **No per-row reduction across lanes yet.** Cross-lane exchange needs a
  shared-memory staging and a barrier; that work is staged in
  [shared memory and barriers](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/shared-memory-and-barriers.md),
  and a two-dimensional workgroup is not yet available
  ([two-dimensional launch](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/two-dimensional-launch.md)).
- **A boundary chosen at run time** has no spelling; a boundary is the
  padding, a clamp, or a branch you write.

None of these is a limitation of the *language*: they are the shape of the
compute layer's current contract, and each has a note that records what
was measured and what is next.

## Summary

- The compute package is a native package; `import "compute.lichen"` needs
  no file.
- A kernel is a body of one lane, launched over an extent, reading and
  writing buffers at its lane's index.
- A kernel's parameter is matched against a struct **constructor** —
  `Par := _x => {.n @any : Int, .in In}` — and `compute.Buf` is a buffer
  whose type states its element type and its length; `compute.range` is the
  lane
  index; `compute.read`/`compute.write` are the buffer reads and writes,
  and a read names its element type at the call site; `compute.buf`
  declares a buffer, `compute.parallel` builds a kernel and `compute.plrun`
  runs it; `compute.collect` reads a buffer back as an array, and it is the
  only way out.
- **The parallel path is under repair**: a kernel body reads `k.in.x`, but
  `k.in` is the constructor `In` rather than an instance, so the compiler
  refuses it with `expected Struct, found ?a => Struct`; the launch then loses
  its field names. The book's programs state their semantics; they do not print
  answers.
- The backend is a string, `"cpu"` or `"gpu"`, and both run the same kernel.
- `compute.jit`/`compute.launch` is the scalar path, and it runs.
- The graph path records a chain and lets the fusion pass merge regions from
  the claims the bodies carry.
- The compute layer's walls are measured and each has a note; they are the
  current contract, not the language's limit.

## Exercises

1. Change the first kernel to triple its input and say what answer the
   program is written to produce.
2. Change the values the fill kernel writes, and say what the collection
   would be.
3. Explain why `compute.collect` needs the `Array` annotation to answer an
   array at all.
4. Write a scalar `compute.jit` kernel that squares its input and launch it.
5. Change the backend string to `"gpu"` and describe what would have to be
   true of the machine for the program to run.
6. Read one of the operator examples (`examples/matmul.lichen` or
   `examples/softmax.lichen`) and identify its claim and its element marks.
7. Pick one of the walls above and read the note it points at; state what
   was measured.
