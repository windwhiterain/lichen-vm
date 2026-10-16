# The transcendental gap, named

> Status: **current** — every figure below is from an RTX 3060 Laptop GPU on this branch, off
> `dev@6861f451`, driven the way [device-body-sweep](device-body-sweep.md) §1 drove its own:
> expand the operator's body with `expand_transcendentals`, dispatch it, and read each lane's
> `to_bits()` back. **Nothing here changed production code, and no test was added.**

[device-body-sweep](device-body-sweep.md) closed its slices with two sentences it could not
convert into a measurement, and named the probe that would:

> *"a precision decision on the transcendentals **larger than the one asked for** — 130 ulp is
> **not a tolerance question**"*

> *"the mechanism behind the transcendental gap. Contraction vs. reduction-division rounding vs. a
> different sequence — the reference table shows the device on both sides of the definition and
> does not say which."*

This slice runs that probe and answers both. **The mechanism is multiply-add contraction**, and it
is not the contraction the previous slice tried to model: the polynomial on its own is not enough,
and the reduction's subtraction on its own is not enough either. The device contracts the Horner
**and** its optimiser puts the two-term Cody–Waite reduction back together into a single term
against the `f32` nearest `π/2` — the exact thing [floating-point](floating-point.md) §8 says the
split exists to prevent. **One model of that reproduces the device's bits at 512 of 512 arguments
for `sin`, 512 of 512 for `cos`, and 512 of 512 for `exp`.**

And the headline correction is the other half of the mandate: **measured in absolute error, "130
ulp" is at most 2 ulp of one.** For `sin` and `cos` the gap is a bounded, modest absolute error and
the verdict *is* a tolerance question — just not one stated in ulps of the answer. For `exp` the gap
is real but it is **the definition's own accuracy, not the device's**: the device sits exactly as far
from the platform's correctly rounded answer as the definition does. The tail is bounded in absolute
terms and **unbounded in relative terms**, and at `θ = 8133.6` the device's `sin` comes back with the
**opposite sign** to the definition's.

## 1. The three candidates, one at a time

### 1.1 Multiply-add contraction — **this one**

**The probe is the one [device-body-sweep](device-body-sweep.md) §6 named**: a kernel of the form
`a·b + c` over a significand grid, compared against `cpu`. One body, `out[i] = a[i]·b[i] + c[i]`
over three buffers, **4096 points chosen because a fused and an unfused reading disagree on them** —
the filter is `f32::mul_add(a,b,c).to_bits() != (a*b + c).to_bits()`, and the grid has to be coarse
enough that `a·b` is inexact, or every product is exact and the two readings coincide.

| spelling | points | device `==` mul-then-add | device `==` fused |
|---|---|---|---|
| `a·b + c` | 4096 | **0 / 4096** (worst 1 ulp away) | **4096 / 4096** (worst 0) |
| `a·b − c` | 4096 | **4096 / 4096** (worst 0) | 0 / 4096 |

**Measured: this device contracts a multiply feeding an add, and only that.** Every one of the 4096
points lands on the fused reading and on none of the split readings, and the 1 ulp the split column
reports is exactly the difference the filter selected for. The subtract spelling is the control, and
it is a clean negative: **`OpFSub` after `OpFMul` is not contracted**, 4096 of 4096 on the unfused
reading. So the candidate is not "the device computes floats loosely" — it is a specific,
characterised rewrite with a specific shape.

**Measured: contraction alone is not enough.** Modelled over the operator's own body, over 512
arguments in `[−16, 16)`, counting how many of the device's bits each model reproduces:

