# The operator bodies, re-verified under the shipped emitter

> Status: **current** — every after-figure below was measured on **two** devices off
> `dev@0467c698`, by running the **same binary twice** with `VK_DRIVER_FILES` pointed at each
> vendor's ICD manifest: an **NVIDIA GeForce RTX 3060 Laptop GPU** (discrete) and an **Intel(R)
> Iris(R) Xe Graphics** (integrated), each named by `vulkaninfoSDK --summary` under its own
> manifest. The before-column is the published one, which is an RTX 3060 figure throughout, and
> **each row says which device it came from**. **This note changed one test and no production
> code.**
>
> **What is being re-verified** is [contraction-control](contraction-control.md)'s shipped change,
> not its measurement: `KernelBody::exact_float_arithmetic` and the `NoContraction` decoration
> `crates/lichen-compute-gpu/src/spirv.rs` writes for it, whose own §9.3 swept the **raw
> definitions** — `sin`, `cos`, `exp` — and found them bit-exact on both vendors. **The bodies an
> operator is actually made of were not re-run, and that is the gap this note closes**, against
> the table [device-body-sweep](device-body-sweep.md) measured before the fix and the eight
> examples [device-operator-sweep](device-operator-sweep.md) measured before it. Those two notes
> and [transcendental-gap-mechanism](transcendental-gap-mechanism.md) are **referenced here, not
> restated**; §5 corrects the claims of theirs this measurement contradicts, in their own words.
>
> **The one-sentence result: the transcendental is gone from every operator body, and what is
> left is the division.** Four rows became bit-exact or near it and **three did not move at all**,
> and every row that did not move is a row whose gap was a `Div`.

## 1. The method, so a reader can repeat it

**Measured, and the three steps are the ones [device-body-sweep](device-body-sweep.md) §1 used,
unchanged**: compile the body, run the program on `cpu`, run the identical program with only the
backend literal `"cpu"` → `"gpu"`, and compare the two renderings element by element.

| what is fixed | what is swept |
|---|---|
| **the example file**, read from `examples/` through `lichen_tools::readme::read_normalized`, never edited | **the data literal**, substituted in memory — the same substitution [device-operator-sweep](device-operator-sweep.md)'s own method paragraph describes |
| the body's own spelling, except where a row names the one expression that differs | the launch extent, scaled so `int2float k.n` still divides by a power of two |

**The two grids [device-body-sweep](device-body-sweep.md) §1 defines are used as written**: the
numerators **M** `(1 + (m mod 8)/8)·2^⌊m/8⌋` and the divisors **D** `(1 + (m mod 8)/8)·2^(⌊m/8⌋−4)`,
indexed `m mod 64` because each is a **64**-point grid; the `silu` gates `−12 + m/16`; the `exp`
grids `−2 + m/64` and `−16 + m/8`; the trig grid `−8 + m/16`; the `rmsnorm` sums `3 + m/16`.

**Three spellings of a body where the operator has a shared divisor, one expression each**, which
is the substitution [device-body-sweep](device-body-sweep.md) §3.3 used for `silu`:

- `silu_raw` is `g / (1.0 + exp (0.0 − g))`, re-rooted on the kernel's own `act.a` so the second
  kernel's product is not in the measurement; with `u` all ones and the divisor written two other
  ways, `g / (1.0 + g)` and `g / 4.0`.
- `rescale_raw` is `d = sqrt (s / 4.0 + 0.25)` then `(x / d) · w`, with **`s` read at `i` rather
  than at `0`** so one launch carries the whole sweep, `w` all ones, and the numerator grid **M**.
  Its two halves are the divisor `d` alone and `x / d` alone.
- `softmax_raw`'s last line `here / total` with **`total` read at `i` from a swept buffer** and the
  row maximum replaced by `0.0`, so the division's numerator is `exp(x[i])` — the gathered-divisor
  row — and its twelve-row variant keeps the real two-kernel body.

**One process at a time per device**, and the driver `install_default`'s slot is filled once per
process, so each family ran alone.

