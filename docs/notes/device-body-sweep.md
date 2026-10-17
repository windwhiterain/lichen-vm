# The operators' bodies, swept rather than sampled

> Status: **current — every figure below is from an RTX 3060 Laptop GPU on this branch, off
> `dev@fe23041b`, read with the same three steps [device-operator-sweep](device-operator-sweep.md)
> used: compile the body, run it on `cpu`, run it on `gpu`, and read each lane's `to_bits()` from
> both. **What is new is the axis.** That note ran **one example per operator**; this one runs
> **one body per family over a sweep of inputs**, and reports a distribution instead of a worst
> case. The rules the runs meet are [device-leg-of-the-ladder](device-leg-of-the-ladder.md)'s and
> are **referenced, not restated**; the arithmetic is [floating-point](floating-point.md)'s.
> **Nothing here changed production code, and no test was added.**
>
> **Every figure below is the undecorated path, and
> [operator-reverification](operator-reverification.md) re-ran this table's rows through the
> shipped emitter.** Four of them moved and four did not: `silu` **181 → 190 of 256**, `exp`
> **255 → 256** and **244 → 256**, `rmsnorm`'s divisor **141 → 165**, `softmax`'s gathered divisor
> **170 → 178**, while `matmul`, `gemv`, `layernorm`'s exact divisors and this note's own
> seventeen-magnitude reductions held. **The rows that held are the division ones**, and the note
> that re-ran them is where the rule below is either confirmed or narrowed. Nothing on this page
> is retracted: it is the record of the path the decoration was measured against.

[device-operator-sweep](device-operator-sweep.md) left its own hole in its own words, and this note
is the sweep it asked for: *"Eight examples of 4–8 hand-chosen elements cannot rank the
operators"*, and *"`rmsnorm`/`layernorm` are exact **because their divisors were chosen exact** …
**those rows do not exonerate it**. A sweep through one body would rank them."*

**Four claims that measurement forces on the notes this follows, in their own words, and one on
§6 of the arithmetic note.**

