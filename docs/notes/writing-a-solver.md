# Writing a solver: the order that works

A recipe for somebody who wants to write a GPU operator or a time-stepping solver in lichen and has
not read the research. [physics-operators](physics-operators.md) is the record of *how* each rule was
found; this is the order to do things in. The two acceptance samples are
`examples/stencil.lichen` (one dimension) and `examples/torus.lichen` (two), and every claim below is
taken from one of them or from a measurement those notes cite.

## 1. Fix the launch shape first: one lane per element

The only write spelling whose answer does not depend on a schedule is the lane's own index:

```
i = compute.range c.n
compute.write ((compute.Write _)(.to c.out.y, .at i, .value …))
```

`compute.write` names a slot of an output buffer exactly as long as the launch's extent, so this is
a bijection. **Reads may be anywhere** — `i - 1`, `i + 1`, `r * kdim + k`, `(row - 1) * cols + col` —
and a read outside the buffer answers `0`. That is not an error path, it is a boundary condition
(§4). A body that writes at a *computed* address instead is the shape the fusion floor refuses by
name, so do not reach for it.

## 2. Decide what the constants are

The scalar leaves of the parameter struct are positional:

- the **first** leaf is the launch extent;
- the **second** is the divisor `compute.row` and `compute.col` use — if the body reads no
  coordinates, the second leaf is free (put a stride here only when the body *does* use them, and
  then the stride must be exactly this);
- every further leaf is carried at its own class, so a **`Float` leaf is a runtime argument**: `alpha`,
  `lo`, `hi`, `t`. A caller varies it without a new kernel.

```text
Par  = struct{.n Int, .cols Int, .alpha Float, .in In, .out Out}   # cols is the coordinate divisor
Arg  = struct{.n Int, .cols Int, .alpha Float, .I In}              # the arithmetic side, no `.out`
```

A kernel is `(compute.plrun k ((Arg)(…)) : Out)` — the annotation is the result's type, and without it
the value is fine but the type is not.

## 3. Reduce inside the lane, never across lanes

A fold counts down **inside** a single lane, so a per-row reduction launches over the **rows** and
walks the row itself:

```
r = compute.range c.n
@loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + compute.read (… .at (r * c.kdim + (s(0) - 1)))) ~ "combine" ~ "element"))
compute.write ((compute.Write _)(.to c.out.y, .at r, .value (fold (c.kdim, 0.0 ~ "identity"))))
```

The read address becomes computed and the write stays the lane's index — the split `examples/gemv`
has. Mark the body `claim:Reduction(combine,identity,element)` if you want the fusion floor to see it
at all; without a claim the floor refuses the graph with `NotClaimed`, before it asks anything about
addresses.

## 4. Choose the boundary, and know which ones exist

| boundary | spelling | cost |
|---|---|---|
| zero outside the buffer (Dirichlet) | read `i - 1` and `i + 1` and do nothing else | free — this is the default |
| wrap (periodic) | `if i == 0 then read (n - 1) else read (i - 1)`, mirrored at the end | one `Select` per end |
| masked in one axis of a row-major grid | `if col == 0 then 0.0 else read (i - 1)`, mirrored | one `Select` per axis |
| anything the caller *chooses at run time* | **not expressible**: the condition would have to be data | — |

A `Select` is accepted when **both arms are `Float`**. The refusal you may have seen quoted is about
arms computed in *different* classes (`examples/attention`'s `scores`), which a physics body never
needs. In a row-major grid the **vertical** taps are the padding and the **horizontal** ones are not —
`i - 1` at `col = 0` is the previous row's last element — so a grid needs the mask and a 1D body does
not.

## 5. Iterate: array state, and annotate the recursive function

A time step is one dispatch; a solver is the same dispatch repeated. A plain host run is **one pass**
and an operator asked before its operand is decided is never asked again, so the loop's state must be
a value the next iteration can rebuild from:

```
seed = (compute.collect (compute.plrun k (…) : Out).y : array{Float, N})
@loop march = ((s => if s(0) == 0 then s(1) else
    march (s(0) - 1, (compute.collect (compute.plrun k ((Arg)(… .I In(.x (compute.buffer (s(1)) : (compute.Buf Float))))) : Out).y : array{Float, N})))
  : tuple{Int, array{Float, N}} -> array{Float, N})
solved = march (steps, seed)
```

Two rules, both measured:

- **The state is an array, not the kernel's output struct.** A fold carrying the struct never
  resolves — one iteration or three, it prints `raw none`. Rebuilding with `compute.buffer (s(1))`
  is what works.
- **Annotate the recursive function** (`tuple{Int, array{Float, N}} -> array{Float, N}`). A recursive
  function's type follows from its annotation (`language-spec` §3's `f : f` rule); without it the
  answer prints `raw[…] : ?a` — the numbers are right, the type is missing, and nothing downstream can
  use them. Annotating the *root* also works, but it states the type away from the function.

The loop **does dispatch**: iteration composes with kernels, on both backends, with the numbers the
unrolled form gives.

## 6. Declare the answer by hand

Work the arithmetic out yourself and put it in `output = "…"`. The example suite compares that string
to what the program prints, so a declaration transcribed from a run proves nothing. Pick data with an
exact answer — quarters and halves below `2^24`, no `exp`, `sqrt`, `ln` or non-power-of-two division —
and pick it so the boundary matters: a bump at each end shows a Dirichlet leak, two bumps that are
each other's wrapped neighbour show what the mask buys.

A conservation is the strongest declaration available: on a ring or a torus the Laplacian sums to
zero, so the total cannot move, and the declared output is the only place that fact can appear. The
language has no way to *state* an invariant — only a value a caller can check.

## 7. Put it where it will be run

- `examples/<name>.lichen` with `order`, `doc` and `output` → the example suite runs it on `cpu` (use
  `"cpu"` in `compute.parallel`), `sync-readme` regenerates the README, and
  `cargo run -p lichen-compute --example operators_on_device` finds every example containing
  `compute.parallel` and reproduces it on the device against its `cpu` answer.
- Both variables are worth switching on in the `doc`: the `cpu` declaration and the device verdict,
  because they answer different questions.

## 8. What it costs, today

Per extra dispatch of a stencil, measured on this machine (RTX 3060 and its host):

| backend | lanes | per dispatch |
|---|---|---|
| `cpu` | 6 | 3.35 ms |
| `gpu` | 6 | 3.78 ms |
| `cpu` | 1024 | 19.09 ms |
| `gpu` | 1024 | 7.51 ms |

The device loses below a few hundred elements (overhead only) and wins 2.5× at 1024. **Both columns
are dominated by the loop's per-step `compute.collect`**: the array-state loop round-trips the answer
through the host every iteration, because threading a resident buffer is the shape that does not
resolve. So a looped solver pays a readback per step — write it if the answer matters, unroll it if
the cost does, and do not expect kernel-speed arithmetic either way: these are interpreter numbers.

## Where to read more

- [physics-operators](physics-operators.md) — the stencil family, every reading above, and what is
  not expressible yet.
- [device-operator-sweep](device-operator-sweep.md) — the device verdict for every operator example,
  with the method (`operators_on_device`) that reproduces it.
- `examples/stencil.lichen`, `examples/torus.lichen` — the two samples this recipe is written from.
