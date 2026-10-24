# What a device's quotient is, and whether the language can correct it

> Status: **current** — every figure below was measured on **two** devices off
> `dev@adca4e64`, by running the **same binary twice** with `VK_DRIVER_FILES`
> pointed at one vendor's ICD manifest and nothing else: an **NVIDIA GeForce RTX 3060 Laptop
> GPU** (discrete) and an **Intel(R) Iris(R) Xe Graphics** (integrated). The manifests are
> `...\DriverStore\FileRepository\nvamsi.inf_amd64_…\nv-vk64.json` and
> `...\DriverStore\FileRepository\iigd_dch.inf_amd64_…\igvk64.json`; each names one device under
> `vulkaninfoSDK --summary`, and **the per-vendor runs are the only way to say which is which**,
> because the default loader enumerates five. **This note changed no production file and no test.**
>
> **What is being measured** is the one thing [operator-reverification](operator-reverification.md)
> §3 left: what a device's float quotient actually is, isolated from every operator that contains
> one. [floating-point](floating-point.md) §7's numbers and the [body
> sweep](device-body-sweep.md) §2.1's rule are **referenced here, not restated**; §5 and §6 correct
> claims of theirs this measurement contradicts, each in the note's own words with its history kept.
>
> **The one-sentence result: a device does not divide, it multiplies by a reciprocal, and the
> mover rule is an identity rather than a threshold.** For **every** `(a, b)` expr measured on
> **both** vendors there is an `f32` `r` with `a · r` **exactly** equal to the device's answer, and
> for every divisor of a 64-point grid that `r` is **unique**. The lanes that move are the lanes
> where `a · r` and the correctly rounded quotient disagree, and the two vendors' `r` are not the
> same number. **The language cannot write a correction that reaches the correctly rounded
> quotient**, and §7 says exactly which spelling is missing.

## 1. The method, and the grid

**Measured.** The kernel is `z[i] = a[i] / b[i]` and nothing else — two float reads, one `Div`, one
write — so the whole of what is compared is the quotient. It is run twice from one source, once on
`cpu` and once with only the backend literal changed, and the two renderings are scored element by
element. **One process at a time per device**, because `install_default`'s backend slot and the
kernel registry are both process-global.

**The grid is four shapes, and the first two are the grids [device-body-sweep](device-body-sweep.md)
§2.1 already defines, so the two notes' figures can be read against each other.**

| shape | numerators | divisors | lanes |
|---|---|---|---|
| **fixed**, §2.1's own | **M**, `(1 + (m mod 8)/8)·2^⌊m/8⌋`, 64 points | one constant: `1, 2, 4, 8, 3, 5, 7` | 64 each |
| **product** | **M**, 64 points | **D**, `(1 + (m mod 8)/8)·2^(⌊m/8⌋−4)`, 64 points | **4096**, the full 64 × 64 |
| **dense** | every consecutive dyadic in `[1, 2)` at `2⁻¹²` | one constant: `3, 5, 7, 4, 2.5, 0.1` | 4096 each |
| **computed** | `v` over `ln`'s band `[1/√2, √2)`, 4096 lanes | `v + 1.0`, **computed on the device** | 4096 |

**The dense shape is what finds the movers and what measures them.** §2.1's fixed divisors sample
64 numerators against one divisor; the dense shape takes one divisor against every dyadic numerator
in a binade, so the residual distribution is sampled rather than guessed at. **The computed shape
is the one that matters for `ln`**, because its divisor is a value the kernel computes rather than a
constant, and it is where a per-lane rule has to survive contact with an arbitrary divisor.

**What the comparison can and cannot see, stated once, because §5 changes it.** Both backends print
through the language's own printer, so the two columns are compared through printed tokens and a
token is parsed back to its `f32` before any bit is read. **Every token printed by either backend
on every grid in this note parses back to exactly the value that was printed** — see §5.

## 2. The reference, and why it is `cpu`

