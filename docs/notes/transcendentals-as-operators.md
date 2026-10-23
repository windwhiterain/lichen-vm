# Transcendentals as operators: what `ln` costs, and what "the new op" can be

**Status:** landed — `ln` is an operator and the hand-assembled `sin` and `cos`
were switched onto the same mechanism. §3 is the measurement that made it
possible; §5 is the `Invariant` that was withdrawn to allow the switch, and §7
is the gate.

Points at [floating-point](floating-point.md) §6, §7 and §8 for the definitions
themselves and their accuracy; [operators](operators.md) for the primitive set;
[lowering-map](lowering-map.md) for the walk that reaches a kernel body. The
claims this note measures are `ln`'s own; it does not restate the four existing
definitions' accuracy, which that note holds.

## 1. Where the four definitions live today

There are **two mechanisms**, and that is the thing the decision turns on.

| operator | named as | built in | spliced by |
| --- | --- | --- | --- |
| `exp` | `KernelInstr::Exp` | `unary.rs:773` `exp_body` | `body.rs:382` `expand_transcendentals` |
| `sqrt` | `KernelInstr::Sqrt` | `unary.rs:203` `sqrt_body` | the same |
| `floor` | `KernelInstr::Floor` | `unary.rs:667` `floor_body` | the same |
| `sin` | **nothing** | `unary.rs:501` `sin_body` | `compute/body.rs:47` `sin_cos_definition` |
| `cos` | **nothing** | `unary.rs:539` `cos_body` | the same |

`TRANSCENDENTALS` (`unary.rs:14`) is the list the first three are named in, and
`KernelBody::expand_transcendentals` (`body.rs:382`) walks it, rewriting each
named instruction into `KernelBody::inline_definition` (`body.rs:425`) — a rename
of the definition's one parameter to the operand's value id, spliced at the call
site. `sin` and `cos` carry no `KernelInstr`, so the lowerer finds them by a
second table (`compute/body.rs:47`) and calls the same inliner through its own
`inline` (`compute/body.rs:1588`).

So all five are hand-built Rust bodies today. **The difference between the two
columns is only whether the IR names them.** That is the decision this note
takes up.

## 2. What "the new op" can be

### (a) A new `KernelInstr` the emitters implement

A variant on `KernelInstr` (`lib.rs:731`), named in `TRANSCENDENTALS`, defined by
a body of primitives.

- **It rules out** widening the artifact schema being free: `schema::KernelInstr`
  (`runtime/fused/schema.rs:100`) is a closed enum with a `From` arm each way in
  `runtime/fused/convert.rs:390-453`, so a variant is an **additive** widening
  and a `FORMAT_VERSION` decision (`schema.rs:435`) if an old artifact must
  still read.
- **It costs `TargetFacts` nothing.** `TargetFacts` (`lib.rs:605`) is
  `{int_width, lane_width}` — it carries no instruction set, so there is no field
  to add and no digest consequence from `TargetFacts` itself.
- **It changes no existing fragment's digest.** `expand_transcendentals` runs
  *before* the `KernelFragment` is built (`compute.rs:2253`→`2256`→`2258`), so a
  registered fragment never holds an un-expanded transcendental and
  `fragment_digest` never sees the variant. **Measured by reading both sites**,
  not by running the digests.
- **It does not need an emitter to implement it**: each emitter *refuses* an
  un-expanded one by name (`spirv.rs:299`, raised at `spirv.rs:1582`).

This is what `exp`, `sqrt` and `floor` already are, and it is the smallest step.

### (b) A named definition declared in the language

A `compute.lichen` or prelude function the lowerer splices — the reading of
"the hand-assembled ones should be switched to the new op" that goes furthest.

- **Blocked, measured.** A kernel body's `Apply` is supported only for a
  cross-kernel callee; an inline lichen-function call is **refused by name** at
  `compute/body.rs:1012`: *"inline lichen-function calls are not yet supported"*.
  Splicing a language function needs that refusal lifted, which is a change to
  the walk, not to a definition.
- It also loses the *"one list, so the expansion and the instructions cannot
  disagree"* invariant (`unary.rs:11`): a user-writable name can be shadowed or
  redefined, and `TRANSCENDENTALS` would no longer be the whole vocabulary.
