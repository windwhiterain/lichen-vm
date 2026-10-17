# The fused multiply-add: what it is, what it would touch, and what is not measured

> Status: **a proposal, not a change** — **no production file and no test was touched.** Every
> figure below was measured on **two** devices off `dev@12425166`, by running the **same binary
> twice** with `VK_DRIVER_FILES` pointed at one vendor's ICD manifest and nothing else: an
> **NVIDIA GeForce RTX 3060 Laptop GPU** (discrete) and an **Intel(R) Iris(R) Xe Graphics**
> (integrated), the two manifests
> [contraction-control](contraction-control.md) §1 names.
>
> **What is being measured** is the cost of the one capability
> [division-rounding-gap](division-rounding-gap.md) §7 says is missing. That note's numbers, its
> grids and its five compensated spellings are **referenced here, not restated**, as is
> [floating-point](floating-point.md) §7's account of why `floor` was defined in the IR rather
> than emitted and §8's two-term reduction.
>
> **The one-sentence result: the spelling exists in the validator and not in the language.**
> `GLSL.std.450`'s `Fma` is **`spirv-val` ACCEPT** under `--target-env vulkan1.1` in a hand-built
> module **and** in the shipped emitter's own module with four of its multiply–subtract pairs
> rewritten, and **the compensated division built on a host `f32::mul_add` reaches the correctly
> rounded quotient on 24,576 of 24,576 points at one step, from a reciprocal that is one and two
> ulp below the nearest**. **What is not measured is the device**: the folded module cannot be
> dispatched, so §6's zero-ulp claim on a device is **inferred**, and §6 says exactly from what.

## 1. The platform's own fused multiply-add, and the reference it is measured against

**Measured.** The probe is this note's own; the grids are named here because a note's grid is a
fact about its probe. Two sets of six and four 4096-lane grids are used, both over numerators in
`[1, 2)` at `2⁻¹²`: the first is `product` (the **M** × **D** grids of
[division-rounding-gap](division-rounding-gap.md) §1), `dense / 3.0`, `dense / 7.0`,
`dense / 5.0`, `dense / 2.5` and the `ln` band `[1/√2, √2)` with a divisor computed on the lane;
the second is `dense / 3.0`, `dense / 7.0`, `dense / 2.5` and the `ln` band, which is the set the
device runs in §5.

| | figure |
|---|---|
| the host's `f32::mul_add` reads differently from `a*b + c` | **378,468 of 16,777,216** points |
| `mul_add(a, a, -a)` is exactly `0` where the unfused reading is not | **1 of 4096** against **4096** |
| the host's `a / b` answers a hand-written correctly rounded quotient | **200,000 of 200,000**, worst **0** ulp |
| a `f64` round trip agrees with that hand reference | **200,000 of 200,000** |

**So `f32::mul_add` on this host is a correctly fused multiply-add and not a multiply followed by
an add**, and **the host's `f32` division is the correctly rounded quotient** — the same fact
[division-rounding-gap](division-rounding-gap.md) §2 measured on 6,000,000 samples, re-measured
here because this probe carries its own reference. The reference is `f32` inputs decomposed to
`(mantissa, exponent)`, the quotient carried with 25 fraction bits and rounded **once**, with the
division's remainder as a sticky flag: rounding the fixed-point value and the significand
separately is a **double rounding**, and it answers `1/3` as `0x3eaaaaaa` where the correctly
rounded answer is `0x3eaaaaab`. That number is worth recording, because it is **the Iris Xe's
reciprocal for `7.0`'s neighbour** — a reference one ulp low reproduces a device's signature
exactly, and a note that gets this wrong measures the device's bug instead of its arithmetic.

## 2. The compensated division on the host, with and without the fused multiply-add

**Measured, in full.** The sequence is Markstein's: `q₀ = fl(a·y)`, then per step the residual
`a − b·q` and the correction `q + r·y`, with the residual the one instruction that has to be
fused. `y` is `fl(1/b)` by construction, and §2's three columns are that reciprocal, that
reciprocal **one ulp below** the nearest, and **two ulp below** — which is
[division-rounding-gap](division-rounding-gap.md) §4's Iris Xe reading and §3's two-ulp lane, and
is the point of the table: **the correction has to survive a reciprocal the device gets wrong.**

*Exact* is lanes whose answer is the correctly rounded quotient; *worst* is the largest ulp
difference on the grid.