**Measured: the host's own `f32` division is correctly rounded, so `cpu` and the mathematical
quotient are the same number here and the choice between them does not arise.** The probe carries
an integer reference — `a / b` decomposed into `mx · 2^ex` over `my · 2^ey`, normalised to a
quotient in `[1, 2)`, multiplied out in `u128`, and rounded half-to-even by hand — and asks it
against the host's `/` over **6,000,000** operands spanning exponents `2⁻³⁰ … 2²⁹`:

| | figure |
|---|---|
| the host's `f32 /` answers the correctly rounded quotient | **6,000,000 of 6,000,000** |
| worst ulp when it does not | **0** |

**So the brief's question — *the platform's own division or the mathematical quotient* — has one
answer on this machine, and every figure below is stated against both at once.** It also settles
the direction of every gap in this note: **where the two backends differ, `cpu` is right and the
device is one to two units out**, because `cpu` is not a definition this repository chose but the
hardware's own correctly rounded divide, and the device is measured against it on 6 million
samples. That is a fact about this machine's `cpu` backend, and it is **inferred** — not measured —
that a different host's `f32` divide is also correctly rounded; the probe is the instrument that
would settle it elsewhere, and it is deleted with the rest.

## 3. Per device

**Measured, in full.** `N` is the lane count; *correctly rounded* is the count of lanes whose two
tokens are equal, which by §1 is the count of lanes whose `f32`s are equal.

| grid | N | RTX 3060 correct | worst ulp | Iris Xe correct | worst ulp |
|---|---|---|---|---|---|
| **fixed** `/1.0` | 64 | **64** | **0** | **64** | **0** |
| **fixed** `/2.0` | 64 | **64** | **0** | **64** | **0** |
| **fixed** `/4.0` | 64 | **64** | **0** | **64** | **0** |
| **fixed** `/8.0` | 64 | **64** | **0** | **64** | **0** |
| **fixed** `/3.0` | 64 | **48** | **1** | **48** | **1** |
| **fixed** `/5.0` | 64 | **48** | **1** | **48** | **1** |
| **fixed** `/7.0` | 64 | **24** | **1** | **16** | **1** |
| **product**, M × D | 4096 | **3136** | **1** | **2304** | **1** |
| **dense** `/3.0` | 4096 | **2730** | **1** | **2730** | **1** |
| **dense** `/5.0` | 4096 | **3276** | **1** | **3276** | **1** |
| **dense** `/7.0` | 4096 | **1852** | **1** | **1097** | **1** |
| **dense** `/2.5` | 4096 | **3276** | **1** | **3276** | **1** |
| **dense** `/4.0` | 4096 | **4096** | **0** | **4096** | **0** |
| **dense** `/0.1` | 4096 | **4096** | **0** | **4096** | **0** |

**Measured: `0.1` is not a power of two and moves nothing.** The [body sweep](device-body-sweep.md)
§2.1's rule reads *bit-exact whenever the divisor is a power of two and within 1 ulp otherwise*.
`0.1` is the counterexample to the **first** half read as a condition: its reciprocal happens to be
exactly `10.0`, and `a · 10` is exact for every numerator on this grid, so **4096 of 4096 agree on
both vendors**. The rule's second half is a bound and survives; §4 replaces the first half with
something that predicts rather than summarises.

**Measured: the worst is 1 ulp on the isolated quotient on every grid except one.** The single
exception is in §6's `ln` band, where the RTX 3060 answers one lane of 4096 **2 ulp** away. §4 says
what that lane is.

## 4. The mover rule

**Measured: a device's `a / b` is `a · r(b)` for one `f32` `r`, and the lanes that move are the lanes
where that product and the correctly rounded quotient disagree.** The instrument is a **reciprocal
interval**: for one divisor, binary-search the ordered `f32` keys of `r` for the interval that
reproduces the device's answer on every one of its 64 numerators, and intersect the intervals lane by
lane. A non-empty intersection is a proof that a multiply by some reciprocal is what the hardware
did; a width of zero is a proof that the reciprocal is **pinned to one bit pattern**.

**Measured: on all 64 divisors of **D** the interval is non-empty and of width zero, on both
vendors** — so every one of those 64 divisors has a uniquely determined reciprocal, and the rule is
an identity rather than an approximation of one.

