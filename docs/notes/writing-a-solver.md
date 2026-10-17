# Writing a solver: the order that works

A recipe for somebody who wants to write a GPU operator or a time-stepping solver in lichen and has
not read the research. [physics-operators](physics-operators.md) is the record of *how* each rule was
found; this is the order to do things in. The five acceptance samples are
`examples/stencil.lichen` (one dimension), `examples/torus.lichen` (two), `examples/filter.lichen` (a moving window), `examples/convolution2d.lichen` (a two-dimensional one) and `examples/pooling.lichen` (a window max), and every claim below is
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
  `lo`, `hi`, `t`. A caller varies it without a new kernel. **The constants may also be a buffer**: a
convolution's taps are an input buffer plus a `.taps` leaf, so the same kernel is a smoothing filter or
a difference (`examples/filter.lichen`).

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

**The identity is where modelling creeps in.** A sum has `0.0` as a true neutral element; a **max has
none**, so a `rowmax` takes its seed as a leaf and the caller passes something below every datum
(`examples/mathlib.lichen` passes `(0.0 - 100.0)` and says so). That is the one place a reduction
library has to model rather than compute, and the kernel cannot detect a seed that is too high — it
would clip the answer silently. A product-accumulate (`dot`) is back on the honest side: `0.0` really
is its identity.

## 4. Choose the boundary, and know which ones exist

**The index arithmetic is unsigned, and that decides which boundaries are spellable.** `i - 1` at
`i = 0` is not `-1` in a kernel body: it wraps to a huge value. That is *why* the padding works — the
huge index is out of the buffer, so the read answers `0` — and it is why a **lower** clamp has to be
shifted: `max (i - 1) 0` never takes the bound, because the wrapped value is positive. Measured:
`min (max (col - 1) 0) 4` answers `4, 0, 1, 2, 3` for `col = 0..4`
(`zz_replicate_address_probe.rs`). Write `(max i 1) - 1`, which is `max (i - 1, 0)` with no negative
intermediate.

| boundary | spelling | cost |
|---|---|---|
| zero outside the buffer (Dirichlet) | read `i - 1` and `i + 1` and do nothing else | free — this is the default, and the unsigned index is what makes it work |
| wrap (periodic) | `if i == 0 then read (n - 1) else read (i - 1)`, mirrored at the end | one `Select` per end, at the body's **top level** |
| masked in one axis of a row-major grid | `if col == 0 then 0.0 else read (i - 1)`, mirrored | one `Select` per axis, top level; **not spellable inside a fold** |
| replicate (the edge repeats) | `min ((max i 1) - 1) (dim - 1)` per coordinate, then `row * cols + col` | two builtins per axis, no `Select`, **works inside a fold** |
| anything the caller *chooses at run time* | **not expressible**: the condition would have to be data | — |