| model — what the device is assumed to compute | `sin` bits | `cos` bits |
|---|---|---|
| `plain` — the definition, unfused | 103 / 512 (worst 1502 ulp) | 89 / 512 (worst 657) |
| `poly` — **the Horner fused**, nothing else | 101 / 512 | 85 / 512 |
| `reduce` — **the reduction's two subtractions fused**, nothing else | 103 / 512 | 89 / 512 |
| `all` — every multiply-add in the body fused | 101 / 512 | 85 / 512 |
| `recombine_fma` — the two-term reduction put back to one term, fused | 486 / 512 (worst 1 ulp) | 498 / 512 (worst 1 ulp) |
| `recombine_mul` — the same, **not** fused | 267 / 512 (worst 256 ulp) | 231 / 512 (worst 768 ulp) |
| **`both` — the Horner fused *and* the reduction recombined and fused** | **512 / 512** | **512 / 512** |

**Measured: the mechanism holds across the whole declared domain, not only where the first sweep
looked.** Over `sin` and `cos` separately on `[4000, 8192]` and on `(−8192, −4000]`, 2048 arguments
each, `both` reproduces **2048 of 2048** device bits every time, worst 0 ulp. At those angles the
unfused definition reproduces **0 of 2048**.

**`exp` is the simpler half of the answer and it is exact: the Horner fused and nothing else
reproduces 512 of 512 device bits**, worst 0 ulp. Its body has no reduction, only the clamp, the
halving by `2⁻⁷`, ten Horner steps and seven squarings. That is also why `exp` is the operator whose
ulp figure looks worst: each squaring doubles the polynomial's relative error, so a difference of a
**fraction of one ulp** in the Horner comes out at the end of seven of them as **tens to hundreds of
ulp**. The measured tail is 100–171 ulp at six arguments of 256.

### 1.2 The range reduction's division — **not this one**

[floating-point](floating-point.md) §7 records that this device does not round `f32` division
correctly, and §8 notes it is the reason `sqrt`'s reduction multiplies by a constant instead of
dividing by one. That made the reduction's division the obvious second candidate. It is ruled out
**structurally, before any measurement is needed**:

- **`sin_body` and `cos_body` contain no `KernelBin::Div` at all.**
  `crates/lichen-kernel-ir/src/unary.rs` builds them from `reduce_angle` (multiply by
  `INVERSE_TWO_PI`, multiply by `ONE_QUARTER`, multiply by `HALF_PI_HEAD`, multiply by
  `HALF_PI_TAIL`, two subtractions), one `r·r`, and the two Horner polynomials. The angle is turned
  into a quadrant count by multiplying by `2/π`, never dividing by `π/2`.
- **`exp_body` contains no `KernelBin::Div` either.** It halves by **multiplying** by `2⁻⁷`, which is
  [device-body-sweep](device-body-sweep.md) §2.1's exact case besides.

**Measured, in the emitted module rather than in the source**, so the claim does not rest on reading
`unary.rs` right. Every module the probe emitted was disassembled
(`C:\VulkanSDK\1.3.296.0\Bin\spirv-dis.exe`) and censused:

| emitted module | `OpFMul` | `OpFAdd` | `OpFSub` | **`OpFDiv`** | `Fma` / `OpExtInst` |
|---|---|---|---|---|---|
| `sin` | 17 | 11 | 7 | **0** | 0 |
| `cos` | 17 | 11 | 6 | **0** | 0 |
| `exp` | 18 | 10 | 0 | **0** | 0 |
| `sqrt` | 97 | 6 | 0 | **6** | 0 |
| `ln` | 55 | 53 | 1 | **1** | 0 |

The `sqrt` and `ln` rows are what make the census mean something: the emitter **does** emit
`OpFDiv` where a body has one — `sqrt`'s Newton step and `ln`'s `(m−1)/(m+1)` — and emits **none**
for the three in question. §7's hazard is real and it is live in those two; it cannot be what moves
these three, because these three have nothing to round wrongly.

### 1.3 A different sequence — **a different rounding, not a different sequence**

The brief asks for the emitted SPIR-V to be compared with the host's own
`exp_value`/`sin_value`/`cos_value` **instruction by instruction if it is readable**. It is
readable — `spirv-dis` is installed and the disassembly above is from this branch — and the
comparison is the sharpest result in the note:

- **The module is the sequence the definition says it is.** `sin` and `cos` are 578 and 562 words,
  `exp` is 412; the opcodes are `OpFMul`/`OpFAdd`/`OpFSub` in the order
  `crates/lichen-kernel-ir/src/unary.rs` builds them, the constants are the class's own, and there is
  no `Fma`, no `OpExtInst`, and nothing from `GLSL.std.450`. **There is no native `sin`, no native
  `exp`, and no library call in the module** — the host reading this note can check that in one
  command, and §1.2's census is the evidence.
- **So the divergence is entirely below SPIR-V, in the driver's optimiser.** The module says "multiply,
  then add"; the device does one rounding. That is why the `a·b + c` probe in §1.1 is decisive where a
  disassembly is not: SPIR-V cannot see this rewrite, and neither can any module-level inspection.

**And the rewrite is not only the fma.** The second half of the device's behaviour is visible only
in the numbers: `r = (|x| − q·HEAD) − q·TAIL` is two products and two subtractions against a `π/2`
that `floating-point.md` §8 splits deliberately, so that `q·HEAD` is an exact multiple of `2⁻¹¹` and
the first subtraction is Sterbenz-exact. The device's optimiser **folds the two products into one
against the single-rounded `f32` nearest `π/2`** and fuses the subtraction into the same operation:

```
recombined:  r = fma(-q, HEAD + TAIL, magnitude)      # the device
split:       r = (magnitude - q*HEAD) - q*TAIL        # the definition
```

Measured, `HEAD + TAIL` as an `f32` **is** the `f32` nearest `π/2` — both `1.5707964`, which is why
`recombine_fma` and `single_pi_fma` score identically in §1.1's table. What that costs is the whole of
the gap: the single-rounded `π/2` is **4.371139e-8 above the real one**, so the residual carries
`q · 4.37e-8` of drift, and at `q = 4` that is `1.75e-7` — which is precisely the `1.7508864e-7`
absolute error this note measures at `θ = 6.3125`, the argument
[device-body-sweep](device-body-sweep.md) §3.2 named as `sin`'s worst mover.

**Verdict: a different sequence is not this one.** The sequence is the definition's, module for
module; the *rounding* is a driver's, and it is a rounding this slice can name exactly.

## 2. Absolute error, not ulp

**Measured.** The grids are [device-body-sweep](device-body-sweep.md) §1's, so the bit-exact counts
reproduce that note figure for figure; `one ulp of one` is `2⁻²³ = 1.1920929e-7`, and "absolute" is
`|device − definition|` at the same point.

| sweep | bit-exact | worst ulp | **worst absolute** | **as ulp of one** | within one ulp of one | beyond one ulp of one |
|---|---|---|---|---|---|---|
| `sin`, 256 args, `[−8, 8)` | 91 (35.5%) | 94 | `1.7881393e-7` | **1.50** | 214 / 256 | 42 |
| `cos`, 256 args, `[−8, 8)` | 71 (27.7%) | 117 | `2.3841858e-7` | **2.00** | 211 / 256 | 45 |
| `sin`, 512 args, `[−1, 0)` | 437 (85.4%) | 2 | `1.1920929e-7` | **1.00** | 512 / 512 | 0 |
| `cos`, 512 args, `[−1, 0)` | 394 (77.0%) | 1 | `5.9604645e-8` | **0.50** | 512 / 512 | 0 |
| `exp`, 256 args, `[−16, 16)` | 244 (95.3%) | 171 | `5e1` | `4.2e8` | 250 / 256 | 6 |

**Measured: "130 ulp" collapses to 1.5 ulp of one for `sin` and 2.0 for `cos`.** The ulp figure was
the metric's artefact, and this note says which: at the worst point `sin(6.3125)` is `2.93e-2`, whose
own ulp is `1.86e-9`, while the **absolute** error is `1.75e-7` — `94` ulp of the answer and `1.47`
ulp of one at the same time. The answer's grid shrinks as it approaches its zero crossing and the
error does not, exactly as [device-body-sweep](device-body-sweep.md) §3.2 inferred. **The rule for a
reader is that a `sin`/`cos` tolerance is an absolute bound, not a ulp count**, and over `[−8, 8)` the
bound is **2 ulp of one**.