| | divisors | non-empty | width zero | `r` is the correctly rounded `fl(1/b)` | `r` is `trunc(1/b)` |
|---|---|---|---|---|---|
| **RTX 3060** | 64 | **64** | **64** | **64** | 8 |
| **Iris Xe** | 64 | **64** | **64** | **40** | 32 |

**Measured: the two vendors' reciprocals are different numbers, and that is the whole difference
between their rows.** On the RTX 3060 the reciprocal is the correctly rounded `fl(1/b)` for **64 of
64** divisors. On the Iris Xe it is correctly rounded for **40 of 64** and **one unit in the last
place below it** for the other **24**, and on those 24 the value the device uses **is** the
truncation of `1/b` toward zero. Four divisors pinned directly, off the **dense** grid:

| divisor | RTX 3060 `r` | Iris Xe `r` | `fl(1/b)` | Iris Xe's gap |
|---|---|---|---|---|
| `3.0` | `0x3eaaaaab` | `0x3eaaaaab` | `0x3eaaaaab` | **0** |
| `7.0` | `0x3e124925` | **`0x3e124924`** | `0x3e124925` | **1 ulp low** |
| `2.5` | `0x3ecccccd` | `0x3ecccccd` | `0x3ecccccd` | **0** |
| `0.1` | `0x41200000` | `0x41200000` | `0x41200000` | **0** |

**Measured: the identity holds where the divisor is computed rather than constant.** Over the 4096
lanes of the `ln` band, where the divisor is `v + 1.0` computed on the device and is different on
every lane, the reciprocal interval is **non-empty on 4096 of 4096 lanes on both vendors**. So the
rule is not an artefact of a sweep over constants: **a device's quotient is a multiply by a
reciprocal of its divisor, whatever produced that divisor.** On the lanes where the interval alone
pins `r`, that `r` is **not** the correctly rounded reciprocal on **404 of 2975** (RTX 3060) and
**353 of 2964** (Iris Xe) — so outside the 64-point grid the RTX 3060's reciprocal is *also*
sometimes below the nearest one, and the Iris Xe's is more often.

**Measured: the threshold reading is not the rule, and the residual does not separate the movers.**
The probe computed, for every lane, the exact position of the true quotient inside its rounding
interval in units of one ulp — `0` is the correctly rounded answer, `±1/2` is a midpoint — and the
two do not separate. On **dense `/3`** the movers reach `|t| = 1/3` and the quiet lanes sit at
`t = 0`, so a mover is *not* a lane near a boundary; on **dense `/4`** there are no movers and
`max|t| = 0`; and on **dense `/0.1`**, which has no movers at all, `min|t| = 0.125`. **The
[numerator-significand reading](device-body-sweep.md) §2.1 offers is a correlate of the reciprocal
rule and not its cause**: which numerators move is decided by `r`, and the significand of the
numerator is only how the resulting disagreement happens to fall. **The two-ulp lane in §3 is the
same story** — at `v = 0.78116655` the RTX 3060's answer `0xbdfb9dd5` is two units from the
correctly rounded `0xbdfb9dd3`, and **neither** `fl(1/b)` **nor** `trunc(1/b)` reproduces it, so that
lane is a case where the driver's reciprocal is two units below the nearest rather than one.

**So the rule, in one line: the mover set is exactly `{ a : a · r(b) ≠ fl(a / b) }`, where `r(b)` is
the vendor's reciprocal approximation, and the count follows from `r` alone.** That is a stronger
statement than a bound because it is **predictive**: given `r`, every mover is named in advance, and
§3's `dense / 7.0` row — `1852` on the RTX 3060 against `1097` on the Iris Xe — is exactly the
count predicted by two different `r` for one divisor.

## 5. The printer, and a correction to the note that measured it

**Measured: the printer round-trips, and the `inexact_printing` column in
[operator-reverification](operator-reverification.md) §3 measured something else.** The probe reads
each printed token back to an `f32` and asks whether re-printing that `f32` with the printer's own
spelling — `f32`'s positional `Display` with a forced `.`, which is
[`float_literal` in `lichen-render`](../../crates/lichen-render/src/render.rs)'s whole body —
reproduces the token:

