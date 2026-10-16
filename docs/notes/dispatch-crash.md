# The sampler's crash, localised to one emitted id

> Status: **the defect is fixed** — §7 records the repair, its branch (`feature/fcond`) and the
> sampler's device result. Everything below §7 is the localisation, measured on an RTX 3060 Laptop
> GPU and an Intel Iris Xe on this machine off `dev@8e6095b5`, on branch `feature/crash`, and is
> kept as it was written: **that pass changed no production code and added no test.**

[device-leg-of-the-ladder](device-leg-of-the-ladder.md) §1 left the greedy sampler as *"the
emitter is not at fault, the crash is at dispatch"* and *"Unknown: why the driver dies"*, with
the stated reason that **"`spirv-val` is not installed"**. **Both halves of that are wrong, and
this note is the correction.** The emitter *is* at fault, in one line, and a validator is
installed.

**The short answer.** The module `spirv::compile` emits for the sampler **references an id that
no instruction defines**. That is a malformed module, `spirv-val` rejects it by name, and the
crash is a driver's shader compiler dereferencing the dangling id — not a driver bug on a good
module, and not a disagreement about arithmetic.

## 1. A validator was available, and it reads the words

**Measured: `spirv-val` is installed on this machine** at
`C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe`, together with `spirv-dis`, `spirv-opt`, `spirv-as`
and `glslangValidator`. The repository already depends on it by name —
`crates/lichen-compute-gpu/tests/compute_gpu/spirv_validation.rs:23` spells `spirv-val` and
*skips* when it is absent. **So the previous slice's tool was on `PATH` and its skip was read as
an absence.** `crates/lichen-compute-gpu/examples/emit-spv.rs` likewise already existed to dump
a module for exactly this.

Two independent readings agree, and neither is an inference from a crash code:

| reader | answer on the 444-word sampler module |
|---|---|
| `spirv-val --target-env vulkan1.1` | **REJECT** — `ID '10[%10]' has not been defined` / `%75 = OpSelect %uint %74 %uint_1_0 %10` |
| Khronos validation layer (`VK_LAYER_KHRONOS_validation`) at `vkCreateShaderModule` | **ERROR/SPEC** — `VUID-VkShaderModuleCreateInfo-pCode-08737`, *"`pCreateInfo->pCode` (spirv-val produced an error)"* |

The layer's message quotes the same sentence as `spirv-val` because the layer **shells out to
`spirv-val`**. They are one reading, not two — worth saying, because "two validators agreed"
would have overstated it. **What is independent is the third column below**, which is the
driver's own reaction.

## 2. The crash is the driver's compile of a malformed module

**Measured.** Handing the raw words to Vulkan with no pipeline of ours in the way puts the crash
at a named call, on **two different vendors' devices**:

| step | Intel Iris Xe | NVIDIA RTX 3060 Laptop GPU |
|---|---|---|
| `vkCreateShaderModule` | **OK** | **OK** |
| `vkCreateComputePipelines` | **crash `0xC0000005`** | **crash `0xC0000005`** |

`GpuContext::new` picks the discrete GPU (`dispatch.rs:638-651`), so the device
[device-leg](device-leg-of-the-ladder.md) §1 measured is the RTX 3060; this machine's index 0
is the Intel Iris Xe, and **it dies on the same words**. A defect in one vendor's compiler
would not usually take both, and neither dies at `vkCreateShaderModule`, which is where the
validation layer caught the module. **This is the measurement that separates "our module is
malformed" from "this driver has a bug"** — and it answers the environmental question: the
module is malformed, so no driver is required to misbehave for the process to die.

## 3. The module, and the one id

**Measured.** The committed `examples/sample.lichen:141-146` `greedy_raw` emits **444 words**,
`Bound: 87`. Ids 8–12 are the module's scalar constants:

```
%8  = OpConstant %uint 0      %9  = OpConstant %uint 1
%10 = <nothing>               %11 = OpConstant %float 1     %12 = OpConstant %float 0
```

and the fold's step ends:

```
%74 = OpFOrdGreaterThan %bool %73 %57
%75 = OpSelect %uint %74 %uint_1_0 %10      <-- %10 is never defined
%76 = OpIMul %uint %uint_1_1 %75
```

`%10` is named **once** in the whole module and defined **nowhere**. `spirv-dis --raw-id`
confirms: no instruction has `%10` as its result.

**Measured: the defect is in the constant pool, and it is one missing instruction.**
`as_class`'s `Kind::Condition` arm (`crates/lichen-compute-gpu/src/spirv.rs:3517-3531`)
materialises a boolean into a scalar class with exactly the shape the module shows:

