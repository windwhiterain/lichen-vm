# The contraction can be forbidden

> Status: **current** — every figure below is measured on **two** devices on this branch, off
> `dev@71adaa1a`: an RTX 3060 Laptop GPU (NVIDIA driver 596.36.0.0) and an Intel Iris Xe
> Graphics (Intel driver 101.6790), reached by running the **same binary twice** with
> `VK_DRIVER_FILES` pointed at each vendor's ICD manifest. **Nothing here changed production
> code, and no test was added.** [transcendental-gap-mechanism](transcendental-gap-mechanism.md)
> established the mechanism; this note answers the two questions its own §7 left open — can the
> rewrite be forbidden, and is it one device's or the platform's — and it corrects one of its
> claims.

Two verdicts, one per job, and the second one is not the one the first note's wording predicts.

- **Job 1: a control exists, and it buys everything.** `OpDecorate %id NoContraction` —
  decoration `42`, core SPIR-V since `1.0`, capability `Shader` — makes both devices
  reproduce the language's definition **bit for bit** on every grid measured, the entire
  trigonometric tail included, and it costs **three words per float arithmetic instruction**
  and **no measurable time**.
- **Job 2: the mechanism is the platform's, not one vendor's** — with one exception that
  corrects [transcendental-gap-mechanism](transcendental-gap-mechanism.md) §1.1. The Intel
  Iris Xe contracts `a·b + c` **4096/4096** exactly as the RTX 3060 does, reproduces the same
  `sin`/`cos`/`exp` bits at every grid including a wrong-sign point to the digit, **and
  contracts `a·b − c` as well — 4096/4096 to the fused reading, where the RTX 3060 leaves it
  uncontracted.** The "control" the prior note used to characterise contraction is a property of
  one optimiser, not of SPIR-V.

## 1. The two devices, and how one binary reached both

**Measured** with `vulkaninfoSDK.exe --summary` (`$VULKAN_SDK\Bin`), which reports five physical
devices on this machine:

| index | vendor | device | type | driver |
|---|---|---|---|---|
| GPU0 | `0x8086` | Intel(R) Iris(R) Xe Graphics | integrated | 101.6790 |
| GPU1 | `0x8086` | Microsoft Direct3D12 (Intel(R) Iris(R) Xe Graphics) | integrated | 26.2.99 |
| **GPU2** | `0x10de` | **NVIDIA GeForce RTX 3060 Laptop GPU** | discrete | 596.36.0.0 |
| GPU3 | `0x10de` | Microsoft Direct3D12 (NVIDIA GeForce RTX 3060 Laptop GPU) | discrete | 26.2.99 |
| GPU4 | `0x1414` | Microsoft Basic Render Driver | CPU | 26.2.99 |

`GpuContext::new()` takes a **discrete** device when there is one, so an unmodified run lands on
GPU2. The three rows that are not the two vendors' own drivers are the D3D12 translation layer
and the basic render adapter, and they are named here so a reader does not have to re-enumerate
to learn they were present and not used.

**Measured: the loader's driver selection is what separates the two runs, and it needs no code
change.** `VK_DRIVER_FILES` pointed at one ICD manifest makes the loader enumerate exactly one
device. The two manifests are
`C:\Windows\System32\DriverStore\FileRepository\nvamsi.inf_amd64_d70fc260a6763dac\nv-vk64.json`
and `C:\Windows\System32\DriverStore\FileRepository\iigd_dch.inf_amd64_38e51a298b862feb\igvk64.json`;
`vulkaninfoSDK --summary` under each reports one device and the name above. `VK_ICD_FILENAMES`
behaves the same way. **So every number in both jobs comes from the identical executable, the
identical module, and the identical grids** — the only difference is which ICD the loader was
allowed to load. One process at a time per device throughout, as the [body
sweep](device-body-sweep.md) §1 requires.

## 2. Job 2: the same probe, the second vendor

### 2.1 `a·b + c` and `a·b − c`, both spellings, both devices