| spelling | `fl(1/b)` | `fl(1/b)` − 1 ulp | `fl(1/b)` − 2 ulp |
|---|---|---|---|
| `plain`, `a / b` | **24,576 / 24,576**, 0 | **24,576 / 24,576**, 0 | **24,576 / 24,576**, 0 |
| Markstein, **no** fused multiply-add, 2 steps | 22,555 / 24,576, 1 | 21,113 / 24,576, 1 | 22,125 / 24,576, 1 |
| Markstein with a **Dekker two-product residual**, no fused multiply-add, 2 steps | 22,754 / 24,576, 1 | 19,681 / 24,576, 2 | 19,029 / 24,576, 2 |
| **Markstein on `mul_add`, 1 step** | **24,576 / 24,576, 0** | **24,576 / 24,576, 0** | **24,576 / 24,576, 0** |
| **Markstein on `mul_add`, 2 steps** | **24,576 / 24,576, 0** | **24,576 / 24,576, 0** | **24,576 / 24,576, 0** |
| **Markstein on `mul_add`, 3 steps** | **24,576 / 24,576, 0** | **24,576 / 24,576, 0** | **24,576 / 24,576, 0** |

**Measured: the fused multiply-add is the whole difference, and one step is enough.** No spelling
without it reaches the correctly rounded quotient at any reciprocal quality, and the Dekker
residual — the strongest control the language can build today — is the **worst** of the three once
the reciprocal is degraded. A second run over §1's four-grid set reads the same: **16,384 of
16,384 exact at a zero worst ulp** in all three `mul_add` columns, against **14,824**, **14,033**
and **14,330** without, at a one-ulp worst.

**Two spellings of this probe's own, said plainly so they are not confused.**
[division-rounding-gap](division-rounding-gap.md) §7 reports a fifteen-operation `markstein` that
is **24,576 of 24,576** on the host. That is **not** the row above: this probe's no-`mul_add`
spellings are its own, written for a stack IR with no named local, and they are a different
sequence. **The two figures are not comparable and the note does not compare them.** What §2
establishes is the *relative* statement the comparison cannot: with `mul_add` in place the same
iteration reaches zero, without it it does not, **at every reciprocal quality**, and the control
that recovers the residual exactly by hand does not recover the quotient.

## 3. What the emitted instruction is, and what the validator says

**Measured, by hand.** `Fma` is **`GLSLstd450Fma = 50`** in the SDK's
`GLSL.std.450.h`, and the SDK grammar lists it as three `IdRef` operands — `'a'`, `'b'`, `'c'` —
with **no capability of its own**. The opcodes are `OpExtInst` **12** and `OpExtInstImport`
**11**, read off `spirv.core.grammar.json` and confirmed by the disassembly below. The whole
question is whether `Fma` is expressible at all under `--target-env vulkan1.1`, and a module of
**69 words** is enough to answer it. `C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe` is
**SPIRV-Tools v2024.4**, whose largest Vulkan target is 1.3.

| hand-built module, one `Fma` | `spirv-val --target-env vulkan1.1` |
|---|---|
| the minimal one: `OpCapability Shader`, `OpExtInstImport "GLSL.std.450"`, one `OpExtInst … Fma` | **ACCEPT**, exit `0` |
| the same, with `OpCapability Shader` left out | **REJECT** — `Operand 2 of MemoryModel requires one of these capabilities: Shader` |
| the same, with `OpExtension "SPV_KHR_shader_float_controls"` **instead of** the import | **REJECT** — `OpExtInst set Id 1 does not reference an OpExtInstImport result Id` |
| the minimal one **plus** `OpDecorate %result NoContraction` | **ACCEPT**, exit `0` |
| the minimal one over `OpTypeFloat 64`, **plus** `OpCapability Float64` | **ACCEPT**, exit `0` |

`spirv-dis` prints the minimal module as:

```
               OpCapability Shader
          %1 = OpExtInstImport "GLSL.std.450"
               OpMemoryModel Logical GLSL450
               OpEntryPoint GLCompute %5 "main"
               OpExecutionMode %5 LocalSize 1 1 1
       %void = OpTypeVoid
      %float = OpTypeFloat 32
  %float_1_5 = OpConstant %float 1.5
         %10 = OpVariable %_ptr_Private_float Private %float_1_5
          %5 = OpFunction %void None %4
          %7 = OpLabel
          %6 = OpLoad %float %10
          %9 = OpExtInst %float %1 Fma %6 %6 %float_1_5
               OpReturn
               OpFunctionEnd
```