```rust
Kind::Condition => {
    ...
    code.push(Inst::new(op::SELECT, vec![
        ids.type_of(class), result, slot.id, ids.one_of(class), ids.zero_of(class),
    ]));
```

`ids.one_of(Int)` is `%9` and **is** emitted; `ids.zero_of(Int)` is `%10` and **is not**. The
pool that emits them is `spirv.rs:3338-3368`, and its two arms disagree:

| `ids.class` | `one_integer` | `zero_integer` |
|---|---|---|
| `ScalarClass::Int` (`spirv.rs:3340-3347`) | emitted | emitted |
| `ScalarClass::Float` (`spirv.rs:3366`) | **emitted** (`vec![ids.uint, ids.one_integer, 1]`) | **never emitted** |

**So the rule the module needs is: *a float module that materialises a boolean into the integer
class* names a constant the float arm of the pool does not declare.** That is the feature, and
it is nameable, specific, and a defect in the emitter.

**Measured: why only this shape reaches it.** The float arm declares `one_float` and
`zero_float`, so a boolean materialised into **float** arithmetic — the straight-line
`1.0 * (a > 0.0)` in the table below — is fine. The sampler reaches the **integer** class
because its lerp is integer arithmetic on `s(0) - 1` and `s(2)`, and that is exactly what the
`int2float` on the fold's result then converts. The comparison itself must be a **float**
comparison: a fold whose comparison is `Int` is refused earlier, by the lowering's
block-parameter class rule, before anything is emitted.

## 4. Module reduction

**Measured.** Every row is the same 444-word module with one patch, re-dispatched through raw
Vulkan on the RTX 3060. `OpSelect` is opcode 169; `result %75`'s `else` arm is operand 5.

| # | patch | words | `spirv-val` | `vkCreateComputePipelines` |
|---|---|---|---|---|
| M0 | none | 444 | REJECT | **crash `0xC0000005`** |
| M1 | `%75` operand 5: `%10` → `%8` (a `uint 0`) | 444 | ACCEPT | survived |
| M2 | `%75` operand 5: `%10` → `%46` (the other `uint 0`) | 444 | ACCEPT | survived |
| M3 | `%75` operand 5: `%10` → `%9` (a `uint 1` — **wrong value**, but defined) | 444 | ACCEPT | survived |
| M4 | `%75` operand 3 (the condition): `%74` → `%68`, **`%10` left alone** | 444 | REJECT | **crash `0xC0000005`** |
| M5 | **insert** the missing `OpConstant %uint %10 0` (4 words), change nothing else | 448 | ACCEPT | survived |

**The narrowest set whose removal stops the crash has cardinality one: the single operand
`%10`.** M4 is what makes that a finding rather than a coincidence — it rewrites a *different*
operand of the *same* `OpSelect` to a perfectly defined id, and the module still crashes; M3
rewrites `%10` to an id that gives the **wrong answer** and the module is accepted. So what the
driver dies on is not the value, not the `Select`, and not the comparison: it is the
**reference**.

M5 is the complement and the confirmation: rather than avoiding the dangling id, **define it**.
Four added words and nothing else changed, and the module validates and compiles.

## 5. Body reduction

**Measured.** Each body is run through the installed backend, its module dumped, and the module
re-read by `spirv-val` and by the driver.

| body | what changed | words | `spirv-val` | driver | full dispatch |
|---|---|---|---|---|---|
| `greedy_raw` (`sample.lichen:141`) | — | **444** | REJECT | crash | `0xC0000005` |
| b3 | drop the `max` combine, keep the third slot | **429** | REJECT | crash | `0xC0000005` |
| b6 | `>` → `>=` (the predicate variant) | 444 | REJECT | crash | `0xC0000005` |
| b4 | the same comparison **outside** a `@loop` | 252 | ACCEPT | ok | ran `[1.0, 1.0, 1.0, 1.0]` |
| control | `a + a`, one lane | 248 | ACCEPT | ok | ran `[6.0, 18.0, 8.0, 18.0]` |
| b2 | two-slot fold | — | — | — | **refused before dispatch** |
| b5 | no buffer read at all | — | — | — | **refused before dispatch** |
| b7 | an `Int` comparison in the fold | — | — | — | **refused before dispatch** |

**The minimal crashing body reached is b3, at 429 words**, and it is the committed sampler with
the `max` removed. Three things this rules out, each by a row rather than by argument:

- **Not the predicate.** b6 writes `>=` and crashes identically; §4's M1–M3 already showed the
  value is irrelevant. No `OpFOrd*`/`OpFUnord*` variant is the feature.
- **Not "a comparison multiplied into arithmetic".** b4 does exactly that, outside a loop, and
  it **validates and runs**. **The `@loop` is required.**