| grid | tokens that round-trip |
|---|---|
| every grid in §3, both vendors | **every lane** |
| `softmax`'s gathered divisor, exponents `−16 + m/16`, both vendors | **256 of 256** on `cpu` and **256 of 256** on the device |
| `ln`'s band, 4096 lanes, both vendors | **4096 of 4096** on `cpu` and **4096 of 4096** on the device |

**The claim this contradicts, quoted.** *operator-reverification.md* §1: the printer *"writes **eight
significant digits** in fixed notation"*; §3: the `softmax` row's *"`inexact_printing = 46 of 78`"*,
and *"**That first row is a bound, not a figure, and the reason is the printer and not the device**"*
— because its answers *"run down to `1.8e-6`, where the printer's eight significant digits are about
the same width as one `f32` ulp, so **46 of its 78 differing tokens are printed too coarsely to
recover the bits behind them**"*, and §7 lists *"**Whether the printer's eight significant digits
hide a mover anywhere in §2**"* as unknown.

**What is measured instead.** The printer does not truncate to eight significant digits; it prints
`f32`'s shortest positional round-tripping form. The 46 tokens were compared against Rust's
`Debug` spelling, which differs from the printer's in exactly the cases §5's table used to find:
a whole number prints `16.0` through the printer and `16.0` under `Debug` but `16` under `Display`,
and a small value prints `0.0000018` where `Debug` writes `1.8e-6`. **Those are spelling
differences, not lost bits.** With the comparison taken against the printer's own spelling, the
same row is **178 of 256 on the RTX 3060 and 158 of 256 on the Iris Xe at a 1-ulp worst, an exact
figure and not a bound** — and §7's "whether the printer hides a mover" is answered for every grid
in §3: **it hides none.**

**The history is kept, not withdrawn.** §1's *inexact_printing* column is still a correct instrument
— it counts tokens whose printed form does not survive a round trip through one specific formatter —
and it is still the reason that note held one row back. What moved is the reading: it was a
statement about `Debug` and `Display`, and it was reported as a statement about the printer. The
row it qualified is now an exact figure, and the two device rows underneath it are unchanged by
this note.

## 6. The operators' own divisors

**Measured: every operator row in [operator-reverification](operator-reverification.md) §3 reproduces
on the same binary and the same two devices, which is what licenses reading them as instances of §4.**
Nothing here is a new operator figure; it is the same arithmetic, isolated.

| the operator's own divisor | RTX 3060 | Iris Xe | §4 explains it |
|---|---|---|---|
| `rmsnorm`'s Newton `sqrt`, 256 lanes | **220**, worst **1** | **230**, worst **1** | — it divides nowhere itself; its own 2-ulp worst is the `Div`'s |
| `rmsnorm`'s `Div` alone, `x / d` | **165**, worst **2** | **165**, worst **2** | **yes** — `d` is a per-lane computed divisor, §4's computed shape |
| `rmsnorm`'s whole body, `(x / d) · w` | **165**, worst **2** | **165**, worst **2** | **yes** — figure for figure the `Div`'s |
| `softmax`'s gathered divisor, exponents `−16 + m/16` | **178**, worst **1** | **158**, worst **1** | **yes** — a per-lane gathered divisor |
| `softmax`'s gathered divisor, exponents `m/16` | **177**, worst **1** | **170**, worst **1** | **yes** |
| `ln`'s `(v−1)/(v+1)`, 4096 lanes over the band | **2931**, worst **2** | **2944**, worst **1** | **yes** — 4096 of 4096 lanes carry a reciprocal (§4) |

**Measured: `ln`'s three denominators, one lane each, and the Iris Xe's is the only one that fails
the test.** The `ln` test's six arguments reduce into the band at `v ∈ {0.75, 1.25, 0.9765625}`,
which are the reduced arguments behind the denominators `1.75`, `2.25` and `1.9765625` that
[floating-point](floating-point.md) §7 names:

| reduced `v` | denominator | RTX 3060 | Iris Xe | the device's answer against the correctly rounded one |
|---|---|---|---|---|
| `0.75` | `1.75` | **0 ulp** | **1 ulp** | Iris Xe `0xbe124924`, correctly rounded **`0xbe124925`** |
| `1.25` | `2.25` | **0 ulp** | **0 ulp** | both `0x3de38e39` |
| `0.9765625` | `1.9765625` | **1 ulp** | **1 ulp** | both `0xbc4246d5`, correctly rounded `0xbc4246d4` |

**That first row is the sharpest instance in this repository, and it is now a division rather than a
body.** `ln(1.5)` reduces to `v = 0.75`, `1.5 / 2 = 0.75` exactly, so the operand is exact and the
**only** thing between the two backends is `−0.5 / 1.75`. The RTX 3060's reciprocal for `1.75` is
the correctly rounded one and the Iris Xe's is not, and the Iris Xe answers one unit low. **This is
[operator-reverification](operator-reverification.md) §3's `ln` failure, reduced to a single `Div`
with one numerator and one denominator** — and the third row is new: **`v = 0.9765625` moves on the
RTX 3060 too**, at 1 ulp, which that note did not measure because it swept the whole body rather
than the divisor. The body absorbs it (`ln(1000.0)` is bit-exact on both vendors), which is the
[floating-point](floating-point.md) §7 relation in the other direction: a body can be exact while
its divisor is not.

**Measured: the operator rows follow from §4 rather than from the operators.** Every row that moves
is a row whose divisor is not a power of two, and §4 says which lanes of it move and why; every row
that is bit-exact is a row whose divisor **is** one, or — `softmax`'s and `rmsnorm`'s own rows show
this — a row where `a · r(b)` happens to equal the correctly rounded quotient on every lane it
runs. The count is a property of the operands, not of the operator.

## 7. Can the language close it, and what does it cost

**Measured: a compensated division is expressible in the language today, it buys most of the gap,
and it does not reach zero ulp.** Five spellings were written out in the kernel language over
`rmsnorm`'s own 256-lane divisor grid — numerators the **M** grid, divisors `sqrt(s/4 + 0.25)` —
and run on both devices. *Same* counts lanes where the device agrees with `cpu`; *worse* counts
lanes where the device is further from the correctly rounded quotient than `cpu` is:

| spelling | what it is | RTX 3060 same / worst / worse | Iris Xe same / worst / worse |
|---|---|---|---|
| **`plain`** | `a / b` | **180** / 1 / 76 | **180** / 1 / 76 |
| **`recip`** | `a * (1.0 / b)` | **222** / 2 / 21 | **218** / 2 / 22 |
| **`nr1`** | one Newton–Raphson step on `1/b`, then multiply | **222** / 2 / 23 | **223** / 2 / 24 |
| **`nr2`** | two Newton–Raphson steps | **234** / 1 / 16 | **231** / 2 / 17 |
| **`markstein`** | Dekker two-product, exact residual, one correction | **230** / 1 / 26 | **203** / 1 / 53 |

**Measured: the shortfall is the device's, not the spelling's.** The same `markstein` sequence run on
the host in `f32` is **24,576 of 24,576 exact** over the four quotient grids and the `ln` band, at a
**zero** worst ulp. **So the algorithm is correct and the device does not follow it**, and the reason
is §4: every step of the sequence is built out of the device's own `a / b` and `1.0 / b`, so the
approximate reciprocal the driver supplies is an **input** to the correction and not something the
correction can recover.

**Measured: the correction reaches 234 of 256 and stops, and it costs about fifteen kernel
instructions.** `markstein` is fifteen float operations where `plain` is one, and it is worth
**+50 lanes** on the RTX 3060 (`180 → 230`) and **+23** on the Iris Xe (`180 → 203`) with no
production change and no new operator. **It does not reach zero ulp on either vendor**, and the two
vendors disagree about how close it gets, which is the §4 finding showing through the fix.

