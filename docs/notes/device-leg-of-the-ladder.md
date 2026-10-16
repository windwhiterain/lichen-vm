# The ladder's device leg, measured

> Status: **current** — every verdict below is from an RTX 3060 Laptop GPU at `feature/dleg`, off
> `dev@0ec49dbf`, except §5, which is from `feature/gdev` off `dev@74a46613` and corrects §1, §3
> and §4 on three points, each marked where it is corrected. The operators themselves are
> [llm-operators](llm-operators.md)'s and their arithmetic is [floating-point](floating-point.md)'s;
> neither is restated here.
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
| [§5](#5-the-generation-loop-reaches-a-device-after-all) | Q5 — what shape the generation loop's device run needs |

**Verdict per operator.**

| operator | device verdict | evidence |
|---|---|---|
| KV-cache append (`kvcache.lichen`) | **refused by name**, `InputShorterThanCount` | *"input buffer 2 holds 2 element(s) but the run covers 8 index/indices"* |
| KV-cache append, incoming row padded to the count | **runs, bit-exact** | §2 |
| scoring row `score_raw` (`decode_loop.lichen:173`) | **refused by name**, `InputShorterThanCount` | *"input buffer 1 holds 2 element(s) but the run covers 4 index/indices"* |
| one decode step, chained (`decode_loop.lichen`) | **refused by name** at its *first* kernel | the append's sentence, verbatim |
| softmax's row maximum (`decode_loop.lichen:182`) | **runs, bit-exact** | `[3.0, 3.0, 3.0, 3.0]` |
| greedy sampler (`sample.lichen`, `topk.lichen`, `generate.lichen`) | **fixed — all three now run, bit-exact** | [dispatch-crash](dispatch-crash.md) §7, and §5 for `generate` |
| top-k (`topk.lichen`) | **runs, bit-exact** on two 15-arrays | as above |
| generation loop (`generate.lichen`) | **runs, bit-exact** — its refusal was a *host* input beside a wider launch, not the sampler's own one-element pick | §5 |

**Of the four questions, the short answers are:** the append and the row maximum **run** and
agree **bit for bit**; the scoring row and the whole decode chain **refuse by name** on one rule;
**the sampler now runs and agrees bit for bit** — the crash it died on was the emitter's missing
constant, and [dispatch-crash](dispatch-crash.md) §7 has the repair and the measurement; and the
**runtime stop condition is host-only**, because the only spelling that brings a sampled token to a
guard is a host `compute.collect`. ~~The loop that would read it never got a device leg.~~
**Corrected by §5: it has one now.** The loop runs three steps on the device and answers the
declared line element for element, and §5 also corrects *which input* was refusing — the sampler's
own one-element pick never was.

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

**Corrected by §5, and it is a narrowing of this section's own rule: it sees the *host* inputs
only.** Every refusal above names a buffer the host holds. `staged_bytes`
(`crates/lichen-compute-gpu/src/dispatch.rs:965-987`) is the whole of the check and it is written
over `BatchInput::Host` alone — its own invariant says a device input stages nothing and has no
length to check — and `BatchInput`'s other two arms are `Resident`, a buffer the backend already
holds, and `Produced`, an earlier dispatch's output
(`crates/lichen-kernel-ir/src/lib.rs:308-316`, whose invariant says the host arm is the only one
uploaded as the batch is recorded). **So a launch output feeding the next launch is outside this
rule entirely**, whatever its length, and that is what §5 measures: a one-element sampler output
beside a five-index append is not what refuses.

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
the fragment is not refused. ~~**the emitter is not at fault and the fragment is not refused**. The
crash is at the **dispatch**,~~ and the single ingredient separating the last two rows from the
row above is **a comparison whose `0`/`1` answer is multiplied into arithmetic inside the fold's
step** — `(1 * (read > s(1)))`. Everything else the committed body does was removed one at a time
and each removal ran. **Corrected by
[the sampler's crash, localised to one emitted id](dispatch-crash.md): the emitter *is* at fault,
the crash is at `vkCreateComputePipelines` rather than at a dispatch of ours, and the single
ingredient is narrower than "a comparison multiplied into arithmetic" — it is a **boolean
materialised into the integer class inside a `@loop` in a float module**, which names a constant
the float arm of the emitter's pool never declares.** The bisect rows above are kept as the
record; that note has the reduction.

**Unknown.** *Why* the driver dies. ~~`spirv-val` is not installed, so nothing checks the module
before the driver sees it, and the 444 words were never validated.~~ **Corrected by
[the sampler's crash, localised to one emitted id](dispatch-crash.md): `spirv-val` *is* installed
(`C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe`), and it rejects the 444 words by name — the module
references `%10`, an id no instruction defines. The cause is the emitter, in
`crates/lichen-compute-gpu/src/spirv.rs`, and the crash is a driver's compile of a malformed
module.** That note carries the reduction and the fix site; this note's bisect above is kept as
the record that opened the question. **The defect is fixed by that note's §7**, which adds the one
missing `OpConstant` to the float arm of the emitter's constant pool, pins the shape with a
regression test, and measures `sample.lichen` and `topk.lichen` running **bit-exact** on the
device; `generate.lichen` now stops at this note's own upload rule instead of crashing.

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

**Measured by §5: the comparison inside the *sampler* does not move.** It was an open question here
because the fragment crashed, so the seed's one other use — the `>` that picks the position — was
never executed on a device. `examples/generate.lichen` runs `greedy_raw` inside a three-step loop
on the device and answers the declared sequence `2, 0, 3` and the declared last logit row, every
entry bit-identical to `cpu`, so the sampler's comparison against the seed is now executed on a
device **thirty times** across the three steps and does not move.

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

**Measured, and it never got that far.** ~~`generate.lichen`'s device run dies on fragment `#2` —
the sampler — before the loop's first step completes, so **the stop condition was never exercised
on a device at all**.~~ **Corrected by §5: the run does not die, it refuses, and once the refusal
is answered the stop condition *is* exercised on a device** — three times, once per step, on a
kernel-sampled token. What that changes is not the host-only reading below, which stands, but the
basis for it and the claim that nothing had reached that far.

**Inferred:** the runtime stop is **host-only** today. The basis is the enumeration above, which is
a source reading rather than an attempt, plus the fact that the one program that would have
exercised it never reached the device. **§5 exercises it, and the reading is unchanged**: the
guard is still a host `compute.collect`, so the device answers the arithmetic and the host decides
when to stop.

**Unknown:** whether a device-side stop is **capable**. Nothing in the surface expresses it, and
this slice did **not** hand-build a fragment to find out — so this is *"the spelling is missing"*,
not a measured absence of capability, and it should be read that way. The `WorkBudget` bound
(`llm-operators.md`: 500 unmarked, 2 000 marked) is host-side either way and was not re-measured.

## 5. The generation loop reaches a device after all

**Measured on `feature/gdev`, off `dev@74a46613`, on the RTX 3060 Laptop GPU.** The one program
this note called refused is now run on both backends, and the finding is not the one the refusal
was read as.

**What was refused was not the sampler's pick.** `examples/generate.lichen` feeds the KV-cache
append a key row it just sampled: `tok` is `compute.plrun greedy` and `.nk tok.p` names **that
launch's own output buffer**, one element long because the sampler runs `.n 1`. This note's
verdict table, and the sentence in `examples/generate.lichen` that recorded it, both read that one
element as the cause. **It is not.** `tok.p` is a `BatchInput::Produced` and §1's check is written
over `BatchInput::Host` alone, so the sampler's output is outside the rule at whatever length. The
input that refused is **`nv`**, the fourth field of `ArgK`'s `.I KVIn` and a top-level
`compute.buffer` the host builds. `examples/generate.lichen`'s own decision (3) had already
recorded the fact that made this harmless — with `dim = 1`, `column` is `0` at every lane, so `nv`
is read at index `0` alone — and the device's rule cannot see that.

| `nv` | `nk` | `cpu` | `gpu` |
|---|---|---|---|
| 1 element (as committed) | `tok.p`, 1 element, a launch output | `([1.0, 2.0, 2.0, 0.0, 3.0], 3, [-1.0, -4.0, -7.0, 3.0])` | **refused**: `input buffer 3 holds 1 element(s) but the run covers 5 index/indices` |
| **4** elements | `tok.p`, 1 element, a launch output | the same | **refused**: `input buffer 3 holds **4** element(s) but the run covers 5` |
| **5** elements | `tok.p`, 1 element, a launch output | the same | `([1.0, 2.0, 2.0, 0.0, 3.0], 3, [-1.0, -4.0, -7.0, 3.0])` — **bit-exact** |
| **5**, `[0, 9, 9, 9, 9]` | `tok.p`, 1 element, a launch output | the same | the same — **only index `0` is read** |
| **6** elements | `tok.p`, 1 element, a launch output | the same | the same |
| 5 elements | a **host** buffer of 1 element | a different program | **refused**: `input buffer **2** holds 1 element(s) but the run covers 5` |

**Three readings, each from a row rather than an argument.** The fourth row is the semantics: the
four elements the device required to be added are **never read**, and filling them with `9.0`
changes nothing on either backend, so the repair is inert arithmetic rather than a second program.
The last row is the host/device split itself — moving `.nk` from the sampler's output buffer to a
host buffer of the **same one element** moves the refusal from slot `3` to slot `2`, which is the
only place the rule is applied and the only place this note's own examples put a short input
beside a wide launch. And the third row is the repair: **one `compute.buffer`'s width**, with no
body, no launch and no guard touched.

**Measured: the answer is bit-exact, and nothing was loosened to get there.** `([1.0, 2.0, 2.0,
0.0, 3.0], 3, [-1.0, -4.0, -7.0, 3.0])` on **both** backends, entry for entry, so there is **no ulp
distance to report** — the sequence, the trip count and the last logit row are the same values. The
guard runs on the host three times either way, so this is a statement about the arithmetic and not
about where the stop condition is decided; §4's host-only reading is untouched. All three kernels
are on the device, and their emitted modules were handed to `spirv-val`
(`C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe`, `--target-env vulkan1.1`) rather than reasoned about
from behaviour: the registry the run itself filled, read after a successful three-step run, holds
three fragments, and **all three are ACCEPT** — `greedy_raw` at 448 words with one input and one
output, `logits_raw` at 378 with two and one, `append_raw` at 434 with four and two.

**The two shapes that were worse, each measured rather than argued.** The brief's other two
answers were run too:

| shape | `cpu` | `gpu` | verdict |
|---|---|---|---|
| **pad `nv` to the launch count** (chosen) | `([1.0, 2.0, 2.0, 0.0, 3.0], 3, [-1.0, -4.0, -7.0, 3.0])` | the same | **bit-exact, and the sampler's output never leaves the device** |
| **rebuild the incoming row on the host** from the collected token, `compute.buffer [bit, bit, bit, bit, bit]` | the same | the same | runs and bit-exact, but a host round trip per step and the sampler's output is discarded |
| **launch over what the buffer holds**, `.n 5` → `.n 1` | `([1.0], 2, [2.0, 4.0, 0.0, 5.0])` | `([1.0], 3, [-1.0, -4.0, -7.0, 3.0])` | **the two backends disagree** |

**The last row is the finding worth keeping.** A one-index launch is not refused — the rule is
satisfied, because the only short input is gone from the check — but **four of the five cache
slots are never written**, and what a lane reads from an unwritten slot is **not the same thing on
the two backends**: `cpu` reports a trip count of `2` and a last logit row of `[2.0, 4.0, 0.0,
5.0]`, the device reports `3` and `[-1.0, -4.0, -7.0, 3.0]`. **This is `wontfix:D17`'s
every-position-written rule showing up as a silent wrong answer rather than as a refusal**, and it
is why the third row is the spelling to take: **padding keeps every position written, and a launch
that does not write them is not a cheaper program but an undefined one.** The middle row is the
honest evidence that **the capability was never missing** — the host has two working answers to
this rule — and padding is preferred only because it keeps the sampler's own output buffer on the
device, which is `examples/decode.lichen`'s chaining, and costs no copy.

**Not changed, and said so rather than implied.** No production code moved: the repair is one
`compute.buffer`'s width in an example, `staged_bytes` and `BatchInput` are read, not edited, and
`wontfix:D17` is untouched — the append still writes both outputs on every lane of a five-lane
launch. **No test was added**; the runs above were driven by probes that were deleted afterwards.

## What was measured, inferred and unknown

**Measured.** Every verdict, value and refusal above; the fragment census; the bisect table; the
`0xc0000005` exit codes; the `-inf` bits; the renderer's decimal; and, in §5, the `nv` widths `1`
and `4` and `5` and `6`, the `[0, 9, 9, 9, 9]` row, the host-`nk` row, the two rejected spellings,
the bit-exact device answer, the three word counts, and the three `spirv-val` verdicts.

**Inferred.** That the runtime stop is host-only, from an exhaustive reading of the compute
namespace rather than an attempt. **§5 exercised it and the reading stands.**

**Unknown.** ~~Why the driver dies on the sampler's module;~~ **answered** by
[the sampler's crash, localised to one emitted id](dispatch-crash.md) — the module names an
undefined id and `spirv-val` says so by name. ~~Whether the sampler's `>` against the seed would
move;~~ **answered** by §5 — it does not. Whether a device-side stop condition is capable;
why a one-index launch reads different unwritten slots on the two backends (§5's last row is the
measurement, not the explanation); whether any operator of the ladder **not** reached here
(`softmax`'s 111-instruction body, `attend`, `ln`, `sin`, `cos` in a ladder body) diverges — none
of them reached the device, and §8's five-ulp `sin`/`cos` result is a standalone measurement, not
one of these programs.
