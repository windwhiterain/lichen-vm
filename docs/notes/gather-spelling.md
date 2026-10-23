# The one-buffer gather: a wall, or an unnamed spelling?

> Status: **current** — four shapes measured at `dev@176b2f20` on this machine, each with the
> program that decided it, plus the same spelling at `k = 3`. **All four are expressible today**,
> and the reason is arithmetic rather than a missing operator: under the concatenation layout the
> slot a lane fills **is** its own lane index, so there is nothing to concatenate. The one real
> wall found sits **beside** `keep` rather than in it (§2.4) and is a dominance rule about `@loop`,
> not about gathering. §1 is read from the sources and commits nothing but citations.
>
> **§2.4's dominance rule was re-measured at `dev@eb3132ae`, because
> `examples/topk.lichen` (`94cb6ad6`) contradicted it: the same refusal still reproduces, but the
> condition this note gave for it was too strong, and §2.4 and §3 now state the measured one.**
>
> Points at: [`crates/lichen-compute`](../../crates/lichen-compute/) (`src/compute.rs`,
> `src/compute/body.rs`). Referenced rather than restated:
> [llm-operators](llm-operators.md) (the ladder; the greedy-sampling row),
> [wall-claims-audit](wall-claims-audit.md) (the method — a probe under `probe/`, the
> `lichen-compiler` binary, every claim with the program that decided it),
> [two-dimensional-launch](two-dimensional-launch.md) §2.1 (a **transposing write**, already
> measured, on both backends),
> [code-audit](code-audit.md) (`P1-42`, `wontfix:D17` — a contract, not a defect), and the
> examples themselves (`sample.lichen`, `decode_loop.lichen`, `decode.lichen`, `kvcache.lichen`,
> `matmul.lichen`).

`examples/sample.lichen` measured that **top-`k` is work rather than a wall** for `k = 2` — a
second fold wholly in `Int` answers two ranks — and then named the step it did **not** take:

> **What was not reached is the one-buffer answer**: a launch of `n` lanes cannot write `k` slots
> per lane, and nothing concatenates `k` `n`-long buffers. `examples/decode_loop.lichen`'s `keep`
> is the gather — it launches over the wider extent and reads the step's buffer at `lane % k` — but
> it needs the ranks already addressable, and getting them there is the step not taken.

This note turns that sentence into four measurements. **The question is a spelling question
first**: is the gather impossible, or merely unnamed?

## §1 Read, not measured — the three facts the verdicts stand on

These cost no compilation and commit no claim of their own beyond what the sources say.

### 1.1 The write-side rule, and where it lives

`crates/lichen-compute/src/compute.rs:5903-5910` is `run_parallel_kernel`, and its invariant is
the whole of the write side:

> The output count is the fragment's `outputs` — a static fact, not a discovery of which slots
> were written — and every ordinal is written on every index, so each buffer is fully defined.
> The result equals the sequential loop's iff no two indices write the same slot: the partition
> decides a collision.

Two clauses do the work here, and they are **not the same clause**:

- **The buffer's length is the launch's extent**, a static fact of the fragment. It is not a
  bound the body may exceed and it is not something the body negotiates. `examples/kvcache.lichen`
  measures the consequence — a write past that span is **dropped in silence**,
  `compute.rs:6260-6262` (*a write outside the span is dropped rather than aliasing another
  worker's slots*) — so a kernel that wrote *fewer* distinct slots than its extent has an
  unwritten tail.
- **Every ordinal is written on every index**, which is `D17`: a `compute.write` is **the value of
  an output position and not an effect**, so a body that does not reach its codomain position does
  not "write nothing", it computes a dead expression whose value nobody reads.

Neither clause says a lane may write **more than one slot of one buffer**, and neither says a
launch of `n · k` lanes is wrong. What the extent clause forbids is a *shorter* buffer than the
address space the body names; what `D17` fixes is the **one value per codomain position**, which
is a statement about the codomain's *positions* and says nothing about how many lanes there are.

### 1.2 What `keep` does today, and what it needs from its input

`examples/decode_loop.lichen`'s sixth kernel, verbatim:

```text
keep_raw = (k : ParT) => {
  lane = compute.range k.n
  base = k.j * k.dim
  held = compute.read ((compute.Read _)(.from k.in.x, .at lane))
  here = compute.read ((compute.Read _)(.from k.in.o, .at (lane % k.dim)))
  fresh = (lane >= base) & (lane - base < k.dim)
  mine = if fresh then here else held
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (mine ~ "element")))
}
```

It is a **scatter-merge**, and every clause of it is a clause of the rules above:

| clause | what it is |
|---|---|
| `.at lane` on **both** the read of `x` and the write | the write address **is** the lane index; nothing here is a permutation |
| `.at (lane % k.dim)` on the read of `o` | a **computed read address** whose formula is total over `[0, k.n)` — the operator's own note records why it is `%` and not `lane - base`: the latter leaves the two-element buffer on most lanes and answers `0` past a host buffer's end |
| `mine = if fresh then here else held` | a `Select` whose **two arms are both `Float` scalars** — the same shape `examples/kvcache.lichen`'s `append_raw` writes as `if fresh then (compute.read …) else old_k` |
| one `compute.write` per lane | `D17`'s one value per position |

**What `keep` needs from its input is two things and both are already met**: a source buffer
*longer than or equal to* the launch's extent (`x`, the transcript, four slots over a count of
four), and a source buffer it may read at a **computed, total** address (`o`, the step's output,
two slots). **What it does not have, and this is the whole of §2's shape 4, is a way to name
more than one step output.** `ParT`'s `.in` is `struct{.o (compute.Buf Float), .x (compute.Buf
Float)}` — exactly one `.o`. A `k`-way gather needs `k` of them, or one buffer `k` times longer,
and `keep` as committed can reach neither.

### 1.3 The two reading forms a buffer already supports

`examples/decode.lichen`'s chain reads one position-major cache buffer under **two different
index forms**, and the disagreement is deliberate:

| stage | read | fold axis |
|---|---|---|
| `score_raw` | `k.in.w` at `i * k.kdim + (s(0) - 1)` | output = **position**, fold = head dimension |
| `weigh_raw` | `k.in.v` at `(s(0) - 1) * k.dim + column` | output = **column**, fold = position |

`llm-operators.md` records the consequence — written at the score stage's form, the weighting stage
answers `[6.0754085, 33.36727]`, wrong in both elements, **with no error and no report line**.
Two facts follow for this note, and only the second is about the gather:

1. **The disagreement is a caller's, and it is silent.** A buffer is a flat span; which axis is
   the fold is entirely the body's own arithmetic. That is the same fact
   [two-dimensional-launch](two-dimensional-launch.md) §2.1 measured when it found that *nothing
   above the body knows a second dimension exists* — the flattening is the body's own choice.
2. **The two forms are not a permutation and not a gather.** They are two *independent* readings
   of the same bytes, and each one is affine in the lane index. Whether a body can read at an
   address that is a **permutation** of its lane index — which is what a concatenation is — is a
   separate question, and §2 measures it rather than inheriting the answer.

## §2 The verdicts

| # | shape | verdict |
|---|---|---|
| 1 | **One lane writing several slots** — a launch of `n · k` lanes, every lane computing which slot it fills | **expressible today** — §2.1 |
| 2 | **A kernel reading another kernel's output at a per-lane computed offset** | **expressible today** — §2.2 |
| 3 | **A transposing or scattering read** | **expressible today** — §2.3 |
| 4 | **The `keep` shape itself** | **expressible today for a gather; the wall beside it is real but narrower than first recorded** — §2.4 |
| 5 | *(not a shape of the brief)* **the same spelling at `k = 3`** | **expressible today, no new mechanism** — §2.5 |

Every program below ran under `probe/` with the `lichen-compiler` binary built into a private
target, exactly as [wall-claims-audit](wall-claims-audit.md) records its method. **The probe
files are removed**; each is quoted by enough of its body to be re-typed. The scores are
`examples/sample.lichen`'s — `3 9 4 9 / 5 1 5 2 / 7 7 7 7 / 8 6 4 2 / 1 2 3 4` — over five rows
of width four, and `k = 2`, so every launch below is either `n = 5` or `n · k = 10`.

### 2.1 Shape 1 — expressible today, and the concatenation is free

**The program.** Kernel A is `examples/sample.lichen`'s `greedy_raw` **verbatim** — one launch of
`n = 5` lanes, one write each, the `@loop` untouched. Kernel B is **one launch of `n · k = 10`
lanes**, and its whole body is:

```text
  lane = compute.range k.n
  rank = lane / k.rows
  row = lane % k.rows
  base = row * k.w
  first = float2int (compute.read ((compute.Read _)(.from k.in.p, .at row)))
  @loop next = s => … (s(1) + ((((1 * ((float2int (…at (base + s(0) - 1))) >= s(1))) * (1 * ((s(0) - 1) != first))) * ((float2int (…at (base + s(0) - 1))) - s(1)))) ~ "combine", (s(2) + (…)) ~ "combine")
  mine = if rank == 0 then first else (next (k.w, 0, 0))
  at_m = rank * k.rows + row
  at_t = row * k.ranks + rank
  (compute.write (… .to k.out.m, .at at_m, .value (int2float mine ~ "element")), compute.write (… .to k.out.t, .at at_t, .value (int2float mine ~ "element")))