**Measured: the answer to "does it need an `OpCapability`" is no, and the answer to "does it need
the extension import" is yes.** `Shader` is already declared by every module this backend emits,
so **no capability is added**; the `OpExtInstImport` is not optional and no `OpExtension` stands in
for it. The `Float64` row is a control, not a plan: `Fma` is accepted over a 64-bit float too, and
this language has no `Float64` ([floating-point](floating-point.md) §4.1 settles that), so it costs
nothing here.

**Measured: the `NoContraction` decoration is accepted on an `OpExtInst Fma` result, and is a
no-op.** That matters because [contraction-control](contraction-control.md) §9.1 decorates **every
float result** of a marked body, and this note's §4 module is such a body. `NoContraction` forbids
a driver from contracting an `OpFMul` into an `OpFAdd`; an `OpExtInst Fma` is already fused, so the
decoration asks for nothing. **It is accepted, and it is inert** — which is the right thing for it
to be, since §9.1's guarantee is about the *other* instructions in the body.

**Measured, as three `REJECT`s while the module was being built, and each one is a place the
emitter would have to be right.** The logical layout puts **`OpExtInstImport` before
`OpMemoryModel`** — after it, the validator says `ExtInstImport is in an invalid layout section`.
`OpConstant` is module-scope — inside a function it says `Constant cannot appear in a function
declaration`, which [spirv.rs](../../crates/lichen-compute-gpu/src/spirv.rs)'s own section comment
already states. And a Vulkan entry point takes **no** parameter —
`[VUID-StandaloneSpirv-None-04633]` — which is why the minimal module carries its operand in a
`Private` variable rather than a `FunctionParameter`.

## 4. The same substitution in the shipped emitter's own module

**Measured.** The minimal module answers the validator's question; it does not answer whether the
substitution applies to a module this repository emits. So the probe took the words
`spirv::compile` returned for the compensated division, folded every `OpFMul` that feeds exactly
one `OpFSub` into one `OpExtInst … Fma`, added the import, dropped the four dead multiplies and the
decorations that named ids that are gone, and validated the result.

| the compensated division's module | words | `OpFMul` | `OpFSub` | `OpExtInst Fma` | `OpFNegate` | import | validator |
|---|---|---|---|---|---|---|---|
| as `spirv::compile` emitted it | **482** | 7 | 4 | 0 | 0 | 0 | — |
| with `Fma` folded in | **484** | 3 | **0** | **4** | **4** | **1** | **ACCEPT**, exit `0` |

The folded body is four instructions and their negations:

```
         %83 = OpExtInstImport "GLSL.std.450"
         %85 = OpFNegate %float %58
         %59 = OpExtInst %float %83 Fma %53 %55 %85
         %86 = OpFNegate %float %59
         %64 = OpExtInst %float %83 Fma %86 %62 %47
         %87 = OpFNegate %float %75
         %76 = OpExtInst %float %83 Fma %70 %72 %87
         %88 = OpFNegate %float %76
         %81 = OpExtInst %float %83 Fma %88 %79 %64
```

**Measured: the substitution is a two-word surcharge, not a saving, and it needs a negation the
emitter does not emit.** The two `OpExtInst Fma` a pair replaces cost **8 words each**, the same
as the `OpFMul` + `OpFSub` (**4 + 4**) they replace, so **the arithmetic is free**; the import is
**6 words** and each `OpFNegate` is **4**. Against those, the four dead multiplies and their
decorations come back. The net is **482 → 484**. **This is not the instruction count an emitter
would produce** — §4's body recomputes `q` and `y` on every step because the IR is a stack with no
named local (§7), and an emitter that kept them in SSA ids would write the sequence as **five
float instructions**: one `OpFDiv`, one `OpFMul`, two `OpExtInst` and one `OpFNegate`. The figure
to carry forward is the **6-word import** and the **8-for-8 substitution**, not this body's shape.

