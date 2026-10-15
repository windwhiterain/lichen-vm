# Auditing the recorded walls: eight capability claims, measured

> Status: **current** — eight claims re-measured at `dev@3d2a39dd` on this machine, each with
> the program that decided it. **Four of the eight are weaker than they are written**, and one
> of those four is the wall the ladder has been planning around.
>
> Points at: [`crates/lichen-compute`](../../crates/lichen-compute/) (`src/compute/body.rs`,
> `src/compute.rs`), [`crates/lichen-fusion-bridge`](../../crates/lichen-fusion-bridge/)
> (`src/tiled.rs`, `src/lower.rs`), [`crates/lichen-graph-ir`](../../crates/lichen-graph-ir/)
> (`src/run.rs`). Referenced rather than restated:
> [llm-operators](llm-operators.md) (the ladder and its `attention` finding),
> [code-audit](code-audit.md) (`P1-39`, `P1-42`, `wontfix:D17` — a contract, not a defect),
> [two-dimensional-workgroup](two-dimensional-workgroup.md) §1.1/§4,
> [loop-conversion](loop-conversion.md) §1.1,
> [compute-runtime-scalars](compute-runtime-scalars.md), and the examples themselves
> (`decode.lichen`, `kvcache.lichen`, `gemv.lichen`, `matmul.lichen`, `attention.lichen`).

**Every number below was measured in this session**, with a program under `probe/` and the
`lichen-compiler` binary built into a private target. The probe files are **removed**; each
one is quoted by enough of its source to be re-typed.

## What is measured, what is inferred, what is unknown

- **Measured.** Walls 1, 2, 3 and 6, each with the program and the output; the four verdicts
  are in the table under §1.
- **Measured, on the host only.** Wall 6's two backends: the drop reproduces on `cpu`, and the
  recorded device half was not re-measured here.
- **Measured, from the source rather than from a run.** Wall 7: the tiler's refusal is one
  catch-all match arm, and reading it is how the verdict is decided — see §4, which says so
  plainly and does not claim a run.
- **Inferred.** That the dominance failure of wall 1 is a lowering ordering defect rather than a
  language rule. The refusal and its mechanism are measured; the claim that a mechanical fix
  exists is not, because no fix was tried.
- **Unknown.** Everything in §5 (walls 4, 5, 8), which needs a host program with a GPU backend
  installed or a device present, and neither was reachable from this session's budget.

## §1 The verdicts

| # | The claim as recorded | Verdict |
|---|---|---|
| 1 | A `@loop` nest cannot be applied inside another nest's step | **real** for a nest, and **weaker than stated** in what makes it fail — there is a second, unrecorded wall next to it |
| 2 | There is no loop | **not real** — a loop over dispatches is expressible, measured end to end |
| 3 | Nothing advances a position | **weaker than stated** — a program binding advances a kernel today; what is absent is narrower and stranger |
| 4 | `RaggedInputs` makes the graph leg unable to carry a GEMV | not reached — §5 |
| 5 | The device refuses a reduction shorter than its output | not reached — §5 |
| 6 | A write past the count is dropped in silence | **a spelling gap** for the drop, and a **third, unnamed** silent no-op beside it |
| 7 | Under the fused reduction schedule a projection cannot read the invocation | **real**, and the new runtime scalar did not reach it |
| 8 | The workgroup geometry is the last wall for a tiled matmul | not reached — §5, with the note's own measurement quoted |

## §2 Wall 1 — the nest wall is real, and its cause is narrower than recorded

**The claim** ([llm-operators](llm-operators.md), the `attention` row and its finding):
*"A `@loop` nest cannot be applied inside another nest's step, and that is attention's real wall
(a rule, not a missing feature)."* The mechanism is named: `lower_loop_steps` sets the step's
terminator **after** its operands ([`body.rs:1190-1209`](../../crates/lichen-compute/src/compute/body.rs)),
while demand runs from the codomain outwards (`:222`).

**The failure reproduces, and it is the recorded one.** A module-level nest applied inside
another nest's step:

```lichen
@loop trip = s => if s(0) == 0 then s(1) else trip (s(0) - 1, (s(1) + s(2) * int2float s(0)) ~ "combine", s(2))
@loop outer = s => if s(0) == 0 then s(1) else outer (s(0) - 1, (s(1) + trip (k.m, 0.0, (compute.read ((compute.Read _)(.from k.in.x, .at (s(0) - 1)))))) ~ "combine" ~ "element")
```