**What the comparison can and cannot see, stated once.** Both backends print the collected array
through the language's own printer, and it writes **eight significant digits** in fixed notation.
A token that is equal on the two sides is one `f32`, whatever the decimal cannot carry, so the
**bit-exact counts are exact**. A token that differs is a value that differs, so the **worst ulp
and worst absolute are exact wherever both sides printed in a form that round-trips**; the probe
counts the movers where one side did not and prints that count as `inexact_printing`, and **one
row of §3 has a non-zero one and is reported as a bound rather than a figure.** This is a
property of the printer, not of either backend, and it is the one measurement caveat in this note.

## 2. Per operator, before and after

**Measured, in full.** The before-column is [device-body-sweep](device-body-sweep.md) §§2–4 and
[device-operator-sweep](device-operator-sweep.md) §1, every one of them an **RTX 3060** figure on
the undecorated path. The after-column is this note, measured on the RTX 3060 and on the Iris Xe.

| operator | body | before (RTX 3060) | after, RTX 3060 | after, Iris Xe |
|---|---|---|---|---|
| **`silu`** | 256 gates, `g / (1 + exp(−g))` | 181/256, worst **158** | **190/256, worst 1**, abs `2.38e-7` | **192/256, worst 1**, abs `2.38e-7` |
| | its `Div` alone, `g / (1 + g)` | 186/256, worst 1 | **186/256, worst 1**, abs `4.77e-7` | **186/256, worst 1**, abs `4.77e-7` |
| | its `Div` by `4.0` | 256/256, worst 0 | **256/256, worst 0** | **256/256, worst 0** |
| | the example's own four elements | 3/4, worst **1** | **3/4, worst 1** | **4/4, worst 0** |
| **`rope`** | the example's own eight | 6/8, worst **3** | **6/8, worst 1**, abs `4.77e-7` | **7/8, worst 1** |
| | 512 lanes, 256 angles over `[−8, 8)` | *no baseline* | **512/512, worst 0** | **512/512, worst 0** |
| **`softmax`** | gathered divisor, 256 exprs | 170/256, worst **118** | **178/256, worst 1** — see §3 | **158/256, worst 1** — see §3 |
| | the same grid, exponents shifted by 16 | *no baseline* | **177/256, worst 1**, abs `2.5e-1` | **170/256, worst 1**, abs `2.5e-1` |
| | the real two-kernel body, twelve rows | 37/48, worst 1 | **38/48, worst 1**, abs `5.96e-8` | **38/48, worst 1**, abs `5.96e-8` |
| | the example's own four elements | 4/4, worst 0 | **4/4, worst 0** | **4/4, worst 0** |
| **`rmsnorm`** | `rescale_raw`, 256 lanes | 141/256, worst **2** | **165/256, worst 2**, abs `1.53e-5` | **165/256, worst 2**, abs `1.53e-5` |
| | its Newton `sqrt` alone | 184/256, worst 1 | **220/256, worst 1**, abs `2.38e-7` | **230/256, worst 1**, abs `2.38e-7` |
| | its `Div` alone | 141/256, worst 2 | **165/256, worst 2** | **165/256, worst 2** |
| | the example's own four elements | 4/4, worst 0 | **4/4, worst 0** | **4/4, worst 0** |
| **`layernorm`** | the example's own four elements | 4/4, worst 0 | **4/4, worst 0** | **4/4, worst 0** |
| **`attention`** | the example's own eight | 2/8, worst **1** | **2/8, worst 1**, abs `4.77e-7` | **2/8, worst 1**, abs `4.77e-7` |
| **`matmul`** | the example's own four elements | 4/4, worst 0 | **4/4, worst 0** | **4/4, worst 0** |
| **`gemv`** | the example, **refused by name** | refused; padded, 4/4 | **refused**; padded **4/4, worst 0** | **refused**; padded **4/4, worst 0** |
| **`exp`** (the operator the others share) | 256 arguments, `[−2, 1.984]` | 255/256, worst **107** | **256/256, worst 0** | **256/256, worst 0** |
| | 256 arguments, `[−16, 15.875]` | 244/256, worst **130** | **256/256, worst 0** | **256/256, worst 0** |