- **Not `max`, not `int2float`, not the `-inf` seed, not the lerp's value** — each was removed
  in turn by the earlier slice and each ran; b3 removes the `max` and still crashes.

**Three honest negatives.** b2, b5 and b7 are all refused *before emission* by the same
lowering rule — *"the block parameter `ValueId(n)` is read as one class in one place and another
in another (`[Int, Float]`)"* — so **429 words is the smallest crashing body that could be
reached, not a proven minimum.** b2 wants a two-slot fold whose accumulator is `Float` while its
counter is `Int`; b5 wants a float slot carrying a fold seed; b7 wants an `Int` comparison in a
module whose class is `Float`. Each is the class rule, not the crash, and none of them can say
whether the minimum is smaller than 429.

## 6. What a fix would touch

**Not touched by that slice.** It localises; the fix is a separate slice and the user's decision.
**§7 records what the fix turned out to be.**

**The emitter, one line, in `crates/lichen-compute-gpu/src/spirv.rs:3357-3367`** — the
`ScalarClass::Float` arm of the constant pool omits `zero_integer` while naming
`ids.one_integer`. Emitting the missing `OpConstant` there is what M5 shows is sufficient.
Whether the right repair is *emit it*, or *share one pool between the two arms*, or *assert at
`assemble`'s end that every operand names a defined id*, is a design question this note does not
decide.

**Explicitly not implicated, each on evidence:**

| candidate | verdict | evidence |
|---|---|---|
| the dispatch path (`dispatch.rs`) | **not implicated** | it hands `spirv::compile`'s words to `create_shader_module` unchanged; the module is already malformed before any of its own logic runs, and M5 fixes it with the dispatch untouched |
| a capability declaration | **not implicated** | the module declares only `OpCapability Shader` and `OpTypeFloat 32`; M1/M2/M3 change one operand id and nothing about capabilities, and M5 adds a constant |
| the device layer / this driver | **not implicated** | two vendors' compilers die on the same words, and every variant with a defined operand compiles on both |
| `code-audit.md` `wontfix:D17` | **does not apply** | D17 is that a `compute.write` is the value of an output position and not an effect. It governs *which* slot a write fills; this is a **missing `OpConstant` in a module that does declare its slots**. Read as a contract, it is untouched here |
| the `-inf` seed, `max`, `int2float`, the lerp | **each ruled out** | §5 |

**What let this ship** is worth one line: `spirv_validation.rs` validates hand-built bodies and
`emit-spv.rs` dumps one, but **no test in the tree emits a float module that materialises a
boolean into the integer class**, which is the one shape that reaches the missing constant. That
is the coverage gap, and closing it needs the user's permission like any other test.

## 7. The fix, and the sampler's device result

**Measured on `feature/fcond`, off `dev@b80c6cea`, on the RTX 3060 Laptop GPU.** The question §6
left open — *emit it, share one pool, or assert at the end* — is answered by the first two
together, because the cause was not one missing line but **an asymmetry between two arms of one
match**, and a third copy of the same width rule.

**The repair.** `spirv.rs`'s pool declared the two constants differently per module class:

| `ids.class` | `one_integer` | `zero_integer` | `one_float` | `zero_float` |
|---|---|---|---|---|
| `Int` | emitted | emitted | emitted | emitted |
| `Float` | emitted | **never emitted** | emitted | emitted |

