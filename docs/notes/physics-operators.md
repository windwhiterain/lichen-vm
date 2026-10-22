# Physics operators: a stencil, and the boundary that is the padding

`examples/stencil.lichen` is the ladder's first **non-ML** operator and its first whose **read set
is wider than its lane**: every operator below it reads `x[i]`, or a slice from `0`, while this one
reads `i - 1` and `i + 1` as well. One explicit step of the 1D heat equation is enough to state the
shape, and it turns out to need no case analysis at all.

## The shape

`y[i] = x[i] + alpha * (x[i - 1] - 2 * x[i] + x[i + 1])` — the three-point Laplacian, one lane per
element, the write at the lane's **own** index (`compute.range c.n` with `.at i`), and three reads at
`i`, `i - 1`, `i + 1`. `alpha` is a **`Float` leaf**, so the diffusion number is a caller's argument
rather than a literal (`e24d00fb`); the first scalar leaf is the extent and the second is free
because the body reads no coordinates (`crates/lichen-compute/src/compute/body.rs:788-815`).

## The boundary condition is the padding

At `i = 0` the left tap indexes outside the buffer, and at `i = n - 1` the right one does. An
out-of-buffer read **answers `0`** (`crates/lichen-compute/src/compute.rs:6234`, the same rule that
makes `examples/gemv.lichen` answer `0` for `x`'s fourth element), so both ends are
**Dirichlet-zero** with no branch, no `Select` and no clamp — the operator has no case analysis in
it.

That this holds on the **device** too is the measurement, not an assumption: the input is uploaded
at its own length and the tail past it is zeroed on the device, so a lane whose index runs off the
end reads that zero. `crates/lichen-tools/examples/operators_on_device.rs` answers
`agrees, 18 of 18 element(s)` on the RTX 3060 for `stencil.lichen`'s three chains of six — this one, on
`x = 4 0 0 0 0 4` with `alpha = 0.25`, is `2 1 0 0 1 2` by hand.

**A different boundary is a different operator, and that is the surface's shape rather than this
file's choice.** A periodic boundary needs integer modulo on the index; a clamped one needs `max`;
a boundary that is *a parameter* needs a `Select` — and a `Select` refuses when its two arms are
computed in different classes (`examples/attention.lichen`'s `scores` refusal, fixed at
`b6dea46c`). So the honest statement is that this file spells the one boundary the language can
spell without a conditional, and the others are work rather than impossibility.

**In two dimensions only half of that survives, and the half that does not is worth naming
up front**: the vertical taps are the padding, the horizontal ones are not, because a row-major
flattening makes them cross into the neighbouring row. See §What the family needs next, where the
second chain and the mask are the measurement.

## The read set is wider than the lane, and that has two consequences

- **The read set is not a prefix of the buffer.** The upload-width rule ("a host input is uploaded
  at its own length, the padding past it zeroed") is what makes the right tap defined; a device
  run that uploaded `count x element` would have made it a stale byte instead, which is why the
  second row of `docs/notes/lichen-compute-gpu.md`'s history mattered here.
- **The surplus lanes.** `LOCAL_SIZE_X` is 64 and this run covers six indices, so 58 lanes are
  surplus. A surplus lane's fabricated coordinates would put its write at `i < 6` if nothing
  guarded it — the class of silent wrong answer measured on `examples/kvcache.lichen`'s append
  before the guard landed. A six-element stencil is therefore also a **device check of the
  tail-lane guard**, and it agrees, so the guard holds on a body whose read addresses are computed.

## Stability is physics, not an invariant

The explicit scheme is stable only for `alpha <= 1/2`, and the operator cannot enforce it: `alpha`
is a runtime leaf by design, so a caller passing `1.0` gets a wildly wrong step rather than a
refusal. That is the same trade the surface makes everywhere — a `compute.write` is the value of a
position and the language checks types, not schemes (`docs/notes/code-audit.md`, `wontfix:D17`).
What the example *can* pin is that the arithmetic is exact for its own data: every value is a
quarter or a half below `2^24`, so no `exp`, `sqrt`, `ln` or division is involved and the two
backends agree bit for bit.

## What the fusion floor makes of it

**The floor refuses the family at its precondition, not at an address.** Through the graph leg all
three shapes run on the device and answer exactly what the example's host path answers —
`2 1 0 0 1 2`, `3 1 0 0 1 3` and `1 1 0 0 1 1 1 0 0` — measured by
`crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs`'s `stencil_dirichlet`,
`stencil_ring` and `stencil_grid` cases, which print the same two runs (`recorded` with no pass
installed, `floored` with it). With the floor installed, each is refused **`NotClaimed`**: *"no
claim is registered for the fragment at 0, so it is in no row of the digest join and no algebra can
be derived for it; a composed fragment is exactly this, because composition registers neither body it
joins"*.

That sentence is the floor's **precondition** rather than a wall about addresses: its model starts
from a `claim:Reduction(combine,identity,element)`, and a stencil has no reduction in it to claim.
`examples/gemv.lichen` reaches the address questions (`RegionIsNotAPair`, then `BeyondANode`) only
because it *has* such a claim. So the stencil family is outside the floor's model today, and
"outside" here means **before** the question of whether two stencils could fuse — which is the honest
answer, because the floor's algebra is about reductions and a stencil is pointwise arithmetic with
neighbour reads.

One reading trap, since the probe prints it: it does not force the program's type node, so **every**
case there prints `raw[…]: ?a`, including the ones known to be good. The numbers are the reading;
the type is not.

### The claim is the gate, and a claimed reduction still cannot be a node

The same probe now carries four operators that have nothing else in common, and the pattern is the
floor's model rather than any one spelling:

| operator | claim | refusal |
|---|---|---|
| the 1D stencil (a step, no reduction) | none | `NotClaimed` |
| `filter.lichen`'s convolution (`y[i] = Σ_k w[k]·x[i−k]`, runtime weights) | `Reduction(combine,identity,element)` | `RegionIsNotAPair` **and** `BeyondANode` |
| `mathlib.lichen`'s `rowsum` (`r * kdim + k`) | same | `RegionIsNotAPair` **and** `BeyondANode` |
| `mathlib.lichen`'s `scale` (elementwise) | none | `NotClaimed` |

So the **claim** is exactly the gate between "the floor refuses before asking anything" and "the floor
asks and refuses", and **every claimed reduction lands on the same two sentences**: the region holds
one instance (`a merge is one producer and one consumer`), and then the instance's `element` role is
read as consuming *"the value another node produced"* — while in all three programs it consumes a
**parameter**, and the node is the graph's first and only producer. That is the classification
question first raised for `gemv`, now measured on three operators with nothing in common beyond the
claim, so it is the floor's model of a reduction and not one program's spelling.

The floor's accepted shape is still a producer→consumer **pair**. A single operator is outside it
whether or not it is claimed: without a claim it does not get in, and with one it gets in only as half
of a pair it does not have.

## What the family needs next

- **Iteration is measured, and the three shapes do not agree.** On the ring's own data, three
  straight-line `plrun`s answer `2.1875 1.3125 0.5 0.5 1.3125 2.1875`, typed `array{Float, 6}`, and
  the total is **`8` at every step** — the unrolled solver works and its conservative property is
  visible in the declared output, which is the form an example can pin.
  A top-level `@loop` around the same dispatch behaves differently depending on what it carries:
  - **state is the kernel's buffer** (`seed = looped.y`, each iteration handing `s(1)` to `.x`): the
    loop dispatches, answers the same six numbers, prints `array{Float, 6}`, and needs **no
    annotation** — the state's type comes from the seed. It is the spelling both examples use, and the
    cheapest one: no `collect` inside the step (§What a step costs).
  - **state is a plain `array{Float, 6}` and each step builds its input with `compute.buffer (s(1))`**:
    the loop **dispatches and answers the same six numbers**, and its type is **undecided** —
    the printer shows `raw[raw 2.1875, …]: ?a` — until the recursive function is annotated, when it
    prints the typed array. Both spellings are measured on *this* program by
    `crates/lichen-compute/tests/lichen_compute/zz_fold_type_probe.rs`, whose four readings are
    `[straight]` (control, typed), `[loop]` (`?a`), `[loop_root_annotated]` (`(out : array{Float, 6})`,
    typed) and `[loop_recursion_annotated]`
    (`((s => …) : tuple{Int, array{Float, 6}} -> array{Float, 6})`, typed).
  - **state is the `XOut` struct, with the recursive function annotated** (`tuple{Int, XOut} -> XOut`):
    it works exactly as the array state does — same six numbers, typed `array{Float, 6}`. **Unannotated**
    it prints `raw none: array{Float, 6}` and records **no diagnostic at all**, which is worse than the
    array state's unannotated `?a`: an array loses only its type, a struct loses the value itself. So
    the rule is the recursion annotation again, and every state works once it is stated. **Re-measured
    at `dev@9f2f63f6`, which contains `function-normalization`'s structural fix (`9fad30c7`): still
    `raw none`**, so that fix does not reach this path. (`4b832f58` is the record of the experiments,
    not the fix, and the first re-measurement was taken before the fix landed.)
  - **`(XOut)(.y x)` is not a usable constructor** for a seed: it yields a value nothing can read.
    Seed the loop with a real kernel result's `.y` instead.

  **Answered: the `?a` is the recursion-annotation rule, not the dispatch.** A loop whose body
  contains no dispatch at all prints the same shape — unannotated `@loop r = s => … r (s(0) - 1, 7)`
  gives `r: ?a -> ?b` and `out: raw 7: ?c`, while `@loop r = ((s => …) : tuple{Int, Int} -> Int)`
  gives `r: tuple{Int, Int} -> Int` and `out: Int` (the type system's owner measured both on a toy
  loop, with and without an application in the body). A recursive function's type follows from its
  annotation (`language-spec.md` §3's `f : f` rule; `examples/gcd.lichen` states it), so the
  array-state loop is typed by annotating `run` — the dispatch is not what loses the type.

  What follows for a solver: **it can be looped and typed.** Write it unrolled, or as a fold over an
  array state whose recursive function carries `tuple{Int, array{Float, 6}} -> array{Float, 6}`; an
  example that declares an output should prefer the annotated loop or the unrolled form, and the
  root annotation works too but states the answer's type away from the function that computes it.
- **Two dimensions is measured now, and it half-falsified what this note first said.** `compute.row`
  and `compute.col` work, with the **second** scalar leaf as the divisor for both coordinates, so
  `cols` goes second and the strides after it (`examples/matmul.lichen` states the rule for its own
  `k`). But the boundary is **not** the same padding question four times. The vertical taps are —
  `(row +/- 1) * cols + col` runs off the end and answers `0` — and the **horizontal** ones are not:
  `i - 1` at `col = 0` is the *previous row's last element* and `i + 1` at `col = cols - 1` is the
  *next row's first*, so a flat neighbour walk **wraps**. The second chain of
  `examples/torus.lichen` masks them, `if col == 0 then 0.0 else read (i - 1)` and the same at
  `cols - 1`, and **a `Select` whose two arms are both `Float` is accepted** — the refusal this note
  cites from `attention.lichen`'s `scores` was about arms computed in *different* classes — so the
  horizontal boundary costs one `Select` per direction and nothing else. Measured: `agrees, 18 of 18
  element(s)` on the RTX 3060 for `torus.lichen`'s two chains, so the mask is right on the device too.
  **A torus wraps all four taps** — `rows = c.n / c.cols`, `above`/`below` selecting the last and
  first rows and `left`/`right` the last and first columns — and then every element contributes to
  exactly two taps, so **the total cannot move**: the plane sums to `8` and so do two wrapped steps,
  against the Dirichlet grid's step which sums to `5`. That is the second chain in
  `examples/torus.lichen`, looped once, and that file answers `agrees, 18 of 18 element(s)`; the one-dimensional half, including its own solver loop, is `examples/stencil.lichen`, `agrees, 18 of 18` of its own.
- **A periodic boundary is measured, and it changes the answer in one place.** A `Float`-armed
  `Select` is accepted, so the ring is the wrap done on purpose:
  `if i == 0 then read (n - 1) else read (i - 1)` and the mirror at `n - 1`. On the **same**
  `4 0 0 0 0 4`, the Dirichlet step answers `2 1 0 0 1 2` and the ring answers `3 1 0 0 1 3` — the
  two edges stop leaking — and all three chains together answer
  `agrees, 18 of 18 element(s)` on the RTX 3060. What is *not* expressible is a boundary that is a
  **runtime parameter**: the condition would have to be data, and the surface has no such `Select`.
- **A conserved quantity can be *read* but not *stated*.** The ring's total is unchanged — `8` in,
  `8` out — while the Dirichlet step's is `6`, because the Laplacian sums to zero over a ring, so
  the conservation is visible in the declared output and nowhere else. The language has no place for
  a physical invariant, which means a solver's correctness rests on the output a caller chose to
  declare. That is honest rather than convenient, and it is what anybody writing a solver in this
  language has to live with.

## What a step costs

The ladder has had correctness readings only; this is the first cost reading it records. It was taken
by dispatching the same stencil kernel 1 and 101 times through the annotated loop and taking the
difference, so the compile and the seed are paid by both runs and what is left is 100 dispatches. Wall
clock around the whole run, on
this machine's RTX 3060 and its host:

| backend | lanes | `steps = 1` | `steps = 101` | per extra dispatch |
|---|---|---|---|---|
| `cpu` | 6 | 35.7 ms | 372.0 ms | **3.36 ms** |
| `gpu` | 6 | 41.0 ms | 464.0 ms | **4.23 ms** |
| `cpu` | 1024 | 88.5 ms | 2009.6 ms | **19.21 ms** |
| `gpu` | 1024 | 66.5 ms | 787.5 ms | **7.21 ms** |

Three readings, and the third is the one that matters:

- **At six lanes the device is slower**: 4.23 ms against 3.36 ms, because at six lanes there is
  nothing but dispatch overhead and the device path pays more of it.
- **At 1024 lanes it is 2.8 times faster**: 7.21 ms against 19.21 ms, because the host interprets
  element by element while the device runs sixteen workgroups. Per element that is 0.0070 ms against
  0.0186 ms — both of them enormous next to a hand-written kernel, which is what an interpreter
  costs.
- **The array-state loop's cost is its per-step `compute.collect`**, which round-trips the answer
  through the host every iteration. That is measurable, because the **buffer** state exists and does
  not collect: a fold threading the kernel's buffer answers the same three steps, typed, and the
  dispatch timing read both states. Per extra dispatch:

  | backend | lanes | buffer state | array state |
  |---|---|---|---|
  | `cpu` | 6 | 3.25 ms | 3.36 ms |
  | `cpu` | 1024 | 17.06 ms | 19.21 ms |
  | `gpu` | 6 | 3.38 ms | 4.23 ms |
  | `gpu` | 1024 | **3.29 ms** | **7.21 ms** |

  At 1024 lanes on the device the readback is **over half the array-state cost** (3.9 ms of
  7.21), and it is the reason the device's advantage looked like 2.5× when it is really 5.2×: the
  same kernel against the same host is `17.06 / 3.29`. At six lanes the two states are within noise,
  because there is nothing to read back that the dispatch did not already cost.

  **This note said earlier that a buffer-threading loop did not exist and was the measurement to
  make.** That was wrong, and the error is worth keeping visible: round 36 tried a fold carrying the
  kernel's *struct* (which prints `raw none`), concluded that threading the buffer failed too, and
  wrote the conclusion down as measured. Round 43 tried `s(1)` as a buffer and it works — the struct
  is the shape that fails, not the buffer. A capability claim about a spelling is a hypothesis until
  that spelling is run.

### What the fold costs

The same probe now carries a second kernel, both lifted verbatim from the examples that compile them, so
the difference is a fold and six more reads: the three-read stencil (`examples/stencil.lichen`) and the
nine-tap convolution (`examples/filter.lichen`), buffer state, median of three runs.

| backend | lanes | stencil (3 reads) | convolution (9-tap fold) | pooling (9-tap max) |
|---|---|---|---|---|
| `cpu` | 6 | 3.60 ms | 3.91 ms | 4.53 ms |
| `gpu` | 6 | 3.89 ms | 3.88 ms | 4.60 ms |
| `cpu` | 1024 | 16.16 ms | 68.13 ms | **69.22 ms** |
| `gpu` | 1024 | 3.79 ms | 3.89 ms | **4.84 ms** |

The host pays for the window and the device does not: nine taps cost the host **four times** the body
(16.16 to 68 or 69) and the device **3%** for a multiply-add fold or **28%** for a window max. So the
device's advantage grows with the work per element — 4.3x for the body at 1024 lanes, **17.5x** for the
nine-tap fold, 14.3x for the window max — which is the other side of the six-lane reading, where
dispatch overhead is all there is.

Two details the table shows and prose alone would not. **The two folds cost the same on the host**
(68.13 against 69.22): there, the per-element interpretation dominates and the kind of combine does not
matter. **On the device `max` costs more than a multiply-add** (4.84 against 3.89), and the pooling
window also reads three rows of a padded buffer rather than one, so part of that gap is the working
set rather than the operator.

The numbers are machine- and state-dependent: every figure is the **median of five** runs, with the min
and max printed beside it, and the spread runs from about 5% on the host to about 20% on the device's
six-lane case. They are readings of a shape, not benchmarks.