```

**The output.**

```text
([3.0, 2.0, 3.0, 0.0, 3.0, 1.0, 0.0, 0.0, 1.0, 2.0], [3.0, 1.0, 2.0, 0.0, 3.0, 0.0, 0.0, 1.0, 3.0, 2.0]): tuple{array{Float, 10}, array{Float, 10}}
```

**What decides it.** Two kernels, `k` ranks, and **one `10`-slot buffer per layout**, no copy
between them:

- `.m` is the **concatenation** — `[3, 2, 3, 0, 3]` then `[1, 0, 0, 1, 2]`, rank zero's five then
  rank one's five, end to end in one buffer.
- `.t` is the **transpose** — rank zero and rank one **interleaved per row**, `[3, 1, 2, 0, 3, 0,
  0, 1, 3, 2]`.

Both are derived by hand before the run. Rank zero is `greedy_raw` verbatim, so it is
`[3, 2, 3, 0, 3]` — `sample.lichen`'s own declared row, arrived at by a different route. Rank one
is the `Int` excluding fold: row 0 `3 9 4 9` loses position `3` and answers `1`; row 1 `5 1 5 2`
loses `2` and answers `0`; row 2 `7 7 7 7` loses `3` and answers `0`; row 3 `8 6 4 2` loses `0`
and answers `1`; row 4 `1 2 3 4` loses `3` and answers `2`.

**The finding that matters, and it is cheaper than the sentence said.** Under the
concatenation layout the slot `rank · n + row` **is the lane index itself**, whenever `rank` and
`row` are recovered from it as `rank = lane / n` and `row = lane % n`. So **the concatenation is
the identity on the lane index**: a launch of `n · k` lanes writing at `.at lane` *is* `k` `n`-long
buffers laid end to end, and there is nothing to concatenate. The row-major layout is the one that
costs, and it costs **nothing but the address arithmetic the body already had to write** — both
landed from the same launch, in the same body, into two different outputs.

### 2.2 Shape 2 — expressible today

**The program.** Kernel A `greedy_raw` verbatim again; kernel B is a launch of `10` lanes whose
body is `row = lane / k.rows`, `first = compute.read (… .from k.in.s, .at row)`, and one write at
`.at lane`.

**The output.**

```text
[3.0, 3.0, 3.0, 3.0, 3.0, 2.0, 2.0, 2.0, 2.0, 2.0]: array{Float, 10}
```

**What decides it.** `row` is `lane / 5`, so lanes 0–4 read `p[0] = 3` and lanes 5–9 read `p[1] =
2` — **the second kernel is reading the first kernel's output at a per-lane computed offset that
is not its own index**, and the answer shows the two groups. This is the exact thing
`examples/decode_loop.lichen`'s `keep` needs and the thing its own note calls the step that was
not taken. Chaining is what `kvcache.lichen` already measures; **what is new here is only that the
offset is derived from the lane index by arithmetic rather than supplied**, and `p3` says that
costs nothing.

### 2.3 Shape 3 — expressible today, and it is a plain transpose

**The program.** One launch of `6` lanes, `.rows = 2`, `.ranks = 3`, over
`p = [0, 1, 2, 3, 4, 5]`:

```text
  lane = compute.range k.n
  rank = lane / k.rows
  row = lane % k.rows
  here = compute.read ((compute.Read _)(.from k.in.p, .at (row * k.ranks + rank)))
  compute.write ((compute.Write _)(.to k.out.q, .at (rank * k.rows + row), .value (here ~ "element")))
