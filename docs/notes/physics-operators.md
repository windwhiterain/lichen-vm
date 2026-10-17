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
- **Two dimensions** needs `compute.row`/`compute.col` and then the positional rule bites: the
  **second** scalar leaf is the divisor for both coordinates, so the strides go after it
  (`examples/matmul.lichen` states the rule for its own `k`). Four neighbours make the boundary the
  same padding question four times.
- **A periodic or clamped boundary** is the first thing in this family that needs a conditional, and
  the class rule above is what decides whether it can be spelled at all today.
- **A conserved quantity** — mass, energy — cannot be stated: the language has no place for a
  physical invariant, so the declared output is the only acceptance there is. That is honest rather
  than convenient, and it is what a solver written in this language has to live with.