- A definition the lowerer splices is **duplicated at every call site** — the
  inliner has no call, only a copy — so a prelude definition costs every program
  that names it.

### (c) A named operator whose body is built from the language's own operators

Keep the single `TRANSCENDENTALS` list, but state each sequence as data over
`KernelBin` rather than as Rust that calls `body.add_op`. This is a smaller
change than (b) and a larger one than (a), and on measurement it buys nothing
today: the bodies are already built from exactly the primitives both backends
emit, which is what floating-point §6 and §7 record as the reason for the
shared definition in the first place.

## 3. What `ln` needs — measured

### 3.1 The recorded blocker is stale

floating-point §7 says `ln` is deferred because its range reduction "needs
`floor`, a shift or a bitcast — the primitive set has none of the three"
(§6, at `:491`), and §7 at `:592` repeats it. **That is now true of two of the
three, not three.** `KernelInstr::Floor` exists (`lib.rs:774`) and is in
`TRANSCENDENTALS` (`unary.rs:17`). A **shift** (`KernelBin`, `lib.rs:680`) and a
**bitcast** are still absent — **measured**, by reading both enums.

But neither is needed, because the reduction a shift or a bitcast would serve is
reachable by a **fold that multiplies by powers of two**, which is the fold
`sqrt` already uses (`unary.rs:163` `rescale`, coarse `2^32` then fine `2^-4`).
The track is a float accumulating exact powers of two, so `k` is exact without an
integer exponent at all.

### 3.2 The fold reaches the same place, in 42 steps

Reduce into **`[1/√2, √2)`** — *not* `[1, 2)` — by three straight-line phases, then
evaluate `ln(m) = 2·atanh((m−1)/(m+1)) = 2z·S(z²)` by Horner over
`S = Σ z^(2i)/(2i+1)`, `i < 10`, and add `k·ln 2`.

- **Measured, exhaustive:** over **all 2,130,706,431** positive `f32` bit
  patterns, **0** fall outside the band and **0** go non-finite. The step counts
  are **5** coarse (`×2^32`, while `v < 1/√2`), **32** fine (`×2^-4`, while
  `v ≥ 16·(1/√2)`), **5** fine (`×2^-1`, while `v ≥ √2`) — **42 conditional
  scalings**, the same order as `sqrt`'s 45 (`unary.rs:222`, `:233`).
- This **refutes §7's cost estimate** at `:594`, which put the unrolled reduction
  at "about 250 conditional scalings" and the definition at "roughly 1,500
  instructions". The estimate missed that one `2^32` step does the work of 32
  unit scalings — the same economy `sqrt` already uses. Measured: **42**, not 250.

### 3.3 The band must be `[1/√2, √2)`, and that is the whole trick

**Measured, and this is the one finding that would have cost a slice.** The
obvious reduction to `[1, 2)` is **unusable**: it lands operands arbitrarily
close to `2`, so the answer is formed as `ln(v) − ln 2` with two terms near
`0.693` whose difference is near zero. Traced: at `x = 0.99999994` the `[1,2)`
fold gives `v = 1.9999999`, `k = −1`, and a **relative error of 1.000** — the
answer is wrong by a factor of two. Over `[0.5, 2)` exhaustively the `[1,2)`
fold's worst relative error is **1.000**.

Reducing to `[1/√2, √2)` bounds `|ln(v)| ≤ ln √2 = 0.347`, so whenever `k ≠ 0`
the answer's magnitude is at least `0.347` and the final subtraction cannot
cancel catastrophically; and whenever `k = 0` — which is every operand near one —
there is no subtraction at all.

- **Measured:** over `[0.5, 2)` **exhaustively** (16,777,216 values) the worst
  relative error is **2.271e-7** and the worst error is **2 ulp** of the answer.
  Over a 10,098,135-value sample across every binade the worst relative error is
  **2.182e-7**, also 2 ulp.
- **Measured:** the 10-term series alone, evaluated in `f64` against the same
  series at 40 terms over the whole band, truncates at **2.923e-10** relative —
  below a hundredth of the class's own resolution, so **the series is not the
  limiting term** and more coefficients would buy nothing.
- **Measured:** the subnormal tail (8,388,607 values) has a worst **absolute**
  error of **7.897e-6** on answers whose magnitude is near 100.