```

**The output.**

```text
[0.0, 3.0, 1.0, 4.0, 2.0, 5.0]: array{Float, 6}
```

**What decides it.** That is the `2 × 3 → 3 × 2` transpose, and the **read address is a genuine
permutation of the lane index**: lane `1` reads `p[3]` and writes slot `1`. So a body can do what
§1.3 held open — read at an address that is a permutation, not merely an affine form of its own
index. `llm-operators.md` records *there is no spelling that transposes a buffer*; measured here,
**that sentence is true of a caller trying to transpose two separate operators' boundaries and
false of the capability** — a launch whose read and write addresses are different affine
functions of the lane index transposes, and
[two-dimensional-launch](two-dimensional-launch.md) §2.1 had already measured the write half of
this on the device (`out.z[c * 2 + r]` → `[1, 4, 2, 5, 3, 6]`). **A rank-wise gather is therefore
no more than the permutation, and it runs.**

### 2.4 Shape 4 — `keep` is reusable as written, and one wall sits beside it

**The program.** `keep_raw` with **two** step outputs instead of one, the rank taken from the lane
index and the address into each source the computed `row`:

```text
  lane = compute.range k.n
  rank = lane / k.rows
  row = lane % k.rows
  mine = if rank == 0 then (compute.read ((compute.Read _)(.from k.in.a, .at row))) else (compute.read ((compute.Read _)(.from k.in.b, .at row)))
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (mine ~ "element")))
```

**The output.**

```text
[10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0]: array{Float, 10}
```

**What decides it.** `keep` was written with exactly one `.o` in `.in`, and the obstacle to a
`k`-way gather looked like naming `k` of them. **The obstacle is only the field list.** The two
arms of the `Select` are both **reads**, which is the shape `examples/kvcache.lichen`'s
`append_raw` already writes as `if fresh then (compute.read …) else old_k`, so widening `Src` to
`k` fields is a data change and **no new mechanism at all**. `keep` as committed is reusable here;
what it lacked was a wider struct, not a capability.

### 2.5 The same spelling at `k = 3`, so the answer is not a two-rank accident

**The program.** A third launch of `n · k = 15` lanes over the `10`-slot buffer §2.1 produced. The
body is §2.1's with one more read and one more guard:

```text
  p0 = float2int (compute.read ((compute.Read _)(.from k.in.p, .at (0 * k.rows + row))))
  p1 = float2int (compute.read ((compute.Read _)(.from k.in.p, .at (1 * k.rows + row))))
  @loop next = s => … (((1 * (ri >= s(1))) * (1 * ((s(0) - 1) != p0)) * (1 * ((s(0) - 1) != p1))) * …)
  mine = if rank == 0 then p0 else (if rank == 1 then p1 else (next (k.w, 0, 0)))
  compute.write ((compute.Write _)(.to k.out.m, .at (rank * k.rows + row), .value (int2float mine ~ "element")))