**Measured, and this is the finding that is not obvious: `z − (x·y)` is `Fma(−x, y, z)` and
`(x·y) − z` is `Fma(x, y, −z)`, so a fused multiply-add needs an `OpFNegate` whichever side the
multiply is on.** `crates/lichen-compute-gpu/src/spirv.rs` has **no** `OpFNegate` opcode at all —
measured, and it is the same fact [contraction-control](contraction-control.md) §9.4 reports as
"zero `OpFNegate`" in the shipped `sin` module. An `fma` that cannot express `a − b·q` is not the
spelling the division needs, so **the emitter would gain an instruction and an opcode, not one.**

## 5. What it would buy on a device, measured where the shipped path can measure it

**Measured, on both devices, through `GpuContext::run`.** Four grids of 4096 lanes, *exact* against
the §1 reference and *worst* in ulp. `decorated` is
[contraction-control](contraction-control.md) §9.1's `exact_float_arithmetic`, so every float result
carries `NoContraction`; `undecorated` is the same body without it, which is where a driver is free
to contract the residual and the correction on its own.

| spelling | RTX 3060 | Iris Xe |
|---|---|---|
| `a / b`, decorated | **10,916 / 16,384**, worst **2** | **10,176 / 16,384**, worst **2** |
| compensated division, decorated | **10,604 / 16,384**, worst **2** | **9,861 / 16,384**, worst **2** |
| compensated division, undecorated | **11,580 / 16,384**, worst **2** | **6,165 / 16,384**, worst **3** |

| grid | `a/b` RTX | `a/b` Xe | compensated, decorated, RTX | …, Xe | compensated, undecorated, RTX | …, Xe |
|---|---|---|---|---|---|---|
| `dense / 3.0` | 2730, 1 | 2730, 1 | 2730, 1 | 2730, 1 | 3413, 1 | 1365, 1 |
| `dense / 7.0` | 1852, 1 | **1097**, 1 | 1706, 2 | 951, 2 | 1999, 1 | 1684, 2 |
| `dense / 2.5` | 3276, 1 | 3276, 1 | 3276, 1 | 3276, 1 | 3276, 1 | 1638, 1 |
| `ln` band | 3058, 2 | 3073, 2 | 2892, 2 | 2904, 2 | 2892, 2 | 1478, **3** |

**Measured, and it is the probe's own check that the reference is right: the `a / b` rows are
[division-rounding-gap](division-rounding-gap.md) §3's rows to the digit** — `2730`, `1852`, `3276`
on the RTX 3060 and `2730`, `1097`, `3276` on the Iris Xe. Two notes written a fortnight apart
measuring the same grids with two independent correctly-rounded references agree on every cell.

**Measured: no shipped spelling reaches the correctly rounded quotient on either device, and the
compensated division is not an improvement.** Under `NoContraction` it is **worse** than the plain
division on both vendors (10,604 against 10,916 and 9,861 against 10,176) — the residual it
computes is built out of the device's own quotient, which is
[division-rounding-gap](division-rounding-gap.md) §4's `r(b)`, so the correction starts from a
worse place than the division it replaces. §7's own finding, reproduced here on a different grid.

**Measured, and it is the sharpest row in this note: on the Iris Xe, letting the driver contract
the residual makes the compensation worse by a factor of two and adds an ulp** — **6,165 of 16,384
at a three-ulp worst**, against the same module decorated. That is
[contraction-control](contraction-control.md) §2.1's `a·b − c` row showing through a case it did
not measure: the Iris Xe contracts a multiply feeding a subtraction **4096/4096**, the RTX 3060
does not, and so the undecorated compensated division is **better on one vendor and much worse on
the other**. **A fused multiply-add would not merely improve this spelling — on one vendor it would
replace a contraction that is currently making the answer worse.**

## 6. The claim this note cannot make

**Stated plainly: the device-side zero-ulp result is INFERRED, not measured here, and the folded
module was never dispatched.**

**Why.** `GpuContext::run` **compiles the fragment itself** and takes no words: its public surface
is `new`, `run`, `run_over_device_extent`, `submit`, `run_batch`, `run_chain`, `fetch` and
`release`, and none accepts a module. Dispatching §4's folded module would need either a production
change in `dispatch.rs` or a second Vulkan path written for the probe, and **this slice changed no
production file**. So §4 is a **validator** result on a real emitter module and nothing more, and
§5 is the only device measurement in this note.

**What the inference rests on, and both parts are labelled.**