A `Select` is accepted when **both arms are `Float`** — **at the body's top level.** Inside a fold body it is refused: the fold is compiled into a template before any apply, and a conditional there has no value to emit (`zz_masked_conv_probe.rs` measures it with a read in the arm and with a bare `0.0`/`1.0`, so the cause is the conditional and not the read). That is why every masked boundary in the samples sits at the top level, and why a masked *window* has to be a caller's zero border or the clamp below. The refusal you may have seen quoted is about
arms computed in *different* classes (`examples/attention`'s `scores`), which a physics body never
needs. In a row-major grid the **vertical** taps are the padding and the **horizontal** ones are not —
`i - 1` at `col = 0` is the previous row's last element — so a grid needs the mask, a clamp, or a
caller-supplied border, and a 1D body needs none of them.

`examples/convolution2d.lichen` carries two of these side by side — the flat address over a
zero-bordered grid, and the replicate clamp over a value on the edge — because that pair is the
cheapest way to see what a boundary decision costs.

**What may sit where, measured** (all three in `zz_clamp_grouping_probe.rs` and
`zz_masked_conv_probe.rs`):

- **An application binds tighter than `+` and `*`**, so `min (max i lo) hi * cols` is
  `(min (max i lo) hi) * cols`. Three spellings of one clamp, only the first parenthesised, answer the
  same `102 102 102 103 104 104` over `i = 0..5`; the `* 10 + 1` variant answers `21 21 21 31 41 41`,
  the clamp's own values, so the arithmetic tail stays outside.
- **`min`/`max` may sit inside a fold even though a `Select` may not**, which is what makes §4's
  replicate boundary spellable in a window. The unsigned-index trap in its spelling, and the shifted
  `(max i 1) - 1` that fixes it, are §4's preamble — measured in `zz_replicate_address_probe.rs`. The
  upper clamp alone was verified there first, and the lower one silently was not: that is the mistake
  the preamble exists to record.
- **An `Int` result cannot join a `Float` fold accumulator.** A fold whose arm is
  `s(1) + (min (max (s(0) - 1) 0) 4)` is refused by the *device* backend, not the parser: *"the block
  parameter is read as one class in one place and another ([Int, Float]). A block parameter is one
  `OpPhi` and has one type"*. The accumulator is one phi, so the integer needs `int2float` before it
  joins. Addresses are exempt because they never enter the accumulator — they live inside `.at`.

## 5. Iterate: thread the kernel's buffer

A time step is one dispatch; a solver is the same dispatch repeated. A plain host run is **one pass**
and an operator asked before its operand is decided is never asked again, so the loop has to be a
`@loop` whose step dispatches. The state should be **the kernel's own buffer**:

```
seed = (compute.plrun k ((Arg)(…)) : Out).y
@loop march = ((s => if s(0) == 0 then s(1) else
    march (s(0) - 1, (compute.plrun k ((Arg)(… .I In(.x s(1)))) : Out).y))
  : tuple{Int, compute.Buf Float} -> compute.Buf Float)
solved = march (steps, seed)
answer = (compute.collect solved : array{Float, N})
```

Three rules, all measured:

- **A buffer threads through and is the cheapest state.** No `collect` inside the step, so the loop
  pays one readback for the whole run instead of one per step — at 1024 lanes on the device that is
  3.29 ms per dispatch against 7.21 ms for an array state. It also needs **no annotation**: the
  state's type comes from the seed.
- **A fold carrying the kernel's output *struct* works only when the recursive function is annotated**
  (`tuple{Int, Out} -> Out`). Unannotated it prints `raw none` and records **no diagnostic**, which is
  worse than the array state's unannotated `?a`: a struct loses the value, an array loses only the type.
  Do not build a seed with `(Out)(.y x)` either — it yields a value nothing can read.
- **If the state must be an array** (because the step needs the numbers on the host), rebuild with
  `compute.buffer (s(1))` and **annotate the recursive function**
  (`tuple{Int, array{Float, N}} -> array{Float, N}`). A recursive function's type follows from its
  annotation (`language-spec` §3's `f : f` rule); unannotated, the answer prints `raw[…] : ?a` — the
  numbers are right, the type is missing, and nothing downstream can use them. Annotating the *root*
  also works, but it states the type away from the function.

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

| backend | lanes | per dispatch, buffer state | per dispatch, array state |
|---|---|---|---|
| `cpu` | 6 | 3.25 ms | 3.36 ms |
| `gpu` | 6 | 3.38 ms | 4.23 ms |
| `cpu` | 1024 | 17.06 ms | 19.21 ms |
| `gpu` | 1024 | **3.29 ms** | **7.21 ms** |

Three readings: the device loses at six lanes, where there is nothing but dispatch overhead; it wins
at 1024, and by **5.2×** rather than the 2.8× an array state makes it look; and the difference between
the two states *is* the loop's readback — 3.9 ms of the array state's 7.21 ms at 1024 lanes on the
device. So thread the buffer (§5) unless the step needs the numbers on the host, and do not expect
kernel-speed arithmetic either way: these are interpreter numbers, and no spelling changes that.

## Where to read more

- [physics-operators](physics-operators.md) — the stencil family, every reading above, and what is
  not expressible yet.
- [device-operator-sweep](device-operator-sweep.md) — the device verdict for every operator example,
  with the method (`operators_on_device`) that reproduces it.
- `examples/stencil.lichen`, `examples/torus.lichen`, `examples/filter.lichen`, `examples/convolution2d.lichen`, `examples/pooling.lichen` — the five samples this recipe is written from.