**Measured: which rows became bit-exact, and which did not.** Say it plainly, because the second
list is the more valuable half.

- **Became bit-exact outright — two rows.** `exp` alone on **both** its grids, **256/256 with a
  zero worst ulp on both vendors**, where it was 255/256 and 244/256. And `rope`'s swept body,
  **512/512**, where the operator had no baseline at all and its example row had been the worst
  row in [device-operator-sweep](device-operator-sweep.md).
- **Became exact everywhere it used to be small.** `rope`'s example row fell from a 3-ulp worst to
  **1**, and `silu`'s 158-ulp tail is **1**. Those are not bit-exact, and the row that says so is
  the next bullet.
- **Did not move at all — four operator rows and two sub-rows.** `attention` is **2 of 8 exact,
  worst 1, on both vendors**, exactly as it was; `rmsnorm`'s example row, `layernorm`'s,
  `matmul`'s, `softmax`'s example row, `matmul` and `gemv` are all **0 ulp** where they were
  **0 ulp**; `silu`'s `Div` alone is **186/256, worst 1** on both vendors where it was
  **186/256, worst 1**; `rmsnorm`'s `Div` alone is **165/256, worst 2** where it was
  **141/256, worst 2**.
- **So: every row that moved is a row whose gap was a transcendental, and every row that did not
  move is a row whose gap is a `Div` or was already zero.** `attention`'s six movers are all
  **1 ulp** past a division by a sum of `exp` over the keys — a divisor that is not a power of
  two, which is [device-body-sweep](device-body-sweep.md) §2.1's rule — and
  [floating-point](floating-point.md) §2.1's rule is the one this measurement confirms rather
  than disturbs. `silu`'s one remaining mover is the same shape, and §3 of
  [device-operator-sweep](device-operator-sweep.md) named it as such at the time: *"the split is
  `exp (0.0 − g)` **and** the division by `1 + exp (0.0 − g)`"*.

**Measured: the body, decomposed, says the same thing.** [device-body-sweep](device-body-sweep.md)
§3.3 separated `silu`'s movers into the `Div`'s one-ulp lanes and the `exp`'s 96–158-ulp tail.
Re-running all three spellings after the fix: the `Div`'s rows are **unchanged to the point** —
186/256, 256/256, worst 1 and 0 — and the body itself is **66 movers on the RTX 3060 at a 1-ulp
worst**, so the three tail lanes are inside the one-ulp class. **The attribution survives the fix
intact and the `exp`'s share of it is now zero.**

**Not re-run, and it is a gap rather than a finding: the seventeen-magnitude reduction sweeps.**
[device-body-sweep](device-body-sweep.md) §4's `matmul` **204 points**, `gemv` **68** and
`attention` **136** are not reproduced here. Its §4 states the formula — seventeen magnitudes
`2⁻⁸ … 2⁸` and the mantissa grid — but **the per-magnitude point count is not derivable from what
the note publishes**: `68 = 4·17` is four output lanes over seventeen magnitudes, and `204` is
**twelve** over seventeen, which the 2×3·3×3 body does not explain. A sweep cut from a different
grid is a different measurement and would not be comparable, so these three rows are **reported as
not re-run** rather than as agreement. The example rows above stand in for the operators and are
exact.

## 3. The division rows, which the fix does not reach

**The question was whether `rmsnorm`'s 141/256 becomes 256/256, and the answer is that it does
not: it is 165/256, with the same 2-ulp worst.** That is the informative half. The other half is
*why*, and it is the same reason on both vendors and in both halves of the body.

| `rescale_raw`, 256 lanes | before, RTX 3060 | after, RTX 3060 | after, Iris Xe |
|---|---|---|---|
| the Newton `sqrt` alone, `sqrt (s/4 + 0.25)` | **184/256**, worst 1 | **220/256**, worst 1 | **230/256**, worst 1 |
| the `Div` alone, `x / d` | **141/256**, worst 2 | **165/256**, worst 2 | **165/256**, worst 2 |
| the whole body, `(x / d) · w` | **141/256**, worst 2 | **165/256**, worst 2 | **165/256**, worst 2 |

