# The fused multiply-add: what it is, what it would touch, and what is not measured

> Status: **shipped**, by the user's decision, in `feature/fmas`. §1–§10 are the **proposal** and
> are left as they were measured; **§11 is the landed state and the device measurement that was
> missing**, and where §6 and §9 say a thing is *inferred* or *unknown*, §11 says what was actually
> run. Every §11 figure was measured on **two** devices off this branch by running **the same
> binary twice** with `VK_DRIVER_FILES` pointed at one vendor's ICD manifest and nothing else:
> `…\DriverStore\FileRepository\nvamsi.inf_…\nv-vk64.json` for the
> **NVIDIA GeForce RTX 3060 Laptop GPU** and `…\DriverStore\FileRepository\iigd_dch.inf_…\igvk64.json`
> for the **Intel(R) Iris(R) Xe Graphics**. The probe is
> `crates/lichen-compute-gpu/examples/fma_probe.rs`.
>
> **The one-sentence result: the device reaches the correctly rounded quotient.** §11.3 measures
> the compensated division written on `KernelInstr::Fma` at **4096 of 4096 bit for bit, a zero
> worst ulp**, on **both** vendors, on all four of §1's division grids, **decorated and
> undecorated** — against a plain division beside it that answers **2730 / 1852 / 3276** on the
> RTX 3060 and **2730 / 1097 / 3276** on the Iris Xe, which are [division-rounding-gap](division-rounding-gap.md) §3's rows **to the digit**.


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

## 11. What landed, and what it measures on a device

> **This section is the landed state.** Sections 1–10 are the proposal, measured on `dev@12425166`
> and left as they were; where they say "inferred" or "unknown", this section says what was run.

### 11.1 The instruction, and where each backend answers it

**`KernelInstr::Fma` is `A·B + C` over three `Float`s, rounded once**, a **three-operand variant**
beside `Bin` in `crates/lichen-kernel-ir/src/lib.rs` — `arity()` answers `3` in the group `Select`
already occupies and `own_class()` answers `Some(ScalarClass::Float)`. **It is not a
`TRANSCENDENTALS` entry**, and the reason is the one §7 gave: that table is the six operators the IR
*defines and inlines*, and a fused multiply-add is native on one target and absent on the other,
which is §7's own test and this one fails it.

| where | what it does | measured where |
|---|---|---|
| `crates/lichen-compute-gpu/src/spirv.rs`, `emit_function` | one `OpExtInst %float %glsl_std_450 Fma %a %b %c`, all three operands forced to `Float` through `as_class`, and the result decorated like any other float result | §11.2 |
| `crates/lichen-compute-gpu/src/spirv.rs`, `assemble` | `%glsl_std_450 = OpExtInstImport "GLSL.std.450"`, **section 2, ahead of `OpMemoryModel`**, and **only when a body emitted an `OpExtInst`** | §11.2 |
| `crates/lichen-compute-gpu/src/spirv.rs`, `operand_class` | `Some(ScalarClass::Float)` at all three positions, so a block parameter an `Fma` reads is classed `Float` | the emitter builds |
| `crates/lichen-runtime/src/wasm/lower.rs` | **refused by name**, with the reason in the error | §11.5 |
| `crates/lichen-runtime/src/wasm/mixed.rs` | all three operands checked against `Float` before that | §11.5 |
| `crates/lichen-fusion-bridge/src/lower.rs` | **`BridgeRefusal::FusedMultiplyAdd`** | §11.5 |
| `crates/lichen-runtime/src/fused/schema.rs` + `convert.rs` | `#[facet(rename = "fma")] Fma`, and two arms in the two-way conversion | §11.6 |