- **Measured (host, §2):** the sequence reaches the correctly rounded quotient on **24,576 of
  24,576** points from a reciprocal that is `fl(1/b)`, **one ulp below** it, and **two ulp below**
  it — the three qualities a device's `r(b)` was measured to take
  ([division-rounding-gap](division-rounding-gap.md) §4: correctly rounded on 64 of 64 divisors on
  the RTX 3060, on 40 of 64 with the other **24 one ulp low** on the Iris Xe, and a two-ulp lane in
  the `ln` band). The bad reciprocal is therefore **an input the fused correction has already
  recovered from**, which is the whole claim the device case turns on.
- **Inferred (definition, §3):** `GLSL.std.450`'s `Fma` is *specified* as one rounding, so a
  conforming device computes it as one. Nothing here measured a device doing so.
- **Not established:** that a driver given a module containing these four `OpExtInst Fma` will not
  rewrite them, will not contract the surrounding arithmetic despite `NoContraction` being on the
  results, and will answer the same bits on a third vendor.

**So the device-side claim to carry forward is: *inferred* from a host experiment with a
deliberately degraded reciprocal, plus a validator that accepts the instruction.** The experiment
that would settle it is one dispatch of §4's module, and it needs a `GpuContext` entry point that
takes words — which is a decision about the backend's surface, not about this note.

## 7. What it would touch, file by file

**No production change is made or proposed here. A new operator in the IR is a contract and
format decision, and the user takes it.**

- **`crates/lichen-kernel-ir/src/lib.rs`, `KernelInstr`.** A **three**-operand variant.
  `arity()` answers `0` for `BufferWriteCall` and `None` for `CallKernel` and `2` for the rest, so
  the new arm joins the one-line group that already carries `Select`; `own_class` answers
  `Some(ScalarClass::Float)` in the group `Exp`…`Cos` already occupy. **It does not go in
  `unary.rs`'s `TRANSCENDENTALS`**: that table is the six operators the IR *defines and inlines*,
  and a fused multiply-add has a native form on one target and none on the other, which is §7's
  test and this one fails it.
- **`crates/lichen-compute-gpu/src/spirv.rs`, three places.**
  - **`mod op`**: `EXT_INST = 12`, `EXT_INST_IMPORT = 11`, `FNEGATE = 127`. **`mod capability`
    gains nothing** — §3 measured that a 32-bit `Fma` needs only the `Shader` this module already
    declares.
  - **`emit_function`**: an arm for the three-operand instruction. **And `OpFNegate`**, which the
    file does not have (§4): a `z − (x·y)` cannot be written without one.
  - **`assemble`**: the `OpExtInstImport` goes **before `OpMemoryModel`**, which in the emitter's
    own numbered sections is ahead of the annotation section §3.4 lists. §3 measured the
    validator's wording for putting it after.
  - **The pipeline cache key is free.** [contraction-control](contraction-control.md) §9.2 already
    settled that a decision in `KernelBody` is inside `fragment_digest`; an operator in the IR is
    the same case, and no `dispatch.rs` change is owed.
- **The language surface, if the route is a named operator** — the same path `ln` took, which is
  the convention [floating-point](floating-point.md) §7 names: a keyword in
  `crates/lichen-language-lex/src/lib.rs`, `parse.rs` and `ast.rs`'s arity table in
  `crates/lichen-language-parser`, `compile.rs` and `resolve/content_key.rs` in
  `crates/lichen-language`, `ir.rs` / `checker/operators.rs` / `program.rs` in
  `crates/lichen-highlevel`, and `compute.rs` / `compute/body.rs` in `crates/lichen-compute`.
  **A tagged `TypeOperator` also reaches `crates/lichen-lowlevel`'s value codec, which is another
  session's crate and this slice did not touch.**
- **`crates/lichen-runtime/src/fused/schema.rs` and `fused/convert.rs`.** A
  `#[facet(rename = "fma")]` variant beside `Ln`, and two arms in `convert.rs`'s
  `schema::KernelInstr` ↔ `ir::KernelInstr` conversion. **The `FORMAT_VERSION` question is
  different here from the one §9.5 recorded**, and the difference is the useful part:
  `exact_float_arithmetic` was a **field**, and a field left out of an old document is a
  **mis-parse**, which is why the version went `2 → 3`. A new **variant** under
  `#[facet(deny_unknown_fields)]` is refused **by name** by an older build, which is the schema's
  own invariant. **Whether the version number moves anyway is the user's call, and this note does
  not make it.**