**Measured**, one body over three buffers, 4096 points chosen by the filter
`f32::mul_add(a,b,c).to_bits() != (a*b + c).to_bits()` and its mirror for the subtract spelling.

**The grid is this slice's own, not [transcendental-gap-mechanism](transcendental-gap-mechanism.md)
§1.1's** — it is quoted here because the point selection is a fact about the grid, and two
different grids can only be compared on their verdicts, not on their points. Three properties
were needed and each was found by the filter coming back empty:

- **the significand has to be coarse.** Thirteen fraction bits (`(1 + m/4096)`) give a product
  needing twenty-six bits, more than an `f32` holds, so `a·b` is inexact. The first two grids
  used — four and three fraction bits — produced *exact* products and not one point passed,
  which is the same requirement [transcendental-gap-mechanism](transcendental-gap-mechanism.md)
  §1.1 states and this slice re-learned by getting it wrong;
- **`a·b` and `c` must share a decade**, or the addition swallows the product's last bit and the
  two readings coincide again (this slice's second empty grid: `a·b` at `2^-32`, `c` at `2^-12`);
- the third axis is walked with two coprime strides, so the triples are not elementwise.

| spelling | device | points | device `==` mul-then-add | device `==` fused |
|---|---|---|---|---|
| `a·b + c` | RTX 3060 | 4096 | **0 / 4096** (worst 1 ulp) | **4096 / 4096** (worst 0) |
| `a·b + c` | Iris Xe | 4096 | **0 / 4096** (worst 1 ulp) | **4096 / 4096** (worst 0) |
| `a·b − c` | RTX 3060 | 4096 | **4096 / 4096** (worst 0) | 0 / 4096 |
| `a·b − c` | **Iris Xe** | 4096 | **0 / 4096** (worst 512) | **4096 / 4096** (worst 0) |

**So the driver crash question and the mechanism question have the same answer on both
vendors, and the negative control does not.** The Iris Xe's fused reading for the subtract
spelling is `f32::mul_add(a, b, -c)` — the addend negated, not the multiplicand — which is the
only one of the two fused spellings that matches at 4096/4096, so the contraction is
`OpFSub`'s own `OpFMul` folded in, not a rewritten sign.

### 2.2 The transcendentals on the Iris Xe

**Measured**, the bodies [transcendental-gap-mechanism](transcendental-gap-mechanism.md)
§1.1 models, evaluated on the device and scored against four readings of the same definition:
`plain` (the definition), `poly` (the Horner fused), `recombined_fma` (the two-term reduction
put back to one term against the single-rounded `π/2` and fused), and `both`.

| grid | points | `plain` | `poly` | `recombined_fma` | `both` |
|---|---|---|---|---|---|
| `sin`, `[−8, 8)` | 256 | 91 (94 ulp) | 89 (94) | 244 (1) | **256 (0)** |
| `sin`, `[−1, 0)` | 512 | 437 (2) | 438 (1) | 486 (1) | **512 (0)** |
| `sin`, `[4000, 8192]` | 2048 | 0 (2617823) | 0 (2617823) | 1941 (1) | **2048 (0)** |
| `sin`, `(−8192, −4000]` | 2048 | 5 (534680) | 5 (534680) | 1957 (1) | **2048 (0)** |
| `cos`, `[−8, 8)` | 256 | 71 (117) | 67 (117) | 250 (1) | **256 (0)** |
| `cos`, `[−1, 0)` | 512 | 394 (1) | 424 (1) | 477 (1) | **512 (0)** |
| `cos`, `[4000, 8192]` | 2048 | 0 (10475776) | 0 (10475776) | 1952 (1) | **2048 (0)** |
| `cos`, `(−8192, −4000]` | 2048 | 5 (5355040) | 5 (5355040) | 1951 (1) | **2048 (0)** |

Every cell is **identical on the Iris Xe and on the RTX 3060** — same bit-exact counts, same
worst ulp. `exp` likewise: `plain` **244/256** (worst 171) and the fused Horner **256/256**
(worst 0), on both. The `plain` column reproduces [device-body-sweep](device-body-sweep.md)
§3.1's `exp` and §3.2's `sin`/`cos` figures figure for figure, so the two vendors are not
merely similar here: **they return the same bits.**

### 2.3 The tail on the Iris Xe

**Measured**, and identical to the RTX 3060 in every figure.

| `θ` in | `[0,512)` | `[512,1024)` | `[1024,1536)` | `[1536,2048)` | `[2048,2560)` | `[2560,3072)` | `[3072,3584)` | `[3584,4096)` |
|---|---|---|---|---|---|---|---|---|
| ulp of one | 116.6 | 235.7 | 354.2 | 473.8 | 592.8 | 713.8 | 836.3 | 953.9 |

| `θ` in | `[4096,4608)` | `[4608,5120)` | `[5120,5632)` | `[5632,6144)` | `[6144,6656)` | `[6656,7168)` | `[7168,7680)` | `[7680,8192)` |
|---|---|---|---|---|---|---|---|---|
| ulp of one | 1074.7 | 1191.5 | 1312.8 | 1430.7 | 1550.2 | 1669.8 | 1786.8 | **1908.5** |

`cos`'s worst absolute error over 5200 arguments at multiples of `π`, on both devices:
**2160/5200 bit-exact, worst 7314 ulp, no sign disagreement**; `sin`'s, on both devices:
**1/5200 bit-exact, worst 1,910,155,776 ulp, signs disagree on 934/5200**, the worst mover at
`k = 2589`, `θ = 8.1335835e3`, where the device answers `+1.103878e-4` and the definition
`−1.1594966e-4`. Those two values are [transcendental-gap-mechanism](transcendental-gap-mechanism.md)
§3's, to the digit, and the `1,910,155,776` is that note's worst ulp figure to the digit.

**And the angle it prints as `8133.6` is the `f32` `8133.5835` at `k = 2589`** — the definition's
own zero-crossing grid lands on a representable `k·π` whose value is `8133.5835`, and `8133.6` is
that number at four digits. The phenomenon the prior note reports is reproduced exactly; only
the label is rounded, and `k = 2589` is the index to quote from now on.

### 2.4 The Iris Xe's own rewriting, where it differs from the RTX 3060's

**Measured**, one body per spelling over 4096 `(q, residual)` pairs with `q` over `[0, 5216)`,
scored against six readings of the reduction: `split` (`(m − q·H) − q·T`), `recombined`
(`m − q·(H+T)`), `fused` (`fma(−q, H+T, m)`), `head-fused`, `tail-fused`, and `summed`
(`m − (q·H + q·T)`).

| spelling | RTX 3060 | Iris Xe |
|---|---|---|
| `split`, `q` and `m` from buffers | **`summed` 4096/4096** | **`fused` 4096/4096** |
| `split`, the tail `q·TAIL` from a third buffer | **`summed` 4096/4096** | **`summed` 4096/4096** |
| `split`, `q` produced on the device by a conversion round trip | **`summed` 4096/4096** | **`fused` 4096/4096** |
| the one-term spelling `m − q·(H+T)` | `recombined` 4096/4096 | `fused` 4096/4096 |
| `split`, under `NoContraction` (§3) | **`split` 4096/4096** | **`split` 4096/4096** |

**Measured, and this is the sharpest form of the mechanism's statement: the two-term
Cody–Waite reduction does not survive on either device, in any of the three spellings this slice
could construct.** The `tail from a buffer` row is the decisive one — with `q·TAIL` read from
memory the optimiser cannot distribute anything, yet the device still returns
`m − (q·H + q·T)`, so what it does is **reassociate the two subtractions**, not only distribute
the two products. And the two devices disagree about how far they go: the RTX 3060 reassociates
in a 279-word module but does not contract there, while the Iris Xe contracts at the same size.
In the full 578-word `sin` body both contract and both distribute — the `both` column of §2.2 is
exact — so **the rewrite a driver performs is a function of the module it is given, and the
split's survival cannot be reasoned about from a small module.**

### 2.5 Verdict, job 2

**Measured: the mechanism is the platform's, on two vendors, and one of the prior note's
characterising measurements is a property of one of them.** The thirty-line probe re-runs on a
second vendor exactly as [transcendental-gap-mechanism](transcendental-gap-mechanism.md) §7
asked, and it reproduces the mechanism bit for bit — plus it shows the `OpFSub` negative
control failing. Neither driver refused a module: **18 of 18 modules dispatched**, nine per
device, and all eighteen are `spirv-val` ACCEPT (§4).

## 3. Job 1: what forbids the contraction, and what it buys

### 3.1 The candidates, and what each is worth

The question is what a driver is allowed to do to a plain `OpFMul`/`OpFAdd`/`OpFSub` chain, and
whether SPIR-V exposes a way to say *do not*. §2.4's census is the same one
[transcendental-gap-mechanism](transcendental-gap-mechanism.md) §1.2 took: `sin` is 17 `OpFMul`,
11 `OpFAdd`, 7 `OpFSub`, **zero `OpFDiv`, no `Fma`, no `OpExtInst`**. So the rewrite happens
below SPIR-V and the only handle is a module-level constraint. Three were found, from the
grammar shipped with the SDK (`$VULKAN_SDK\Include\spirv-headers\spirv.core.grammar.json`) and
from the emitter's own decoration list:

| candidate | where it comes from | `spirv-val --target-env vulkan1.1` | on the device | buys |
|---|---|---|---|---|
| **`NoContraction`** | decoration `42`, **version 1.0, capability `Shader`** | **ACCEPT** | honoured | **everything — §3.2** |
| `NoReassociation` | decoration `4268`, **absent from the SDK's bundled grammar** | ACCEPT | accepted, inert | nothing |
| `FPFastMathMode None` | decoration `40`, version 1.0, but needs capability `FloatControls2` | **9 of 9 REJECT** | ran anyway, inert | nothing |

Three measured verdicts, and the middle one is worth its own paragraph because the tools
disagree about it. `NoReassociation` is accepted by `spirv-val` 2024.4 on a SPIR-V 1.3 module and
by the NVIDIA driver (the dispatches succeed) and **changes nothing**: `a·b + c` is still
**4096/4096 fused**, `sin` is still 91/256 against the definition and `both` is still 256/256.
`spirv-dis` from the same SDK refuses to print it at all — `error: 16: Invalid decoration
operand: 4268` — while `spirv-val` accepts it, so the two tools disagree about whether the
decoration exists. **The control that names reassociation explicitly is the one that does not
work here, and the control that names contraction is the one that does** — which is the whole
finding in one sentence, and it is why the fix has to be the other one.

`FPFastMathMode` is the clearest rejection of the three: **all nine modules `REJECT`** under the
target env with `Operand 2 of Decorate requires one of these capabilities: Kernel
FloatControls2`. The capability is a Vulkan 1.4-era one, this target emits SPIR-V 1.3, and the
driver nonetheless ran all nine and returned the same bits as an undecorated module.

**What is not here is a driver or toolchain switch.** No environment variable, capability,
execution mode or builtin in the grammar states "do not fuse or reassociate this": the precision
knobs the grammar carries are `FPRoundingMode` (39), `FPFastMathMode` (40) and the
`SPV_KHR_float_controls` denormal and rounding modes, and none of them is that statement.
**Searched, not found** — this slice did not drive a vendor's own build flags, and if one
exists it would not be reachable from a SPIR-V module anyway.

### 3.2 `NoContraction`, measured

**Measured.** One `OpDecorate %id NoContraction` on the result id of **every** `OpFNegate`,
`OpFAdd`, `OpFSub`, `OpFMul` and `OpFDiv` in the module — 35 of them in `sin` — inserted at the
end of the annotation section, which is where the logical layout puts a decoration.

| what the probe measures | RTX 3060, plain | RTX 3060, `NoContraction` | Iris Xe, plain | Iris Xe, `NoContraction` |
|---|---|---|---|---|
| `a·b + c`, 4096 points, mul-then-add | 0/4096 | **4096/4096** | 0/4096 | **4096/4096** |
| `a·b + c`, 4096 points, fused | 4096/4096 | **0/4096** | 4096/4096 | **0/4096** |
| `sin`, `[−8, 8)`, 256 | 91/256 | **256/256** | 91/256 | **256/256** |
| `sin`, `[−1, 0)`, 512 | 437/512 | **512/512** | 437/512 | **512/512** |
| `sin`, `[4000, 8192]`, 2048 | 0/2048 | **2048/2048** | 0/2048 | **2048/2048** |
| `sin`, `(−8192, −4000]`, 2048 | 5/2048 | **2048/2048** | 5/2048 | **2048/2048** |
| `cos`, `[−8, 8)`, 256 | 71/256 | **256/256** | 71/256 | **256/256** |
| `cos`, `[−1, 0)`, 512 | 394/512 | **512/512** | 394/512 | **512/512** |
| `cos`, `[4000, 8192]`, 2048 | 0/2048 | **2048/2048** | 0/2048 | **2048/2048** |
| `cos`, `(−8192, −4000]`, 2048 | 5/2048 | **2048/2048** | 5/2048 | **2048/2048** |
| `exp`, `[−16, 16)`, 256 | 244/256 | **256/256** | 244/256 | **256/256** |
| `sin`, 5200 crossings, bit-exact | 1/5200 | **5200/5200** | 1/5200 | **5200/5200** |
| `cos`, 5200 crossings, bit-exact | 2160/5200 | **5200/5200** | 2160/5200 | **5200/5200** |
| `sin`, 5200 crossings, sign disagreements | 934/5200 | **0/5200** | 934/5200 | **0/5200** |
| `cos` tail, worst absolute over `[0, 8192]` | 1908.5 ulp of one | **0.0** | 1908.5 | **0.0** |
| the two-term reduction, all three spellings | `summed`/`fused` 4096/4096 | **`split` 4096/4096** | `summed`/`fused` 4096/4096 | **`split` 4096/4096** |

**Measured: it buys the definition, bit for bit, on both devices, on every grid including the
whole declared domain.** The two numbers that made
[transcendental-gap-mechanism](transcendental-gap-mechanism.md) a defect — the `q·4.37e-8` tail
and the sine that changes sign near `8133` — are **0.0 and 0/5200**. The two-term Cody–Waite
reduction survives, so the accuracy [floating-point](floating-point.md) §8 designed is the
accuracy the device gives. `exp` is the exception that shows what the decoration is and is not:
its decorated column is **256/256 against the definition** and §2.2's `plain` column is still
244/256 against the definition, so the device now answers exactly what `exp_value` answers —
including the `1.2e-5` relative error [floating-point](floating-point.md) §6 measures for that
definition against a correctly rounded value. **The decoration makes the device agree with the
definition; it does not make either of them agree with the platform, and it was never going
to.**

### 3.3 What it costs

**Measured, module shape.** Exactly **three words per float arithmetic instruction** — one
`OpDecorate`, three words, no operand:

| module | plain | `NoContraction` | decorations | float ops |
|---|---|---|---|---|
| `sin` | 578 | **683** | 35 | 17 `FMul` + 11 `FAdd` + 7 `FSub` |
| `cos` | 562 | **664** | 34 | 17 + 11 + 6 |
| `exp` | 412 | **496** | 28 | 18 + 10 |
| `a·b ± c` | 277 | **283** | 2 | 1 `FMul` + 1 `FAdd`/`FSub` |
| the reduction, three spellings | 279 / 286 / 295 | 291 / 295 / 307 | 4 / 3 / 4 | — |

**Measured, time.** 1,048,576 points through one dispatch, best of forty, upload and readback
included — a 52 ms kernel, so the fixed cost that dominates a small dispatch is not what is being
compared:

| device | `sin`, plain | `sin`, `NoContraction` | `exp`, plain | `exp`, `NoContraction` |
|---|---|---|---|---|
| RTX 3060 | 52.55 ms | **51.86 ms** | 52.09 ms | **51.79 ms** |
| Iris Xe | 53.18 ms | **52.53 ms** | 52.63 ms | **52.55 ms** |

**No measurable time cost: the decorated run is 0.3–1.3% *faster* on all four pairs, which is
run-to-run spread rather than a gain** — and the sign is the point, because a control that cost
throughput would not come out ahead on either device. Nothing else about the module changes: no
capability, no execution mode, no `OpExtInst`, no `Fma`. `spirv-dis` shows the decoration
(`OpDecorate %43 NoContraction`) and the arithmetic underneath it is the same
17 `OpFMul` / 11 `OpFAdd` / 7 `OpFSub` as before it.

### 3.4 What using it would touch — and why this slice stops here

**No production change is made or proposed here; the user decides.** [transcendental-gap-mechanism](transcendental-gap-mechanism.md)
§6's first bullet named the fix as *"a contraction-forbidding attribute in the emitted module"*
and said *"the attribute is not a deletion but a constraint on a rewrite the module does not
itself perform"*. That is now measured rather than asserted, and this is what turning it on
would reach — four places, and the third is the one that is easy to miss:

- **`crates/lichen-compute-gpu/src/spirv.rs`, the binary-instruction table in `emit_function`**
  (`(ScalarClass::Float, KernelBin::Add)` and its `Sub` / `Mul` / `Div` neighbours): the place a
  float result id is allocated, which is where a decoration would be owed.
- **The same file, `assemble`'s annotation section**, where every `OpDecorate` this emitter
  writes is pushed today (`ArrayStride`, `Offset`, `Block`, `DescriptorSet`, `Binding`,
  `BuiltIn`). One `NoContraction` per float op joins them there; §3.3 measured the size of that.
- **The pipeline cache key in `dispatch.rs` is `(fragment digests, binding.inputs, binding.outputs)`
  — not the module's words.** A decoration decided by a flag rather than by the IR would
  therefore not be part of the key, and one process could hold a pipeline for a decorated
  fragment under an undecorated request. If the IR carries the decision, the digest carries it
  too, and this does not arise; if a flag carries it, the key has to.
- **The IR has to say which float arithmetic is exact-by-definition**, or the emitter decorates
  everything and the guarantee is bought for the whole backend rather than for the
  transcendentals. `KernelBody::expand_transcendentals` is where the definitions are inlined, so
  it is where a marker would ride.

**And the consequence that decides it is not an arithmetic one.** With `NoContraction` both
devices reproduce the language's definition on every grid measured, which means
[floating-point](floating-point.md) §8's two-term reduction, §7's advice to `sqrt` and this
whole note's subject are all consistent on two vendors at once — and it means the gap
[device-leg-of-the-ladder](device-leg-of-the-ladder.md) §2 recorded is avoidable without a
tolerance. **What is not settled by this measurement is whether the language wants that** — a
backend-wide constraint on a driver's optimiser is a policy, not an arithmetic change, and
[floating-point](floating-point.md) §6's question about a device running a body that does not
reproduce the definition is still open. It is just no longer open *for want of a control*.

## 4. What the validator says

**Measured: 27 of 27 modules dispatched in this slice are accounted for, and every one that
claims to be valid is `spirv-val` ACCEPT** at
`C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe --target-env vulkan1.1` (SPIRV-Tools v2024.4,
whose largest Vulkan target is 1.3):

```
ACCEPT  9 plain            sin 578, cos 562, exp 412, a*b+c 277, a*b-c 277,
                          fold-split 279, fold-opaque 286, fold-recombined 254,
                          fold-device-quadrant 295
ACCEPT  9 NoContraction    683, 664, 496, 283, 283, 291, 295, 260, 307
REJECT  9 FPFastMathMode   718, 698, 524, 285, 285, 295, 298, 262, 311
                          all nine: "Operand 2 of Decorate requires one of these
                          capabilities: Kernel FloatControls2"
```

The `REJECT` row is a result, not a defect: those nine modules are the ones §3.1 says are
inert, and the validator is what says they are not expressible under this target environment at
all. The eighteen `ACCEPT`s extend [device-body-sweep](device-body-sweep.md) §5's and
[transcendental-gap-mechanism](transcendental-gap-mechanism.md) §4's count from thirty-five to
fifty-three, and every divergence in this note is still numerical, inside a module the
specification accepts.

## 5. Corrections to the notes this follows

- ~~[transcendental-gap-mechanism](transcendental-gap-mechanism.md) §1.1: **"this device
  contracts a multiply feeding an add, and only that"**, and *"the subtract spelling is the
  control, and it is a clean negative: `OpFSub` after `OpFMul` is not contracted, 4096 of 4096 on
  the unfused reading"*~~ **True of the RTX 3060, and false as a statement about the platform.**
  On the Iris Xe the subtract spelling is contracted **4096/4096 to
  `f32::mul_add(a, b, -c)`**, worst 512 ulp the other way. The probe, the filter and the grid
  are the same; the tables are in §2.1. **What survives is the positive half and it is now
  two vendors wide**: a multiply feeding an add is contracted on both, so the mechanism is not
  one driver's habit — and the negative control was a habit of one driver's optimiser, not a
  property of `OpFSub`.
- ~~[transcendental-gap-mechanism](transcendental-gap-mechanism.md) §7: *"Whether the rewrite is
  NVIDIA's driver or the Vulkan layer's. The machine is one RTX 3060 Laptop GPU; no second vendor
  was available"*~~ **Closed for the question as asked.** The rewrite happens on both vendors,
  the `a·b + c` reading is the same 4096/4096 on both, and `sin`/`cos`/`exp` return **the same
  bits** on both at every grid. What is still not separable is NVIDIA's optimiser from the
  common part of the two drivers' toolchains, because they share no component this slice can
  switch off — but the rewrite is not the property of one vendor the sentence left open.
- ~~[transcendental-gap-mechanism](transcendental-gap-mechanism.md) §7: *"Whether the two-term
  reduction survives on a compiler that does not contract"*~~ **Measured, and it does not
  survive a small module on either device** — §2.4 — *and it survives both whole ones when the
  module forbids contraction* — §3.2. So the question splits: the split survives on a device
  that is told to keep it, and not otherwise.
- ~~[transcendental-gap-mechanism](transcendental-gap-mechanism.md) §3: *"at `θ = 8133.6`"*~~
  **The argument is the `f32` `8133.5835`, which is `2589·π` as a representable `f32`** — the
  zero-crossing grid's own point, and both values (`+1.103878e-4` against `−1.1594966e-4`)
  reproduce exactly. Nothing about the finding changes; the label should be `k = 2589`.
- **[transcendental-gap-mechanism](transcendental-gap-mechanism.md) §6's first bullet, "a
  contraction-forbidding attribute in the emitted module … or the driver's own switch turned
  off"** — **the second half of it has no referent.** No driver or toolchain switch was found
  (§3.1); the attribute half is measured, and it is `NoContraction`. The list's other three
  bullets are untouched by this slice.
- **[floating-point](floating-point.md) §7 and §8 are not corrected by this slice**, and
  nothing here contradicts them: §7's division hazard is closed by the prior note, §8's
  two-term reduction is vindicated by §3.2, and this note's own `exp` column confirms §6's
  `2.4e-5` is the definition's rather than the device's. **What §8's "What is still not proven"
  does gain is that its open question had a control all along** — the third bullet there says
  the mechanism was open; it was, and it was nameable.

## 6. The kernel work this implies

**No production change is made or proposed here.** What a follow-up slice would have to decide,
one line each:

- **Whether the IR marks float arithmetic that must keep its rounding points**, and therefore
  whether the decoration is per-instruction or per-module. §3.3 measured the whole-module price;
  a per-instruction price has not been measured.
- **Whether `NoContraction` is on by default for the GPU backend or is a knob.** It is a core
  decoration of `Shader` and two vendors honour it, so a per-module opt-in is a language policy
  question rather than a portability risk; §4's eighteen validations are the portability
  evidence, and it is two vendors wide, which is the same width as every other device claim in
  these notes.
- **The pipeline cache key** (§3.4): free if the decision rides in the IR, a change if it
  rides in a flag.
- **What the `spliced_transcendentals` test would say.** Its tolerances are 5 ulp for `sin` and
  6 for `cos` because the device diverges; with the decoration the divergence is zero and those
  numbers would be `0`. **No test was changed here** — the brief forbids production changes, and
  a tolerance is the user's to move.
- **No `wontfix:D<n>` is involved.** [code-audit](code-audit.md) carries no decision over the
  transcendentals, the emitter or the polynomial; the nearest is `wontfix:D9` via `P4-10`, the
  type printer's nesting bound, which this slice does not touch.

## 7. What is measured, what is inferred, what is unknown

**Measured, in full.** §1's five-device census and the two `VK_DRIVER_FILES` runs; §2.1's four
4096-point contraction cells across two devices and two spellings; §2.2's eight model rows and
the `exp` row; §2.3's sixteen buckets and both 5200-crossing sweeps; §2.4's four spellings ×
six readings × two devices; §3.1's three candidates with their validator and driver verdicts;
§3.2's sixteen rows; §3.3's five word counts and four timing pairs; §4's twenty-seven modules.
Every bit-exact count is read off the two backends' own bit patterns at the same point, and
every ulp figure off that same pair.

**Inferred, and not re-derived here.** That `NoContraction` is *why* the decorated modules are
bit-exact rather than some accident of the insertion — the decoration count matches the float
operation count in all five modules (§3.3's table is that identity, measured five times) and
the undecorated modules of the identical fragments are not. That the two vendors share a
toolchain component rather than each having the habit — the sin/cos bits are *identical*, which
is a stronger fact than "both contract", and the most economical reading is that both honour the
same fast-math default.

**Unknown, and this slice closed none of these.**

- **Whether `NoContraction` is honoured by a third vendor.** Two devices, two vendors. The
  [body sweep](device-body-sweep.md) §1's rules and this note's §2 are two samples of a
  population.
- **Whether the decoration is stable across recompiles and pipeline caches.** It is a module
  fact, so it should be as stable as the module, but nothing here ran a second compile with a
  cold cache and compared, and [transcendental-gap-mechanism](transcendental-gap-mechanism.md)
  §7's "is the fusion stable across recompiles" is still open on the undecorated path.
- **Whether `FPFastMathMode` becomes a usable control on a target that declares
  `FloatControls2`.** It is rejected under Vulkan 1.1, the driver ignored it, and its intended
  reading is a *relaxation* rather than a constraint, so this slice would not have chosen it —
  but a target env that declares the capability was not tried.
- **What per-instruction decoration costs in time.** §3.3 prices the whole module, and the
  cheapest version of the fix decorates one operand of the Horner rather than thirty-five ids.
- **What the platform's own `sin`/`cos` answer at the large arguments** — still open in the prior
  note, still open here; the decoration makes the device agree with the definition and says
  nothing about which of the two is closer to correctly rounded.

## 8. What this slice deleted

**Two artefacts, and both are gone.**

`crates/lichen-compute-gpu/examples/cctl_probe.rs` — the scratch driver that ran every
measurement in this note: the contraction probe, the model comparison, the tail sweeps, the
reduction's five spellings, the module dumps and the timings — was **deleted before this note's
commit**. It held no production code and nothing else referenced it.

`crates/lichen-compute-gpu/src/spirv.rs` — the emitter — carried a scratch hook while Job 1 was
measured, an environment-variable-gated pass that decorated every float arithmetic result, since
`spirv::compile` is the only path from a fragment to the module the dispatch builds and the
decoration has to be inside it. **That hook was reverted with `git checkout` and the file is
byte-identical to `dev@71adaa1a`**: the measurements in §3.2 and §3.3 are of a module the
shipped emitter does not build, which is what §3.4 is for. **No example file was edited**, no
`output =` line moved, `git diff dev...HEAD -- examples/` is empty, **and no test was added and
no production file was touched.** The twenty-seven modules are gone with the two artefacts; §4's
word counts and §2.4's census are what remain, and both are re-derivable from `unary.rs` and
`spirv::compile` in one command each.