**`OpFNegate` was measured to be needed and is not there.** §4's finding stands and §11.2 confirms
it: **the compensated division's residual `a − b·q` is `Fma(−b, q, a)`, so a subtraction does need
a negation** — and `spirv.rs` still declares **no `OpFNegate` opcode at all**, because
`KernelBin` has **no negation either** and `Fma` is `A·B + C`. The residual is therefore spelled
`Fma(Bin(Float, Sub)(+0.0, b), q, a)` in the probe, and §11.3 measures that to be exact on
**every lane of every grid**: `fl(+0.0 − b)` is `−b` for every divisor the language admits, because
a zero divisor is undefined. **The cost of that spelling is four words, the same four an
`OpFNegate` would have cost**, and it is a subtraction rather than a negation — which is a trap
at `b = +0.0` and nowhere else.

### 11.2 The modules, through `spirv-val --target-env vulkan1.1`

**Measured, six modules, every one this slice could emit.** `NoContraction` is `exact`, the body's
own `exact_float_arithmetic`; the census is `OpExtInst` / `OpExtInstImport` / `OpDecorate`, and the
id bound is the module's fourth word.

| module | words | `OpExtInst` | `OpExtInstImport` | `OpDecorate` | id bound | `spirv-val` |
|---|---|---|---|---|---|---|
| `a*b + c` on `Fma`, `NoContraction` | **284** | 1 | 1 | 13 | 52 | **ACCEPT** |
| `a*b + c` on `Fma`, undecorated | **281** | 1 | 1 | 12 | 52 | **ACCEPT** |
| `a*b + c` as `Bin` twice, `NoContraction` | **283** | **0** | **0** | 14 | 53 | **ACCEPT** |
| compensated division, `NoContraction` | **303** | 3 | 1 | 15 | 54 | **ACCEPT** |
| compensated division, undecorated | **288** | 3 | 1 | 10 | 54 | **ACCEPT** |
| plain `a / b`, `NoContraction` | **248** | 0 | 0 | 11 | 48 | **ACCEPT** |

**`spirv-dis` prints**, from the two modules that use the instruction:

```
         %1 = OpExtInstImport "GLSL.std.450"
        %50 = OpExtInst %float %1 Fma %43 %46 %49
```

```
         %1 = OpExtInstImport "GLSL.std.450"
        %49 = OpExtInst %float %1 Fma %42 %48 %float_0_0
        %51 = OpExtInst %float %1 Fma %50 %49 %42
        %52 = OpExtInst %float %1 Fma %51 %48 %49
```

**Measured, and this corrects §8's one row.** §8 said the substitution is "8 words against 8 —
free". Through the emitter it is **284 words against 283**, decomposed exactly:
`+6` for the import, `−2` because an `OpExtInst Fma` is **8 words and the `OpFMul` + `OpFAdd` it
replaces is 10**, and `−3` because the product the unfused body had to decorate is **no longer a
value**, so one `NoContraction` fewer is written. **One word, for a body whose arithmetic drops
from two float instructions to one** — and the import is charged **once per module**, not once per
instruction: the compensated division carries **three** `OpExtInst` and **one** import.

**Measured, the two things §3 and §9 left open about the decoration.** A `NoContraction` on an
`OpExtInst Fma` result **validates** (§3's claim, now through the emitter rather than a hand-built
module), and **§9's open question is answered in §11.3: the guarantee survives** — the decorated and
undecorated bodies return the identical bits on every grid on both vendors, which is what an inert
decoration looks like.

### 11.3 The device measurement

**Measured on both vendors, on the grid §1 names** — numerators `1 + i/2^12` for `i < 4096`, four
divisor sets, and three product grids. The reference is the **host's own `f32::mul_add`** for the
product and the **host's `a / b`** for the division, which §1 measured to be the correctly rounded
quotient on 200,000 of 200,000 samples.

**`a*b + c`, against the host's `f32::mul_add`.** *Bit for bit* is the count of lanes whose bits
are the host's; *discriminating* is the count of lanes where `mul_add(a,b,c)` and `a*b + c` differ,
which is the set the grid was chosen to produce.

