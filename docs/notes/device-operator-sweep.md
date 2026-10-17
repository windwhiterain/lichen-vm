# The operators' device legs, one example at a time

> Status: **current** — every verdict below is from an RTX 3060 Laptop GPU on this branch, off
> `dev@c666a372`, and each was taken with the same three steps: read the example, switch **only** the
> backend literal `"cpu"` → `"gpu"`, and compare against the identical program on `cpu`. The rules the
> runs meet are [device-leg-of-the-ladder](device-leg-of-the-ladder.md)'s and are **referenced, not
> restated**; the arithmetic is [floating-point](floating-point.md)'s and [llm-operators](llm-operators.md)'s.
> **Nothing in this note changed production code, and no test was added.**
> **The method is a program now**:
> `crates/lichen-compute/examples/operators_on_device.rs`, and
> [§ The sweep, as a program](#the-sweep-as-a-program) re-measures every example that
> dispatches a kernel at `dev@602480ed`.

[device-leg-of-the-ladder](device-leg-of-the-ladder.md) reached a device for the append, the row
maximum, the sampler, `topk` and the generation loop. It left open, in its own closing **Unknown**,
whether any operator it did *not* reach — `softmax`'s body, `attend`, `ln`, `sin`/`cos` inside a
ladder body — diverges. **That is this note's question**, one example per row, and the answer is
different per operator.

**Two corrections this measurement forces on the notes it follows, kept in their own words, and one
correction to the brief itself.**

- ~~"[device-leg-of-the-ladder](device-leg-of-the-ladder.md) §2: **Measured: every fragment that
  reached the device is bit-exact. There is no divergence to report**, and no tolerance was loosened
  to get there."~~ **False, and it was false from the first operator here.** `rope.lichen` reaches the
  device and **diverges by up to 3 ulp**. §2's own method does not explain the difference away: its
  seven rows are all `@loop` folds, `int2float`, one division and a seed, and a fold's order is the
  order both backends execute. §2's conclusion is kept as the record that opened the question — every
  fragment it measured was bit-exact — and it is **superseded for the ladder as a whole**: a
  transcendental body is where the two backends part company, and no arithmetic tolerance would have
  hidden that, because there is none.
- ~~"[device-leg-of-the-ladder](device-leg-of-the-ladder.md) §2's closing **Unknown**: *whether any
  operator of the ladder **not** reached here … diverges — none of them reached the device, and §8's
  five-ulp `sin`/`cos` result is a standalone measurement, not one of these programs.*"~~ **Answered,
  and the answer is split by body rather than uniform.** `sin`/`cos` **do** reach a device inside
  `rope.lichen` and the operator's answer moves — by **exactly** the amount
  [floating-point](floating-point.md) §8's standalone measurement found, and no more (§2 below). But
  `softmax`'s 111-instruction `exp` body **also** reaches a device, inside `softmax.lichen`, and **it
  does not move** — all four lanes bit for bit, beside a float division (§4 below). **So §8's
  five-ulp `sin` is not a curiosity about a printed value**: it survives a composition that multiplies
  it by two and subtracts, and reaches an operator's own answer. §8's "sampled, not swept" caveat is
  unchanged and still applies to everything it bounds, and §4's zero is a sample of four lanes rather
  than a bound — the two rows together are the reason a per-operator example cannot rank them.

## 1. Verdict per operator

| operator | example | device verdict | worst ulp | `spirv-val` |
|---|---|---|---|---|
| `rope` — the rotation, built on `sin`/`cos` | `rope.lichen` | **runs, diverges** | **3** (of 8 elements, 5 differ) | 3 modules, **3 ACCEPT** |
| `silu` — SwiGLU's gate, an `exp` body | `silu.lichen` | **runs, diverges** | **1** (of 4 elements, 1 differs) | 2 modules, **2 ACCEPT** |
| `softmax` — the row softmax | `softmax.lichen` | **runs, bit-exact** | **0** (of 4 elements) | 2 modules, **2 ACCEPT** |
| `rmsnorm` — a Newton `sqrt` and a division | `rmsnorm.lichen` | **runs, bit-exact** | **0** (of 4 elements) | 2 modules, **2 ACCEPT** |
| `layernorm` — a diamond of two folds and a Newton `sqrt` | `layernorm.lichen` | **runs, bit-exact** | **0** (of 4 elements) | 3 modules, **3 ACCEPT** |
| attention — softmax composed with a gemv | **`attention.lichen`** | **runs, diverges** | **1** (of 8 elements, 6 differ) | 2 modules, **2 ACCEPT** |
| `gemv` | `gemv.lichen` | **refused by name** — the **known** rule | — | 1 module, **1 ACCEPT** |
| `matmul` | `matmul.lichen` | **runs, bit-exact** | **0** (of 4 elements) | 1 module, **1 ACCEPT** |

> **Every row of that table was measured on a device with no `NoContraction`, and two of them
> have since moved. [operator-reverification](operator-reverification.md) §2 re-ran each of these
> eight examples through the shipped emitter on two devices. What changed is recorded in that
> note; what did not is recorded here, because the rows below are the record of how each figure
> was first reached and their histories are worth keeping.**
>
> | operator | here, undecorated (RTX 3060) | re-measured, decorated (RTX 3060 / Iris Xe) |
> |---|---|---|
> | `rope` | 5 of 8 differ, worst **3** | **2 of 8 / 1 of 8**, worst **1** |
> | `silu` | 1 of 4 differ, worst **1** | **1 of 4 / 0 of 4**, worst **1** |
> | `attention` | 6 of 8 differ, worst **1** | **6 of 8 / 6 of 8**, worst **1** — **unmoved** |
> | `softmax` | 0 | **0 / 0** — unmoved |
> | `rmsnorm` | 0 | **0 / 0** — unmoved |
> | `layernorm` | 0 | **0 / 0** — unmoved |
> | `matmul` | 0 | **0 / 0** — unmoved |
> | `gemv` | refused by name | **still refused**, and **4 of 4 exact on both** once the short vector is padded |
>
> **Only `rope` moved, and the rows that held are the informative ones.** `attention`'s six
> differing elements are all **1 ulp** and all sit past a division by a denominator that is a sum
> of `exp` over the keys rather than a power of two — [floating-point](floating-point.md) §2.1's
> rule — and `NoContraction` does not reach a division. §3's `silu` element is the same shape: §3
> itself named the split as *"the `exp (0.0 − g)` **and** the division by `1 + exp (0.0 − g)`"*,
> and it is the division half that is still there. §2's `rope` element is the one row where the
> transcendental alone was the whole story, and it moved.

The evidence, quoted whole, is [§2](#2-rope--the-transcendental-in-an-operators-own-answer) through
[§9](#9-matmul--the-same-family-with-a-long-enough-input). **A third correction, and it is a path
correction: the brief's `examples/attend.lichen` does not exist in this repository.** The operator is
`attend` and the file is `examples/attention.lichen`; the run is that file's, and the note says so
rather than quietly renaming the row. Every other path in the brief resolves as written.

**The refusal is the only one measured, and it is the rule already documented.** `gemv`'s is
[device-leg-of-the-ladder](device-leg-of-the-ladder.md) §1's launch-count/input-length rule,
`InputShorterThanCount`, quoted verbatim in [§8](#8-gemv--refused-by-name-on-the-known-rule) — **not a
new rule**. The other seven operators reach a device on the backend literal alone, every input of
every kernel exactly as long as the launch covers.

## 2. `rope` — the transcendental in an operator's own answer

**Measured: it runs, and it diverges.** The example declares

```
[1.0, 2.0, -1.744977, 4.685622, -7.536518, 2.0496058, -8.0589075, -6.9321]: array{Float, 8}
```

`cpu` answers exactly that, and `gpu` answers

```
[1.0, 2.0, -1.7449768, 4.685622, -7.536519, 2.0496066, -8.058908, -6.9320993]: array{Float, 8}
```

**Measured: the distance per element, in ulp, from the two backends' own bits** — not a summary:

| element | `cpu` bits | `gpu` bits | ulp | `cpu` | `gpu` |
|---|---|---|---|---|---|
| 0 | `3f800000` | `3f800000` | 0 | `1.0` | `1.0` |
| 1 | `40000000` | `40000000` | 0 | `2.0` | `2.0` |
| 2 | `bfdf5b68` | `bfdf5b66` | **2** | `-1.744977` | `-1.7449768` |
| 3 | `4095f09e` | `4095f09e` | 0 | `4.685622` | `4.685622` |
| 4 | `c0f12b28` | `c0f12b2a` | **2** | `-7.536518` | `-7.536519` |
| 5 | `40032cbe` | `40032cc1` | **3** | `2.0496058` | `2.0496066` |
| 6 | `c100f149` | `c100f14a` | **1** | `-8.0589075` | `-8.058908` |
| 7 | `c0ddd3c3` | `c0ddd3c2` | **1** | `-6.9321` | `-6.9320993` |

**Measured: elements 0 and 1 are exact on both, and that is the angle `θ = 0` alone.** The example's
own `doc` says why: `position` is `0, 1, 2, 3` and `inverse_frequency` is all ones, so `θ` is `0, 1, 2,
3`, every one an exact `f32`, and `cos 0 = 1` / `sin 0 = 0` exactly. The rotation's own body pairs
the lanes (`odd = j % 2`, `pair = (j - odd) / 2`), so elements 0 and 1 are pair 0 and elements 2–7
are pairs 1, 2 and 3 — **no pair at a nonzero angle is wholly bit-exact**, and **five of the eight
elements differ**. Element 3 is the one exact element outside pair 0: it is `x[3] · cos 2 + x[2] ·
sin 2`, whose `cpu` and `gpu` answers coincide even though both of its factors do not.

**Measured: the divergence is in the two transcendental kernels, and it does not grow through the
rotation.** The same program re-rooted on the intermediate buffers — the identical source with one
expression appended, so the root is a buffer rather than the rotation — reads:

| buffer | element | `cpu` bits | `gpu` bits | ulp |
|---|---|---|---|---|
| `cosines.c` | 0 (`θ=0`) | `3f800000` | `3f800000` | 0 |
| `cosines.c` | 1 (`θ=1`) | `3f0a5140` | `3f0a5141` | 1 |
| `cosines.c` | 2 (`θ=2`) | `bed51132` | `bed51131` | 1 |
| `cosines.c` | 3 (`θ=3`) | `bf7d7026` | `bf7d7026` | 0 |
| `sines.s` | 0 (`θ=0`) | `00000000` | `00000000` | 0 |
| `sines.s` | 1 (`θ=1`) | `3f576aa4` | `3f576aa4` | 0 |
| `sines.s` | 2 (`θ=2`) | `3f68c7b7` | `3f68c7b8` | 1 |
| `sines.s` | 3 (`θ=3`) | `3e1081c4` | `3e1081c9` | **5** |

**So: `sin 3` is 5 ulp on this device, and it is the rotation's worst *input*.** That is exactly
[floating-point](floating-point.md) §8's worst `sin` figure, at exactly the angle §8's test names
(`3.0`, over `[0.25, 1.0, 3.0, 4.0, 7.0, -1.5]`) — so **the operator reproduces the standalone
measurement rather than exceeding it**, and the composition *absorbs* part of it: the rotation's own
worst is **3** ulp, below the 5 of the sine it multiplies in.

> **Both halves of that are now zero, and the measurement that says so is
> [operator-reverification](operator-reverification.md).** `sin(3.0)` is **0 ulp** on both an RTX
> 3060 and an Intel Iris Xe through the shipped emitter, and the rotation's own worst is **1**.
> What this paragraph found — that the operator reproduces the standalone figure rather than
> exceeding it, and absorbs part of it — still holds; it is the size that moved, and it moved
> because [contraction-control](contraction-control.md) §9's `NoContraction` decoration is what the
> standalone figures were measuring the absence of. The paragraphs below are the record of how
> each of those figures was reached.

**Measured: `cos`'s standalone worst is not in this table.** §8's `cos` worst of 6 ulp is at `-1.5`;
`rope`'s angles are `0, 1, 2, 3`, all non-negative, so its worst `cos` is **1** ulp. The two numbers
are not comparable as "the operator is worse than the standalone"; the operator's *worst case over its
own angles* is 3, and its worst single body is the sine at 5.

**The answer to the question the brief asks — is the transcendental gap an operator problem or a
last-bit curiosity: an operator problem, bounded.** It reaches an operator's own answer, it is not
inflated by composing it (3 < 5), and it is confined to the low bits of every value that is not
exactly representable. A kernel that compares or thresholds on a `sin`/`cos` result near a decision
boundary is where this would bite; a rotation that feeds an accumulator does not care at 3 ulp.

**Inferred: the emitted body is what differs, not the inputs.** Both backends are handed identical
`f32` buffers (`position`, `inverse_frequency`, `x` are exact), and the kernels are pure in their
inputs, so the split is inside the spliced `sin`/`cos` body. That is [floating-point](floating-point.md)
§8's own localisation, unchanged by this measurement, and this note does not re-derive it.

**Measured: `spirv-val` accepts all three of the operator's modules.** The run registers three
fragments — `rotate` (3 inputs, 1 output, **369** words), and the `cos` and `sin` kernels (2 inputs,
1 output each, **594** and **610** words) — and

```
C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe --target-env vulkan1.1
```

returns success on **all three**. **So a module that would be rejected by the validator is not what
is happening here**: the device's answer is a worse number from a module the specification accepts,
not a malformed one.

**The kernel work this outcome implies, in one line each.**

- **A precision decision someone has to take, not a spelling.** The body is correct by
  `spirv-val`; the device executes it and lands up to 5 ulp from the definition. The decision is
  whether an operator built on `sin`/`cos` may carry that, and the answer is a stated tolerance
  beside [floating-point](floating-point.md) §4.4 (which already declares a kernel's float
  **division** unspecified and says nothing about the transcendentals).
- **No emitter fix.** The constant pool that [dispatch-crash](dispatch-crash.md) §7 repaired is the
  same one these three modules go through, and `spirv-val` accepts all three; the divergence is a
  *numerical* one and would need a different `sin`/`cos` algorithm (or an `OpExtInst` import, which
  §7's `floor` decision already rejected on cross-target grounds) to move.
- **Nothing for the launch-count/input-length rule.** Every input of every kernel here is exactly as
  long as the launch covers, so [device-leg-of-the-ladder](device-leg-of-the-ladder.md) §1's upload
  rule is satisfied and never fires — **this operator is not a candidate for that spelling.**

## 3. `silu` — one ulp, and it is already there

**Measured: it runs, and it diverges by one ulp on one element of four.** The example declares

```
[0.0, 0.73105794, -0.13447031, 7.0463777]: array{Float, 4}
```

`cpu` answers exactly that, and `gpu` answers

```
[0.0, 0.73105794, -0.13447031, 7.046378]: array{Float, 4}
```

**Measured: the distance per element, from the two backends' own bits:**

| element | `cpu` bits | `gpu` bits | ulp | `cpu` | `gpu` |
|---|---|---|---|---|---|
| 0 | `00000000` | `00000000` | 0 | `0.0` | `0.0` |
| 1 | `3f3b269d` | `3f3b269d` | 0 | `0.73105794` | `0.73105794` |
| 2 | `be09b296` | `be09b296` | 0 | `-0.13447031` | `-0.13447031` |
| 3 | `40e17bed` | `40e17bee` | **1** | `7.0463777` | `7.046378` |

**Measured: the whole divergence is in the first kernel; the second carries it, it does not make
it.** The operator is two kernels — `silu_raw`, then a plain `a * u` — and re-rooted on the first
one's output buffer:

| | element 0 | element 1 | element 2 | element 3 |
|---|---|---|---|---|
| `cpu` `act.a` | `00000000` | `3f3b269d` | `be89b296` | `3fe17bed` |
| `gpu` `act.a` | `00000000` | `3f3b269d` | `be89b296` | `3fe17bee` |

The one-ulp difference is already in `act.a`. The second kernel multiplies it by `u[3] = 4.0`, an
exact power of two, and the exponent `23` and the ulp both survive — **so the split is `exp (0.0 - g)`
and the division by `1 + exp (0.0 - g)`, and not the product.** Element 0 is `0 / (1 + exp 0) = 0`
exactly, which is why it is the one element whose body is short enough to be exact, and the example's
own `doc` says so.

**Measured: `spirv-val` accepts both modules** — `silu_raw` (1 input, 1 output, **442** words) and
the product (2 inputs, 1 output, **245** words). **ACCEPT, both.**

**The kernel work this outcome implies.**

- **The same precision decision `rope` needs, and this measurement does not make it smaller.**
  One ulp is the smallest distance the class has; an operator whose gate feeds a `gemm` is not
  perturbed by it, and an operator that **thresholds** on a gate would be.
- **A decomposition probe nobody has run: which of the two candidates moved it.** `silu_raw` holds
  one `exp` (the 111-instruction Taylor sum of [floating-point](floating-point.md) §6) **and** one
  float `Div`, and [floating-point](floating-point.md) §7 measures this device's division as
  genuinely not correctly rounded. **This note cannot separate them** — the two are fused in one
  kernel and only one buffer is observable — so naming either is an argument, not a measurement.
  A per-instruction probe would, and is the next thing to write.

## 4. `softmax` — the exponential and the division agree here

**Measured: it runs, and it is bit-exact.** The example declares

```
[0.03205853, 0.08714417, 0.23688345, 0.6439139]: array{Float, 4}
```

`cpu` answers exactly that and `gpu` answers **the same string**, so the four answers are the same
four values:

| element | `cpu` bits | `gpu` bits | ulp |
|---|---|---|---|
| 0 | `3d034fce` | `3d034fce` | 0 |
| 1 | `3db278a5` | `3db278a5` | 0 |
| 2 | `3e729193` | `3e729193` | 0 |
| 3 | `3f24d78b` | `3f24d78b` | 0 |

**Measured: `spirv-val` accepts both modules** — `rowmax_raw` (1 input, 1 output, **336** words) and
`softmax_raw` (2 inputs, 1 output, **726** words). **ACCEPT, both.**

**This is the operator [device-leg-of-the-ladder](device-leg-of-the-ladder.md) §2's closing
**Unknown** named by name**, and it answers the other way from `rope`: the 111-instruction `exp`
expansion **does** reach a device here, inside a real operator, and it **does not move**.

**Measured, and this is the reading to keep: four elements are a sample, not a sweep.**
`softmax_raw` holds **both** of the hazards this device is known for — the `exp` expansion and one
float `Div` (`here / total`) whose divisor is neither a power of two nor one — and it answered all
four lanes bit for bit. That is the same shape of statement [floating-point](floating-point.md) §7
made about `ln`: *"six arguments are a sample, not a sweep"*. **So this row is not an exemption of
either hazard**; it is four lanes of one row `[1, 2, 3, 4]`, and `silu`'s row above moved by one
ulp on the same device, on the same arithmetic family.

**The kernel work this outcome implies.**

- **Nothing to repair.** No spelling is missing, no capability is absent, and no precision decision
  is forced by *this* row. What it does is **weaken a claim**, not make one.
- **A sweep is the work, and it is not this slice's.** The honest reading of `rope` (up to 3 ulp, from
  a 5-ulp `sin`) beside this one (0 ulp, from an `exp` and a `Div`) is that **per-operator examples
  cannot rank the operators' accuracy** — each measures the four lanes it happens to spell. A
  measurement that *could* rank them sweeps arguments through one body, which is the shape
  [floating-point](floating-point.md) §7's and §8's tables already have for the standalone
  definitions.

## 5. `rmsnorm` — a Newton `sqrt` and a division, and both divisors are exact

**Measured: it runs, and it is bit-exact.** The example declares

```
[3.5, 3.0, 2.0, 8.0]: array{Float, 4}
```

`cpu` answers exactly that and `gpu` answers **the same string**, so all four answers are the same
four values:

| element | `cpu` bits | `gpu` bits | ulp |
|---|---|---|---|
| 0 | `40600000` | `40600000` | 0 |
| 1 | `40400000` | `40400000` | 0 |
| 2 | `40000000` | `40000000` | 0 |
| 3 | `41000000` | `41000000` | 0 |

The reduction's own buffer is exact too: re-rooted on `sums.s` it answers `[63.0, 63.0, 63.0, 63.0]`
on both, bits `427c0000` four times.

**Measured: `spirv-val` accepts both modules** — `rescale_raw`, which holds the Newton `sqrt` (3
inputs, 1 output, **1658** words) and `sumsq_raw` (1 input, 1 output, **337** words). **ACCEPT, both.**

**Measured, and this is the reading these two rows have in common: the example chose its divisors.**
`rescale_raw`'s body is `sqrt (s / int2float k.n + 0.25)` then `x / d`. The row is `7, 3, 1, 2`, so
`Σx² = 63`, `/4` is a power-of-two division, `+ 0.25` gives `16`, and `sqrt 16` is `4` exactly — **so
both divisions divide by an exactly representable number.** The example's own `doc` says exactly this
(*"`sqrt` answers `4`, and every division below divides by it exactly"*), and
[floating-point](floating-point.md) §7 measured this device's `/4` as the one divisor of the three it
swept where **none** of 256 quotients differ.

**So this row does not exonerate the Newton `sqrt` on this device, and must not be read as doing so.**
§7's own `sqrt` column is the counter-reading: over one sample set on this same device, **21 of 64
elements differ by one ulp**, while `floor`'s column — which divides nowhere — is identical on all
64. **The Newton iteration moves here; this example's input is one where it does not.** §7's `ln`
paragraph already named the shape of the mistake — *"six arguments are a sample, not a sweep"* — and
this is it again with a denominator chosen rather than swept.

**The kernel work this outcome implies.**

- **Nothing to repair, and nothing decided.** No spelling is missing and no capability is absent.
- **The precision decision §2 asked for is unchanged, and this row is evidence about its *size*, not
  about whether to take it**: on the inputs these examples spell, the normalisers cost 0 ulp and the
  rotation costs 3. **The tolerance a kernel author may rely on is a per-input property, not a per-
  operator one** — which is the argument for stating a tolerance beside
  [floating-point](floating-point.md) §4.4 rather than per operator.

## 6. `layernorm` — the same arithmetic, and a second route

**Measured: it runs, and it is bit-exact.** The example declares

```
[-3.0, -2.0, 1.0, 6.0]: array{Float, 4}
```

`cpu` answers exactly that and `gpu` answers **the same string**:

| element | `cpu` bits | `gpu` bits | ulp |
|---|---|---|---|
| 0 | `c0400000` | `c0400000` | 0 |
| 1 | `c0000000` | `c0000000` | 0 |
| 2 | `3f800000` | `3f800000` | 0 |
| 3 | `40c00000` | `40c00000` | 0 |

Both folds are exact as well: re-rooted, `sm.s` answers `[10.0, 10.0, 10.0, 10.0]` (bits `41200000`)
and `sq.q` `[30.0, 30.0, 30.0, 30.0]` (bits `41f00000`) on **both** backends.

**Measured: `spirv-val` accepts all three modules** — the two folds (1 input, 1 output, **316** and
**337** words) and `normalise_raw`, which holds the two divisions, `m * m` and the Newton `sqrt`
(4 inputs, 1 output, **1734** words). **ACCEPT, three.**

**Measured: this is a different route from the one the example records, and it agrees.**
`layernorm.lichen`'s own `doc` says *"The RTX 3060 answers `[-3.0, -2.0, 1.0, 6.0]` for the same three
kernels through `graphrun`"*. This row is the **direct `compute.plrun` leg with the backend literal
only** — three separate launches, no `compute.graph` and no `graphrun` — and it answers the same four
values element for element. So the example's recorded device claim is confirmed on a route it does
not itself spell, and the same exact-divisor reading applies: `count = int2float k.n` is `4.0` and
`d = sqrt (1.25 + 1.0) = 1.5` exactly, which the `doc` states as deliberate (*"chosen so the
denominator lands on an exact square"*).

**The kernel work this outcome implies.** As §5: nothing to repair, nothing decided, and the same
finding that a kernel's tolerance is a property of its inputs. **The one thing this row adds** is
that the operator's *shape* — a three-kernel diamond, two independent producers feeding one consumer —
costs nothing on a device. The diamond is a scheduling concern, and both backends schedule it the same
way.

## 7. attention — softmax composed with a gemv

**Measured: it runs, and it diverges by one ulp on six of eight elements.** The example declares

```
[2.5101643, 3.5101643, 4.5101643, 5.5101643, 3.924232, 4.924232, 5.9242315, 6.924232]: array{Float, 8}
```

`cpu` answers exactly that, and `gpu` answers

```
[2.510164, 3.510164, 4.510164, 5.510164, 3.924232, 4.9242315, 5.9242315, 6.9242315]: array{Float, 8}
```

**Measured: the distance per element, from the two backends' own bits:**

| element | `cpu` bits | `gpu` bits | ulp | `cpu` | `gpu` |
|---|---|---|---|---|---|
| 0 | `4020a688` | `4020a687` | **1** | `2.5101643` | `2.510164` |
| 1 | `4060a688` | `4060a687` | **1** | `3.5101643` | `3.510164` |
| 2 | `40905344` | `40905343` | **1** | `4.5101643` | `4.510164` |
| 3 | `40b05344` | `40b05343` | **1** | `5.5101643` | `5.510164` |
| 4 | `407b269e` | `407b269e` | 0 | `3.924232` | `3.924232` |
| 5 | `409d934f` | `409d934e` | **1** | `4.924232` | `4.9242315` |
| 6 | `40bd934e` | `40bd934e` | 0 | `5.9242315` | `5.9242315` |
| 7 | `40dd934f` | `40dd934e` | **1** | `6.924232` | `6.9242315` |

**Measured: the whole divergence is in the second kernel.** Re-rooted on the score buffer, the two
backends agree bit for bit — `[1.0, 0.5, 0.0, 1.0]`, bits `3f800000`, `3f000000`, `00000000`,
`3f800000` on both — so `scores_raw` is exact and `attend_raw` (the `@loop` that accumulates
`exp (s - m) * v` and divides by the denominator on the way out) carries all six differing elements.

**This is the row that makes §4's reading sharper rather than softer.** `softmax.lichen` holds the same
`exp` expansion and a float division and answered all four lanes bit for bit; here, **composed with a
gemv, the same family moves by one ulp on six of eight**. The two differences between the programs are
that the divisor here is a sum of `exp` over the keys rather than a row total, and that the `exp` is
inside an accumulating fold. **So `softmax`'s zero and this one's one are two samples of one
arithmetic family, which is the argument §4 made** — and it is now a measurement on both sides of it
rather than an argument on one.

**Measured: `spirv-val` accepts both modules** — **1785** words (2 inputs, 1 output) and **1052**
words (2 inputs, 1 output). **ACCEPT, both.** So the one-ulp gap here is again a numerical gap in a
module the specification accepts.

**The kernel work this outcome implies.**

- **A composition rule someone has to state, not an emitter fix.** One ulp entering an accumulation
  and coming out through a division is arithmetic a kernel author must budget for; what is missing is
  a stated tolerance for "softmax composed with a gemv", not a body that miscompiles.
- **The decomposition probe, again, and now it has a second case.** §3 could not say whether `silu`'s
  one ulp was the `exp` or the `Div`; here `attend_raw` holds **both plus a fold**, and no buffer
  exposes them separately. A single per-instruction probe would serve both rows.

## 8. `gemv` — refused by name, on the known rule

**Measured: it is refused by name**, and the refusal, verbatim:

```
compute.parallel: the "gpu" backend declined this run: input buffer 1 holds 3
element(s) but the run covers 4 index/indices, so a lane would read past it.
```

`cpu` answers `[14.0, 32.0, 50.0, 68.0]: array{Float, 4}` and answers it exactly as declared.

**This is the KNOWN rule, not a new one.**
[device-leg-of-the-ladder](device-leg-of-the-ladder.md) §1's launch-count/input-length rule,
`InputShorterThanCount` — the same sentence that refused the KV-cache append and the scoring row, and
`examples/gemv.lichen`'s **own `doc` already records this sentence verbatim**. **This measurement
reproduces it exactly** and adds one thing the `doc` does not carry.

**Measured: the module is not what refuses.** The run registers **one** fragment — 2 inputs, 1 output,
**378** words — and `spirv-val` answers **ACCEPT**. **So the emitter produces a valid module and the
host-side upload check (`staged_bytes`,
`crates/lichen-compute-gpu/src/dispatch.rs:965-987`) refuses the run before it reaches a pipeline.**
A module that emits *and would be rejected by the validator* is the thing this slice was told to look
for; this is the other shape, and the distinction is worth having in the same breath.

**Measured: the spelling the refusal implies works, and it is inert.** Padding the short vector from
`[1.0, 2.0, 3.0] : array{Float, 3}` to `[1.0, 2.0, 3.0, 0.0] : array{Float, 4}` — **one buffer's width,
nothing else in the program touched** — makes the same body run on the device and answer

```
[14.0, 32.0, 50.0, 68.0]: array{Float, 4}
```

on **both** backends, byte-equal. The fourth element is never read (§5 of
[device-leg-of-the-ladder](device-leg-of-the-ladder.md) is the same argument, measured), so this is one
buffer's width rather than a second program.

**Measured, and this is why the two members of the gemm family come out differently.** `gemv`'s
reduction vector is a **whole buffer** of length `kdim = 3` beside a launch of `M = 4`, so `kdim < M`
and the input is short. `matmul`'s `k` is `3` beside `n = 4` too — **the same `k < n`** — but neither
of its inputs is short, because `a` and `b` are the **full matrices** (6 elements each), not one
reduction vector. **The rule is about buffer length, not about `k`**, and that is why §9 runs and this
section refuses.

**The kernel work this outcome implies.**

- **The spelling that must be written, and it is known: say the input is short and the body reads its
  first `kdim`.** That is `device-leg-of-the-ladder` §1's own reading — *"what is missing is a way to
  say 'this input is short and the body only reads its first `dim`'"* — and it is **not written here**;
  padding an example's buffer is a measurement, not a capability. Every decode-step GEMV meets it, so
  the spelling is worth more than any one example.
- **A capability that is not missing.** The host has two working answers already (pad, or launch over
  what the buffer holds), and `device-leg` §5 measured the second one and found it **silently wrong**
  rather than refused — `wontfix:D17`'s every-position-written rule. **Padding is the spelling to
  take**, for the reason that note gives.
- **No precision decision and no emitter change** for this row: the module is `spirv-val` **ACCEPT**
  and never runs.

## 9. `matmul` — the same family, with a long enough input

**Measured: it runs, and it is bit-exact.** The example declares

```
[58.0, 64.0, 139.0, 154.0]: array{Float, 4}
```

`cpu` answers exactly that and `gpu` answers **the same string**:

| element | `cpu` bits | `gpu` bits | ulp |
|---|---|---|---|
| 0 | `42680000` | `42680000` | 0 |
| 1 | `42800000` | `42800000` | 0 |
| 2 | `430b0000` | `430b0000` | 0 |
| 3 | `431a0000` | `431a0000` | 0 |

**Measured: `spirv-val` accepts its one module** — 2 inputs, 1 output, **417** words. **ACCEPT.** So
the refusal in §8 is not a property of the gemm family's emitted code: **the module that refuses to be
*run* is accepted by the validator, and so is the module that runs.**

**The kernel work this outcome implies.**

- **Nothing to repair.** The only difference between this row and §8 is a buffer length.
- **This row is what makes §8's implication concrete rather than alarming**: the same operator family,
  the same emitter, the same validator verdict — one refuses on a host-side length check and the other
  runs bit-exact. **A GEMV with `kdim ≥ M` needs no work at all**; only the shape `kdim < M`, which
  is every decode-step use, needs the spelling §8 names.

## How each run was driven

**Measured, so the next reader can repeat it.** The CLI installs no backend
([llm-operators](llm-operators.md)), so each run was driven by a scratch Rust example under
`crates/lichen-compute/examples/` — **deleted before the commit**, and named in each commit that
added it. It:

1. reads the example from `examples/` and normalises `\r\n` to `\n` (`lichen_tools::readme`'s own
   `read_normalized`), so the checkout's line endings cannot reach the program's meaning;
2. prints the `output =` the `---…---` block **declares** (`readme::declared_output`);
3. runs the source **unchanged** — this is the `cpu` column, the control;
4. calls `lichen_compute_gpu::install_default()` and runs the source with the **only** edit
   `source.replace("\"cpu\"", "\"gpu\"")`, printing each refusal verbatim as the run reports it;
5. walks the collected root and prints **every float's `to_bits()`**, so the ulp figures are read off
   the two backends' own bit patterns rather than off the rendered decimal;
6. reads `lichen_runtime::kernel_registry::kernels()` after the run, rebuilds each kernel's relative
   launch set (the root plus every kernel it transitively cross-calls, BFS order), emits it with
   `lichen_compute_gpu::spirv::compile` at `Binding { inputs: fragment.inputs, outputs: fragment.outputs }`,
   and pipes the words to `C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe --target-env vulkan1.1 -`.

**Two further probe variants, both additive and both named where they are used.** A *re-rooted* run
appends **one** expression to the source, so the root is an intermediate buffer rather than the
program's own output — it reads an existing binding and changes nothing above it, and it is how §2 and
§3 attribute a divergence to one of an operator's kernels. A *replaced* run substitutes one literal for
another — it is how §8 measures the repair a refusal implies. **Neither edits a file**: the examples in
`examples/` are byte-for-byte as `dev` has them.

**Each operator was run in its own process**, because the registry the run fills and
`install_default()`'s process-wide backend slot are both process-global, and the device takes one
process at a time.

**No example file was edited.** The examples in `examples/` are byte-for-byte as `dev` has them:
`git diff dev...HEAD -- examples/` is empty, and no `output =` line moved. The `"cpu"` → `"gpu"` switch
is made in memory, in the probe, and never written back.

## The sweep, as a program

The method above was a scratch program **deleted before its commit**, which is why its readings could not
be retaken without rewriting it. It is committed now:
`crates/lichen-compute/examples/operators_on_device.rs`.

```
cargo run -p lichen-compute --example operators_on_device
```

It keeps the two rules the prose above states. **One example per process**: the kernel registry and the
installed backend are process-global, so the parent re-executes itself once per program (`--one <path>`)
and never installs a device in its own process. **One edit per run**: the child runs the example unchanged
as the `cpu` control and then the same source with only `"cpu"` replaced by `"gpu"`.

The comparison is on the **rendered** answers, and that is exact for a finite float: the printer is
`f32`'s own shortest round-trip (`crates/lichen-render/src/render.rs:237-256`, stated in
[floating-point](floating-point.md)), so two equal renderings are equal bits. When they are not, the tool
prints the per-element distance in ulp, read off the two renderings.

Re-measured at `dev@602480ed` on the RTX 3060, every example that dispatches a kernel:

| example | device verdict |
|---|---|
| `attention.lichen` | **differs**, 6 of 8, worst **1** |
| `decode.lichen` | **refused** — *input buffer 2 holds 2 element(s) but the run covers 8 index/indices* |
| `decode_loop.lichen` | **refused** — the same sentence |
| `gemv.lichen` | **refused** — *input buffer 1 holds 3 element(s) but the run covers 4* |
| `generate.lichen` | **agrees**, 10 of 10 |
| `kvcache.lichen` | **refused** — the append's 2-of-8 sentence |
| `layernorm.lichen` | **agrees**, 4 of 4 |
| `matmul.lichen` | **agrees**, 4 of 4 |
| `rmsnorm.lichen` | **agrees**, 4 of 4 |
| `rope.lichen` | **differs**, 2 of 8, worst **1** |
| `sample.lichen` | **agrees**, 5 of 5 |
| `silu.lichen` | **differs**, 1 of 4, worst **1** |
| `softmax.lichen` | **agrees**, 4 of 4 |
| `topk.lichen` | **agrees**, 30 of 30 |

**It reproduces [operator-reverification](operator-reverification.md) §2's after-table exactly.** The
eight operators that note re-ran read the same verdicts and the same element counts here — `rope` 2 of 8,
`silu` 1 of 4, `attention` 6 of 8, all at **1 ulp**; `softmax`, `rmsnorm`, `layernorm` and `matmul`
bit-exact; `gemv` refused — now off one command instead of a deleted program. That agreement is the tool's
own control: a re-run that disagreed with it would mean the tool is wrong, not the note.

**The six programs §1's table does not reach read the way
[device-leg-of-the-ladder](device-leg-of-the-ladder.md) recorded them.** `generate`, `sample` and `topk`
agree, and the three programs whose **first** kernel appends a short incoming row (`decode`,
`decode_loop`, `kvcache`) are refused by the same `InputShorterThanCount` sentence, on that first kernel
rather than on a later stage.

**What the program does not do, so it is not read as covering more than it does.** It does not emit the
modules, so §1's `spirv-val` column remains [operator-reverification](operator-reverification.md)'s and is
not re-derived here; and its ulp distance is read off the renderings, which cannot separate two `NaN`s
that render alike.

## What was measured, inferred and unknown

**Measured.** Every row of §1's table, and for each: the `output =` the example **declares**, the
**`cpu`** answer, the **`gpu`** answer or the **verbatim refusal**, and the per-element ulp distance
read off both backends' own bit patterns where they differ. That is, in full:

- **`rope`** — declared, `cpu` and `gpu` strings; 8 of 8 bit pairs; the per-element ulp table; the 8
  bit pairs each for `cosines.c` and `sines.s`; 3 fragments at **369**, **594** and **610** words;
  **3 `spirv-val` verdicts, 3 ACCEPT**.
- **`silu`** — declared, `cpu` and `gpu` strings; 4 of 4 bit pairs; the same for `act.a`; 2 fragments
  at **442** and **245** words; **2 verdicts, 2 ACCEPT**.
- **`softmax`** — declared, `cpu` and `gpu` strings, byte-equal; 4 of 4 bit pairs; 2 fragments at
  **336** and **726** words; **2 verdicts, 2 ACCEPT**.
- **`rmsnorm`** — declared, `cpu` and `gpu` strings, byte-equal; 4 of 4 bit pairs; the same for
  `sums.s`; 2 fragments at **1658** and **337** words; **2 verdicts, 2 ACCEPT**.
- **`layernorm`** — declared, `cpu` and `gpu` strings, byte-equal; 4 of 4 bit pairs; the same for
  `sm.s` and `sq.q`; 3 fragments at **316**, **337** and **1734** words; **3 verdicts, 3 ACCEPT**.
- **`attention`** — declared, `cpu` and `gpu` strings; 8 of 8 bit pairs; the same for `scale.s`;
  2 fragments at **1785** and **1052** words; **2 verdicts, 2 ACCEPT**.
- **`gemv`** — the refusal **verbatim**; the `cpu` string; the `cpu` string of the padded variant
  (byte-equal with the device's); 1 fragment at **378** words; **1 verdict, 1 ACCEPT** — for a module
  that **never ran**.
- **`matmul`** — declared, `cpu` and `gpu` strings, byte-equal; 4 of 4 bit pairs; 1 fragment at
  **417** words; **1 verdict, 1 ACCEPT**.

**Measured, on this device, and it is a fact about the tree rather than about these programs:**
[floating-point](floating-point.md) §7 already records that this device's Newton `sqrt` moves (21 of
64 elements of one sample set differ by 1 ulp) while `floor`'s column is identical on all 64. §5 and
§6 are read against that: **the two normalisers are exact because their divisors are exact, not
because the `sqrt` is exact here.** Nothing in this note re-measured §7.

**Inferred.** That `rope`'s split is inside the emitted `sin`/`cos` body rather than in the inputs,
from the inputs being bit-identical across the two runs and the kernels being pure — not re-derived
here. That §7's attention row's divergence is arithmetic rather than scheduling, from `scores_raw`
being bit-exact and the fold+division kernel carrying all six differing elements.

**Unknown.** *What the operation is* inside the transcendental body:
[floating-point](floating-point.md) §8 already records that it is not FMA contraction, not a
division (these two definitions contain none), not a race, and not the emitter. **Still unknown, and
this note did not close any of these:**

- **Whether the transcendental gap changes a *decision* anywhere in the ladder.** No operator
  measured here compares against a `sin`/`cos` result near a boundary, so this slice can bound the
  *answer* and not the *consequence*. It is the question that would decide the tolerance in §2's first
  bullet.
- **Which instruction moved a given one-ulp divergence.** `silu_raw` holds an `exp` **and** a `Div`;
  attention's second kernel holds an `exp`, a `Div` **and** a fold; neither exposes them separately,
  and [floating-point](floating-point.md) §7's and §8's standalone sweeps are on *spliced bodies*, not
  on these operators. **A per-instruction probe is the one piece of work that would resolve both rows**,
  and nothing here substitutes for it.
- **Whether these rows are representative.** Eight examples, four to eight elements each, and every
  one of them an input a human chose. `rmsnorm` and `layernorm` are exact *because* their divisors
  were picked exact; `softmax` is exact on four lanes; `matmul`'s inputs are integers below `2^24`.
  **The gap between "bit-exact here" and "bit-exact in general" is exactly the gap §7's and §8's
  sweeps measured for the standalone definitions, and this note does not close it for an operator.**
- **Whether the brief's priority order was right.** It put division and transcendentals first
  *"because that is where the device is known to differ"*, and the measurements agree that it was: the
  two diverging bodies with a gap at all are the `sin`/`cos` one and the `exp`-inside-a-fold one.
  **What the order did not predict** is that the plain `exp` in `softmax.lichen`, and both
  normalisers, would be bit-exact — so it was right about where to look and wrong about how much
  would be found there. That is a statement about four-lane examples, and §7's reading applies to it.
