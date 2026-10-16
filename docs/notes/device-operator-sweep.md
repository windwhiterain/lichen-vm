# The operators' device legs, one example at a time

> Status: **current** — every verdict below is from an RTX 3060 Laptop GPU on this branch, off
> `dev@c666a372`, and each was taken with the same three steps: read the example, switch **only** the
> backend literal `"cpu"` → `"gpu"`, and compare against the identical program on `cpu`. The rules the
> runs meet are [device-leg-of-the-ladder](device-leg-of-the-ladder.md)'s and are **referenced, not
> restated**; the arithmetic is [floating-point](floating-point.md)'s and [llm-operators](llm-operators.md)'s.
> **Nothing in this note changed production code, and no test was added.**

[device-leg-of-the-ladder](device-leg-of-the-ladder.md) reached a device for the append, the row
maximum, the sampler, `topk` and the generation loop. It left open, in its own closing **Unknown**,
whether any operator it did *not* reach — `softmax`'s body, `attend`, `ln`, `sin`/`cos` inside a
ladder body — diverges. **That is this note's question**, one example per row, and the answer is
different per operator.

**Two corrections this measurement forces on the notes it follows, kept in their own words.**

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

The evidence, quoted whole, is [§2](#2-rope--the-transcendental-in-an-operators-own-answer),
[§3](#3-silu--one-ulp-and-it-is-already-there) and [§4](#4-softmax--the-exponential-and-the-division-agree-here). The
rows are added here as each operator is measured; the standing question each row answers is named in
its own section. **No refusal has appeared yet**: every operator measured so far reaches a device on
the backend literal alone, so [device-leg-of-the-ladder](device-leg-of-the-ladder.md) §1's upload
rule is satisfied by every input of every one of them.

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

**Each operator was run in its own process**, because the registry the run fills and
`install_default()`'s process-wide backend slot are both process-global, and the device takes one
process at a time.

**One example's edit was reverted after the run.** The example files themselves are **unchanged**:
`git diff dev...HEAD -- examples/` is empty, and no `output =` line moved. The `"cpu"` → `"gpu"`
switch is made in memory, in the probe, and never written back.

## What was measured, inferred and unknown

**Measured.** `rope.lichen`'s declared, `cpu` and `gpu` outputs, quoted whole; the eight per-element
ulp distances and both bit patterns; the eight per-element ulp distances for `cosines.c` and
`sines.s`; the three fragments' word counts and input/output counts; the three `spirv-val` verdicts;
and that all three kernels of the operator reached a device at all. `silu.lichen`'s declared, `cpu`
and `gpu` outputs; its four per-element ulp distances and both bit patterns; the same for `act.a`;
two word counts and two verdicts. `softmax.lichen`'s declared, `cpu` and `gpu` outputs — byte-equal,
so bit-exact by the injectivity of the printer — its four bit patterns, two word counts and two
verdicts.

**Inferred.** That the split is inside the emitted `sin`/`cos` body rather than in the inputs, from
the inputs being bit-identical across the two runs and the kernels being pure — not re-derived here.
For `silu`, that the whole divergence sits in `silu_raw` rather than the product, from the product
being exact and the `act.a` bits already differing — a measurement, not an inference, and stated as
one.

**Unknown.** *What the operation is* inside the body:
[floating-point](floating-point.md) §8 already records that it is not FMA contraction, not a
division (these two definitions contain none), not a race, and not the emitter. **Still unknown, and
this note did not close it**: whether the transcendental gap changes a *decision* anywhere in the
ladder (no operator in it compares against a `sin`/`cos` result near a boundary); **which of the two
candidates inside `silu_raw` moved its one ulp — the `exp` or the `Div`**, which a fused kernel does
not expose; and whether the operators below behave like `rope`, like `silu`, or like `softmax` —
measured per row as they land.