Both arms are now **one shape with no `match` at all**: the float pair is class-independent
(`OpTypeFloat 32` is declared in every module, §4's M1–M3 change nothing about capabilities), and
the integer pair is written at the width `Ids::integer` names through one new helper,
`Ids::integer_constant`. `Literals::get` spelled that width a third time and now calls the same
helper, so a pool constant and a literal cannot be written at different widths again. **The
sampler was not special-cased**: the four constants are declared unconditionally, which §3's M5
already showed is sufficient.

**The sweep, arm by arm.** `ScalarClass` has two variants and two asks of the pool — `one_of` and
`zero_of` — asked from `as_class`'s `Kind::Condition` arm (for whatever class that call carries),
from `as_condition`'s `Kind::Scalar(Int)` arm, and from `as_condition`'s `Kind::Literal` arm:

| arm of the pool | `as_class(_, Float)` | `as_class(_, Int)` | `as_condition(Scalar(Int))` | `as_condition(Literal)` |
|---|---|---|---|---|
| `Int` | declared | declared | declared | declared |
| `Float` | declared | **`zero_integer` missing** | **`zero_integer` missing** | **`zero_integer` missing** |

**One defect, three roads.** §5's note that `as_condition` "also names `ids.zero_of(Int)`" is a
**second reachable route to the same id**, not a separate bug: it emits
`OpINotEqual %bool %r %10` in a float module exactly as `as_class` emits its `OpSelect`. Both are
reachable — `as_condition` is called from `I32WrapI64`, from a `Select`'s selector, and from a
`CondBr`'s condition — and both are now covered by a test. `as_condition`'s `Kind::Scalar(Float)`
arm uses `ids.zero`, which the pool has always declared in both modules.

**The test, and what it saw before the fix.** `crates/lichen-compute-gpu/tests/compute_gpu/
materialised_integer.rs` emits two float modules — the sampler's shape in miniature, and the
`as_condition` route — and asserts `spirv-val`'s verdict through `spirv_validation.rs`'s own
helper. Against **this note's arm, restored by hand**, both fail with the sentence §1 recorded:

```
`spirv-val` rejected a float loop materialising a boolean into the integer class:
error: line 66: ID '10[%10]' has not been defined
  %58 = OpSelect %uint %57 %uint_1 %10

`spirv-val` rejected a float module whose condition is an integer:
error: line 52: ID '10[%10]' has not been defined
  %43 = OpINotEqual %bool %uint_1_0 %10
```

and the loop's own device test **kills the process with `0xC0000005`** — §2's crash, reproduced by
the regression test itself. With the fix, both modules validate and both run: the loop answers
`[3.0, 0.0, 3.0, 0.0]` (three trips of `in[i] > 0.0` over `[1.0, -1.0, 4.0, 0.0]`), and the
`Select` answers `[1.5, -2.25, 0.0, 7.75, -0.5, 3.0]`, **bit-for-bit** what the suite's CPU
reference answers.

**The sampler itself, on the device.** `examples/sample.lichen` with its one backend literal
switched `"cpu"` → `"gpu"`, installed backend, RTX 3060:

| program | `cpu` | `gpu` before the fix | `gpu` after |
|---|---|---|---|
| `sample.lichen` | `[3.0, 2.0, 3.0, 0.0, 3.0]` | **`0xC0000005`** | `[3.0, 2.0, 3.0, 0.0, 3.0]` — **bit-exact** |
| `topk.lichen` | two 15-arrays | **`0xC0000005`** | the same two arrays — **bit-exact** |
| `generate.lichen` | `([1.0, 2.0, 2.0, 0.0, 3.0], 3, [-1.0, -4.0, -7.0, 3.0])` | **`0xC0000005`** | ~~**refused by name**: `input buffer 3 holds 1 element(s) but the run covers 5`~~ → **runs, bit-exact**: see [device-leg](device-leg-of-the-ladder.md) §5 |

So **the sampler is reachable on this device**, which §5 of
[device-leg-of-the-ladder](device-leg-of-the-ladder.md) could not say. The generation loop no
longer crashes. **Corrected by [device-leg](device-leg-of-the-ladder.md) §5: it runs too, and bit
exact**, so the upload rule is no longer what stands between it and a device. ~~It stops at the
upload rule §1 already recorded for the KV-cache append and the scoring row — a short input buffer
beside a wider launch — which is a separate rule from this defect and is **not fixed here**.~~
**What §5 measured is that this note's reading of *which* input was short was wrong**: the
refusing input was `nv`, a host buffer, and not the sampler's own one-element output, which is a
launch output and outside the check. One `compute.buffer`'s width is the whole repair, and it
changes no production code — so the separate rule this note deferred is answered without touching
the emitter at all.

**Not fixed, and not attempted.** §5's three negatives stand (whether 429 words is the minimum);
and `wontfix:D17`, the `code-audit.md` contract §6 declined to implicate, is untouched — no
`Invariant` or `Safety` sentence was added or changed anywhere in this repair.

## What was measured, inferred and unknown

**Measured.** The crash reproduces at `0xC0000005` on a 444-word module; `spirv-val` rejects it
and names `%10`; the Khronos layer reports the same at `vkCreateShaderModule`; the crash is at
`vkCreateComputePipelines` on **both** an Intel Iris Xe and an RTX 3060 Laptop GPU; the whole
of §4 and §5, word for word; `%10` is defined nowhere; the constant pool's two arms disagree as
tabulated; M5's single inserted constant is sufficient.

**Inferred.** That the driver dies *because* it dereferences the dangling id rather than
reaching some later state — from the crash landing in `vkCreateComputePipelines`, from M4
isolating the operand, and from M5 removing it. **The drivers' own stack trace was not
captured**, so the frame is inferred, not read.

**Unknown.** Whether 429 words is the true minimum (§5's three negatives); and whether the
passing bodies in §5 are correct **answers**, which was not checked — `control_add` and `b4` ran,
but against patched modules nothing ran to compare, so **no patched module in this note has been
executed**, only compiled. **§7 answers the two questions this section left open**: the other
route to `zero_integer` is real and is covered, and patched modules now run.