```

**The output.**

```text
[3.0, 2.0, 3.0, 0.0, 3.0, 1.0, 0.0, 0.0, 1.0, 2.0, 2.0, 3.0, 1.0, 2.0, 1.0]: array{Float, 15}
```

**What decides it.** One `15`-slot buffer holding three `5`-long ranks, `3` and `1` carried
through untouched and `2` computed by a fold that excludes two positions at once. **Nothing in the
body is rank-specific**: `rank = lane / rows`, `row = lane % rows` and
`rank * rows + row` are the same three expressions as at `k = 2`, and a rank is a **guard factor and
a `Select` arm**, not a new form. **So a third rank of `examples/sample.lichen` would need no new
mechanism** — one more launch, one more input field, one more guard factor — and this note does not
add it, because a note measures and the example is the next slice's to write.

**The rank-two row is where the hand-derivation and the program disagreed, and the program is
right**, so it is recorded rather than smoothed over. Row 0 is `3 9 4 9`; ranks `0` and `1` are `3`
and `1`, which removes positions `3` **and** `1`, leaving `{3, 4}` at positions `0` and `2` — so
rank two is `2`, which is what the program answers. Reading it as "the other `9` is still there"
gives `3`, and `3` is wrong.

#### The one real wall beside `keep`, and the condition it actually has

`keep` needs its ranks **addressable**, and §2.2 says they can be. The tempting cheaper spelling is
to compute rank zero's position **in the same lane** and use it to exclude, which would make the
whole thing one launch. **That is refused, and the refusal is about a `@loop`'s result crossing
into the next `@loop`:**

```text
error: compute.parallel: Bin(Int, Add) reads ValueId(29), which block 0 defines, and block 0 does not dominate block 2
```

`ValueId(29)` is `first = top (k.w, 0, 0)` — the first nest's result — and `block 2` is the second
nest's step. **The control separates the two readings.** Two `@loop` nests in one body where the
second does **not** read the first's result runs, and answers:

```text
[2.0, 0.0, 0.0, 0.0, 6.0]: array{Float, 5}
```

(two plain argmax folds over the same row, summed — `[1, 0, 0, 0, 3]` twice, the `>=` tie rule
`examples/sample.lichen` measures).

**Both of those still reproduce**, re-typed at `dev@eb3132ae` — the control's row bit for bit, and
the refusal with the same message and a different expr of identifiers, the value and the block
number being what the program changes:

```text
error: compute.parallel: Bin(Int, Add) reads ValueId(22), which block 0 defines, and block 0 does not dominate block 6
```

**What changed is not the tree but the claim.** The sentence this subsection ended on — *a nest's
result may be combined outside any nest, but may not be read by a following nest* — is **wrong on
its second clause**, and `examples/topk.lichen` (`94cb6ad6`) is the program that says so: three
nests in one body, `f1`'s step reading `first` and `f2`'s step reading `first` and `second`, and it
runs and answers the three ranks. **A following nest may read an earlier nest's result.** What is
refused is narrower and needs **two conditions at once**, neither of which carries it alone:

1. **a cross-read** — a nest's result read inside a following nest's step — and
2. **an arithmetic operator outside every nest** — a `Bin`, in the checker's vocabulary — **whose
   operand is a nest's result and whose value a `compute.write` takes.**

Each half alone runs, and the combinations are the whole table:

| conditions | the program | outcome |
|---|---|---|
| (2) alone, on two nest results | the control above — two nests, **neither reading the other**, the **sum written** | **runs** — `[2.0, 0.0, 0.0, 0.0, 6.0]` |
| (2) alone, on no nest result | two nests **with** a cross-read, and a third write of `lane + 1` | **runs** — `([1.0, 0.0, 0.0, 0.0, 3.0], [3.0, 2.0, 1.0, 1.0, 2.0], [1.0, 2.0, 3.0, 4.0, 5.0])` |
| (1) alone | three nests, three writes each taking one rank **directly** | **runs** — `([3.0, 2.0, 3.0, 0.0, 3.0], [1.0, 0.0, 0.0, 1.0, 2.0], [2.0, 3.0, 1.0, 2.0, 1.0])`, `examples/topk.lichen`'s own row |
| (1) and (2) | two nests with a cross-read, and a write of `first + 1` | **refused** — `error: compute.parallel: Bin(Int, Neq) reads ValueId(75), which block 11 receives as a parameter, and block 11 does not dominate block 6` |

**Four readings the bisection rules out, each measured rather than argued, and none of them is the
condition.** The **number of nests**: a three-nest body refuses when it combines its results, and a
two-nest body runs when it writes them directly. The first nest's fold being **`Float`-carrying**:
an all-`Int` first nest with two direct writes runs, and a `Float`-carrying one with a combined
write refuses. The **order** the writes demand the values in: moving the arithmetic write **first**
still refuses, so it is not an ordering artefact of the writes. And the **`~ "element"` mark**:
the refusal stands with the mark removed. Two readings that are the condition are also worth
saying — the operator need not be over **two** nest results (a write of `first + 1` refuses), and
it need not be over the **crossed** one (a write of `second + 1` refuses).

**So the boundary, in the vocabulary the checker emits:** a nest's result that a following nest's
step reads is in scope there and the body lowers; what is refused is that same body also placing a
nest's result as an **operand of a `Bin` outside every nest** whose value a `compute.write` takes —
and the message then reports exactly that shape, a read whose defining block the reading block is
not dominated by. **A `Select` is not it**: §2.1's `mine = if rank == 0 then first else (next …)`
is a selection between two nest results reaching a write, and it runs.
`crates/lichen-compute/src/compute/body.rs:287-296` is where the placement happens: a value first
demanded inside a nest is emitted at that nest's **entry** block. **Why the operator is what turns
the cross-read fatal is not established here** (§4), and no fix is proposed.

**What this costs the ladder, and what it does not.** The escape this subsection used to price —
one launch per rank, so `first` becomes a `compute.read` of the previous kernel's buffer (§2.1,
§2.2) — is **no longer needed for the exclusion itself**, and §3 is corrected below. It is still
what the **gather** wants, for the addressability §2.2 measures, which is a different reason and
survives.

## §3 What top-`k` would cost, given the verdicts

**The gather costs nothing in address arithmetic, and the price is paid in launches.** Both
spellings do **the same arithmetic** — `k` folds over each row, `k` passes over the row's data —
because a rank is a guard factor and a `Select` arm (§2.5), not a separate body. What differs is
how the ranks leave the kernel:

| spelling | launches for `k = 2` | buffers | address arithmetic |
|---|---|---|---|
| the `k`-tuple, **measured at `k = 3` in `examples/topk.lichen`** | **1** | `k`, one per rank | none |
| the one-buffer gather (§2.1) | **`k`** | **1**, of length `n · k` | `rank = lane / rows`, `row = lane % rows` |

So **the one-buffer answer is not free in launches — it costs `k` where the `k`-tuple costs 1** —
and what it buys is a single contiguous buffer in which every rank is addressable by
`(rank, row)` arithmetic alone, which is what makes a consumer like `keep` a three-line body
(§2.4) instead of a `k`-way buffer dispatch. **Neither is the gather being hard; they are two
places to put the same folds.**

**So the honest cost list for top-`k`, given these verdicts:**

- **The concatenation: free.** `rank · n + row` is the lane index, so a `n · k` launch *is* the
  concatenated buffer (§2.1). Nothing in the language had to be added for this and nothing in it
  had to change.
- **The ranks: still one pass each.** Nothing here shortens `k` passes to one. `examples/sample.lichen`
  and [llm-operators](llm-operators.md) both record that a `@loop`'s domain has **one leaf**, so a
  fold returns one value and the maximum is unobservable afterwards; a rank-`r` position needs a
  fold that knows which `r` positions to skip, and that is `r` state slots or `r` separate reads.
  Measured at `k = 3` (§2.5): rank `2` is a fold with **two** guard factors, so the guard is `r`
  factors wide rather than `r` state slots.
- **The tie rule stays per-rank and that is not cosmetic.** In §2.1 rank zero is
  `examples/sample.lichen`'s `Float` fold (`>`, highest position) and rank one is the `Int` fold
  (`>=`, lowest position) — the two spellings the example already names — so the answer on a tie
  row like `7 7 7 7` is rank zero `3` and rank one `0`. **A `k`-rank sampler that wants one rule
  across all `k` ranks has to pick one fold and give every rank it**, and that is a property of
  the body, not of the gather.
- **`float2int` on the data stays.** Rank one is right only for scores in the exactly
  representable integer range, which is `examples/sample.lichen`'s own named cost and is not moved
  by anything measured here.
- **One requirement the gather imposes: the ranks must be addressable, so they must already be
  in a buffer.** That is §2.2, and §2.4's re-measurement at `dev@eb3132ae` is what withdrew the
  second one this list used to carry: a rank that exists only as a `@loop` result inside a lane
  **can** exclude a position in a following nest, and `examples/topk.lichen` builds all three ranks
  in **one** launch and one body. **So the price of the one-buffer answer is still one launch per
  rank** — `k` launches widening one buffer, where the `k`-tuple needs `1` — but the reason is
  **addressability and not a dominance rule**, and there is still **not a new operator**.

## §4 Measured, inferred, unknown

- **Measured, on the host only.** All four verdicts, the `k = 3` run and the one refusal, each with
  the program and the output above, at `dev@176b2f20` through `lichen-compiler`'s `run` against
  the `"cpu"` backend. The device leg was **not** reached: the CLI installs no backend, and
  `examples/decode_loop.lichen` records the same gap for `keep`.
- **Measured, from the source rather than from a run.** That the concatenation layout being the
  identity on the lane index is arithmetic (`rank = lane / n`, `row = lane % n`, so
  `rank · n + row = lane`), not an observation. It is also **falsifiable by the run**: §2.1's
  `.m` and `.t` differ element for element in the lanes where the two layouts differ, which is
  what decides it.
- **Measured.** That the §2.1 spelling reaches **`k = 3` with no new mechanism** (§2.5): one more
  launch, one more input field, one more guard factor, and the same three rank-agnostic
  expressions. A rank is a guard factor and a `Select` arm, not a new form.
- **Inferred.** That the `keep` widening of §2.4 generalises to `k` sources by the same argument.
  Run at `k = 2`; the `Select`'s arms are reads either way, which is what the argument rests on.
- **Unknown.** The **device** leg of every verdict, for the reason above.
- **Measured, at `dev@eb3132ae`.** That §2.4's dominance refusal **still reproduces** — the error
  quoted above, re-typed, and the control's row bit for bit — and that the **condition** §2.4 gave
  for it was too strong: a cross-read alone runs, an out-of-nest operator alone runs, and it is
  the two together that are refused. The table is §2.4's; the programs are re-typed there.
- **Inferred, from the sources rather than from a run.** That the refusal is a lowering-ordering
  artefact and not a statement about what the language may express:
  `crates/lichen-compute/src/compute/body.rs:287-296` places a value first demanded inside a nest
  at that nest's **entry** block, which is why the message names a defining block the reading block
  is not dominated by. **Why the out-of-nest operator is what turns the cross-read fatal is not
  established**, and **no `wontfix:D<n>` in [code-audit](code-audit.md) covers it**, so this is not
  a route to a decision record and **no fix was tried** — the claim that the refusal could be made
  to work by lifting the out-of-nest operator is **not** made here.