**Measured: `nr2` beats `markstein`, and that is the telling row.** Two Newton steps on the
reciprocal — four float operations — reach **234** where the fifteen-operation exact-residual
correction reaches **230** on the RTX 3060. A sequence that only refines the reciprocal cannot
outperform a sequence that computes the residual exactly unless the exact residual is being
computed on a multiply the device is not contracting as written; the more likely reading is that
refining `r` shrinks the product's error faster than the correction shrinks it, and **both** remain
bounded by how far the device's `r` is from the truth.

**Measured: the spelling that reads most naturally does not compile, and the one next to it does.**
`rc = 1.0 / qb` followed by `rc = rc * (2.0 - qb * rc)` — a `let` whose value reads the name it
binds — **overflows the host stack** and the probe process dies with `0xC00000FD` on both vendors,
before anything reaches a device. The same two lines written with a second name
(`rc0 = 1.0 / qb`, `rc = rc0 * (2.0 - qb * rc)`) run, and are the `nr1` row above. **A Newton
iteration is expressible; writing it the way one writes it is not.** That is a front-end finding and
this note does not diagnose it.

**So: is the missing spelling a fused multiply-add?** The brief for this slice holds that *"`NoContraction`
now exists, so an exact product can be *asked for*"*, and that is **not what `NoContraction` is.**
Searched and not found: there is **no** `fma`, `FusedMultiplyAdd` or `OpFma` anywhere in this
workspace, and [contraction-control](contraction-control.md) §3.3's module census already records
that the decorated body carries *"[no] capability, no execution mode, no `OpExtInst`, **no
`Fma`**"*. `NoContraction` is a **prohibition** — it forbids a driver from contracting
`OpFMul` into `OpFAdd` — and a prohibition cannot supply an instruction. **What is missing is the
operator itself**: a fused multiply-add in `PrefixOp` and `KernelInstr`, which would let the Markstein
residual be `fma(-b, q0, a)` in one instruction and would make `markstein` four operations instead of
fifteen. **That the fifteen-operation spelling is expressible and runs is the measurement; that it
does not reach zero ulp is the consequence; the `Fma` is the spelling that would.**

## 8. What a fix would touch

**No production change is made or proposed here. A fix is the next slice and the user decides
between these three.**

- **The emitted division — a correctly rounded quotient would have to be chosen deliberately.** §4
  says the device's `a / b` is `a · r(b)`; a fix at the emitter is a correctly-rounded division
  sequence emitted in place of `OpFDiv`, at the cost of every such division in every module. That is
  expensive, it is a deliberate choice rather than a fix, and **nothing measured here says it is
  wanted** — §7 shows the same correctness is reachable in the language for the cost of fifteen
  instructions, on the operators measured.
- **A documented tolerance, keyed on the divisor.** §4 gives the tolerance a form it did not have: it
  is not *"power of two is exact and everything else is within one or two ulp"* by magnitude but *the
  mover set is `{ a : a · r(b) ≠ fl(a/b) }`*, where `r(b)` is the vendor's reciprocal and is **one ulp
  low on 24 of 64 divisors measured on one vendor and on 0 of 64 on the other**. A tolerance that is
  keyed on **the vendor** and not on the divisor's shape is what this measurement supports, and it is
  a worse tolerance to write down than [floating-point](floating-point.md) §4.4's existing *"a kernel's
  float division is unspecified"*, which says the same thing without pretending to know the size.
- **Nothing at all, because `cpu` is the reference.** §2 measured the host's divide as correctly
  rounded on 6,000,000 of 6,000,000 samples. A device that answers `a · fl(1/b)` is within 1 ulp of
  the platform on **every** grid in §3, and **the emitter already carries `NoContraction` so that the
  transcendental part of these bodies is bit-exact**. The remaining gap is 1 ulp on a quotient, on
  lanes a kernel chooses, and **the honest reading of §4 is that this is a property of the hardware's
  divider and not of this repository's code.**

## 9. Corrections to the notes this follows

**Each is stated in the note it corrects and left in that note's own words**, rather than here. What
follows is the index.

- **[operator-reverification](operator-reverification.md)** §1's *"eight significant digits"* and §3's
  *"`inexact_printing`"* and *"a bound, not a figure"*, and §7's *"whether the printer's eight
  significant digits hide a mover"*: §5 above. **The row is an exact figure — 178 and 158 at a 1-ulp
  worst — and the printer round-trips on every lane measured.**
