# The last instance: is `arithmetic_class`'s open class the defect?

> **Status:** **measurement only; no production change.** The question the previous
> slice left open — [undecided-type-cell](undecided-type-cell.md) §10.2, "B is a
> checker-side question — whether `arithmetic_class`'s `else` branch should decide
> when neither operand states a class" — is answered here in four parts, and the
> answer to the deciding half is **no**: deciding the branch was measured, and it
> leaves `examples/lazy_infinite.lichen` **exactly where it was** while costing
> twelve examples and three language features. §4 corrects §10.2's account of which
> cell is undecided, which the same measurement refutes.
>
> **§5's re-homing is superseded by [index-read-unify](index-read-unify.md)**: the
> cell §5 names is not the cell that prints `?a`, and it is never built by any
> example. §5 is kept as the recommendation that was measured; that note carries
> what the follow-up instrumented run found.
>
> Companions: [undecided-type-cell](undecided-type-cell.md) for the family, the
> three fixes and the history this continues; [operator-polymorphism](operator-polymorphism.md)
> §5 for the `add [1.5, 2.5]` result the open class buys and §7.1 for the
> `raw[?a, ?b]` signature it prints; [compound-element-slots](compound-element-slots.md)
> §"The read path", where §3's remaining hole is re-homed;
> [class-channel](class-channel.md) for the class vocabulary; [operators](operators.md)
> §2 for what a comparison is specified to yield.

Everything below was measured on `dev` at `4766533e`, in the worktree
`.worktrees/acs` on `feature/acs`, through `cargo run -p lichen-compiler run` — the
language front end, which [test-harness](test-harness.md) §"Verify" calls the
faithful runner for a runtime value. The probe that produced the class tables in §2
was reverted before this note was written; §6 names it and what it measured.

## 1. What the refinement assert is for

**The code.** `crates/lichen-highlevel/src/checker/operators.rs`, `arithmetic_class`'s
`else` branch, quoted whole — the tie, the invariant comment, and the registration:

```rust
} else {
    // The two operands are one class: a use at either class
    // commits the whole operation (`add 1 1.5` stays refused).
    self.check_unify(right_ty, left_ty, self.loc(right, 1), DiagKind::BinOp);
    // Invariant: the shared class must lie in the domain; a
    // refinement, so it stays pending while the class is open.

    // Without the assert a non-numeric use is not refused: `run` answers
    // the lazy marker for a class it cannot compute.

    // The domain is a set's value — the members themselves — so the check
    // reads the same graph a library contract will.
    let (int_type, float_type) = (self.int_type, self.float_type);
    let domain = self.array_node(self.current_block, &[int_type, float_type]);
    let operands = self.array_node(self.current_block, &[left_ty, domain]);
    let condition = self.op_node(
        self.current_block,
        P::Operator::from(TypeOperator::InDomain),
        Some(operands),
    );
    self.register_assert(
        condition,
        self.loc(e, 1),
        true,
        AssertSpelling::Refinement { domain },
    );
}
```

The `# Invariant` above it (`operators.rs:90-94`) is the contract deciding would
contradict: *"a concrete `Float` operand selects `Float` for the whole operation, two
stated operands must agree, and two unstated ones share one open class gated by a
refinement — so `max 1 1.5` is refused by the same tie that refuses `1 + 1.5`, and a
`Float` maximum is a `Float` rather than a `select`."*

**Who depends on the class being gated rather than decided.** Three candidates were
named in the brief — a caller, a pass, a diagnostic, a later unify. Measured, all four:

| candidate | verdict | evidence |
|---|---|---|
| **a pass** | **yes, and it is the enforcement** | `Module::check_asserts` (`crates/lichen-lowlevel/src/assert.rs:32-37`): *"An undecided condition records no error and stays pending: the template an apply clone re-checks per call."* Measured: `add ["a", "b"]` fails **twice**, both from the prelude — `assertion failed: expected 1, found 0` at `builtin/core.lichen:3:39` and `3:56`, the two `@in Num` conditions the `else` branch registered. Nothing else refuses that program. |
| **a caller** | **yes, twice** | `max`/`min` — measured on `dev`: `f = x => y => max x y` prints `(?a => ?a -> ?a,): struct{.f ?a -> ?a -> ?a}`, `f 1 2` is `2: Int` and `f 1.5 2.5` is `2.5: Float`. The frozen prelude's `add` — measured: `add [1.5, 2.5]` is `4.0: Float`. |
| **a diagnostic** | **the refusal is, the *flavour* is not** | `AssertSpelling::Refinement { domain }` is registered, but it never reaches a message on this path. `diagnostic.rs:510-518` handles a failing assert whose template is an `AnyNodeId::Static` ref by pushing a `Diag` with **no** `assert_spelling`; the frozen prelude's asserts fail as static templates, so the measured message for `add ["a", "b"]` is the generic `assertion failed: expected 1, found 0`, never `does not satisfy {Int, Float}`. |
| **a later unify** | **no** | Measured: the class is closed at the **apply**, by the argument. `f = x => x + 1` already states `Int -> Int` on `dev`, and `f 1.5` is refused there (`error: expected Int, found Float`, `2:3`) — the open class is not what admits a `Float` to a monomorphic addition. |

**So the answer that decides the slice: deciding earlier is an over-constraint, not
a fix.** The gate is load-bearing for a pass and for two callers, and no later unify
needs it open. §3 measures what it costs.

One reading is worth correcting while it is here. The `stated` test
(`operators.rs:106`) is described in §10.2 as the discriminator between the decided
and undecided controls, and the control table reproduces byte for byte — but the test
is not what the *user's* program turns on, because `+ - * / < <= > >=` are routed onto
the frozen prelude and `arithmetic_class` therefore runs **inside `core.lichen`**
(`crates/lichen-language/src/core.lichen:3-10`). Measured: the open branch fires eight
times while the prelude is checked and **zero times in the user's generation**, for
every control and for `lazy_infinite.lichen`. `max`/`min` are not routed, and they
are the one user-code path that reaches the branch. That is §3's headline: the branch
is a decision about the *prelude's* signature, not about any cell in the user's module.

## 2. Can the class be decided, and who else holds that cell

**The cell is the operand's own.** Measured with the probe: the `else` branch returns
`left_ty`, not a fresh cell, and `check_unify(right_ty, left_ty)` has already merged
the two — every recorded call reads `left == right`. So "deciding it" is deciding the
**left operand's class**, which is the polymorphism the `# Invariant` names. There is
no separate shared cell to decide without also deciding the operand.

**Holders, counted.** Class membership read after the run, `n` = members and
`decided` = members holding a value:

