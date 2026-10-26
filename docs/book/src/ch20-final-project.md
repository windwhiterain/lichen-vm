# A Final Project: Mean-centering, End to End

This chapter builds a complete program in the compute layer: given a row
of floats, subtract the row's mean from every element. It is two operator
kernels — a reduction that sums the row, and an elementwise kernel that
subtracts the mean — and one host-side kernel that fills the row, because
the surface has no array-to-buffer conversion. It is small enough to hold
in one screen while touching buffers, struct constructors, claims, folds,
launches, and the answer check.

It is the shape of a real operator. `layernorm` (in
[`examples/layernorm.lichen`](https://github.com/windwhiterain/lichen-vm/blob/dev/examples/layernorm.lichen)) is the
same program with two reductions and a square root; everything else in the
transformer's operator set is built the same way.

## The plan

1. A **fill kernel** writes the row itself: the surface has no
   array-to-buffer conversion, so the host builds its input with a kernel
   like every other buffer.
2. A **reduction kernel** reads the row and writes its sum into a
   one-element buffer. The decrease is an `@loop` fold inside its own
   lane, so no cross-lane communication is needed.
3. An **elementwise kernel** reads the sum, divides it by the row's length
   to get the mean, and writes `x[i] - mean` for its lane.
4. The **host program** runs the three kernels and reads the result back.

The two operator kernels carry a claim — `Reduction(combine,identity)` for
the sum, `Reduction(element)` for the subtraction — so the fusion
machinery knows what shape each one is even though this program runs them
separately. The fill kernel carries no claim: it is how the host gets
data in, not a shape the floor reads.

## The program

```lichen
---
  compute = import "compute.lichen"
---
Row  := _x => {.x (compute.Buf (compute.BufParam{.type Float, .length 4}))}
Sums := _x => {.s (compute.Buf (compute.BufParam{.type Float, .length 4}))}
ParS := _x => {.n @any : Int, .in Row}
row_sum_raw := (k <: ParS) => {
  i := compute.range k.n
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read (compute.Read{.from k.in.x, .at s(0) - 1}) : Float)) ~ "combine"))
  b_s := compute.buf (compute.BufParam{.type Float, .length k.n})
  (Sums{.s (compute.write (compute.Write{.to b_s, .at i, .value (fold (k.n + 0, 0.0 ~ "identity"))}))})
}
row_sum := compute.parallel (row_sum_raw ~ "claim:Reduction(combine,identity)") "cpu" Sums
Moments := _x => {.x (compute.Buf (compute.BufParam{.type Float, .length 4})), .s (compute.Buf (compute.BufParam{.type Float, .length 4}))}
Centred := _x => {.y (compute.Buf (compute.BufParam{.type Float, .length 4}))}
ParC := _x => {.n @any : Int, .in Moments}
centre_raw := (k <: ParC) => {
  i := compute.range k.n
  m := (compute.read (compute.Read{.from k.in.s, .at 0}) : Float) / int2float k.n
  x := (compute.read (compute.Read{.from k.in.x, .at i}) : Float)
  b_y := compute.buf (compute.BufParam{.type Float, .length k.n})
  (Centred{.y (compute.write (compute.Write{.to b_y, .at i, .value ((x - m) ~ "element")}))})
}
centre := compute.parallel (centre_raw ~ "claim:Reduction(element)") "cpu" Centred
Fill := _x => {.n @any : Int}
fill_raw := (c <: Fill) => {
  i := compute.range c.n
  d := compute.buf (compute.BufParam{.type Float, .length 4})
  v := 1.0 + int2float i
  (Row{.x (compute.write (compute.Write{.to d, .at i, .value v}))})
}
fill := compute.parallel fill_raw "cpu" Row
x := (compute.plrun fill (Fill{.n 4}) : Struct).x
s := (compute.plrun row_sum (compute.A Row{.n 4, .I (Row{.x x})}) : Struct)
out := (compute.plrun centre (compute.A Moments{.n 4, .I (Moments{.x x, .s s.s})}) : Struct)
compute.collect out.y : Array
```

**This program does not run today**, for the reason Chapter 17 states: the
compute surface has not caught up with the constructor model, and the
parallel lowering of a kernel that carries buffers and structs is refused
with a *definition cycle* diagnostic. The arithmetic it is written to do is
exact: the row is `1, 2, 3, 4`, whose mean is `2.5`, so the values it
writes are `-1.5`, `-0.5`, `0.5`, `1.5`, every one of them exact in
`f32`. The book does not print that line as the program's answer,
because the program does not produce one yet.

## Walking through it

**The types.** `Row`, `Sums`, `Moments` and `Centred` are ordinary struct
constructors (Chapter 8), and each carries its buffers — a buffer's type
states its element type and its length. The kernel's parameter is a
constructor the kernel is matched against, `ParS := _x => {.n @any : Int,
.in Row}`: the launch's extent and the input struct. There is no output
group in the parameter: a kernel's **result** is its codomain, and the
body declares the buffer it writes and hands it back, so `Sums` is the
reduction's result type.

**The reduction.** The interesting line is the `@loop` fold (abridged):

```text
@loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read …) ~ "combine"))
```

`s(0)` is the remaining count and `s(1)` the accumulator — the fold's
state is a **tuple**, read by position. The fold counts **down** to zero
in its own lane, reading the row at `s(0) - 1` — an in-lane loop, not a
cross-lane reduction. The read's `: Float` is its element type, stated at
the call site (Chapter 17), and the accumulator's seed, `0.0 ~ "identity"`,
is marked as the fold's identity; each addend is marked as the `combine`
— the two marks the `Reduction(combine,identity)` claim names. The fold's
trip count is `k.n + 0` — the launch's extent, read through an
arithmetic node, because the extent's own node cannot carry a loop's
count, and the arithmetic leaves it the same number. Spelled `k.n`, the
sum comes out zero and the program would answer the row unchanged.

**The elementwise kernel.** `m` is the mean: the sum buffer's one element
divided by the row's length, converted with `int2float`. Each lane reads
its own `x[i]`, subtracts `m`, and writes at its own index into the buffer
the body declared, `b_y := compute.buf …`; the written version of that
buffer is what the body returns. The claim `Reduction(element)` says the
kernel's per-lane work is the `element` mark.

**The host.** There is no array-to-buffer conversion, so the host's first
job is the **fill kernel**: it declares a buffer, writes the row
`1, 2, 3, 4` at its lane's index, and returns it — and the buffer it
returns is the input of the reduction. Then `compute.plrun` runs one
kernel with a typed argument and a typed result, and `compute.collect`
reads a buffer back as an array. The three `plrun`s run the kernels one
after the other — the centring kernel reads the sum kernel's `s.s` buffer,
so the dependency is the data, not a schedule.

**The check.** The `collect` annotation, `: Array`, is what makes the
printed value an array rather than an open buffer answer. Every value in
the answer is exact, so the hand-computed output is a real check rather
than a model of an approximation (Chapter 16's `output` promise).

## Making it a layer norm

The next step is the operator this one is half of. To go from centering to
`y = (x - mean) / sqrt(var + eps) * w`:

1. Add a second reduction for `Σx²`, in the same shape as `row_sum` (a
   second kernel writing a second one-element buffer).
2. In the elementwise kernel, compute `var = Σx²/n - mean²` from the two
   buffers, take `sqrt (var + eps)`, and divide.
3. Read `eps` as a scalar leaf of the parameter's constructor, as the
   layernorm example does: a scalar argument goes after the extent leaf, and
   `examples/matmul.lichen` records what happens when a leaf is put in the
   wrong place.

The repository's `layernorm` example is that program, with a claim
`Reduction(combine,identity,element)` on the normalise kernel, and the
`softmax` example is a diamond of the same kind: two producers, one
consumer.

## Making it an example

Put the file in `examples/` with a header and let the suite check it
(Chapter 16):

```text weight.lichen
---
  order = "40"
  output = "[-1.5, -0.5, 0.5, 1.5]: Array"
  doc = "Subtract the row mean: y[i] = x[i] - mean(x). Two kernels because the
  sum is a reduction and the subtraction is elementwise."
---
compute = import "compute.lichen"
… the program above …
```

Running `cargo run -p lichen-tools --bin sync-readme` fills the `output`
line from the program's actual answer, and the suite test makes the
promise hold from then on. That is the whole publishing workflow: the
program, its promise, and the tool that keeps them equal — and, while the
parallel path is under repair, the promise is a claim about what the
program will answer once the lowering lands.

## Where to go next

You have now seen the language's model (Parts I and II), its type system
(Part III), and its programs (this part). The places to go deeper:

- the [language spec](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/language-spec.md) — the precise contract for
  everything the book taught informally;
- the [feature notes](https://github.com/windwhiterain/lichen-vm/tree/dev/docs/notes/) — each one records a decision, a
  measurement, and the alternatives that were rejected;
- [`examples/`](https://github.com/windwhiterain/lichen-vm/tree/dev/examples) — the living specification: matmul,
  softmax, layernorm, attention, and the rest of the operator set, each
  with its worked-out answer;
- the compute layer's notes — the [graph path](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/compute-graph-jit.md),
  the [operator ladder](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/llm-operators.md), and the
  [device legs](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/device-leg-of-the-ladder.md).

lichen is young, and the book describes the compiler as it is. If
something here no longer matches what the compiler does, the compiler is
right — and the fix belongs in both, in the same commit.

## Summary

- A real operator is a small number of kernels: a reduction, an
  elementwise step, and a host that sequences them by data.
- The fold counts down inside its lane; the marks name its combine and
  identity; the claim names the whole kernel.
- The host builds its input with a fill kernel, runs with `compute.plrun`,
  and reads back with `compute.collect`; the collect's annotation decides
  the array's type.
- The program's parameters and results are struct **constructors**, and the
  kernels are matched against them with `<:`, which applies the constructor
  once.
- A layer norm is this project plus a second reduction and a square root.
- While the parallel path is under repair, the book states what a kernel
  program means rather than printing an answer it does not produce.
- Publishing a program is the program, its `output` promise, and the tool
  that keeps them equal.