**Measured: the decoration moved the `sqrt` and left the `Div` at its own figure, and the body is
the `Div`.** The `sqrt` gained **36 points on the RTX 3060** and **46 on the Iris Xe**, and its
worst is still **1 ulp** — a Newton iteration built on a division cannot be repaired by forbidding
a fusion, but it is not the binding constraint. The `Div` moved by **24 points and not one ulp**,
and the whole body is **figure for figure the `Div`'s**, on both vendors, which is exactly the
relation [device-body-sweep](device-body-sweep.md) §2.2 measured before the fix and still holds.
**A better `sqrt` changes which lanes move and how many; it does not make the quotient's rounding
correct.** So the rule §2.2 states — *a kernel's float division agrees bit for bit whenever the
divisor is a power of two, and to within 1 ulp otherwise* — **survives the decoration unchanged,
with the bound it is stated at still the right one.**

**Measured: `softmax`'s gathered-divisor row lost its 118-ulp tail and kept the 1-ulp one.** The
before figure splits into 77 movers at 1 ulp and 9 at the `exp`'s tail; after the fix the tail is
gone and what is left is the division's own class.

| `exp (x[i]) / total[i]`, 256 exprs | before, RTX 3060 | after, RTX 3060 | after, Iris Xe |
|---|---|---|---|
| the note's grid, exponents `−16 + m/16` | **170/256**, worst **118** | **178/256**, worst **1** | **158/256**, worst **1** |
| — of whose movers printed inexactly | — | **46 of 78** | **51 of 98** |
| the same grid shifted by 16, exponents `m/16` | *no baseline* | **177/256**, worst **1**, `inexact_printing = 0` | **170/256**, worst **1**, `inexact_printing = 0` |

**That first row is a bound, not a figure, and the reason is the printer and not the device.** Its
answers run down to `1.8e-6`, where the printer's eight significant digits are about the same
width as one `f32` ulp, so **46 of its 78 differing tokens are printed too coarsely to recover
the bits behind them**. What survives that limit is still decisive: **the 78 tokens that differ
are confirmed movers and no two of them are further than 1 ulp apart as printed**, so the **118-ulp
tail is gone** and the worst is **1 ulp to within the printing**. The second row is the same
division over a grid whose answers are large enough for the printer to carry, and it is **exact**:
**177/256 on the RTX 3060 and 170/256 on the Iris Xe, worst 1 ulp, `inexact_printing = 0`.** The
two vendors differ from each other by more than they differ from the before-figure, which is the
[device-body-sweep](device-body-sweep.md) §2.1 rule showing through a different lane set and not a
disagreement about the operator.

**Measured, and the sharpest form of the whole finding: `ln` is the one spliced body that still
moves, and it moves only on the Iris Xe.** `crates/lichen-compute-gpu/tests/compute_gpu/
spliced_transcendentals.rs` holds `ln` to **0** and already did;
[operator-reverification](operator-reverification.md) §4 has the run. On an RTX 3060 all six
arguments are bit-exact; **on an Intel Iris Xe, `ln(1.5)` answers `0x3ecf9921` against the
definition's `0x3ecf9920` — 1 ulp — and the test fails.** `ln`'s body is the one
[transcendental-gap-mechanism](transcendental-gap-mechanism.md) §1.2 counts an `OpFDiv` in: one,
in `(m − 1)/(m + 1)`, with denominators `1.75`, `2.25` and `1.9765625`. **`NoContraction` forbids a
driver's fusion and reassociation; it says nothing about how a driver rounds a quotient.** That
is [floating-point](floating-point.md) §4.4's position, unchanged, and §7's block declining to
claim the division findings fixed is **confirmed rather than left open**.

## 4. The test, before and after, and its output on both devices

**The change is the user's decision**, taken on [contraction-control](contraction-control.md) §9.3's
zero. It is the only test change in this note, and no production file was touched.

**Before** — `crates/lichen-compute-gpu/tests/compute_gpu/spliced_transcendentals.rs`, the bounds
as `dev@0467c698` has them:

```rust
/// `sin` is asked over all four residues of the reduction and over a negative.
#[test]
fn a_spliced_sin_is_the_definition_the_cpu_runs() {
    let Some(context) = common::context("a_spliced_sin_is_the_definition_the_cpu_runs") else {
        return;
    };
    agrees(
        &context,
        "sin",
        KernelInstr::Sin,
        &[0.25, 1.0, 3.0, 4.0, 7.0, -1.5],
        lichen_kernel_ir::sin_value,
        5,
    );
    println!("ran on {}", context.device_name());
}

/// `cos` is asked over the same arguments, so the expr reads beside each other.
#[test]
fn a_spliced_cos_is_the_definition_the_cpu_runs() {
    let Some(context) = common::context("a_spliced_cos_is_the_definition_the_cpu_runs") else {
        return;
    };
    agrees(
        &context,
        "cos",
        KernelInstr::Cos,
        &[0.25, 1.0, 3.0, 4.0, 7.0, -1.5],
        lichen_kernel_ir::cos_value,
        6,
    );
    println!("ran on {}", context.device_name());
}
```

**After** — the same file, the two bounds taken to zero, and one line of history on each doc
comment:

```rust
/// `sin` over all four reduction residues and a negative.
/// Bound was 5; see `operator-reverification.md`.
#[test]
fn a_spliced_sin_is_the_definition_the_cpu_runs() {
    let Some(context) = common::context("a_spliced_sin_is_the_definition_the_cpu_runs") else {
        return;
    };
    agrees(
        &context,
        "sin",
        KernelInstr::Sin,
        &[0.25, 1.0, 3.0, 4.0, 7.0, -1.5],
        lichen_kernel_ir::sin_value,
        0,
    );
    println!("ran on {}", context.device_name());
}

/// `cos` over the same arguments, so the expr reads beside them.
/// Bound was 6; same change and same note.
#[test]
fn a_spliced_cos_is_the_definition_the_cpu_runs() {
    let Some(context) = common::context("a_spliced_cos_is_the_definition_the_cpu_runs") else {
        return;
    };
    agrees(
        &context,
        "cos",
        KernelInstr::Cos,
        &[0.25, 1.0, 3.0, 4.0, 7.0, -1.5],
        lichen_kernel_ir::cos_value,
        0,
    );
    println!("ran on {}", context.device_name());
}
```

**The doc comments are held to [comment-policy](comment-policy.md)'s unheaded budget of 120
characters, which is why the history is one short line and the pointer rather than a sentence.**
The first form this note quoted was 153 and `comments-check` refused it — against the note, by
reading the fenced `///` lines out of it, which is a sharper instrument than it looks.

**Measured: the test bites at zero and passes there, on both devices.** `cargo test -p
lichen-compute-gpu --test compute_gpu -- --test-threads 2 --nocapture`, one process at a time per
device:

| device | `sin` | `cos` | `ln` | suite |
|---|---|---|---|---|
| **RTX 3060 Laptop GPU** | **0 ulp** over 6 elements | **0 ulp** over 6 elements | **0 ulp** over 6 elements | **88 passed / 0 failed / 0 ignored** |
| **Intel Iris Xe** | **0 ulp** over 6 elements | **0 ulp** over 6 elements | **1 ulp** at `1.5` — **fails** | **87 passed / 1 failed / 0 ignored** |

`sin(3)` — the angle [floating-point](floating-point.md) §8 named as its worst, and the one
[device-operator-sweep](device-operator-sweep.md) §2 measured at 5 ulp — reads
`device 1.4112002e-1 (0x3e1081c4) cpu 1.4112002e-1 (0x3e1081c4) 0 ulp` on the RTX 3060, against
`0x3e1081c9` there before. `cos(-1.5)`, that note's 6-ulp point, reads `0x3d90deab` on both.