### 3.4 What this rules out, and what it does not

It rules out the whole family of "unblock `ln` with a shift, a bitcast, or a
multi-block inliner" answers recorded at floating-point §7 `:604`: **none of
them is needed**, because the reduction is straight-line and the primitives
present already carry it. It rules out a `[1,2)` reduction of any accuracy.

**Unknown, and not claimed:** this measures the *definition*, in the order a
spliced body would execute it. It does not measure the **GPU** column. floating-
point §8's last paragraph records that the SPIR-V path for a spliced body is
untested, and that carries over unchanged. Nor does it address the division in
`(m−1)/(m+1)`: `OpFDiv` is the one float operation this device does not round
correctly (floating-point §7 `:569`), so **`ln` as defined here would carry a
known cross-backend divergence the other four do not**, and that is the one
measured result below that argues against landing it unchanged.

## 4. The recommended first step

**Add `ln` as a `KernelInstr` with a definition in `TRANSCENDENTALS`, and leave
`exp`, `sqrt`, `floor`, `sin` and `cos` exactly where they are.** It is
alternative (a), which is what the language already does for three of the five,
and §2's measurement is that it costs no `TargetFacts` field and no existing
digest.

**It rules out**, as a first step: the language-declared definition (§2b, blocked
at `compute/body.rs:1012`), and the state-table/`KernelUn` redesign that §6 of
floating-point rejected for the one tractable operator it had.

### 4.1 If the first step is instead "declare the existing four in the language too"

That is blocked on **`compute/body.rs:1012`**, and only on that: the lowerer
refuses an inline lichen-function call by name, so nothing written in a
`.lichen` file can be spliced into a kernel body today. Lifting it is a change
to the walk (`compute/body.rs:1009`), and it would additionally give up
`TRANSCENDENTALS`' *"one list"* invariant (`unary.rs:11`) for a user-writable
vocabulary. That is a larger step than adding `ln`, and it is **not** what this
note recommends first.

## 5. The step that unifies the hand-assembled expr — and the sentence it withdrew

The user's decision includes switching the hand-assembled ones to the new op.
For `sin` and `cos` that means moving them into `TRANSCENDENTALS` as
`KernelInstr::Sin`/`Cos` and deleting `sin_cos_definition` (`compute/body.rs:47`)
with the `Lower::inline` it fed. **Done**, and it is a genuine simplification: one
mechanism instead of two, and the `Select` class-agreement hazard a body spliced
by the lowerer could carry is gone.

It required withdrawing one `Invariant` sentence, quoted here in full because the
rule is that a contract changes only when the change is named.

Before, on `sin_cos_definition`:

> **`sin` and `cos` only**, and they are the whole of `crates/lichen-kernel-ir/src/unary.rs`
> the language's own lowering splices rather than emits: they carry no `KernelInstr` of
> their own, because the claim schema the runtime carries is a closed list of the
> instruction set and widening it is not this slice's to do.

After, on `Lower::transcendental`:

> **One instruction, not an inline expansion**, for every transcendental: `exp`,
> `sqrt`, `floor`, `sin`, `cos` and `ln` are each named in
> [`lichen_kernel_ir::TRANSCENDENTALS`], and both backends lower the one sequence
> that list gives them. **`sin` and `cos` used to be the two operators the lowerer
> spliced itself** rather than emitting, because the claim schema the runtime
> carries was read as a closed list of the instruction set and widening it was not
> this slice's to do. That reason no longer holds: the schema already carries
> `Exp`, `Sqrt` and `Floor`, so the list was closed against `sin` and `cos` by
> choice and not by structure.

**This is a correction, not a weakening.** The sentence's own stated reason — *the
schema is a closed list of the instruction set* — was already false when it was
written down: `schema::KernelInstr` carries `Exp`, `Sqrt` and `Floor`
(`runtime/fused/convert.rs:396-398`), measured by reading both `From` arms. The
list was closed against `Sin` and `Cos` by choice and not by structure, so the
change withdraws a stale justification rather than trading a guarantee away.

### 5.1 The second `Invariant`, and the fifth reader that closed the asymmetry