| program | cell | class | `n` | `decided` | members |
|---|---|---|---|---|---|
| `f = x => [x, @shallow [x + 1, x + 2]]` ; `g = f 0` ; `g(1)` | the array's element cell | `44v1` | **3** | **0** | `44v1` (the `x + 1` apply's `c`), `29v1` (the element cell), `62v1` (the `x + 2` apply's `c`) |
| the same program, after the call | the cell that **prints** `?a` | `133v1` | **3** | **0** | `133v1`, `151v1`, `160v1` — the per-call clone of the class above |
| the same, with `(x : Int)` | identical | `44v1` / `133v1` | 3 / 3 | 0 / 0 | identical |
| `x = 0` ; `xs = [x, @shallow [x + 1, x + 2]]` ; `xs(1)` | the array's element cell | `19v1` | 11 | **11** | every member decided |
| `examples/lazy_infinite.lichen` | **the array's element cell** | `22v1` | **15** | **15** | `22, 32, 12, 328, 263, 269, 397, 301, 307, 466, 370, 376, 491, 501, 511` |
| the same | the two `f`-apply result cells | — | **1 each** | **0** | `43v1`, `51v1` |
| the same | the three cells that **print** `?a`/`?b`/`?c` | — | **1 each** | **0** | `102v1`, `142v1`, `195v1` |
| the same | `inf = f 0`'s result cell | `66v1` | 3 | 3 | `66v1`, `71v1`, `234v1` |

Module scale, for the "how many check paths" half: `lazy_infinite.lichen` ends its run
with **89 undecided classes**, 87 of them singletons, and exactly two non-singleton
(`{28v1, 21v1}` at `n=2` and `{89v1, 232v1, 116v1, 156v1}` at `n=4`). The decided
control `x = 0 ; …` ends with **two**, both singletons. Neither count is a class
`arithmetic_class` could reach: §1 measured that the branch does not run in the user's
generation at all for the routed operators, and the one class it does reach in user
code — `max`/`min` over two unstated operands — is a single class per program, the
operator's own operands.

**So the blast radius of deciding the branch is bounded and small: one class per
program that spells `max`/`min` over two unstated operands, and zero user classes for
the eight routed operators.** §3 measures what that costs anyway.

## 3. Blast radius, measured: D1

**The experiment.** The minimal form of "decide it": in `arithmetic_class`'s `else`
branch, `check_unify(left_ty, self.int_type, …)` alongside the existing tie, and the
tail returns `self.int_type` instead of `left_ty`. Nothing else changes. Reverted
after the run.

| program | `dev` | with D1 |
|---|---|---|
| `examples/lazy_infinite.lichen` | `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}` | **unchanged** |
| `f = x => [x, @shallow [x + 1, x + 2]]` ; `g = f 0` ; `g(1)` | `[raw 1, raw 2]: array{?a, 2}` | **unchanged** |
| the same with `(x : Int)` | `[raw 1, raw 2]: array{?a, 2}` | **unchanged** |
| `f = x => [x, @shallow [x + 1, x + 2]]` ; `g = f 0` ; `g` | `(0, [raw none, raw none]): tuple{Int, array{?a, 2}}` | **unchanged** |
| `add [1.5, 2.5]` | `4.0: Float` | `error: expected array{Int, 2}, found array{Float, 2}` |
| `poly = x => y => x + y` ; `poly 1.5 2.5` | `4.0: Float` | `error: expected Int, found Float` |
| `f = x => y => max x y` ; `f 1.5 2.5` | `2.5: Float` | `error: expected Int, found Float` |
| `f = x => y => max x y` (signature) | `?a -> ?a -> ?a` | `Int => Int -> Int` — decided, and **wrongly**: the class was polymorphic |
| `add ["a", "b"]` | `assertion failed: expected 1, found 0` ×2, from `core.lichen:3` | `error: expected array{Int, 2}, found array{string, 2}` — the refinement's own refusal is gone |
| `f = x => x + 1` (signature) | `(Int => Int,): struct{.f Int -> Int}` | `(Function,): struct{.f raw[Function, …]}` — **a printer regression**: the frozen signature stops rendering |
| `add [1, 1.5]`, `max 1 1.5`, `f 1.5` | already refused | unchanged — the stated branch was never the question |

`cargo test -p lichen-compute --test lichen_compute examples`, `--test-threads 2`:

| | `dev` | with D1 |
|---|---|---|
| clean | 42 | **30** |
| panicked | — | `attention`, `convert`, `decode`, `decode_loop`, `generate`, `layernorm`, `rmsnorm`, `rope`, `silu`, `softmax` |
| drifted | `lazy_infinite.lichen` | `import/geometry.lichen`, `import/math.lichen`, `lazy_infinite.lichen` |

**Net: twelve examples move, zero are fixed.** Ten `compute` examples stop running and
the two `import` examples lose the `raw[?a, ?b]` their `output =` already declares
([undecided-type-cell](undecided-type-cell.md) §10.3 — those two *declare* `?a`, and
D1 changes what they print without changing what they are allowed to declare). One
language signature regresses to an unrenderable form. Three measured features are
lost: the `Float` add, the `Float` maximum, and the polymorphic `x + y`.

**Why D1 cannot work, stated as a mechanism rather than as the table.** Deciding the
branch decides the **prelude's** parameter cell. A user's cells are decided by the
**argument** at the apply, which is why `f = x => x + 1` already reads `Int -> Int` on
`dev` without any help. Deciding the prelude's cell does not reach the clone's cells —
the clone is built per call and its own class is closed by the argument, not by its
template — so the user's `?a` is untouched. §2's table says the same thing from the
other end: in `lazy_infinite.lichen` the array element class is **fully decided**
(15 of 15) while the cells that print `?a` are undecided **singletons**.

## 4. The correction §10.2 needs

[undecided-type-cell](undecided-type-cell.md) §10.2 wrote, of `lazy_infinite.lichen`:

> The join **runs**: measured, `inf = f 0`'s result cell `71v1` sits in a class whose
> representative `66v1` is decided. What is undecided is the **array's element type
> slot**.

`71v1` and `66v1` reproduce exactly — `inf = f 0`'s result cell is in class `66v1`,
`n=3`, all three decided. The second half does not:

- **The array's element type slot is decided.** Measured: element cell `32v1`, class
  `22v1`, **`n=15`, 15 decided**.
- **The apply's own cell is not.** Measured: the two `f`-apply result cells `43v1` and
  `51v1` are **undecided singletons**, and so are the three cells that print
  `?a`/`?b`/`?c` (`102v1`, `142v1`, `195v1`).

§9.6's probe — "`lazy_infinite.lichen` … `269v1 → 328v1`, `307v1 → 397v1`,
`376v1 → 466v1` … **all members decided**" — is the source of the slip, and its cells
are all members of class `22v1`: the **array element** class. A decided *array element*
class was read as a decided *apply* cell. The lesson is the one §9.6 already records
about itself: a probe that reports the class it happens to stand on reports the class,
not the node the reader had in mind.

For the §10.2 controls the shape is the reverse of `lazy_infinite.lichen`'s and the
note's sentence happens to fit them: there the array's element cell `29v1` and the two
apply cells `44v1`/`62v1` are **one** undecided class of three. Two programs, two
shapes, one account — which is why the correction is a correction rather than a
replacement.

**What survives.** The attribution to `arithmetic_class` is right in one specific and
useful sense, and §1 measures it: `arithmetic_class`'s open branch is the only place in
the language that would decide the shared class, so it is the right place to have
looked. The conclusion §10.2 drew from it — "B is a checker-side question … and it is
the user's call, as §7 framed A's" — is wrong, and §3 says why: it is not a
checker-side question at all, because deciding there does not reach any of B's cells.

## 5. Verdict, and what stays broken under each option

> **The re-homing below is superseded by [index-read-unify](index-read-unify.md) §2.**
> It is kept as the recommendation that was measured, and it is wrong about the
> cell: `check_index`'s `lazy_index_path` `elem_cell` is never built by any example
> in the suite — measured, 734 `check_index` calls over all 52 files, **zero** on
> the lazy arm — and the `?a` cells come from the *sibling* read in
> `checker/structs.rs`. The "missing write" is not a write; that note measures it.

**Recommendation: leave `arithmetic_class` alone and re-home family B.** Deciding the
branch is measured over-constraint (§3: twelve examples, three features, one printer
regression, zero fixes). What the measurement actually points at is the *other* half
of the same chain: in `Checker::check_index`
(`crates/lichen-highlevel/src/checker/indexing.rs:70-83`), a container whose type is
undecided at check time gets

```rust
let elem_cell = match element {
    Some(element) => element,
    None => self.lazy_index_path(
        array_ty,
        &[
            shape::TYPE_SHAPE_SLOT,
            shape::ARRAY_TYPE_ELEMENT_SLOT,
            shape::PAIR_TYPE_SLOT,
        ],
    ),
};
```

and **that** `elem_cell` is the cell the three `?a`/`?b`/`?c` print from — measured, one
undecided singleton each. The missing write is the index read's carry from a container
element cell that is already decided (`15 of 15` in `lazy_infinite.lichen`), which is
[compound-element-slots](compound-element-slots.md) §"The read path", in the same
`lichen-lowlevel` neighbourhood as the `pair_type_half` fix that closed the other
three, and **not** a checker decision. A fix there would be its own slice and needs
your decision; this one touches nothing.

| option | verdict | what remains broken | `examples` / `sync-readme` |
|---|---|---|---|
| **fix `arithmetic_class` (D1)** | **rejected, measured** | everything in §3's right column | **more blocked than now.** `sync-readme` re-derives every `output =` from a live run, so it would fail on ten examples and rewrite `import/math.lichen`'s and `import/geometry.lichen`'s declarations against an output the language no longer intends |
| **leave it, fix the index read** | **recommended — and taken.** [index-read-unify](index-read-unify.md) §7 landed it, on the `@shallow` mark and not on this read | nothing: `lazy_infinite.lichen` reads its declared form | **unblocked.** The survey is `clean=43 panicked=[] drifted=[]`, so `sync-readme` has nothing left to rewrite |
| **leave it and declare the drift intended** | **not available, and now moot** | — | [undecided-type-cell](undecided-type-cell.md) §5 rules this out on the merits: `operators` §2 specifies `Int`, and three of the four already print `Int` for a neighbouring element of the same shape. The drift it would have committed is gone |

## 6. Measured, inferred, unknown

**Measured** — the §10.2 control table and `lazy_infinite.lichen`, re-run and
reproducing byte for byte; `clean=42 panicked=[] drifted=["lazy_infinite.lichen"]` on
`dev`; every class table in §2 and every cell in §4; that `arithmetic_class`'s open
branch fires eight times in the prelude's generation and **zero** in the user's, for
all four controls and for `lazy_infinite.lichen`; `max`/`min` reaching it in user code;
`add [1.5, 2.5]`, `poly 1.5 2.5`, `max x y` on `f 1.5 2.5`, `add ["a", "b"]`,
`add [1, 1.5]`, `max 1 1.5`, `f 1.5`, and the `f = x => x + 1` signature, on `dev`; the
`Refinement` spelling not reaching a message, and `diagnostic.rs:510-518` as the reason;
the whole of D1's table and its `examples` result.

**Inferred** — that D1's twelve-example cost generalises to other programs, from one
`examples` run over 52 files; that `check_index`'s `lazy_index_path` is where the
remaining carry belongs, from the cells measured in §4 pointing at it rather than from
a trace of the read; that §1's three gate-dependents are the only ones, from the four
candidates named in the brief plus the register at `operators.rs:135` and the one
reader of its spelling at `render.rs:299`.

**Unknown** — whether the index read's carry has a fix that does not cost the open
class. Nothing here measures one; §5 says where to look, not what to write.
**[index-read-unify](index-read-unify.md) answers it: there is no carry to write.**
The cell §5 names is never built, the cell that prints `?a` is a different one, and
what the chain needs is the type of a `@shallow` infinite tail — which the mark,
not the read, kept undecided. **§7 of that note changed the mark instead, by the
user's decision, and `lazy_infinite.lichen` now reads its declared form.**
`arithmetic_class` itself is untouched: the `?a` that remains on
`f = x => [x, @shallow [x + 1, x + 2]]` is this note's own class, and D1's verdict
over it stands.

**Not this note's subject** — the `Refinement` *flavour* being unreachable. Measured,
recorded in §1, and left alone: [operator-polymorphism](operator-polymorphism.md) §8.5
records that spelling as closed-landed, and the gap is the static-template arm of
`diagnostic.rs`, not `arithmetic_class`.

**No production change.** The probe — a `crates/lichen-highlevel/src/probe.rs` module
plus six `eprintln!` call sites in `checker/operators.rs`, `checker/indexing.rs`,
`checker/lambda.rs`, `checker/tuples.rs`, `checker.rs`, `crates/lichen-highlevel/src/lib.rs`,
`crates/lichen-language/src/lib.rs` and `crates/lichen-language/src/run.rs` — and D1's
two lines in `arithmetic_class` were both reverted with `git checkout` before this note
was committed; `git diff dev...HEAD --name-only` names this note,
`docs/README.md`, and §4's correction to
[undecided-type-cell](undecided-type-cell.md), and nothing else.
`crates/lichen-lowlevel/src/evaluation.rs` and `crates/lichen-lowlevel/src/gc.rs` were
not touched, and no `Invariant` or `Safety` section was added or changed.