answers, verbatim:

```text
error: compute.parallel: Bin(Int, Add) reads ValueId(28), which block 7 receives as a parameter, and block 7 does not dominate block 2
```

which is [llm-operators](llm-operators.md)'s recorded refusal with its own words and its own
block numbers (`Bin(Float, Div)`, `ValueId(32)`, blocks 7 and 2), the operator and the value id
being what the data changes.

**So the verdict is `real`, and the escape hatches are closed — measured one at a time.** Four
spellings that the claim implies should work, and what each does:

| spelling | result |
|---|---|
| the step's whole next state **is** the inner nest's result, with no arithmetic after it | refused — *"block 2's transfer to block 1 reads ValueId(28), which block 7 receives as a parameter, and block 7 does not dominate block 2"* |
| the inner nest demanded from the body's **entry block first**, so its nest is entered before the outer step is lowered | refused — the same `Bin(Int, Add)` dominance error |
| the inner loop as an ordinary **unmarked recursion** at module level (so the conversion unrolls it) | refused — *"kernel body Apply is supported only for a cross-kernel (kernel-value) callee v1; inline lichen-function calls are not yet supported"* |
| the inner nest bound in a **nested block** instead of a step | **runs** — `[6.0, 6.0]`, which is `3 + 2 + 1` |

That last row is the interesting one, and it is why the claim is **weaker than stated**. Nesting
is not what the wall is about. Two probes separate them:

- A **plain block expression** inside a nest's step runs (`{ s(1) + int2float (s(0) * k.m) }` →
  `[9.0, 9.0]`).
- A **nested block with its own binding** in a kernel body runs (`{ q = read × i; q + 1.0 }` →
  `[1.0, 2.0]`).

So blocks in steps are fine, and a nest in a block is fine. What is refused is the nest **as a
backedge operand**, where the enclosing block reads a value the inner nest computed.

**And there is a second wall beside it that no note records: a nest may be applied at most once
per kernel body.** The same nest applied twice at the body's top level — no nesting anywhere,
each application a plain statement — is refused:

```text
error: compute.parallel: block 6's transfer to block 5 reads ValueId(21), which block 2 defines, and block 2 does not dominate block 6
```

The control matters: **two** module-level nests, each applied once, run and answer `[12.0,
12.0]`. `examples/attention.lichen`'s `attend` is exactly that control — `top`, `den` and `acc`
are three *distinct* nests applied once each, which is why it lowers. **The one-kernel form it
cannot express would have needed the score's `j` fold inside the key fold, so it is blocked by
both walls at once, and the note names only the first.**

The message names the cause in every case — `which block N … does not dominate block M` — so
this is **not** a route to `wontfix:D<n>`: it is a real limit with a real diagnosis, and the
diagnosis is a value-domination problem in the lowering rather than a statement about what the
language may express.

## §3 Wall 2 — there is a loop, and it is a recursive function

**The claim** ([code-audit](code-audit.md) `P1-39`, `todo`): *"no `for`, no `while`, no loop
keyword in any example; `@loop` is a fold inside one lane; `compute.call` inside a parallel body
is refused"*. The question this note asks is the one the examples do not: **whether a loop is
expressible at all.**

**It is, and it runs.** A recursive function that dispatches a kernel per step and threads the
output buffer:

```lichen
drive = s => if s(0) == 1 then { cur = s(1); (compute.plrun append ((Arg)(.n 8, .cap 4, .dim 2, .t 1, .I In(.k cur.k, .nk row))) : Out) }
          else { cur = s(1); drive (s(0) + 1, (compute.plrun append ((Arg)(.n 8, .cap 4, .dim 2, .t 0, .I In(.k cur.k, .nk row))) : Out)) }
final = drive (0, Out(.k kbuf))
```

answers `[1.0, 2.0, 1.0, 2.0, 0.0, 0.0, 0.0, 0.0]` — **bit-identical to the same two `plrun`s
written out with no recursion at all**, which was run as the control and answers the same row.
The base case alone also runs (`[0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0]`, row `t = 2` written
at slots 4 and 5).