This section first recorded an asymmetry left in place on purpose: a `ln` in a registered fragment read
as its **expanded primitive chain** while an `exp` read as the operator, because `inlined_transcendental`
said **"The four are disjoint"** and no reader was added for `ln`. **The user then decided to add the
fifth reader and edit that sentence**, so the asymmetry is closed (`41066dcf`): the contract now reads
**"The five are disjoint"**, and a registered `ln` reads as `UnaryOp::Ln` exactly as `exp` does. It says
*five* rather than naming them, because `inlined_operand` requires **every** instruction of a definition
with matching opcode and arity — a match is the whole instruction set, and two definitions cannot claim
one subtree — so the count is what the contract asserts.

**What that slice also corrected, which this note had wrong.** The bridge was **not** missing an `Ln`
reading in general: `crates/lichen-fusion-bridge/src/lower.rs` already had an arm reading `UnaryOp::Ln`,
and it is **dead for a registered fragment** — `expand_transcendentals`
(`crates/lichen-kernel-ir/src/body.rs:398`) rewrites the instruction **in place**, so the value that held
`KernelInstr::Ln` goes on to hold the definition's last instruction and `match op` is reached with the
chain's own opcode, never `Ln`. **After expansion the reader list is the only live path.** `floor` is the
contrast and is untouched: it has no reader and its arm **refuses**, so it still reads as the `Select` its
expansion holds. Measured before and after: `Err(BridgeRefusal::Select)` becomes
`Ok(UnaryOp::Ln.apply(2 · datum))`, with `floor` unmoved.

## 6. Decision record

**No `wontfix:D<n>` line covers the definitions or the primitive set** — measured
by reading `code-audit.md`'s table and its nine `wontfix:D` rows. Nothing in the
decision record forbids this work.

## 7. The gate

Every example that exercises a transcendental prints **exactly** its declared
line, before and after this change. The "before" column is the run taken at
`f9b8852c` before anything was touched; the three transcendental exprs
(`silu`, `rope`, `rmsnorm`) are the ones that would move if a definition's shape
had moved.

| example | operators | declared **and** printed, identically |
| --- | --- | --- |
| `silu` | `exp` | `[0.0, 0.73105794, -0.13447031, 7.0463777]: array{Float, 4}` |
| `rope` | `sin`, `cos` | `[1.0, 2.0, -1.744977, 4.685622, -7.536518, 2.0496058, -8.0589075, -6.9321]: array{Float, 8}` |
| `attention` | `exp` | `[2.5101643, 3.5101643, 4.5101643, 5.5101643, 3.924232, 4.924232, 5.9242315, 6.924232]: array{Float, 8}` |
| `decode` | `exp` | `[8.888384, 10.773098]: array{Float, 2}` |
| `decode_loop` | `exp` | `([8.888384, 10.773098, 11.235222, 13.9678135], [11.235222, 13.9678135], 2): tuple{array{Float, 4}, array{Float, 2}, Int}` |
| `layernorm` | `exp` | `[-3.0, -2.0, 1.0, 6.0]: array{Float, 4}` |
| `rmsnorm` | `sqrt` | `[3.5, 3.0, 2.0, 8.0]: array{Float, 4}` |
| `softmax` | `exp` | `[0.03205853, 0.08714417, 0.23688345, 0.6439139]: array{Float, 4}` |

**No example moved**, which is the evidence that switching `sin` and `cos` onto
`expand_transcendentals` spliced the *same* sequences the lowerer used to.

## 8. What is still not measured

- **The GPU column of `ln`.** §3 measures the definition, in the order a spliced
  body executes it. floating-point §8's last paragraph records that the SPIR-V
  path for a spliced body is untested, and `ln` inherits that gap — as do the
  `sin` and `cos` bodies it now reaches through the same rewrite.
- **`ln`'s own emitted-vs-modelled check.** floating-point §8 records the lesson
  that a Rust mirror agrees with `f64` whether or not the body does, and that only
  a check of emitted against modelled catches the difference. `ln_value` was
  measured (§3.3, reproducing 2 ulp) but **no test runs the spliced body** the way
  `trig_domain.rs` does for `sin`/`cos`. That is the check to write first.
- **The division-free series.** The one argument against landing `ln` unchanged
  is `(m-1)/(m+1)`. It is cheap to state as a follow-up and is not attempted
  here; a reduction to a narrower band would shrink `z` and let a polynomial in
  `z` be replaced by one in `z^2` with no division at all.