- **The wasm backend.** **wasm's MVP has no fused multiply-add**, so an `Fma` in the IR would have
  to be **defined** there exactly as `sqrt`, `floor` and `ln` are defined, which is
  [floating-point](floating-point.md) §7's whole reason for preferring a definition. **That is a
  second definition to keep in agreement with a device's, and its accuracy is a separate
  measurement that this note did not take.** This is the largest single item on the list and it is
  not in the emitter.
- **`wontfix:D<n>`.** **None is involved.** Re-checked on this branch:
  `docs/notes/code-audit.md` carries **no decision over the emitter, the extension set or the
  transcendentals**. The two decisions that exist near it are `wontfix:D9` (the frontend's
  nesting-depth limit) and `wontfix:D17` (a `compute.write` the codomain does not reach), and
  neither covers an instruction set.

## 8. What it would cost

| | figure | measured where |
|---|---|---|
| the `OpExtInstImport "GLSL.std.450"` | **6 words, once per module that uses it** | §3, read off the emitted words |
| one `OpExtInst … Fma` against the `OpFMul` + `OpFAdd` it replaces | **8 words against 8 — free** | §3 |
| an `OpFNegate` | **4 words per negation**, and a subtraction needs one | §4 |
| a new capability | **none** | §3 |
| a pipeline-cache change | **none**, if the operator rides in the IR | §7, by §9.2's precedent |
| a `FORMAT_VERSION` bump | **the user's**, and unlike §9.5's it is not forced by a mis-parse | §7 |
| the compensated division's float instructions | **five** against nine written without a local, or fifteen in §7's spelling | §4, §2 |
| a definition for the wasm backend, and its own accuracy measurement | **not measured** | §7 |

**Read as one line: the module costs six words and the vocabulary costs a definition on a second
backend plus a decision about an artifact format.** Neither is large; the second is not the
emitter's to make alone.

## 9. What is measured, what is inferred, what is unknown

**Measured, in full.** §1's four host facts and the 200,000-sample reference check; §2's five
spellings × three reciprocal qualities × 24,576 points, and the four-grid repeat; §3's five
hand-built modules under `--target-env vulkan1.1` with each verdict quoted, the two word counts,
the enumeration value and the three layout refusals; §4's rewrite of the shipped emitter's own
module with its census, its disassembly and its `ACCEPT`; §5's three spellings × four grids × two
devices, and the agreement with `division-rounding-gap` §3's rows to the digit.

**Inferred, and not re-derived here.** That a device would reach the correctly rounded quotient
from §4's module — §6 states both halves and neither is a device measurement. That the two probes
in this note measured the same reference: §5's `a/b` rows agreeing with §3's is evidence for it,
not a proof.

**Unknown, and this slice closed none of these.**

- **Whether any device, on any vendor, answers `OpExtInst Fma` in one rounding.** Two devices, two
  vendors, and this note ran neither of them on the folded module.
- **Whether a driver would rewrite the `Fma` or reassociate around it** under `NoContraction` on the
  results — §3 measured the decoration is *accepted* and *inert on paper*, and acceptance is not
  behaviour.
- **What a fused multiply-add costs on wasm.** §7 says the IR would have to define it; its
  accuracy on that target is unmeasured, and [floating-point](floating-point.md) §7 is the
  precedent for how a definition's accuracy is established.
- **Whether the `NoContraction` guarantee survives an `OpExtInst` in a marked body.** §3's decorated
  module validates; whether both vendors still reproduce the definition bit for bit with a fused
  instruction inside the body is §9.3's question with a new operand shape, and it was not run.
- **Whether a reciprocal worse than two ulp low still converges.** §2's degradation stops where
  `division-rounding-gap` §3 measured the hardware going.

## 10. What this slice deleted

**One probe, and it is gone.** `crates/lichen-compute-gpu/examples/fma-probe.rs` — the scratch
driver that ran every §2 and §5 measurement, carried the correctly-rounded reference, the word
level decode, the `Fma` fold and the dead-code pass — was **deleted before this note's commit**. It
held no production code and nothing else referenced it. **No example file was edited**, no
`output =` line moved, and **`git diff dev...HEAD -- examples/` is empty**. The two host probes and
the five hand-built modules lived under `target/`, which is not a tree, and are left there.