**Measured: `exp` is the one operator where the absolute error is genuinely large, and the reason is
not the device.** Six arguments of the 256 sit beyond one ulp of one, at **100, 109, 136, 140, 150 and
171 ulp** of their own answers; at the worst of them, `x = 15.125`, the definition answers
`3.704265e6` and the device `3.7043078e6`, an absolute error of `42.75` and a **relative error of
`1.15e-5`**. That is the same order as [floating-point](floating-point.md) §6's own measured accuracy
for the definition (**worst relative error `2.4e-5` against the correctly rounded value, set by the
seven squarings**). And the decisive comparison, measured against the platform's own correctly rounded
`f32::exp`:

| operator | definition vs platform | device vs platform | device strictly closer | definition strictly closer | tied |
|---|---|---|---|---|---|
| `exp`, `[−16, 16)` | 2 / 256 exact, worst **130 ulp** | 2 / 256 exact, worst **130 ulp** | 8 | 4 | 244 |
| `sin`, `[−8, 8)` | 218 / 256 exact, worst **1 ulp** | 87 / 256 exact, worst **94 ulp** | 4 | **161** | 91 |
| `cos`, `[−8, 8)` | 205 / 256 exact, worst **1 ulp** | 67 / 256 exact, worst **118 ulp** | 2 | **183** | 71 |

**So the prior note's verdict needs its two halves separated.** For `sin` and `cos` the device is
genuinely the worse of the two — further from the platform's own answer on 161 and 183 of 256 — and
that is a real, nameable defect (§1.1). For `exp` it is **not**: the device and the definition are
**the same distance from the platform on 244 of 256 arguments and the same 130 ulp worst**, and the
device is strictly closer on 8 and further on 4. `exp`'s 171 ulp is **two roundings of one
`1e-5`-accurate scheme disagreeing**, not a device that cannot do arithmetic. It is a tolerance
question about the definition, not a device question at all.

## 3. The tail: out to `θ = 8192`

**Measured, and what happens to the argument reduction there is the whole story.** The reduction is
`q = ⌊|x|·2/π + 1/2⌋` by truncation, then `r = |x| − q·(π/2)_f32` on the device. Nothing about that
changes with `θ`; what changes is `q`. At the domain edge `|x| = 8192 = 2¹³`, `q` reaches `5215` — the
bound `HALF_PI_HEAD`'s own invariant states — and the residual therefore carries `5215 · 4.371139e-8`.

**Measured: the absolute gap is linear in `q`, and the linearity is the proof of §1.1's mechanism.**
`cos` over `[0, 8192]` in sixteen buckets of 512 angles, worst absolute error per bucket, in units of
one ulp of one:

| `θ` in | `[0,512)` | `[512,1024)` | `[1024,1536)` | `[1536,2048)` | `[2048,2560)` | `[2560,3072)` | `[3072,3584)` | `[3584,4096)` |
|---|---|---|---|---|---|---|---|---|
| ulp of one | 116.6 | 235.7 | 354.3 | 473.8 | 592.8 | 713.8 | 836.3 | 953.9 |

| `θ` in | `[4096,4608)` | `[4608,5120)` | `[5120,5632)` | `[5632,6144)` | `[6144,6656)` | `[6656,7168)` | `[7168,7680)` | `[7680,8192)` |
|---|---|---|---|---|---|---|---|---|
| ulp of one | 1074.7 | 1191.5 | 1312.8 | 1430.7 | 1550.2 | 1669.8 | 1786.8 | **1908.5** |