- **[device-body-sweep](device-body-sweep.md)** §2.1's *"bit-exact exactly when the divisor is a
  power of two, and within 1 ulp otherwise"* and *"the movers are the `…01` significands"*: §4 above.
  **The power-of-two half is not a condition — `0.1` is 4096 of 4096 on both vendors — and the mover
  set is predicted by the vendor's reciprocal rather than by the numerator's significand.** The
  bound's size and the per-divisor tables are unchanged.
- **[floating-point](floating-point.md)** §7: its paragraph's *two numbers* were taken on the
  undecorated path and its own block says so. **They stand as recorded** — §3 re-measures the same
  shape under the shipped emitter and the figures are unchanged in kind. **Its sentence "the
  reduction multiplies and never divides" is now the right rule for a second reason**, §4: dividing
  would not have helped, because the multiply the reduction uses is exactly the multiply the device
  is already doing wrongly. **§4.4's "a kernel's float division is unspecified" is unchanged** and
  is what §8's third bullet argues for.

## 10. What is measured, what is inferred, what is unknown

**Measured, in full.** §2's 6,000,000-sample host reference; §3's fourteen rows on two devices;
§4's reciprocal intervals on 64 divisors and on 4096 computed-divisor lanes on both vendors, the
four pinned reciprocals, the residual distributions, and the two-ulp lane; §5's round-trip counts;
§6's six operator rows and three `ln` denominators; §7's five spellings on two devices, the
24,576-sample host `markstein`, and the stack overflow.

**Inferred, and not re-derived here.** That the Iris Xe's below-nearest reciprocals are a truncation
of `1/b` — **measured** on the 24 divisors of §4's table where the two coincide, and **inferred**
for the rest from the count. That the driver *chooses* `trunc(1/b)` rather than that `trunc(1/b)`
coincides with what it chose: §4 measured only that `r` equals the truncation, not that anything
computed it that way. That the two-ulp `ln`-band lane is a reciprocal two units below the nearest:
§4 measured that neither `fl(1/b)` nor `trunc(1/b)` reproduces it, and did not measure what does.

**Unknown, and this note closed none of these.**

- **Whether `r(b)` is a table.** §4 pinned `r` on 64 divisors and 4096 computed-divisor lanes and did
  not read the vendor's reciprocal routine, so **a predictor of `r` for an arbitrary divisor is not
  established**, and §4's rule is *post hoc* per divisor rather than a closed form.
- **Whether the rule holds off the float grids.** Every divisor here is in `[2⁻⁴, 2)` and every
  numerator in `[1, 7.5]`; subnormal quotients, overflow and the sign of a negative quotient were
  **not** measured, and §4's reciprocal reading would need restating at the extremes.
- **Whether `a · r(b)` is what the hardware does or what the compiler produced.** The probe measures
  the emitted module's answer; `NoContraction` forbids contraction but the emitter is the same one on
  both vendors, and **whether the reciprocal arrives from a driver approximation or from an
  instruction the compiler chose was not separated.** `spirv-val`/`spirv-dis` were available and
  **no module shape changed**, so neither was run; that is the one instrument named by the brief and
  not used, and §4's claim is a claim about answers, not about the module.
- **Whether `nr2`'s edge survives other divisors.** §7 measured 256 lanes on one divisor grid on two
  vendors; a spelling that reaches 234 of 256 there may not reach it elsewhere.

## 11. What this slice deleted

**One probe, and it is gone.** `crates/lichen-compute/examples/div_probe.rs` — the scratch driver
that ran every §2 to §7 measurement, carrying the integer correctly-rounded reference, the
reciprocal-interval search, the five compensated spellings and the host-side `markstein` sweep — was
**deleted before this note's commit**. It held no production code and nothing else referenced it. **No
example file was edited**, no `output =` line moved, `git diff dev...HEAD -- examples/` is empty, and
**no production file and no test was touched** — the only file this slice adds is this note and the
index row beside it. The two devices' run logs were read off the console and are not kept.