| grid | discriminating lanes | RTX 3060, fused | Iris Xe, fused | RTX, `Bin` twice | Iris, `Bin` twice |
|---|---|---|---|---|---|
| `product` (`a`, `b`, `c` three strides of the grid) | **0** | **4096 / 4096**, 0 ulp | **4096 / 4096**, 0 ulp | 4096 / 4096, 0 ulp | 4096 / 4096, 0 ulp |
| `cancel` (`c = −(a·b)`, rounded) | **1255** | **4096 / 4096**, 0 ulp | **4096 / 4096**, 0 ulp | 2841 / 4096 | 2841 / 4096 |
| `band` (`a` in `[1/√2, √2)`, `b = 3.0`) | **647** | **4096 / 4096**, 0 ulp | **4096 / 4096**, 0 ulp | 3449 / 4096, 1 ulp | 3449 / 4096, 1 ulp |

**Measured: the two devices agree with the host bit for bit on every discriminating point, and the
control is what makes the row mean anything** — on `cancel` the *unfused* body gets 2841 of 4096 and
on `band` 3449, because the driver is not contracting a multiply into an add it was told not to.
The `product` row is reported as it came out: **it discriminates nothing**, because three values in
`[1, 2)` leave the addend large enough that the second rounding never bites. It is a control, and it
is stated as one.

**The compensated division, against the correctly rounded quotient.** One step: `q0 = Fma(a, y, 0)`,
`r = Fma(−b, q0, a)`, `q1 = Fma(r, y, q0)` — §2's `Markstein on mul_add, 1 step`, written in the IR.

| grid | RTX 3060, compensated | Iris Xe, compensated | RTX, plain `a / b` | Iris, plain `a / b` | host, same sequence |
|---|---|---|---|---|---|
| `dense / 3.0` | **4096 / 4096, 0 ulp** | **4096 / 4096, 0 ulp** | 2730, 1 ulp | 2730, 1 ulp | 4096 / 4096, 0 ulp |
| `dense / 7.0` | **4096 / 4096, 0 ulp** | **4096 / 4096, 0 ulp** | 1852, 1 ulp | **1097**, 1 ulp | 4096 / 4096, 0 ulp |
| `dense / 2.5` | **4096 / 4096, 0 ulp** | **4096 / 4096, 0 ulp** | 3276, 1 ulp | 3276, 1 ulp | 4096 / 4096, 0 ulp |
| `ln` band, divisor on the lane | **4096 / 4096, 0 ulp** | **4096 / 4096, 0 ulp** | 3000, 2 ulp | 3001, 2 ulp | 4096 / 4096, 0 ulp |

**Every cell of the two plain-division columns is [division-rounding-gap](division-rounding-gap.md)
§3's, to the digit, on both vendors** — including the one that distinguishes them, `1097` against
`1852` on `dense / 7.0`. Three notes written a fortnight apart, with two independent
correctly-rounded references and two different probe drivers, agree on every cell.

**Measured, and this is the result the whole thread was for: a compensated division written on the
instruction reaches the correctly rounded quotient on the device, on both vendors, on every grid,
with and without `NoContraction`.** §5's table is the shape of what that replaced — **10,604 of
16,384** on the RTX 3060 and **9,861 of 16,384** on the Iris Xe, both at a **two-ulp worst**, both
*worse* than the plain division they were meant to fix — against **4096 of 4096 at zero** here. The
Iris Xe is the sharper half of the claim: it is the device whose `r(b)` is **one ulp low on 24 of
64** divisors, and §2 established on the host that Markstein on `mul_add` recovers the quotient
**exactly from a reciprocal one and two ulp low**. The device needed no degraded reciprocal to be
handed — its own `OpFDiv` supplied the bad one.

**Measured, and it is the one thing a validator cannot tell you.** Both vendors answer
`OpExtInst Fma` in **one rounding**, on every point, and neither driver reassociates around it
under `NoContraction`. §6 listed three things as "not established"; two of them are now measured on
two vendors.