Each bucket adds a constant ≈ `119.3` ulp of one per 512 of `θ`, which is `0.233` ulp of one per unit
of `θ`; `q` grows by `0.6366` per unit of `θ`, so the error per unit of `q` is `0.366` ulp of one =
**`4.36e-8`, and `(π/2)_f32 − π/2 = 4.371139e-8`**. The gap at the domain edge is
`2.2751372e-4 = 1908.5` ulp of one, which is `q ≈ 5206 · 4.37e-8`. Measured independently, a fine sweep
of 256 angles centred on each of `k·π` gives the same constant: `8.74e-8` at `k = 1`,
`8.74e-6` at `k = 100`, `4.37e-5` at `k = 500`, `8.74e-5` at `k = 1000`, `1.75e-4` at `k = 2000`,
`2.19e-4` at `k = 2500` — `2k · 4.37e-8`, to the digit.

**Measured: what happens at the zero crossings is where the ulp metric finally fails completely.**
Over **5200 arguments** at multiples of `π` across the whole domain, `sin` is bit-exact on **1** and
the worst ulp distance is **1,910,155,776**. That number is not a defect of the metric here — it is the
metric correctly reporting something. At `θ = 8133.6` the definition answers `−1.1594966e-4` and the
device answers `+1.103878e-4`: **the same sign as a sine near `π`, and the opposite sign.** The
absolute error is `2.27e-4` against an answer of order `1e-4`, so the **relative** error is `1.95`.

**So the tail is bounded in absolute terms and unbounded in relative terms, and both facts are
consequences of one another.** The bound is `q · 4.37e-8 ≤ 2.28e-4` for `|θ| ≤ 8192`, because the
definition's own domain ends there — and [floating-point](floating-point.md) §8 already records that
above `ANGLE_LIMIT` the answer is silently the sine of 8192. But nothing bounds the ratio of that
error to an answer that happens to be small, and **at a zero crossing the ratio has no bound at all
within the domain.** A bounded absolute gap is a tolerance; an unbounded relative one is a defect.
Which of the two this is depends entirely on whether the language ever takes a `sin` or a `cos` of a
large argument — and for `rope`, whose angles are positions, the answer is no, which is why this is a
statement about the domain rather than a bug report.

## 4. What the validator says

**Measured: 7 of 7 emitted modules are `spirv-val` ACCEPT**, each piped to
`C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe --target-env vulkan1.1`:

```
ACCEPT  sin           578 words     ACCEPT  sqrt        1576 words
ACCEPT  cos           562 words     ACCEPT  ln          1582 words
ACCEPT  exp           412 words     ACCEPT  a*b+c        277 words
                                      ACCEPT  a*b-c        277 words
```

The last two are §1.1's contraction probe. This extends [device-body-sweep](device-body-sweep.md)
§5's twelve from twenty-eight modules to thirty-five, and it keeps that note's standing conclusion:
**every divergence here is numerical, inside a module the specification accepts.** Nothing in §1 is
an emitter defect — the emitter writes the definition, faithfully, and the divergence is introduced
afterwards by a component that has no obligation to preserve `f32`'s rounding points.

## 5. Corrections to the notes this follows

- ~~[device-body-sweep](device-body-sweep.md) §3.1: `exp` over 256 arguments in `[−16, 15.875]`
  disagrees **"by up to 130 ulp"**~~ **The 130 is the distance from the platform, not from the
  definition; the cpu-versus-gpu worst on that grid is 171 ulp.** The bit-exact count (244/256) and
  the twelve moving arguments reproduce exactly, and that note's own footnote already distinguishes the
  two quantities — the sweep table's `worst` column header does not, and this note's §2 measures both.
- ~~[device-body-sweep](device-body-sweep.md) §3.1: *"For `exp` and `sin` the device is on both sides
  of the `cpu` column … which a shared sequence evaluated with different **rounding** can produce"~~
  **The first half stands and the reading is now specific; the second half does not generalise to
  `exp`.** The device is on both sides for `sin` (4 nearer, 161 further) and for `cos`, but for `exp`
  it is **no further from the platform than the definition on any of the 256 arguments** — 130 ulp
  worst for both, tied on 244. "A shared sequence evaluated with different rounding" is exactly right,
  and §1.1 measures which rounding.
