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

The program reads three floats from a buffer, doubles each one, and reads
the result back:

```lichen
---
  compute = import "compute.lichen"
---
In  = struct{.x (compute.Buf Float)}
Out = struct{.y (compute.Buf Float)}
Par = compute.P (compute.KT Float)(.I In, .O Out)
double_raw = (k : Par) => {
  i = compute.range k.n
  x = compute.read ((compute.Read _)(.from k.in.x, .at i))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (x * 2.0 ~ "element")))
}
double = compute.parallel (double_raw ~ "claim:Elementwise(element)") "cpu"
b = (compute.buffer ([1.0, 2.0, 3.0] : array{Float, 3}) : (compute.Buf Float))
out = (compute.plrun double ((compute.A In)(.n 3, .I In(.x b))) : Out)
compute.collect out.y : array{Float, 3}
```

```text
[2.0, 4.0, 6.0]: array{Float, 3}
```

There is a lot in that program, so let us take it apart.

**The data is buffers, and the shape is a struct.** `compute.Buf Float` is
a buffer of floats. The kernel's input and output are struct types —
`In`, `Out` — and a kernel's parameter type is
`compute.P (compute.KT Float)(.I In, .O Out)`: a parallel kernel over
`Float` cells, taking one `In` and producing one `Out`.

**The body runs once per lane.** `k : Par` is the kernel's parameter. The
first line, `i = compute.range k.n`, is the lane's own index — the kernel
is launched over `n` lanes, and `compute.range` answers which one this
invocation is. The rest of the body reads its input at `i`, computes, and
writes its output at `i`. That shape — one lane per element, read at the
lane index, write at the lane index — is the compute language's model.

**Reads and writes name their buffer and position.** `compute.read` takes
a `Read` value built with `(.from buf, .at i)`; `compute.write` takes a
`Write` built with `(.to buf, .at i, .value v)`. The `_` placeholders are
the constructors' own type parameters (Chapter 5's `_`), which the checker
decides.

**Marks label the computation.** The body is marked
`~ "claim:Elementwise(element)"` before `compute.parallel` builds the
kernel from it, and the written value is marked `~ "element"`. Marks are
compile-time metadata (Chapter 13); the compute layer's fusion machinery
reads them to know what shape a kernel has. A claim that does not match
the body is refused by the fusion pass, not by the language.

**The launch is typed.** `compute.plrun kernel argument : Out` runs the
kernel once; the argument is built with `compute.A In` and carries the
extent (`.n 3`) and the input struct (`.I In(.x b)`). `compute.collect
out.y : array{Float, 3}` reads a buffer back as an ordinary array, and the
annotation is what decides the resulting array type.

**The backend is a string.** `compute.parallel … "cpu"` binds the kernel
to the CPU path. The other backend is `"gpu"`; the same kernel program,
dispatched to the device backend, runs the kernel's compiled form on a
GPU. The example suite's kernels run on `"cpu"` so the suite is
device-independent; the device legs are measured separately (below).

## The scalar JIT path

Not every kernel needs a buffer. A scalar function can be compiled and
launched on its own:

```lichen
---
  compute = import "compute.lichen"
---
k_double = compute.jit (y : Int => y + y)
k_outer  = compute.jit (x : Int => compute.launch k_double x)
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
- **No runtime-length arrays.** A buffer's length is a launch parameter; an
  array's length is in its type (Chapter 7).
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
- `compute.P`/`compute.KT` describe a parallel kernel's parameter;
  `compute.Buf` is a buffer; `compute.range` is the lane index;
  `compute.read`/`compute.write` are the buffer reads and writes;
  `compute.parallel` builds a kernel and `compute.plrun` runs it;
  `compute.collect` reads a buffer back as an array.
- The backend is a string, `"cpu"` or `"gpu"`, and both run the same kernel.
- `compute.jit`/`compute.launch` is the scalar path.
- The graph path records a chain and lets the fusion pass merge regions from
  the claims the bodies carry.
- The compute layer's walls are measured and each has a note; they are the
  current contract, not the language's limit.

## Exercises

1. Change the first kernel to triple its input and check the answer.
2. Fix the lane count to 2 and explain why `compute.collect` still needs the
   `array{Float, 3}` annotation to answer a 3-element array.
3. Write a scalar `compute.jit` kernel that squares its input and launch it.
4. Change the backend string to `"gpu"` and describe what would have to be
   true of the machine for the program to run.
5. Read one of the operator examples (`examples/matmul.lichen` or
   `examples/softmax.lichen`) and identify its claim and its element marks.
6. Pick one of the walls above and read the note it points at; state what
   was measured.