### 11.4 What was *not* changed, and what wiring it into `ln` would touch

**`ln`'s body was not touched, and this is the user's decision rather than this slice's.**
`docs/notes/operator-reverification.md` records that `ln` is the transcendentals' only body with an
`OpFDiv` and that it is **1 ulp wrong on the Iris Xe**. Wiring `Fma` into it would touch:

- **`crates/lichen-kernel-ir/src/unary.rs`, `ln_body` and `ln_value`.** The two-term reduction's
  residual is the one instruction that must be fused, and §3 of [floating-point](floating-point.md)
  writes it with `Bin` twice. `ln_value` is the host's own reading of that body — the definition the
  emitters inline — so **changing the body changes the reference**, and
  `spliced_transcendentals.rs` is held to **`0` ulp** against it.
- **`crates/lichen-fusion-bridge/src/lower.rs`'s `inlined_transcendental` reader** and the
  `UnaryOp::Ln` projection beside it, since the fragment a dispatcher registers would no longer
  hold the sequence `kernel-ir` wrote.
- **The pipeline cache.** `contraction-control.md` §9.2 already settled that a decision inside
  `KernelBody` is inside `fragment_digest`, and an `Fma` inside a body is the same case — so the
  key moves by construction and `dispatch.rs` is untouched. **This slice measured that: no
  `dispatch.rs` line changed.**
- **`crates/lichen-compute-gpu/tests/compute_gpu/spliced_transcendentals.rs`**, whose bound is the
  thing that would move.

**It would change `ln`'s bits on both vendors**, which is why it is a separate decision and not a
consequence of shipping the instruction.

### 11.4a What wiring it in would buy, and the identity that makes it a construction

§11.4 says it would change `ln`'s bits and leaves the size open. The size is **0 ulp**, and the argument is
a line-by-line identity rather than a measurement of the composite:

| step | `ln_body` (the device's sequence) | `ln_value` (the host reference) | same? |
|---|---|---|---|
| reduction | `ln_scale` × (5 coarse + 32 fine + 5 unit), same constants, same comparisons | `ln_reduce_value`, same order, same constants | yes |
| the quotient | `(value - 1) / (value + 1)` | the same division | yes |
| the series | Horner over `LN_COEFFICIENTS[..9]` in reverse | the same loop, same order | yes |
| the answer | `(z * 2) * series + track * LN_TWO` | `2.0 * z * series + track * LN_TWO`, same association | yes |

**The one operation that differs is the quotient**, which is exactly what `ln_body`'s own `# Invariant`
states. So once the quotient is the correctly rounded one, every later operation receives the same inputs on
both sides and rounds the same way — the device's answer becomes the host's **bit for bit**, not merely close.
Two facts carry that:

- the compensated division is **4096 of 4096, worst 0 ulp** on precisely this band (`[1/sqrt 2, sqrt 2)`,
  §11.3's `band` grid), so the quotient it produces is the correctly rounded one;
- **default Rust does not contract** `a * b + c` into a fused multiply-add, so the host side rounds the way
  the decorated device does.

**Which is why both sites move together.** Changing `ln_body` alone would leave `ln_value` on the old sequence,
and `spliced_transcendentals` compares the device against `ln_value` — the bound would then be broken by the
**reference** rather than by the device.

**What this does not measure:** the composite on the device. The claim is an inference from the quotient's
measurement plus the identity above, and it is labelled as one. It is falsifiable in one run: if the rewired
body is not bit-identical to `ln_value` over the band, one of the two facts above is wrong.

**And the `# Invariant` above `ln_body` would become false.** That sentence states the up-to-a-unit-in-the-last-
place cross-backend divergence; a correctly rounded quotient removes it, so the sentence needs editing — and
that is an `# Invariant` change, so it is the owner's decision, taken with the rewiring itself rather than
after it.

### 11.5 What the two backends that cannot run it do

**Both refuse it by name, and the brief's own question — whether `sqrt`/`floor`/`ln` are host calls
on the wasm path or refusals — has the answer: refusals.** `lower_instr`'s arms for the six
transcendentals return an error naming `KernelBody::expand_transcendentals`; they are not host
calls. The mechanism for an instruction wasm has none is therefore **the refusal**, and `Fma` gets
one:

```
compute.wasm: `Fma` is a fused multiply-add, and wasm's MVP has no instruction for one. A
multiply followed by an add rounds twice, which is a different program, so there is nothing
here to lower it to — and the definition `KernelBody::expand_transcendentals` inlines for what
this target lacks cannot be written out of two roundings either
(`docs/notes/fused-multiply-add.md` §7)
```

**The definition was considered and is wrong, and the reason is the measurement, not taste.**
[floating-point](floating-point.md) §7's rule is that the IR *defines* what the two targets cannot
agree on natively, so that both compute the same bits. **A fused multiply-add cannot be defined out
of two roundings** — Dekker's two-product recovers the product's residual exactly, and §2's table
is that the quotient it corrects is still **19,029 of 24,576 at a two-ulp worst**. An IR definition
would therefore be a *different program* from the one the GPU runs, which is the failure
[floating-point](floating-point.md) §7's rule exists to prevent. **A named refusal is the honest
answer and a silently unfused module is not.**

The fusion bridge refuses it too, for the same shape of reason in reverse: over the reals
`A·B + C` and a multiply then an add are **one number**, so the symbolic language cannot hold the
two apart, and projecting it would let the bridge certify a fusion the device would not reproduce.

### 11.6 The artifact format, and the tests

**`FORMAT_VERSION` was not bumped, and §7's analysis is the reason.** `Fma` is a **variant** under
`#[facet(deny_unknown_fields)]`, not a field: an older build reads a document carrying it and
**refuses it by the name `"fma"`**, which is the schema's own invariant, where the field
`exact_float_arithmetic` that took the version `2 → 3` would have been **mis-parsed**. The version
moves for a mis-parse, and this is not one. **Whether it moves anyway is the user's call, and this
slice did not make it.**

**No test was added.** Two **existing** tests carry an exhaustive match over `KernelInstr` — a
tripwire the tree put there on purpose ("an instruction added to `KernelInstr` is a compile error
here rather than a variant the ledger silently leaves out") — so both needed an arm to keep the
crate building: `class_agreement.rs`'s `every_variant`/`named`/`unasked` ledger, and
`gpu_matches_cpu.rs`'s hand-written reference, which reads `Fma` as the **host's `a.mul_add(b, c)`**
so the reference rounds once as the instruction promises. **Neither is a new assertion.**

**A pin is worth having and this slice did not write one.** The strongest candidate is
`spirv_validation.rs`: a `Fma` module beside the two `Bin` ones, validated under
`--target-env vulkan1.1`, which would hold the emitter to §11.2's six `ACCEPT`s. **That is a test,
and tests are the user's to authorise.**

### 11.7 What is still open

- **The two refusals are named, not resolved.** A body holding an `Fma` does not run on the wasm
  backend at all, so the language surface that would reach it — a keyword, an arity, a tagged
  `TypeOperator`, a `TypeOperator` arm in the value codec — is **unbuilt and unbuilt on purpose**.
- **The `0 − b` spelling is a subtraction, not a negation.** It is exact for every divisor the
  language admits and it is four words, but a `KernelInstr::Negate` and an `OpFNegate` would say
  what it means. **That is a second instruction and the user's to authorise.**
- **Two vendors were measured.** Whether a third answers `OpExtInst Fma` in one rounding, and
  whether any driver reassociates around one, is not.
- **The wasm refusal has no accuracy question attached**, because there is no definition to
  measure — which is §9's third bullet answered by *not shipping a definition*, with the reason.