**Measured: the suite's counts are unmoved by the tightening, on both devices.** Before it, the
same suite on the same two devices: **88 / 0 / 0** on the RTX 3060 and **87 / 1 / 0** on the Iris
Xe; after it, **88 / 0 / 0** and **87 / 1 / 0**. **The Iris Xe's one failure is
`a_spliced_ln_is_the_definition_the_cpu_runs`, it was failing before this change at the same 1
ulp, and this change did not touch it.** The 88/0/0 that
[contraction-control](contraction-control.md) §9.6 records is the RTX 3060 figure alone, which is
why the Iris Xe's `ln` is not in it.

**Measured: the rest of the loop closes.** The examples survey is
**`clean=43 panicked=[] drifted=[]`**, `lichen-compute` is **204 passed / 14 failed / 2 ignored**
with the same fourteen names, all in the tiling, pack-placement and fold-datum areas and none of
them touching a transcendental or the emitter, and `git diff dev...HEAD -- examples/` is **empty**
— no example file was edited and no `output =` line moved.

## 5. Corrections to the notes this follows

**Each is stated in the note it corrects and left in that note's own words**, rather than here.
What follows is the index of them.

- **[device-operator-sweep](device-operator-sweep.md)** §1's table: **only `rope` moved.** Its
  "worst **3**", its five differing elements and the 5-ulp `sin 3` are **1 ulp and two differing
  elements** on an RTX 3060 and one on an Iris Xe; `silu`'s, `attention`'s, `softmax`'s,
  `rmsnorm`'s, `layernorm`'s and `matmul`'s rows are **unchanged**, and `attention`'s six
  differing elements are still six. §2's "the operator reproduces the standalone measurement" and
  "the composition absorbs part of it" **still hold** — the size moved, the reading did not.
- **[device-body-sweep](device-body-sweep.md)**'s header table: four of its rows moved and four
  did not, and the rows that did not move are the division ones. **Nothing on that page is
  retracted**; it is the record of the path the decoration was measured against, and it says so.
- **[floating-point](floating-point.md)** §7: its block saying the division findings are **"not
  re-measured, and deliberately not claimed fixed"** is now the measured position rather than a
  restraint — §3's rows and `ln`'s Iris Xe failure are that measurement — and **§4.4's "a kernel's
  float division is unspecified" is unchanged.**
- **[floating-point](floating-point.md)** §8: the sentence "worst **5 ulp for `sin`** … **6 ulp
  for `cos`**" is **0 and 0** in the test itself, which is now the file's own bound.
- **[contraction-control](contraction-control.md)** §6's bullet and §9.6's paragraph, both of
  which say **"No test was changed"**: that was true of that slice and **is no longer true**, and
  each now says so beside the sentence it corrects.

## 6. What this means for the kernel, and what a fix would touch

**No production change is made or proposed here; a fix is the next slice.** The measurement draws
the line in one place, and both halves of it are measured rather than argued.

- **The transcendental is no longer the gap in any operator body measured here.** `exp` alone is
  **256/256 on both grids on both vendors**, `silu`'s 158-ulp tail is **1**, `rope`'s 3 is **1**,
  and `rope`'s swept body is **512/512**. **What remains is a division, and
  [contraction-control](contraction-control.md) was never going to move it.**
- **The remaining gap is [floating-point](floating-point.md) §4.4's unspecified division**, and
  this note gives it three measured instances on two vendors: `rmsnorm`'s `Div` at **165/256 and
  2 ulp**, `softmax`'s gathered divisor at **1 ulp**, `attention`'s at **1 ulp on six of eight
  lanes**, and `ln` at **1 ulp on the Iris Xe** — the one body with no power-of-two divisor
  anywhere in it that still moves.