- ~~"[device-operator-sweep](device-operator-sweep.md) §1: `silu` … **worst ulp 1** (of 4
  elements, 1 differs)."~~ **True of the example, false as the operator's figure.** Over 256 gates
  the same body answers **`silu`'s worst is 158 ulp**, and 75 of 256 lanes differ. The one-ulp
  figure was the example's own gate; the distribution has a long tail that four lanes cannot see
  ([§3.3](#33-which-instruction-moved-silu-the-per-instruction-answer)).
- ~~"[device-operator-sweep](device-operator-sweep.md) §2: **the operator reproduces the standalone
  measurement rather than exceeding it**, and the composition *absorbs* part of it: the rotation's
  own worst is **3** ulp, below the 5 of the sine it multiplies in."~~ **The direction is right and
  the bound does not exist.** `rope`'s four angles are `0, 1, 2, 3`; over a sweep of angles the
  device's `sin` reaches **94 ulp** and its `cos` **117 ulp** standalone
  ([§3.2](#32-sin-and-cos-alone)). The rotation *did* absorb them at those four angles — it is
  the **inputs**, not the composition, that kept `rope`'s figure at 3.
- ~~"[device-operator-sweep](device-operator-sweep.md) §5: *"this example chose its divisors"* …
  **the two normalisers are exact because their divisors are exact**."~~ **The direction survives,
  the strength does not.** `rmsnorm`'s divisor swept over 256 values is bit-exact at **141 of
  256 (55%)** ([§2.2](#22-rmsnorms-divisor-swept)). An exact divisor is neither necessary nor
  sufficient for the whole sweep to be exact; the example's `sqrt 16 = 4` is one point of the 55%,
  not the reason.
- ~~"[floating-point](floating-point.md) §6: `KernelBody::expand_transcendentals` inlines that
  sequence into the body `lichen-compute` registers, so a backend never meets an `Exp`: **both
  targets run the same primitive IR**, which is what makes their answers **the same bits** rather
  than merely the same function."~~ **The shared definition is real and measured; the same bits
  are not.** Over 256 arguments the `cpu` column **is** `exp_value` bit for bit, and the device
  differs from it on **12 of 256, by up to 130 ulp**
  ([§3.1](#31-exp-alone)). The same reading holds for `sin`, `cos` and `sqrt`, where the device is
  *further* from the platform's own answer than the definition is on 161 of 256 `sin` arguments.

## 1. The method, so a reader can repeat it

**Every sweep below is a formula over the point index, not a random draw.** Two grids carry most
of it, both eight mantissas by eight exponents, so every value is exactly representable in `f32`
and a reader can regenerate any point by hand:

| grid | formula | points | range |
|---|---|---|---|
| **M**, numerators | `(1 + (m mod 8) / 8) · 2^⌊m/8⌋` | 64 | `[1, 7.5]` |
| **D**, divisors | `(1 + (m mod 8) / 8) · 2^(⌊m/8⌋ − 4)` | 64 | `[2⁻⁴, 1.9375]` |
| exp, wide | `−16 + m / 4` | 128 | `[−16, 15.75]` |
| exp, fine | `−2 + m / 64` | 256 | `[−2, 1.984]` |
| exp, reference | `−16 + m / 8` | 256 | `[−16, 15.875]` |
| trig, coarse | `−8 + m / 16` | 256 | `[−8, 7.9375]` |
| trig, fine | `−1 + m / 512` | 512 | `[−1, 0)` |
| `silu` gates | `−12 + m / 16` | 256 | `[−12, 3.9375]` |
| `rmsnorm` sums | `3 + m / 16` | 256 | `[3, 18.9375]` |
| softmax, (exponent, total) | `−16 + m / 16` paired with `D[m mod 64]` | 256 | as above |
| magnitudes, the reductions | `grid · 2^e`, `e` from `−8` to `8` | 17 | `2⁻⁸ … 2⁸` |

**Three points about what a "point" is, because they are what make the sweep an operator's body
rather than a list of kernels.**

- **The body is the operator's own.** `silu_raw`, `rescale_raw`, `softmax_raw`, `sumsq_raw`,
  `matmul_raw`, `gemv_raw`, `attend_raw` and `scores_raw` are spelled as
  [examples](device-operator-sweep.md)'s files spell them. The reductions and `softmax`'s
  two-kernel body are the **example files themselves**, read from `examples/`, with only the data
  literal substituted in memory and the backend literal changed; the other bodies are the same
  text with one difference, named at each site below.
- **A sweep makes a per-lane varying divisor where the operator has a shared one.** `rescale_raw`
  reads `s` at index `0`; the sweep reads it at `i`, so each lane gets a different divisor and one
  launch covers the whole sweep. The arithmetic per lane is unchanged, and the extent is scaled so
  that `int2float k.n` still divides by a power of two exactly.
- **`softmax`'s divisor sweep replaces the fold with a buffer.** The last line
  `here / total` is the operator's; `total` comes from a swept buffer rather than from the fold,
  and the **real** two-kernel body is run separately over twelve rows to anchor it.

**A note on two printed ranges.** The probe labelled the fine trig grid `[−1, 1)` and the softmax
exponent axis `[−16, 16)`; the formulas above are the correct ranges (`[−1, 0)` and `[−16, 0)`).
The values swept are the ones the formulas say — only the two labels were wrong, and the figures
below are unaffected.

**Each family ran in its own process**, because `install_default()`'s backend slot and the kernel
registry are both process-global and the device takes one process at a time.

## 2. Family: the division paths

### 2.1 The divider alone

**Measured: a kernel's float division is bit-exact between the two backends exactly when the
divisor is a power of two, and moves by one ulp otherwise.** One body, `z[i] = a[i] / b[i]`, the
numerator grid **M** of 64 points against a fixed divisor:

| divisor | bit-exact | differ | worst ulp |
|---|---|---|---|
| `1.0` | **64 / 64** | 0 | 0 |
| `2.0` | **64 / 64** | 0 | 0 |
| `4.0` | **64 / 64** | 0 | 0 |
| `8.0` | **64 / 64** | 0 | 0 |
| `3.0` | 48 / 64 (75%) | 16 | 1 |
| `5.0` | 48 / 64 (75%) | 16 | 1 |
| `7.0` | 24 / 64 (37.5%) | 40 | 1 |

**Measured: which numerators move is not the magnitude, it is the significand.** For `/3` the
sixteen movers are the numerators whose significand ends in `…01` (`1.25, 1.75, 2.5, 3.5, 5, 7,
10, 14, 20, 28, 40, 56, 80, 112, 160, 224`), and the `cpu` answers all carry a `…5555` or
`…5556` significand — the cases where the correctly rounded quotient sits closest to a rounding
boundary and a one-ulp difference in the quotient's last bit is all that separates the two
roundings. The `/5` movers are the `…001` significands and the `/7` movers are the wider set a
divisor with no short period produces.

**Measured: the rule holds on independent pairs too.** 64 (numerator, divisor) pairs taken from
**M** and **D** with the divisor index stepped by 37 — coprime with 64, so the two grids are
paired independently rather than elementwise — move the same way, and **no pair exceeds 1 ulp**.

### 2.2 `rmsnorm`'s divisor, swept

**Measured.** `rescale_raw`'s body, `d = sqrt(s / 4 + 0.25)` then `(x / d) · w`, with the sum of
squares swept over 256 values in `[3, 19)` and the numerator grid **M**:

| body | bit-exact | differ | worst ulp |
|---|---|---|---|
| the Newton `sqrt` alone | 184 / 256 (71.9%) | 72 | **1** |
| the `Div` alone | 141 / 256 (55.1%) | 115 | **2** |
| the whole `rescale_raw` | **141 / 256 (55.1%)** | **115** | **2** |

**Measured: the whole body is the `Div`, and the `sqrt`'s movement is absorbed.** The full body's
distribution is the `Div`'s distribution, figure for figure. The Newton `sqrt` moves on 72 of 256
arguments, but every one of those is at most 1 ulp, and the division that follows rounds the same
way whichever side of the `sqrt`'s last bit it starts from.

**Measured: `sqrt`'s rule is not "the argument was chosen exact".** Grouping the same 256 by what
the answer looks like:

| the `cpu` divisor … | points | of which moved |
|---|---|---|
| squares back to the argument **exactly** (`d·d == a` bit for bit) | 124 | **8** |
| does not | 132 | 64 |
| is a power of two | 2 | **0** |

**Measured: this closes the hole [device-operator-sweep](device-operator-sweep.md) §5 named.** The
prior sweep's caveat — that §7's `sqrt` column moves on 21 of 64 and the example's row does not
exonerate it — is now measured through the operator: **the Newton `sqrt` does move on this
device, on 72 of 256 of the operator's own arguments, at 1 ulp**, exactly as
[floating-point](floating-point.md) §7's column said it would. The example's `sqrt 16 = 4` is
simply one of the 184 arguments where it does not.

### 2.3 `softmax`'s divisor, swept

**Measured: the last line alone**, `exp (x) / total`, over 256 (exponent, divisor) pairs from the
softmax grid: **170 / 256 bit-exact (66.4%), 86 differ, worst 118 ulp**. The distribution splits
in two — 77 of the movers are the division's 1 ulp, and 9 are the `exp`'s tail, the same tail
[§3.1](#31-exp-alone) measures.

**Measured: the real two-kernel body over twelve rows**, `rowmax_raw` then `softmax_raw`, four
lanes each, 48 points: **37 bit-exact, 11 differ, worst 1 ulp**. Which rows move:

| row | lanes moved |
|---|---|
| `[1, 2, 3, 4]` — the example's own | **0 of 4** |
| `[0, −1, −2, −3]` — the shift-invariant equivalent | 0 of 4 |
| `[0, −0.25, −0.5, −0.75]`, `[0, −4, −8, −12]`, `[0, −8, −16, −24]` | 0 of 4 each |
| `[7.0463777, 0.73105794, −0.13447031, 0]` | 0 of 4 |
| `[0, −2, −4, −6]`, `[0, 1, 2, 4]`, `[−3, −2, 1, 6]` | 1 of 4 each |
| `[0, −0.5, −1.5, −4.5]` | 3 of 4 |
| `[3, −1, −2, −7.0463777]` | 4 of 4 |

**Measured: the example's row is exact because of its divisor, not because of its values.**
`[1,2,3,4]` and `[0,−1,−2,−3]` share one total, `1 + e⁻¹ + e⁻² + e⁻³`, and both are exact;
`[0,−2,−4,−6]` has a different total and moves a lane. That is [§2.1](#21-the-divider-alone)'s
rule seen through an operator: a non-power-of-two divisor, and whether a given lane moves depends
on the numerator beside it.

### The rule the division family suggests

> **A kernel's float division agrees bit for bit between `cpu` and `gpu` whenever the divisor is a
> power of two — 256 of 256 points on this device — and agrees to within 1 ulp otherwise, with the
> lanes that move decided by the numerator's significand rather than its magnitude. An operator's
> accuracy is therefore set by **whether its divisor happens to be a power of two**, and where it
> is not, by the significands of the numerators beside it. `rmsnorm`'s accuracy is its `Div`'s
> accuracy; its Newton `sqrt` is not the binding constraint.**

## 3. Family: the transcendentals

### 3.1 `exp` alone

**Measured.**

| sweep | bit-exact | differ | worst ulp |
|---|---|---|---|
| 128 arguments, `[−16, 15.75]` | 121 / 128 (94.5%) | 7 | **150** |
| 256 arguments, `[−2, 1.984]` | 255 / 256 (99.6%) | 1 | **107** |
| 256 arguments, `[−16, 15.875]` | 244 / 256 (95.3%) | 12 | **130** |

**Measured: the gap is not a last-bit curiosity and it is not everywhere.** It is absent at 95% of
arguments and, where it is present, it is **two orders of magnitude larger than an ulp class**.

**Measured, and this is what discriminates "different rounding" from "different algorithm": every
`cpu` column below is the language's own definition, bit for bit, and both backends are compared
against the platform's own `f32` answer.**

| operator | `cpu` == platform | `cpu` worst | `gpu` == platform | `gpu` worst | `gpu` strictly closer | `cpu` strictly closer | tied |
|---|---|---|---|---|---|---|---|
| `exp` | 2 / 256 | 130 ulp | 2 / 256 | 130 ulp | **8** | **4** | 244 |
| `sin` | **218 / 256** | **1 ulp** | 87 / 256 | **94 ulp** | 4 | **161** | 91 |
| `cos` | **205 / 256** | **1 ulp** | 67 / 256 | **118 ulp** | 2 | **183** | 71 |
| `sqrt` | 191 / 256 | 1 ulp | **215 / 256** | 1 ulp | **51** | 27 | 178 |

(The two `worst` columns are distances **from the platform's own answer**, which is a different
quantity from the cpu-versus-gpu distances in [§3.2](#32-sin-and-cos-alone) below; `sin`'s is 94
either way and `cos`'s is 117 cpu-versus-gpu against 118 from the platform.)

**Measured: the two backends are not running the same arithmetic.** For `exp` and `sin` the device
is on both sides of the `cpu` column — sometimes nearer the platform's answer, sometimes further —
which a shared sequence evaluated with different *rounding* can produce and a shared sequence
evaluated identically cannot. For `sqrt` the device is **nearer the correctly rounded root than the
language's Newton definition on 51 of 256 arguments and further on 27**.

**Measured: the argument decides, and the way it decides is not monotone.** The twelve `exp`
arguments that move are `−12.875, −12.25, −11.25, −9.25, −8.5, −5.5, 9.375, 9.875, 12, 14.875,
15.125, 15.25` — scattered across the range, not clustered at an end.

### 3.2 `sin` and `cos` alone

**Measured.**

| sweep | bit-exact | differ | worst ulp |
|---|---|---|---|
| `sin`, 256 angles, `[−8, 7.9375]` | 91 / 256 (**35.5%**) | 165 | **94** |
| `cos`, 256 angles, `[−8, 7.9375]` | 71 / 256 (**27.7%**) | 185 | **117** |
| `sin`, 512 angles, `[−1, 0)` | 437 / 512 (85.4%) | 75 | **2** |
| `cos`, 512 angles, `[−1, 0)` | 394 / 512 (77.0%) | 118 | **1** |

**Measured: the range is the rule, and within a range the *size of the answer* is.** Over
`[−8, 8)` fewer than a third of the angles agree and the tail reaches 94 ulp; over `[−1, 0)` more
than three quarters agree and the tail is 2 ulp. Inside the coarse sweep the worst movers are all
near a **zero crossing**: `sin(−6.3125)` is `−0.029310495` and moves 94 ulp, `cos(−7.875)` is
`−0.021016818` and moves 117 ulp — while `sin(−8) = −0.989` moves 1 ulp.

**Inferred, from those two figures and not re-derived here: the gap is a near-constant *absolute*
error in the reduced argument, and the ulp grid shrinks with the answer.** `sin(−6.3125)`'s 94
ulp is `1.7e-7` absolute, against one ulp of its own argument (`6.3125`) of `4.8e-7` — the error
is *under* one ulp of the argument. So the reader's rule is: **the ulp figure is largest where the
answer is smallest, and an ulp tolerance stated for a transcendental has to be stated for its
zeros, not for its body.**

### 3.3 Which instruction moved `silu`: the per-instruction answer

**Measured.** The prior sweep's §3 said this could not be separated — *"`silu_raw` holds one `exp`
**and** one float `Div` … **This note cannot separate them** … naming either is an argument, not a
measurement"*. It is a small step, and it is done. Four spellings of the **same kernel** over the
same 256 gates, differing in one operator each:

| spelling | bit-exact | differ | worst ulp |
|---|---|---|---|
| `g / (1 + exp (0 − g))` — the body | 181 / 256 (70.7%) | 75 | **158** |
| `exp (0 − g)` alone | **253 / 256 (98.8%)** | **3** | 150 |
| `g / (1 + g)` alone — the `Div`, the `exp` removed | 186 / 256 (72.7%) | 70 | **1** |
| `g / 4.0` alone — the `Div` by a power of two | **256 / 256** | **0** | **0** |

**Measured: the answer is both, and the two are separable by size.**

- **The `Div` moved the one-ulp class — 72 of the body's 75 movers.** It is the only spelling that
  moves at a 1-ulp rate, and `g / 4.0` is the same `Div` with a power-of-two divisor and moves
  nowhere, which is [§2.1](#21-the-divider-alone)'s rule reproduced inside the operator.
- **The `exp` moved the tail — 3 lanes, at 96, 146 and 158 ulp in the body against 109, 140 and
  150 ulp for the `exp` alone.** Those are the same three gates (`−12.0`, `−9.875`, `−9.375`) in
  both spellings, so the attribution is per point and not per count.
- **The 70 movers of the isolated `Div` are not the 72 movers of the body**, because
  `1 + exp(−g)` is a different divisor than `1 + g` and the movers follow the divisor's significand.
  Same instruction, same rule, a different subset of lanes.

**Measured, and it is the correction this note's first bullet rests on: the `Div` is not the worst
thing in `silu`.** Three gates out of 256 answer 96–158 ulp apart, and they are the `exp`.

### The rule the transcendental family suggests

> **A transcendental is not a last-bit hazard and it is not uniform.** `exp` agrees at 95% of
> arguments and disagrees by up to 130 ulp at the rest; `sin` and `cos` agree at 28–36% of angles
> over `[−8, 8)` and at 77–85% over `[−1, 0)`. The `cpu` column is the language's definition bit
> for bit in every case, so **the two backends are not evaluating the same arithmetic on this
> device**, and for `sin` the device is *further* from the platform's own answer than the
> definition is on 161 of 256 arguments. Inside a range, **the ulp distance tracks the size of
> the answer, not the size of the argument** — the tail is at the zeros.

## 4. Family: the reductions

**Measured, each over seventeen magnitudes `2⁻⁸ … 2⁸` and the mantissa grid, on the example's own
bodies read from `examples/` with only the data literal and the backend literal changed:**

| operator | points | bit-exact | worst ulp |
|---|---|---|---|
| `matmul` — a multiply-and-add fold, 2×3 · 3×3 | 204 | **204 (100%)** | **0** |
| `gemv` — the same fold, padded to the launch | 68 | **68 (100%)** | **0** |
| attention — `scores_raw` then `attend_raw` | 136 | 103 (75.7%) | **2** |

**Measured: a fold of multiplies is bit-exact over sixteen decades.** `matmul` and `gemv` hold
every one of their 272 outputs, at every scale, and `gemv` runs at all — **0 refusals** across the
seventeen magnitudes, which is the padded spelling [device-operator-sweep](device-operator-sweep.md)
§8 measured once, now swept. **A `kdim < M` GEMV needs the padding and nothing else**; the
arithmetic behind it is not a second source of divergence.

**Measured: attention's divergence is not a function of magnitude.** The example's own inputs move
6 of 8 lanes at 1 ulp; the swept grid moves 33 of 136 lanes with a 2-ulp worst, and the worst sits
at `2⁻³`, in the middle of the range rather than at an end. Attention is the one operator in this
note whose gap **grows by a ulp under composition** (1 → 2), which is
[device-operator-sweep](device-operator-sweep.md) §7's reading measured on both sides of it.

## 5. What the validator says

**Measured: every module these sweeps emitted is `spirv-val` ACCEPT — 12 of 12.** The eleven
sweep programs of [§1](#1-the-method-so-a-reader-can-repeat-it) register twelve kernels between
them, and each is compiled at its own binding and piped to
`C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe --target-env vulkan1.1 -`:

```
ACCEPT  1 in, 1 out,  416 words     ACCEPT  1 in, 1 out,  562 words
ACCEPT  1 in, 1 out,  421 words     ACCEPT  1 in, 1 out,  578 words
ACCEPT  1 in, 1 out,  336 words     ACCEPT  2 in, 1 out,  442 words
ACCEPT  1 in, 1 out, 1576 words     ACCEPT  2 in, 1 out,  448 words
ACCEPT  1 in, 1 out, 1622 words     ACCEPT  2 in, 1 out,  245 words
ACCEPT  2 in, 1 out,  726 words     ACCEPT  3 in, 1 out, 1658 words
```

**Measured: the 1658-word module is `rescale_raw`**, the same count
[device-operator-sweep](device-operator-sweep.md) §5 reported for it on its own — an independent
route to the same module. **So nothing here is a malformed module.** Every divergence in this note
is a numerical one in a module the specification accepts, which is
[device-operator-sweep](device-operator-sweep.md)'s standing conclusion and this note extends from
sixteen modules to twenty-eight.

## 6. The kernel work each finding implies

**No production change is made or proposed here.** What a fix would touch, one line each:

- **A stated tolerance for a kernel's float division, keyed on the divisor's shape.** The
  arithmetic is unchanged — it would be a sentence beside [floating-point](floating-point.md) §4.4
  beside the one that already declares a kernel's float division unspecified, extended to say
  *power of two is exact and everything else is within one ulp, with the movers decided by the
  numerator's significand*.
- **A precision decision somebody has to take about the transcendentals, and it is larger than
  the one §2 asked for.** 130 ulp on `exp` and 117 on `cos` are not a tolerance question, they
  are a question of whether a device may run a body that does not reproduce the language's
  definition. It touches `crates/lichen-kernel-ir`'s `TRANSCENDENTALS` decision — either the
  expansion is made to evaluate identically on both targets, or the agreement rule beside it is
  restated in ulps.
- **A probe that does not exist yet, and is the one thing here that would close the last gap:**
  whether the transcendental gap is the device contracting multiply-adds or the range reduction's
  division rounding differently. A kernel of the form `a·b + c` over a significand grid, compared
  against `cpu`, separates the two in one run. [floating-point](floating-point.md) §8 records the
  question as open for `sin`/`cos` on spliced bodies; this note measures the size and does not
  answer it.
- **No emitter fix and no missing spelling.** Twelve of twelve modules validate, `matmul` and
  `gemv` are bit-exact over sixteen decades, and the one refusal is the padding
  [device-leg-of-the-ladder](device-leg-of-the-ladder.md) §1 already names and this note confirms
  seventeen times over.
- **No change to the ladder's precision decision from `matmul` and `gemv`.** 272 bit-exact outputs
  across `2⁻⁸ … 2⁸` mean the gemm family needs no tolerance of its own, which is
  [device-operator-sweep](device-operator-sweep.md) §9's reading with a distribution behind it.

## 7. What was measured, what is inferred, and what is unknown

**Measured, in full.** [§2.1](#21-the-divider-alone)'s seven divisor columns and the 64
independent pairs; [§2.2](#22-rmsnorms-divisor-swept)'s three bodies and the `sqrt`'s grouping;
[§2.3](#23-softmaxs-divisor-swept)'s 256 pairs and twelve rows; [§3.1](#31-exp-alone)'s three
sweeps and the three-operator reference table; [§3.2](#32-sin-and-cos-alone)'s four sweeps;
[§3.3](#33-which-instruction-moved-silu-the-per-instruction-answer)'s four spellings and the
per-point attribution; [§4](#4-family-the-reductions)'s 408 reduction points; and 12 `spirv-val`
verdicts, 12 ACCEPT. Every ulp figure is read off the two backends' **own** bit patterns.

**Measured, and it is a fact about this tree rather than about these programs.** The `cpu` column
is `lichen_kernel_ir`'s own definition **bit for bit** in every operator above. That is the
reading [floating-point](floating-point.md) §6 predicted and did not measure on a device, and it
is what makes the device column a measurement of the emitters rather than of a different `cpu`.

**Inferred, and not re-derived here.** That the transcendental gap is a near-constant absolute
error in a reduced argument, from the two figures in [§3.2](#32-sin-and-cos-alone) — a constant
`1.7e-7` absolute against a `4.8e-7` ulp of the argument at the worst point. That `rmsnorm`'s
`sqrt` movement is absorbed by the following division, from the full body's distribution matching
the `Div`'s figure for figure. That `softmax`'s example row is exact because of its divisor,
from two rows sharing one total being exact and a third with another total not.

**Unknown, and this note did not close any of these.**

- **What the transcendental body is doing differently on the device.** Contraction of
  multiply-adds, a different rounding in the range reduction's division, or a different sequence
  altogether — the reference table shows the device is on *both* sides of the definition, and it
  does not say which. [floating-point](floating-point.md) §8 already records this as open; the
  probe that would close it is named in [§6](#6-the-kernel-work-each-finding-implies).
- **Whether the tail is bounded over the domain.** 150 ulp is the worst of a 128-point sweep over
  `[−16, 16)`. The definition's own domain is wider (`|θ| ≤ 8192` for the trigonometic
  reductions), and nothing here says what the tail does at `θ = 4000`.
- **Whether attention's 2-ulp worst is the bound.** One hundred and thirty-six points is a
  distribution, not a bound, and attention's fold-plus-division is the only body here whose worst
  *grew* under composition.
- **Whether these grids are representative of a real workload.** Every point here is exact in
  `f32` by construction, which is what makes a sweep reproducible and what makes it unlike the
  distributions a trained model's activations actually take.

## 8. What this slice deleted

**One probe, and it is gone.** `crates/lichen-compute/examples/dsweep2_probe.rs` — the scratch
driver that ran every sweep in this note — was **deleted before this note's commit**. It held no
production code and nothing else referenced it. **No example file was edited**: the three
reduction bodies were read from `examples/` and substituted in memory, `git diff dev...HEAD --
examples/` is empty, and no `output =` line moved. **No test was added and no production file was
touched.**

## The `layernorm` gap, closed: the one-pass identity's accuracy is its condition number

The sweep above says `layernorm` got no independent sweep, because it "shares `rmsnorm`'s arithmetic
family". Measured, that is right for everything the two families share and wrong for the one thing they do
not: `rmsnorm` has no mean, and `layernorm`'s variance is the **difference of two computed values**,
`v = Σx²/n − m·m`.

On a device, that costs **0.25 ulp of the operands' magnitude**: over 4096 rows built as a large mean plus a
small spread, the worst absolute error was **7.98** on `m² ≈ 2.56e8`, whose ulp is **32**. That is better
than one ulp — and it is not the interesting number:

| row | `m² / v` | what `v` retains |
|---|---|---|
| the example's `1, 2, 3, 4` (`m = 2.5`, `v = 1.25`) | **5** | all of it, which is why that example is exact |
| the probe's rows (`m` up to `1.6e4`, `v` near `1`) | **~5e8** | about **28 bits**, a relative error near **1e-1** |

**So `layernorm`'s accuracy is its identity's condition number, not the device's arithmetic.** `rmsnorm`
cannot show this: its divisor is a sum of squares, with no subtraction of two large values. The example's
`output` line is exact because its row is in the benign regime — the example says so itself, "The value is
still chosen so the denominator lands on an exact square" — but its surrounding sentences are about `sqrt`,
not about this.

**What this does not measure:** the `sqrt`'s own distribution on a `v` that is not an exact square. That
was measured through `rmsnorm` (184 → 220 of 256, worst 1 ulp) and the sequence is shared, so it is
referenced rather than repeated.