**So the verdict is `not real`, and it is the most consequential finding here.** Two of the
three recorded reasons are still true and do not carry it: there is no `for`/`while` keyword
(there is no iteration *form*), `@loop` is still a fold inside one lane, and a `compute.call`
inside a parallel body is still refused (wall 1's table measures a neighbouring refusal; the
`P1-39` row's own text is **self-contradictory** — its first paragraph says "Both halves are now
closed" and its table two paragraphs later lists all three shapes as refused, which is worth
one reader's decision on its own). What changes the verdict is the third: **a lichen function
recurses**, and a recursive function may dispatch a kernel on each step, so an `N`-step decode
loop is written once rather than `N` times.

The cost, stated so nobody buys this as more than it is: the trip count is a **function's own
state**, so nothing else in the program can see it, and §4's finding applies to every one of the
`N` launches inside it.

## §4 Wall 3 — a program binding does advance a kernel today

**The claim** ([decode.lichen](https://github.com/windwhiterain/lichen-vm), "Nothing advances a
position … `t` is a leaf of stage 1's argument, written out as the literal `2`"): *"There is no
way to advance `t`: it is a parameter the host hands in, and the language has no mutable state to
hold a counter between calls."*

**A program binding becomes a runtime scalar, and the kernel follows it.** One binding, one
launch:

```lichen
t = 1
one = (compute.plrun append ((Arg)(.n 8, .cap 4, .dim 2, .t t, .I In(.k kbuf, .nk row))) : Out)
```

answers `[0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 0.0, 0.0]` — row `t = 1` written at slots 2 and 3. **The
literal in `decode.lichen` is a choice, not a wall**, and `t = s(0)` no more than `t = s(0) + 1`
is.

**What is genuinely absent is narrower, and it is a silent one.** The same value handed over from
a **function parameter** does not reach the kernel:

```text
error: compute.parallel: instruction 15 mixed an integer and a float in one operation. `Int` and `Float` do not convert in either direction, so nothing here can make the two operands meet: this is a malformed fragment rather than an unsupported shape.
```

and the same value read out of a **recursive function's state** produces no refusal at all —
`drive (s(0) + 1, plrun (… .t s(0) …))` collects `raw none`, an empty buffer, with **no error,
no refusal and no report line**.

That is the shape the roadmap needs. A top-level binding works because it is decided before the
kernel is compiled from its template; a parameter and a recursion state are not, so the fragment
is built over an undecided cell. The second case is the one that should be reported: **the same
malformation that names itself loudly for a parameter answers silently as an empty buffer for a
recursion state**, and the recursion is the spelling §3 says to use.

## §5 Walls 4, 5 and 8 — not reached, and why

- **Wall 4 (`RaggedInputs` and a GEMV).** The graph leg is unreachable from the CLI —
  `compute.graph` answers *this graph dispatches to a device, but no compute backend is
  installed* — so the measurement needs a host program with a GPU backend installed, which this
  session did not reach. **What the recorded claim already concedes** is that equalising the two
  input lengths does not make a GEMV run: [gemv.lichen](https://github.com/windwhiterain/lichen-vm)
  records that the run then reaches submit and is refused on the **class** arm instead. So even
  a successful padding measurement would have shown a *spelling gap for `RaggedInputs`* and left
  the graph leg refused by a different, separately recorded refusal — a verdict narrower than
  "the graph leg cannot carry a GEMV".
- **Wall 5 (a chained decode step on the device).** Needs a device leg. The hypothesis on offer
  is that the weighting stage's reduction is *longer* than its output, so a chained step may reach
  a device where a lone GEMV does not; that is reasoned from two recorded measurements and, as
  `decode.lichen` itself says, **not measured there either**.
- **Wall 8 (the workgroup geometry as a tiled matmul's last wall).** What the note itself
  measures is quoted here rather than re-run, because it is the answer to the question and it is
  already on the record: a **rectangular workgroup compiles, validates and runs** on the RTX 3060
  with `OpExecutionModeLocalSize = (16, 4, 1)`
  ([two-dimensional-workgroup](two-dimensional-workgroup.md) §4), so **the module contract is not
  the wall**; and the same §4 measures **a staged `Shared` exchange at `count = 64` under a
  rectangle answering `136 = 1 + … + 16` and writing nothing past index 15 — "a wrong answer, a
  truncated write, and no error, no refusal, no report line"**. That is the class a tile's partial
  sums would pass through. So wall 8 as posed is **not an expressibility question at all**: the
  rectangle lands, and what is left is the silent-wrong-answer class the same section measures,
  which is a correctness gate rather than a feature.

## §6 Wall 6 — the drop is a spelling gap, and a third silent no-op sits beside it

**The claim** ([kvcache.lichen](https://github.com/windwhiterain/lichen-vm)): a write past the
count is dropped in silence on both backends, measured at `[0.0, 0.0]` for the one-row kernel a
decode loop wants.

**The drop reproduces**, verbatim and unchanged: one lane per column of the new row writing at
`t * dim + d` over a count of `2` answers `[0.0, 0.0]`.

**And the count can be raised so the write lands inside it — measured.** Raise the count to
`(t + 1) · dim` and let lane `i` write at `i`, reading the new row when `i >= t * dim`:

```lichen
@loop ... raw = (k : Par) => {
  i = compute.range k.n
  fresh = i >= k.t * k.dim
  column = i - k.t * k.dim
  old_k = compute.read ((compute.Read _)(.from k.in.k, .at i))
  new_k = if fresh then (compute.read ((compute.Read _)(.from k.in.nk, .at column))) else old_k
  compute.write ((compute.Write _)(.to k.out.k, .at i, .value (new_k ~ "element")))
}
```

`k = 1 2 3 4 0 0 0 0`, `nk = 9 8`, `n = 6`, `t = 2` answers **`[1.0, 2.0, 3.0, 4.0, 9.0, 8.0]`** —
a six-long cache with **one row** written. So the drop is **a spelling gap**: it is avoidable,
and `examples/kvcache.lichen`'s whole-cache rewrite is one instance of the same trade.

**What that trade is, stated so the gap is not oversold.** The count is not free to choose: it
is the output buffer's length, and `compute.collect` hands back every element of it, so raising
it to `(t + 1) · dim` hands back **a prefix**, not a fixed-capacity cache. The count has to grow
with `t`, so the cache a program threads is a different length at each step. That is why the
committed example keeps a fixed capacity and rewrites the whole buffer, and it is the honest
reason rather than a preference.

**And a third thing sits beside both, which nothing records.** Aiming the write at the **input**
buffer — the in-place spelling, the only one that could leave the rest of the cache alone — runs
and does nothing:

```lichen
compute.write ((compute.Write _)(.to k.in.k, .at (k.t * k.dim + d), .value (compute.read ((compute.Read _)(.from k.in.nk, .at d)) ~ "element")))
```

collects `[1.0, 2.0, 3.0, 4.0, 0.0, 0.0, 0.0, 0.0]`, **the cache unchanged, with no error and no
report line**. So the surface has three answers for "write one row of the cache", and only the
second writes anything. That one is a candidate for the decision record; it is **not** filed as a
defect here, because [code-audit](code-audit.md)'s `wontfix:D17` is a contract about what a
write **is** — *the value of an output position and not an effect* — and a write to an input is
exactly the spelling that sentence declines.

## §7 What would change which wall needs a decision

1. **Wall 2 needs no decision.** A decode loop is expressible; the roadmap rows that plan for one
   should be re-read against §3 before they are costed.
2. **Wall 3 needs a decision about the silent case, not the capability.** A top-level binding
   already advances a kernel. What is missing is that a *loop-carried* value cannot, and that the
   failure is **silent for a recursion state** and loud for a parameter.
3. **Wall 1 needs a decision about scope.** Two limits share one recorded name; a reader told
   "a nest inside a step is refused" will not predict that two applications of one nest are also
   refused, which is the rule `examples/attention.lichen` actually sits on.
4. **Wall 6 needs a decision about the third spelling only.** The drop is avoidable; the in-place
   write is the one that would need a contract.
5. **Walls 4, 5 and 8 need no decision yet**, and wall 8 needs the question re-asked: it is not
   expressibility.

## §8 Recovered measurements, and the two claims that were wrong

- **A kernel body may not apply an ordinary lichen function at all**, `compute.call` aside:
  *"kernel body Apply is supported only for a cross-kernel (kernel-value) callee v1; inline
  lichen-function calls are not yet supported"*. This is why §2's unmarked-recursion escape is
  closed, and it is a statement about **every** helper call, not about loop conversion.
- **A `@loop` nest bound inside a nested block inside a kernel body is refused by a different
  rule** — *"a kernel body reached a node with neither a value nor an operation (node=NodeId(Nv1))"* —
  not by the conversion. The walk that finds a nest does not reach a nested block's bindings, so
  the spelling in `examples/attention.lichen` (all three nests at the body's top level) is the
  only one that lowers.
- **"Nothing advances a position" is wrong as written** and "there is no loop" is wrong outright.
  Both were steering work that the surface already supports.