- **A fix for the division is not an emitter fix and would not be a smaller `NoContraction`.** It
  would reach one of two places, and which one is a decision nobody has taken:
  - **a stated tolerance keyed on the divisor's shape**, the sentence
    [device-body-sweep](device-body-sweep.md) §6's first bullet already names, beside
    [floating-point](floating-point.md) §4.4 — *power of two is exact and everything else is
    within one or two ulp, with the movers decided by the numerator's significand*. **§3's `sqrt`
    row is the evidence this needs and did not have**: a body that *contains* a division is
    bit-exact over more of its inputs than one that divides by it, which is a statement about
    where the tolerance belongs that no sweep before this one could make.
  - **or the bodies that divide nowhere**, which is the reading `rmsnorm`, `softmax` and
    `attention` all support: each already multiplies by a reciprocal where the arithmetic allows
    it, and the change would be to make the division *optional* — `ln`'s `(m−1)/(m+1)` and
    `rmsnorm`'s `x / d` are the two sites in this tree where a body inlines a definition **and**
    divides, and they are the two that still move. That reaches
    `crates/lichen-kernel-ir/src/unary.rs`'s `ln_body` and the language's own lowering of a `/`,
    **not** the emitter.
- **The policy question [device-body-sweep](device-body-sweep.md) §6's second bullet poses is
  narrower than it was, and this note does not close it.** A device now reproduces a body bit for
  bit wherever the body contains no division, which is most of what these eight operators run. What
  [floating-point](floating-point.md) §6 still asks — *whether a device may run a body that does
  not reproduce the language's definition at all* — is **a question about the division**, and it
  now has a measured size: **one or two ulp, on five lanes in six, on two vendors.**

## 7. What is measured, what is inferred, what is unknown

**Measured, in full.** Every after-figure in §2's table and §3's three tables — 8 operators, 2
devices, 19 rows — read off the two backends' own renderings of the same program; the two
`spirv-val`-free device names from `vulkaninfoSDK --summary` under each manifest; the test file
before and after and its per-device output; the GPU suite's counts on both devices before and
after; the examples survey's `clean=43 panicked=[] drifted=[]`; `lichen-compute`'s
**204 / 14 / 2** with the fourteen names.

**Inferred, and not re-derived here.** That `attention`'s six movers are its final division rather
than its `exp`, from `scores_raw` having been bit-exact in the run that note recorded and from
every mover being exactly 1 ulp where the gathered-divisor rows are also 1 ulp — the attribution is
from the sizes, and no probe separated them inside `attend_raw`. That `silu`'s 66 remaining movers
are its `Div`, from its `Div`-alone row being **186/256, unchanged**, against a body at 190/256 —
**the arithmetic of that comparison is a subtraction, and what it establishes is an upper bound on
the body's movers, not a per-lane attribution.**

**Unknown, and this note closed none of these.**

- **Whether the printer's eight significant digits hide a mover anywhere in §2.** Every row with
  `inexact_printing = 0` is exact; the one row with a non-zero count is reported as a bound, and
  **whether any of its equal tokens is in fact a differing value is not measured.** §3's shifted
  grid is the same division on a scale the printer carries, and it does not close it either — it
  answers with a different lane set.
- **Whether the Iris Xe's `softmax` and `silu` counts differ from the RTX 3060's because of the
  device or because of the lane set.** They differ by more than either differs from the
  before-figure, and this note did not run a body both vendors answer identically to separate the
  two.
- **Whether a body with no `OpFDiv` in it is now exact everywhere, or only over these grids.**
  `rope`'s 512/512 and `exp`'s 256/256 are grids; [transcendental-gap-mechanism]
  (transcendental-gap-mechanism.md) §7's "is the fusion stable across recompiles and pipeline
  caches" is untouched here, and a cold-cache recompile was not run.
- **The seventeen-magnitude reduction sweeps** of [device-body-sweep](device-body-sweep.md) §4 —
  `matmul` 204, `gemv` 68, `attention` 136 — **are not re-run**, and §2 says why.

## 8. What this slice deleted

**One probe, and it is gone.** `crates/lichen-compute/examples/rever_probe.rs` — the scratch
driver that ran every §2 and §3 measurement, reading each example from `examples/`, substituting
the data literal in memory, running each program on both backends and scoring the two renderings —
was **deleted before this note's commit**. It held no production code and nothing else referenced
it. **No example file was edited**, no `output =` line moved, `git diff dev...HEAD -- examples/` is
empty, **and no production file was touched** — the only code change in the branch is the two
bounds in §4. The two run logs the §4 table is read out of were left under `target/`, which is not
a tree.
