# The ladder's device leg, measured

> Status: **current** — every verdict below is from an RTX 3060 Laptop GPU at `feature/dleg`, off
> `dev@0ec49dbf`. The operators themselves are [llm-operators](llm-operators.md)'s and their
> arithmetic is [floating-point](floating-point.md)'s; neither is restated here.
> **Nothing in this note changed production code, and no test was added.**

The ladder's verdicts were **host-only** because the CLI installs no backend
([llm-operators](llm-operators.md), `floating-point.md` §7). This note drives the same programs
through the installed `ParallelBackend` instead: each example is read from `examples/`, its
**only** edit is the backend literal `"cpu"` → `"gpu"`, and the run is compared against the
identical program on `cpu`.

**Which question each section answers.**

| section | question |
|---|---|
| [§1](#1-does-it-reach-a-device-at-all) | Q1 — does it reach a device, and if not, what refuses |
| [§2](#2-where-it-does-run-does-the-device-answer-what-cpu-answers) | Q2 — bit-exact, or how far apart |
| [§3](#3-the-randomness-disguised-as-arithmetic) | Q3 — the `((0.0 - 1.0) / 0.0)` seed |
| [§4](#4-the-stop-condition-on-a-device) | Q4 — is the sampled token available device-side |

**Verdict per operator.**

| operator | device verdict | evidence |
|---|---|---|
| KV-cache append (`kvcache.lichen`) | **refused by name**, `InputShorterThanCount` | *"input buffer 2 holds 2 element(s) but the run covers 8 index/indices"* |
| KV-cache append, incoming row padded to the count | **runs, bit-exact** | §2 |
| scoring row `score_raw` (`decode_loop.lichen:173`) | **refused by name**, `InputShorterThanCount` | *"input buffer 1 holds 2 element(s) but the run covers 4 index/indices"* |
| one decode step, chained (`decode_loop.lichen`) | **refused by name** at its *first* kernel | the append's sentence, verbatim |
| softmax's row maximum (`decode_loop.lichen:182`) | **runs, bit-exact** | `[3.0, 3.0, 3.0, 3.0]` |
| greedy sampler (`sample.lichen`, `topk.lichen`, `generate.lichen`) | **emits, then the driver takes the process down** | `STATUS_ACCESS_VIOLATION` after a 444-word module |
| top-k (`topk.lichen`) | **emits, then the driver takes the process down** — on fragment `#0`, the same sampler | as above |
| generation loop (`generate.lichen`) | **emits, then the driver takes the process down** — on fragment `#2`, the same sampler | as above |

**Of the four questions, the short answers are:** the append and the row maximum **run** and
agree **bit for bit**; the scoring row and the whole decode chain **refuse by name** on one rule;
the sampler **is not reachable on this device** — it lowers, it emits, and the driver kills the
process; and the **runtime stop condition is host-only**, because the only spelling that brings a
sampled token to a guard is a host `compute.collect`, and the loop that would read it never got a
device leg.

## 1. Does it reach a device at all?

**Measured.** Two outcomes, and neither is the ladder's arithmetic.

**A refusal by name — one rule, two operators.** The device's upload rule compares an input's
**length** against the **launch count**, not against the addresses the body reads. `kvcache.lichen`
launches 8 lanes and hands the kernel an incoming row `nk`/`nv` of **2** elements, because the body
reads it at `compute.row`, which ranges over a column:

```
error: compute.parallel: the "gpu" backend declined this run: input buffer 2 holds 2
element(s) but the run covers 8 index/indices, so a lane would read past it.
```

The scoring row is the same rule on the other side: `.n 4, .kdim 2`, so the query `x` is 2 long
and the launch covers 4:

```
error: compute.parallel: the "gpu" backend declined this run: input buffer 1 holds 2
element(s) but the run covers 4 index/indices, so a lane would read past it.
```

That **confirms** what `llm-operators.md` predicted for a lone GEMV. What it **corrects** is the
note's open hypothesis that *"a chained step may reach a device before a lone GEMV does"*: measured,
the chain's **first** kernel is the append, so `decode_loop.lichen` refuses on the append's
sentence and **never reaches the score stage at all**. Both halves of that sentence are now
measured, and the hypothesis is false.

**This is the spelling, not the capability.** Padding the incoming row to the count — changing
nothing but `nk`/`nv` from 2 elements to 8 — makes the same body run and agree, in §2. So the
append *is* dispatchable; what is missing is a way to say "this input is short and the body only
reads its first `dim`".

**A crash, not a refusal — the sampler.** `sample.lichen`, `topk.lichen` and `generate.lichen` all
exit `0xc0000005` (`STATUS_ACCESS_VIOLATION`) inside the `gpu` run. This is **not** a refusal by
name and not a wrong answer: there is no diagnostic at all. Bisected by measurement:

| fragment | emits | dispatches |
|---|---|---|
| `a + a`, one lane | 229 words | **runs** |
| `a / 4.0`, one lane | 222 words | **runs** |
| `((0.0 - 1.0) / 0.0)` written to every slot, no fold | 206 words | **runs** |
| `int2float lane`, no fold | 206 words | **runs** |
| `@loop` sum fold, two-slot state | 341 words | **runs** |
| `@loop` `max` fold, two-slot state, `-inf` seed (the ladder's own `rowmax_raw`) | 332 words | **runs** |
| `@loop` fold, **three**-slot state, `max`, `-inf` seed, `int2float` of the result | 392 words | **runs** |
| the same body with the seed made finite (`(0.0 - 1000.0) / 1.0`) | 448 words | **crashes** |
| **the committed `greedy_raw`**, `sample.lichen:141-146` | 444 words | **crashes** |

`spirv::compile` is the seam: **every one of these emits a module**, the greedy one among them, so
**the emitter is not at fault and the fragment is not refused**. The crash is at the **dispatch**,
and the single ingredient separating the last two rows from the row above is **a comparison whose
`0`/`1` answer is multiplied into arithmetic inside the fold's step** — `(1 * (read > s(1)))`.
Everything else the committed body does was removed one at a time and each removal ran.

**Unknown.** *Why* the driver dies. `spirv-val` is not installed, so nothing checks the module
before the driver sees it, and the 444 words were never validated. **This is a production defect
and it is not fixed here**: reaching the sampler on a device needs a source change in the emitter
or the lowering, which is a different slice and the user's decision.

## 2. Where it does run, does the device answer what `cpu` answers?

**Measured: every fragment that reached the device is bit-exact. There is no divergence to report,
and no tolerance was loosened to get there.**

| program | `cpu` | `gpu` | |
|---|---|---|---|
| KV-cache append, row padded to 8 | `([1.0, 2.0, 3.0, 4.0, 100.0, 200.0, 0.0, 0.0], [5.0, 6.0, 7.0, 8.0, 300.0, 400.0, 0.0, 0.0])` | the same | **bit-exact** |
| softmax's row maximum, `[1.0, 3.0, 1.0, 0.0]` | `[3.0, 3.0, 3.0, 3.0]` | the same | **bit-exact** |
| `@loop` sum fold over the sampler's own five rows | `[25.0, 13.0, 28.0, 20.0, 10.0]` | the same | **bit-exact** |
| `@loop` three-slot fold, no comparison | `[10.0, 10.0, 10.0, 10.0, 10.0]` | the same | **bit-exact** |
| `int2float lane` | `[0.0, 1.0, 2.0, 3.0]` | the same | **bit-exact** |
| `a / 4.0` | `[0.25, 0.5, 0.75, 1.0]` | the same | **bit-exact** |
| the seed alone | `-inf`, bits `0xff800000` | the same | **bit-exact** |

This **confirms** `llm-operators.md`'s claim that *"the direct device leg agrees with `cpu` element
for element once the incoming row is padded"*, which had only been inferred.

**Measured, and incidental: the renderer prints an `f32` infinity as a decimal.** The same seed
program renders as

```
[-680564693277057700000000000000000000000.0, ...]
```

on **both** backends, while reading the value past the renderer gives `-inf`, bits `0xff800000`.
So the number is right and the **printed** form is wrong. It is not a device finding and it is
**not fixed here**; it is reported because a note that quotes rendered output cannot be trusted at
an infinity.

## 3. The randomness disguised as arithmetic

**Measured: the seed does *not* survive lowering as a constant — it survives as a division.** A
census of the committed fragments, instruction by instruction:

```
sample.lichen#0:    34 instruction(s), 1 float Div, 0 integer Div, 1 Conv
sample.lichen#0:    finite float constants ["0.0", "1.0", "0.0", "0.0"]
sample.lichen#0:    non-finite float constants []
```

The tree is `Const(Float, 0.0)`, `Const(Float, 1.0)`, `Bin(Float, Sub)`, `Const(Float, 0.0)`,
**`Bin(Float, Div)`**. There is **no `0xff800000` anywhere in the fragment**. The same is true of
every seeded fragment in the ladder — `topk#0`, `decode_loop#2` (the row maximum), `decode_loop#3`
(softmax's 111-instruction `exp` expansion) and `generate#2` each carry one float `Div` and no
non-finite constant. The `-inf` folding of `79defe18` is a **`lichen-fusion` canonicalisation**
(`normalize.rs:97-111`), so it is what the *fusion* path sees; the *lowering* path hands the
division to the shader.

**Measured: the division still gives the right infinity on this device.** §7 records that this
device's `OpFDiv` is not correctly rounded — 85 of 256 quotients differ for `/3`, 139 for `/7`. A
division **by zero** is not one of those: `seed_alone` answers `-inf` with bits `0xff800000` on
both backends, bit for bit. So the hazard §7 measures does **not** reach this seed, and the reason
is structural rather than lucky — the divisor is exactly zero, where there is no correctly rounded
answer to miss.

**Measured: no comparison against it moves.** The row maximum carries the same `-inf` seed and
compares against it on every step, through `max`, and answers `[3.0, 3.0, 3.0, 3.0]` on the device
**bit for bit** with `cpu`. That is the one place in the ladder where a comparison against the
seed is actually exercised on a device, and it does not move.

**Unknown.** Whether the comparison inside the *sampler* moves. It is the fragment that crashes, so
the seed's one other use — the `>` that picks the position — was never executed on a device.

## 4. The stop condition on a device

**Measured.** The guard reads the token this way (`examples/generate.lichen:207`):

```
bit = (compute.collect tok.p : array{Float, 1})[0]
if bit == sentinel then (s(0) + 1, cached, scored) else generate (s(0) + 1, cached, scored)
```

`compute.collect` is the **only** operator in the compute namespace that brings a buffer element to
the host; `compute.read` takes a `compute.Buf` solely inside a kernel body. The namespace is
enumerated in full at `crates/lichen-compute/src/compute.rs:6905-6920` — fourteen names, and none
of them is a device-side predicate, a device-resident scalar read, or a conditional launch. **So
the value the guard needs is available device-side only as a host value fetched back**, and the
recursion that consumes it is itself a host value.

**Measured, and it never got that far.** `generate.lichen`'s device run dies on fragment `#2` —
the sampler — before the loop's first step completes, so **the stop condition was never exercised
on a device at all**.

**Inferred:** the runtime stop is **host-only** today. The basis is the enumeration above, which is
a source reading rather than an attempt, plus the fact that the one program that would have
exercised it never reached the device.

**Unknown:** whether a device-side stop is **capable**. Nothing in the surface expresses it, and
this slice did **not** hand-build a fragment to find out — so this is *"the spelling is missing"*,
not a measured absence of capability, and it should be read that way. The `WorkBudget` bound
(`llm-operators.md`: 500 unmarked, 2 000 marked) is host-side either way and was not re-measured.

## What was measured, inferred and unknown

**Measured.** Every verdict, value and refusal above; the fragment census; the bisect table; the
`0xc0000005` exit codes; the `-inf` bits; the renderer's decimal.

**Inferred.** That the runtime stop is host-only, from an exhaustive reading of the compute
namespace rather than an attempt.

**Unknown.** Why the driver dies on the sampler's module; whether a device-side stop condition is
capable; whether the sampler's `>` against the seed would move; whether any operator of the ladder
**not** reached here (`softmax`'s 111-instruction body, `attend`, `ln`, `sin`, `cos` in a ladder
body) diverges — none of them reached the device, and §8's five-ulp `sin`/`cos` result is a
standalone measurement, not one of these programs.
