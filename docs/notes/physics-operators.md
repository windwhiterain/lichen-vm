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
end reads that zero. `crates/lichen-compute/examples/operators_on_device.rs` answers
`agrees, 6 of 6 element(s)` on the RTX 3060 for `x = 4 0 0 0 0 4`, `alpha = 0.25`, whose answer is
`2 1 0 0 1 2` by hand.

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

## What the family needs next

- **A second step** is a second `plrun` over the first step's output: no new spelling, and the
  answer is exact only for the same reason (the arithmetic is multiply-add and subtract).
- **Two dimensions is measured now, and it half-falsified what this note first said.** `compute.row`
  and `compute.col` work, with the **second** scalar leaf as the divisor for both coordinates, so
  `cols` goes second and the strides after it (`examples/matmul.lichen` states the rule for its own
  `k`). But the boundary is **not** the same padding question four times. The vertical taps are —
  `(row +/- 1) * cols + col` runs off the end and answers `0` — and the **horizontal** ones are not:
  `i - 1` at `col = 0` is the *previous row's last element* and `i + 1` at `col = cols - 1` is the
  *next row's first*, so a flat neighbour walk **wraps**. The second chain of
  `examples/stencil.lichen` masks them, `if col == 0 then 0.0 else read (i - 1)` and the same at
  `cols - 1`, and **a `Select` whose two arms are both `Float` is accepted** — the refusal this note
  cites from `attention.lichen`'s `scores` was about arms computed in *different* classes — so the
  horizontal boundary costs one `Select` per direction and nothing else. Measured: `agrees, 15 of 15
  element(s)` on the RTX 3060, both chains together, so the mask is right on the device too.
- **A periodic boundary is measured, and it changes the answer in one place.** A `Float`-armed
  `Select` is accepted, so the ring is the wrap done on purpose:
  `if i == 0 then read (n - 1) else read (i - 1)` and the mirror at `n - 1`. On the **same**
  `4 0 0 0 0 4`, the Dirichlet step answers `2 1 0 0 1 2` and the ring answers `3 1 0 0 1 3` — the
  two edges stop leaking — and all three chains together answer
  `agrees, 21 of 21 element(s)` on the RTX 3060. What is *not* expressible is a boundary that is a
  **runtime parameter**: the condition would have to be data, and the surface has no such `Select`.
- **A conserved quantity can be *read* but not *stated*.** The ring's total is unchanged — `8` in,
  `8` out — while the Dirichlet step's is `6`, because the Laplacian sums to zero over a ring, so
  the conservation is visible in the declared output and nowhere else. The language has no place for
  a physical invariant, which means a solver's correctness rests on the output a caller chose to
  declare. That is honest rather than convenient, and it is what anybody writing a solver in this
  language has to live with.