- ~~[floating-point](floating-point.md) §8: *"it is not fused-multiply-add contraction, which was
  modelled three ways (`v·z + c` contracted, the reduction's `a − b·c` contracted, and the final
  `r·(v·z + 1)` contracted through the last Horner add) and reproduces none of the device's values"*~~
  **Contraction is the mechanism, and the three sites tried were the wrong three.** This slice models
  it ten ways; the Horner fused alone gives 101/512, the reduction's two subtractions fused in place
  gives 103/512, and the two together give **512/512**. The site the previous slice did not try is the
  one that carries the error: not the reduction's subtraction contracted, but the reduction's
  **structure changed** — the two-term Cody–Waite split recombined into a single term. The
  lesson generalises past trigonometry: *modelling a rewrite at the sites you expect is not the same
  as modelling what the optimiser did to the expression.*
- ~~[floating-point](floating-point.md) §8, *"What is still not proven"*: *"The one division the
  language leaves unspecified (§4.4) is still a live hazard"*~~ **It is a live hazard and it is not
  this one.** `sin`, `cos` and `exp` emit **zero** `OpFDiv` (§1.2's census, against `sqrt`'s six and
  `ln`'s one). For these three the division question is closed, and the note's standing advice to
  `sqrt` — reduce by multiplying — is vindicated rather than contradicted.
- **[device-body-sweep](device-body-sweep.md) §6's third bullet** — the probe that "does not exist
  yet", the `a·b + c` kernel over a significand grid — **exists, and it is §1.1.** The one thing that
  note said it would close is closed.
- **[device-body-sweep](device-body-sweep.md) §3.2's inference** — that the gap is "a near-constant
  absolute error in the reduced argument, and the ulp grid shrinks with the answer" — **is confirmed
  and given its constant**: the absolute error is `q · 4.371139e-8`, which over the angles where it is
  worst is `1.5`–`2.0` ulp of one, and it is `q` rather than a constant that makes the tail grow.

## 6. What a fix would touch

**No production change is made or proposed here; the user decides.** Four things a fix could reach,
one line each, and what this note says about each.

- **A contraction-forbidding attribute in the emitted module, or the driver's own switch turned off.**
  This is the only candidate that would restore the definition on both targets at once, and the note's
  §1.2 census says where to put it: the module has **no `Fma` and no `OpExtInst` to remove**, so the
  attribute is not a deletion but a *constraint on a rewrite the module does not itself perform*. Its
  whole effect is one flag, and the fact that §1.1's model reproduces the device exactly means the
  probe to verify it is already written.
- **The emitted polynomial and its constants.** Nothing here needs a new coefficient or a new degree:
  the coefficients are the same on both sides and the difference is a rounding, not a truncation
  (§1.1's `exp` row, where the fused Horner reproduces the device at 512/512 with the definition's own
  coefficients). This line is **not** where the fix is.
- **The range reduction.** This is where the arithmetic would have to change, but not because the
  reduction is wrong — §1.1's `recombine_fma` model shows the *device's* answer is the one-term
  reduction, so a fix that made the device reproduce the two-term reduction would have to make the
  optimiser not fold `q·HEAD + q·TAIL`, which is the same flag as the first line and not an arithmetic
  change. What **is** worth deciding is whether a definition whose accuracy depends on an 11-bit split
  surviving an optimiser's algebra is the definition the language wants; §3 shows what it costs when
  the split does not survive — `q · 4.37e-8`, and a sine that changes sign near `8133`.
- **A documented tolerance with a stated bound.** The arithmetic unchanged, a sentence beside
  [floating-point](floating-point.md) §6 and §8, and §2's table is the measurement it rests on:
  **`sin`/`cos` are within 2 ulp of one, absolute, over `[−8, 8)`; `exp` agrees with its own
  definition to within the definition's own `1.2e-5` relative error; and over `|θ| ≤ 8192` the
  trigonometric gap is `|q| · 4.4e-8` absolute, unbounded in relative terms near the zeros.**
  Whether that is enough for the operators that use them is a decision about the language, not about
  the device — and it runs into a standing rule rather than into open ground.
  [device-leg-of-the-ladder](device-leg-of-the-ladder.md) §2 records that every fragment which reached
  the device was bit-exact and that **"no tolerance was loosened to get there"**, so the first
  trigonometric tolerance the language ever states is also the first time that sentence stops being
  true. §3's `2 ulp of one` over `[−8, 8)` is the bound such a sentence would carry, and it is the
  *user's* to decide whether the rule bends.

**And one thing this note does not propose:** [code-audit](code-audit.md) carries no `wontfix:D<n>`
over the transcendentals, the emitter or the polynomial. The nearest decided item is **`wontfix:D9`**
via **`P4-10`**, and `P4-10` is the nesting bound on the type printer — it is about recursion depth
in `render`, and it is **not** a decision about any of this. The question
[floating-point](floating-point.md) §6 poses, whether a device may run a body that does not reproduce
the language's definition, is **still open**, and this note answers the mechanism half of it and not
the policy half.

## 7. What is measured, what is inferred, what is unknown

**Measured, in full.** §1.1's two 4096-point contraction verdicts and the ten-model comparison over
512 arguments for each of `sin`, `cos` and `exp`; §1.3's four 2048-argument re-runs at
`[4000, 8192]` and `(−8192, −4000]`; §2's five sweeps and the platform-distance table;
§3's 8193-angle uniform sweep, its 5200 zero-crossing arguments and its six fine crossings; §4's
seven `spirv-val` verdicts, seven ACCEPT; and §1.2's opcode census over five disassembled modules.
Every ulp figure is read off the two backends' **own** bit patterns, and every absolute figure off the
same pair, at the same point.

**Inferred, from those numbers and not re-derived here.** That the recombination is a driver
optimiser folding `q·HEAD + q·TAIL` into `q·(HEAD+TAIL)` — the identity is arithmetic and the two
`f32` constants are the same value, so the model and the fold agree by construction, but nothing in
this slice observes the optimiser itself. That `exp`'s 100–171 ulp tail is the seven squarings
amplifying a sub-ulp difference in the Horner: the factor `2⁷ = 128` is arithmetic and the tail's size
is measured, but the attribution of the tail to the squarings rather than to the degree is an inference
from their size, not a sweep of `HALVINGS`.

**Unknown, and this note did not close any of these.**

- **Whether the rewrite is NVIDIA's driver or the Vulkan layer's.** The machine is one RTX 3060
  Laptop GPU; no second vendor was available, so "this device" is a fact about this device. §1.1's
  probe is the thing to re-run on another one, and it is thirty lines.
- **Whether the two-term reduction survives on a compiler that does not contract.** The `both` model is
  a fit to one device's output; it would be a mechanism if a second device either reproduced it or
  reproduced `plain`, and that is a measurement this slice had no way to make.
- **What the platform's own `sin`/`cos` would say at the large arguments** where §3 measures the
  device `2.27e-4` away. `f32::sin` is a library call in the host runtime and is not readable here;
  §3's table reports the definition's distance from the platform over `[−8, 8)` and stops there,
  which is a gap rather than a finding.
- **Whether the fusion is stable across recompiles.** [device-body-sweep](device-body-sweep.md) §3.1
  records determinism for `sin` at six angles; this slice did not re-check it, and a rewrite applied by
  a cost model is not obviously stable where one applied by an algebraic simplifier is.

## 8. What this slice deleted

**One probe, and it is gone.**
`crates/lichen-compute-gpu/examples/tgm_probe.rs` — the scratch driver that ran every measurement in
this note, contraction probe, absolute sweeps, model comparison, tail sweep, module dump and all — was
**deleted before this note's commit**. It held no production code and nothing else referenced it.
**No example file was edited**, no `output =` line moved, `git diff dev...HEAD -- examples/` is empty,
and **no test was added and no production file was touched.** The seven modules it emitted are gone
with it; §4's word counts and §1.2's census are what remain of them, and both are re-derivable from
`unary.rs` and `spirv::compile` in one command each.
