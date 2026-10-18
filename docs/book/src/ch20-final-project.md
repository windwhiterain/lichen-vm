# A Final Project: Mean-centering, End to End

This chapter builds a complete program in the compute layer: given a row
of floats, subtract the row's mean from every element. It is two kernels —
a reduction that sums the row, and an elementwise kernel that subtracts
the mean — and it is small enough to hold in one screen while touching
buffers, structs, claims, folds, launches, and the answer check.

It is the shape of a real operator. `layernorm` (in
[`examples/layernorm.lichen`](../../../examples/layernorm.lichen)) is the
same program with two reductions and a square root; everything else in the
transformer's operator set is built the same way.

## The plan

1. A **reduction kernel** reads the row and writes its sum into a
   one-element buffer. The decrease is an `@loop` fold inside its own
   lane, so no cross-lane communication is needed.
2. An **elementwise kernel** reads the sum, divides it by the row's length
   to get the mean, and writes `x[i] - mean` for its lane.
3. The **host program** creates the buffer, runs the two kernels, and reads
   the result back.

Each kernel carries a claim — `Reduction(combine,identity)` for the sum,
`Reduction(element)` for the subtraction — so the fusion machinery knows
what shape each one is even though this program runs them separately.

## The program

```lichen
---
  compute = import "compute.lichen"
---
Row  = struct{.x (compute.Buf Float)}
Sums = struct{.s (compute.Buf Float)}
ParS = compute.P (compute.KT Float)(.I Row, .O Sums)
row_sum_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.s, .at i, .value (fold (k.n, 0.0 ~ "identity"))))
}
row_sum = compute.parallel (row_sum_raw ~ "claim:Reduction(combine,identity)") "cpu"
Moments = struct{.x (compute.Buf Float), .s (compute.Buf Float)}
Centred = struct{.y (compute.Buf Float)}
ParC = compute.P (compute.KT Float)(.I Moments, .O Centred)
centre_raw = (k : ParC) => {
  i = compute.range k.n
  m = compute.read ((compute.Read _)(.from k.in.s, .at 0)) / int2float k.n
  x = compute.read ((compute.Read _)(.from k.in.x, .at i))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((x - m) ~ "element")))
}
centre = compute.parallel (centre_raw ~ "claim:Reduction(element)") "cpu"
x = (compute.buffer ([1.0, 2.0, 3.0, 4.0] : array{Float, 4}) : (compute.Buf Float))
s = (compute.plrun row_sum ((compute.A Row)(.n 4, .I Row(.x x))) : Sums)
out = (compute.plrun centre ((compute.A Moments)(.n 4, .I Moments(.x x, .s s.s))) : Centred)
compute.collect out.y : array{Float, 4}
```

```text
[-1.5, -0.5, 0.5, 1.5]: array{Float, 4}
```

The row is `1, 2, 3, 4`, whose mean is `2.5`, so the answer is
`-1.5, -0.5, 0.5, 1.5` — every value exact in `f32`, because every number
involved is a small integer or a half.

## Walking through it

**The types.** `Row`, `Sums`, `Moments` and `Centred` are ordinary nominal
structs (Chapter 8), and each carries its buffers. The kernel's parameter
type is `compute.P (compute.KT Float)(.I … , .O …)`: the element class
`Float`, one input struct, one output struct.

**The reduction.** The interesting line is the `@loop` fold (abridged):

```text
@loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read …) ~ "combine"))
```

`s(0)` is the remaining count and `s(1)` the accumulator. The fold counts
**down** to zero in its own lane, reading the row at `s(0) - 1` — an
in-lane loop, not a cross-lane reduction. The accumulator's seed,
`0.0 ~ "identity"`, is marked as the fold's identity, and each addend is
marked as the `combine` — the two marks the `Reduction(combine,identity)`
claim names.

**The elementwise kernel.** `m` is the mean: the sum buffer's one element
divided by the row's length, converted with `int2float`. Each lane reads
its own `x[i]`, subtracts `m`, and writes at its own index. The claim
`Reduction(element)` says the kernel's per-lane work is the `element` mark.

**The host.** `compute.buffer` uploads the array to a buffer;
`compute.plrun` runs one kernel with a typed argument and a typed result;
`compute.collect` reads a buffer back as an array. The two `plrun`s run
the kernels one after the other — the second reads the first's `s.s`
buffer, so the dependency is the data, not a schedule.

**The check.** The `collect` annotation, `array{Float, 4}`, is what makes
the printed type the array rather than an open buffer answer. Every value
in the answer is exact, so the hand-computed output is a real check rather
than a model of an approximation (Chapter 16's `output` promise).

## Making it a layer norm

The next step is the operator this one is half of. To go from centering to
`y = (x - mean) / sqrt(var + eps) * w`:

1. Add a second reduction for `Σx²`, in the same shape as `row_sum` (a
   second kernel writing a second one-element buffer).
2. In the elementwise kernel, compute `var = Σx²/n - mean²` from the two
   buffers, take `sqrt (var + eps)`, and divide.
3. Read `eps` as a scalar leaf of the parameter struct, as the layernorm
   example does: a scalar argument goes after the extent leaf, and
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
  output = "[-1.5, -0.5, 0.5, 1.5]: array{Float, 4}"
  doc = "Subtract the row mean: y[i] = x[i] - mean(x). Two kernels because the
  sum is a reduction and the subtraction is elementwise."
---
compute = import "compute.lichen"
… the program above …
```

Running `cargo run -p lichen-tools --bin sync-readme` fills the `output`
line from the program's actual answer, and the suite test makes the
promise hold from then on. That is the whole publishing workflow: the
program, its promise, and the tool that keeps them equal.

## Where to go next

You have now seen the language's model (Parts I and II), its type system
(Part III), and its programs (this part). The places to go deeper:

- the [language spec](../../language-spec.md) — the precise contract for
  everything the book taught informally;
- the [feature notes](../../notes/) — each one records a decision, a
  measurement, and the alternatives that were rejected;
- [`examples/`](../../../examples) — the living specification: matmul,
  softmax, layernorm, attention, and the rest of the operator set, each
  with its worked-out answer;
- the compute layer's notes — the [graph path](../../notes/compute-graph-jit.md),
  the [operator ladder](../../notes/llm-operators.md), and the
  [device legs](../../notes/device-leg-of-the-ladder.md).

lichen is young, and the book describes the compiler as it is. If
something here no longer matches what the compiler does, the compiler is
right — and the fix belongs in both, in the same commit.

## Summary

- A real operator is a small number of kernels: a reduction, an
  elementwise step, and a host that sequences them by data.
- The fold counts down inside its lane; the marks name its combine and
  identity; the claim names the whole kernel.
- The host uploads with `compute.buffer`, runs with `compute.plrun`, and
  reads back with `compute.collect`; the collect's annotation decides the
  array type.
- A layer norm is this project plus a second reduction and a square root.
- Publishing a program is the program, its `output` promise, and the tool
  that keeps them equal.
