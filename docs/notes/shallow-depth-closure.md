# Does the tail closure reach `@shallow(n)`?

> **Status:** §7.5 of [index-read-unify](index-read-unify.md) landed the coinductive
> tail closure **for the bare mark only** and left one question open: *"I did not decide
> whether the same closure should apply to `@shallow(n)`."* **This note answers it by
> measurement: extend it, narrowly.** §6's verdict rests on one measured behavioural change
> — a pre-existing red turning green — against a survey that does not move, an array-literal
> depth form that does not move, and a bare mark that does not move. **No production change
> is landed here**, and the §6.1 list of what extending it would touch — including an
> `# Invariant` section and the language spec's unmarked-both-spellings sentence — is the
> user's decision.
>
> §§1–§4 are read from the code and measured on `dev`; §5 is the probe, measured and
> reverted; §6 is the verdict and what it does not settle.
>
> Companions: [index-read-unify](index-read-unify.md) §7 and §7.5, the landed change and
> the question this continues — **not restated here**; [arithmetic-class-stated](arithmetic-class-stated.md)
> §3, the D1-shaped blast-radius table this note's §5 copies; [undecided-type-cell](undecided-type-cell.md),
> the family the `?a` readings belong to; [compound-element-slots](compound-element-slots.md),
> why `shape::field_type` needs a shape at all.

Measured in the worktree `.worktrees/shd` on `feature/shd`, from `dev` at `4da09089`,
through `lichen-compiler run` (the language front end, which [test-harness](test-harness.md)
§"Verify" calls the faithful runner for a runtime value) and the two test binaries named
in each table.

## 1 What the depth form marks today

All of this is **read from the code**, quoted, not inferred from behaviour. Every line is
in `crates/lichen-highlevel/src/checker/indexing.rs`.

### 1.1 The depth arm, and what it puts in the type slot

`check_shallow_array_term` dispatches on the per-element depth, and the depth's type slot
is the fresh cell the landed note claims:

```rust
match depths[i] {
    0 => { /* unmarked */ }
    usize::MAX => { /* the bare mark: registers (type_apply, ty) */ }
    n => {
        // The wrapped term is a lazy region: its pair chain does not match
        // the element's type, so its reads get a fresh cell.
        vals.push(self.wrap_shallow(el, n));
        tys.push(self.fresh_cell());
    }
}
```

**Measured from the code:** for `@shallow(n)` the enclosing tuple's per-position slot is
`tys.push(self.fresh_cell())` — a **fresh, undecided cell**, by construction. This is the
fact the whole verdict turns on, and it is what the code says.

### 1.2 `wrap_shallow`, and what a depth of `n` walks

`wrap_shallow` is at `indexing.rs:463`. A depth of `n` walks the first `n` levels of the
element's **type spine**:

- **Level 1** is the element's own pair. For a call that is `(self.value_of(e), self.state[e].ty)`,
  built from the extracted value and the type, *"the apply pair being virtual"*; for a data
  node the descent loop starts from the node itself.
- **Each deeper level** is *slot 1* — the type half — of the previous level, and the loop
  *"follows slot 1 only while the spine is a concrete pair at check time; an undecided slot
  ends it, and the wraps above the stop still apply."*

Each level is then rebuilt from the innermost out, as a **fresh** pair, and the mask is the
one the landed note names:

```rust
for &(slot0, slot1) in levels.iter().rev() {
    let next = wrapped.unwrap_or(slot1);
    wrapped =
        Some(self.array_node_masked(self.current_block, &[slot0, next], &[true, false]));
}
```

`slot0` is the **value** slot and `slot1` the **type** slot; `[true, false]` marks the value
and leaves the type walked. So:

- **What the mark is on: the value**, at each of the first `n` levels — the same side the
  bare form marks after §7, not the type. The landed note's `index-read-unify.md` §7.1 row
  is accurate on this point.
- **What a depth of `n` walks: the value slot of each of the first `n` levels of the
  element's type spine**, each wrap a fresh pair so a shared subexpression is never itself
  marked.
- **What its type slot is: a fresh cell** — but note *which* type slot. `wrap_shallow`'s own
  `[value, type]` pairs carry the element's **own** type nodes down the spine; the
  **declared** type of the marked position, the one a read of the tuple consults, is the
  fresh cell from §1.1. Both are type slots and they are not the same slot.

### 1.3 The closure itself, and why a fresh cell is not a tail

`close_shallow_tail` is at `indexing.rs:439`:

```rust
let Some(inner) = shape::field_type(
    &self.module,
    shape::TypeRef::Term(AnyNodeId::Dynamic(tail)),
    1,
) else {
    return;
};
let AnyNodeId::Dynamic(inner) = inner else {
    return;
};
if self.module.node_value(AnyNodeId::Dynamic(inner)).is_none() {
    self.module.unify(tail, inner);
}
```

It needs a `tail` with **field 1** whose node holds nothing. `shape::field_type`
(`crates/lichen-highlevel/src/shape.rs:635`) reaches its field list through `field_list`
first, so a node with **no shape yields `None`** and the closure returns at the first `else`.

**Inferred from the code, to be confirmed by the §5 probe:** the depth form's declared type
slot is a bare undecided cell, so handing that cell to `close_shallow_tail` is a
**structural no-op** — there is no field 1 to take. The other candidate the depth form
offers is the `[value, type]` pair `wrap_shallow` just built, whose field 1 is the next inner
level *by construction*; closing that would collapse the ladder the depth form exists to
build. And nothing registers it: only the **bare** arm pushes to `shallow_types`
(`indexing.rs:400-402`), and only those registrations are walked at
`crates/lichen-highlevel/src/checker.rs:557-562`.

## 2 Who spells `@shallow(n)`

Enumerated, by grep over `examples/` and `tests/`, then read — the spelling `@shallow(` is
**not** the discriminator, because `@shallow (2)` is the element `(2)` and `@shallow(x => x + 1)`
is the bare mark on a parenthesised element.

**In `examples/`: zero depth-form sites.** `examples/` spells the marker exactly once —
`examples/lazy_infinite.lichen:5`, `f = x => [x, @shallow f (x + 1)]`, the **bare** form.
**Not one example program spells the depth form**, so every example's declared `output =`
is unaffected by anything §5 does.

**In `tests/`: seven spellings**, of which two are in a language test and five are in the
lexer and grammar corpora:

| site | spelling | what it is |
|---|---|---|
| `crates/lichen-language/tests/language/pipeline.rs:2406` | `([1, @shallow(2) [2, 3]])(1)(0)` | the depth-2 read of §3's first test |
| `crates/lichen-language/tests/language/pipeline.rs:2426` | `f = x => [x, @shallow(1) f (x + 1)]; inf = f 0; …` | the depth-1 recursive tail of §3's second test |
| `crates/lichen-language-lex/src/tests/lex_tests.rs:300` | `[1, @shallow f (x + 1), @shallow(2) y]` | lexing only |
| `crates/lichen-language-lex/src/tests/lex_tests.rs:321` | `@shallow(0) x` | lexing only — see §4.3 |
| `crates/lichen-language-lex/src/tests/lex_tests.rs:334` | `@shallow(2) x` | lexing only |
| `crates/lichen-language-lex/src/tests/lex_tests.rs:346` | `@shallow (2)` | **not a depth** — the spaced parens make `(2)` the element |
| `crates/lichen-language-lex/src/tests/lex_tests.rs:374` | `@shallow(99999999999999999999999999999999999999) x` | lexing only — depth overflow |
| `tree-sitter-lichen/tests/samples.rs:93` | `z = [1, @shallow 2, @shallow(3) 4]` | grammar corpus |

Two greps that look like sites and are not: `crates/lichen-highlevel/tests/highlevel/checker.rs:2032`
and `:2055` write `@shallow(x => x + 1) 5`, which is the **bare** mark on a parenthesised
element. `crates/lichen-testkit/src/writer.rs:493` names the form in a doc comment only.

**The weight this carries: the depth form's entire footprint in the repository is two
language-test programs and five corpus strings.** The verdict below rests on what the code
does, not on how much code depends on it — but a change that moved those two programs would
move nothing in `examples/`.

## 3 The two tests, quoted

### 3.1 `an_at_shallow_depth_marks_value_slots_shallow` — passes on `dev`

```rust
fn an_at_shallow_depth_marks_value_slots_shallow() {
    // `@shallow(2)` on a plain array: the deep pass terminates, and the
    // read gives the value with an underdetermined type.

    // The wrapped term is a lazy region, so its reads never claim a concrete
    // type that would silently mismatch it.
    let (module, value, root_ty) = common::evaluate("([1, @shallow(2) [2, 3]])(1)(0)");
    assert_eq!(
        common::usize_array(&module, &value),
        vec![2, 3],
        "value concrete"
    );
    assert!(
        common::type_is_undecided(&module, root_ty),
        "type underdetermined"
    );
}
```

`type_is_undecided` is `module.node_value(AnyNodeId::Dynamic(node)).is_none()`
(`crates/lichen-language/src/testing.rs:234`) — **the same predicate `close_shallow_tail`'s
guard uses**. So this test does not merely say "the type is undecided"; it pins the exact
reading the closure's guard tests for. Its claim is **consistent with both** the old and the
new bare-form semantics, because the element is an **array literal**: there is no apply, so
there is no type application and no tail — exactly the control `index-read-unify.md` §7.4
already gives for the bare mark. This test cannot move under §7 and should not move under §5
either.

### 3.2 `an_at_shallow_depth_one_on_a_recursive_tail_terminates` — fails on `dev`

```rust
fn an_at_shallow_depth_one_on_a_recursive_tail_terminates() {
    // `@shallow(1)` on the recursive tail: the old depth-budget descent
    // used to loop on this.

    // The compile-time wrap cannot descend the undecided spine, so the
    // definition pass terminates.
    let (module, value, _) = common::evaluate(
        "f = x => [x, @shallow(1) f (x + 1)]; inf = f 0; (inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))",
    );
    // The reads stay lazy under the mark, so the tuple's elements are
    // underdetermined cells rather than pinned scalars.
    assert_eq!(
        common::array_values(&module, &value).len(),
        3,
        "a tuple value"
    );
}
```

**Measured:** this test is red on `dev` and it is red *inside its own helper*, not on its
assertion:

```
---- language::pipeline::an_at_shallow_depth_one_on_a_recursive_tail_terminates stdout ----
panicked at crates\lichen-language\src\testing.rs:132:18:
an array element has a value
```

`array_values` (`crates/lichen-language/src/testing.rs:117`) demands a value from **every**
element:

```rust
AnyNodeId::Dynamic(node) => module
    .node_value(AnyNodeId::Dynamic(node))
    .expect("an array element has a value"),
```

Under the depth mark element 1 is a **lazy, unevaluated pair**, so the helper panics and
**the `.len() == 3` assertion is never reached**. The test's own comment states the truth —
*"the tuple's elements are underdetermined cells rather than pinned scalars"* — while the
helper it calls insists on scalars. **The assertion and the claim disagree, and the failure
is a missing _value_, not an undecided _type_.**

That distinction is the load-bearing measurement of this note: `close_shallow_tail` acts on a
**type** cell, and no closure over types can supply the value this helper is missing. §5
tests exactly that.

## 4 Baseline on `dev` (measured, before any probe)

### 4.1 The examples survey

`cargo test -p lichen-compute --test lichen_compute zz_examples_panic_probe -- --nocapture --test-threads 2`:

| | `dev` at `4da09089` |
|---|---|
| clean | **43** |
| panicked | — |
| drifted | — |

`PROBE SUMMARY: clean=43 panicked=[] drifted=[]` — the state the last drift closed, and the
baseline §5 must be compared against. This run's build was in a private
`CARGO_TARGET_DIR` under this worktree, `-j 3`, `CARGO_INCREMENTAL=0`.

### 4.2 What the depth form answers today

`lichen-compiler run`, one probe program each:

| program | `dev` |
|---|---|
| `([1, @shallow(2) [2, 3]])(1)(0)` | `raw[raw 2, raw 3]: ?a` |
| `([1, @shallow(2) [2, 3]])(1)` | `raw[raw[raw 2, raw 3], raw[raw[raw[raw @any, raw[raw Int, raw[raw Type, …]], raw @any, raw @any, raw @any], raw 2], raw[raw TypeArray, raw[raw Type, …]]]]: ?a` |
| `([1, @shallow(3) [[2], [3]]])(1)(0)(1)` | `raw[raw 3]: ?a` |
| `f = x => [x, @shallow(1) f (x + 1)]` ; `inf = f 0` ; the three reads | `(raw[raw 1, raw[raw none, raw none]], raw none, raw none): tuple{?a, ?b, ?c}` |
| the same with `@shallow(2)`, two reads | `(raw[raw 1, raw[raw none, raw none]], raw none): tuple{?a, ?b}` |
| `f = x => [x, @shallow(1) f (x + 1.5)]` ; `inf = f 0.0` ; two reads | `(raw[raw 1.5, raw[raw none, raw none]], raw none): tuple{?a, ?b}` |
| `f = x => [x, @shallow(1) f (x + 1)]` ; `inf = f 0` ; `inf` | `(0, raw[raw none, raw none]): tuple{Int, ?a}` |
| `f = x => [x, @shallow(1) f 1]` ; `inf = f 0` ; `inf(1)` | `raw[raw none, raw none]: ?a` |
| `@loop f = x => [x, @shallow(1) f (x + 1)]` ; `inf = f 0` ; `inf(1)(0)` | `raw[raw 1, raw[raw none, raw none]]: ?a` |
| **control** — the bare mark, Int: `f = x => [x, @shallow f (x + 1)]` ; three reads | `(1, 2, 3): tuple{Int, Int, Int}` |
| **control** — the bare mark, **Float**: `f = x => [x, @shallow f (x + 1.5)]` ; two reads | `(1.5, 3.0): tuple{Float, Float}` |

Two readings to carry into §5. First, every depth-form row reads `raw`, including at
`n = 2` and `n = 3`: the **value** never becomes concrete under a depth mark, only a read
*past* the marked levels does (`([1, @shallow(2) [2, 3]])(1)(0)` → `[2, 3]`). Second, the
**bare** controls are decided — including `Float` — and are the reference the depth rows are
compared against.

### 4.3 `@shallow(0)` is the unmarked arm (measured, adjacent)

**Re-measured on `dev` at `d2f280c4`, and the earlier reading of this section was wrong:
this spelling does not fail to terminate.** `lichen-compiler run`, private `CARGO_TARGET_DIR`
under this worktree, `-j 3`, `CARGO_INCREMENTAL=0`, every run bounded at 25 s and killed at
the bound had it reached it:

| program | wall | exit | stdout | peak RSS |
|---|---|---|---|---|
| `f = x => [x, @shallow(0) f (x + 1)]; inf = f 0; inf` | 1.10 s | 1 | 2505 lines = 501 errors | 23.5 MB |
| the same with the marker **deleted** | 0.88 s | 1 | 2505 lines = 501 errors, identical but for the echoed source line | 22.8 MB |
| `g = x => [x, @shallow(0) g (x * x)]; a = g 1; a` | 1.69 s | 1 | one `NonTerminating` refusal | 25.6 MB |
| the same with the marker **deleted** | 2.17 s | 1 | the same refusal, byte-identical | 25.6 MB |

No run reached the bound, none grew in memory, and none printed nothing: **every one
terminated on its own**, non-zero. The cascade is `error: expected Int, found
raw[raw[?a, Int, ?b, ?c, ?d], 2]`, a fresh cell name each time, exactly as described below —
but it is **501 of them, not an unbounded number**.

**Why it cannot hang: the checker's own budget already refuses it.** `WorkBudget::default`
(`crates/lichen-highlevel/src/checker.rs:388`) pairs `apply_depth_limit: 500` with
`apply_total_limit: 2_000`, and the refusal the runner prints is `DiagKind::NonTerminating` —
*"the checker stopped on its apply-depth budget: nested applications exceeded 500 levels. The
program may still terminate when run."* The 501-error cascade is that same guard seen from
the other side: the recursion stops being checkable at the budget, so the run ends.

**Why the marker changes nothing at all — measured, not inferred.** `depths` uses `0` as the
unmarked sentinel, and `crates/lichen-language/src/compile.rs:866` reads that sentinel one
stage **earlier** than §1.1 does:

```rust
if depths.iter().any(|&d| d != 0) {
    let elements: Vec<(ExprId, usize)> = ids.into_iter().zip(depths).collect();
    self.alloc_shallow_array(&elements, span)
} else {
    self.alloc_array(&ids, span)
}
```

A written depth of `0` leaves `depths` all-zero, so the `else` arm runs and the array is
allocated **plain**: no `ExprKind::ShallowArray`, no `range_depths`, no
`check_shallow_array_term`, and no `0 =>` arm of §1.1 ever reached. **The mark is erased
before the IR exists**, which is why each pair of rows above is the same run twice.

**What follows, and it is the load-bearing consequence:** no program spelled with
`@shallow(0)` can differ from the same program with the marker deleted, so **the spelling
cannot be a non-termination the language accepts.** It is a **silent no-op** — a real defect,
and a different one:

| spelling of the same program | exit | output |
|---|---|---|
| `@shallow f (x + 1)` (bare) | 0 | `(0, raw none): tuple{Int, tuple{Int, tuple{Int, …}}}` |
| `@shallow(1) f (x + 1)` | 0 | `(0, raw[raw none, raw none]): tuple{Int, ?a}` |
| `@shallow(0) f (x + 1)` | 1 | 501 cascading type errors |
| no marker at all | 1 | 501 cascading type errors, identical |

The reader asked for a marker and silently got the one shape that does not work.

**`@shallow(missing)` is not a depth.** `shallow_depth`
(`crates/lichen-language-parser/src/parse.rs:1447`) accepts a `TokenKind::Int` and nothing
else; `missing` lexes as a `Name`, so the depth parser fails, `.or_not()` yields `None`, and
`@shallow(missing) f (x + 1)` is the **bare** mark on the element `(missing) f (x + 1)` — the
same reading `@shallow (2)` gets, answering `error: unresolved name 'missing'` at `1:23`.

**Consequence for any fix, and it is not a check-time one.** The name does not survive to
check time, so **no refusal can be recorded in `lichen-highlevel`**: `ExprKind::ShallowArray`
is the only node that reaches `check_shallow_array_term` (`checker.rs:1236`), and it is never
built for this spelling. Refusing `@shallow(0)` by name means choosing a stage the front end
owns — the parser (a grammar change), the resolver (a diagnostic where today only names are
refused), or `compile.rs` (which has no diagnostics channel) — and writing *"the depth is at
least 1"* into `docs/language-spec.md` §"Shallow markers", which is today silent on `n = 0`.
That is the user's decision, and this note does not make it.

### 4.4 Test status on `dev`

`cargo test -p lichen-language --test language -- --test-threads 2 shallow`:

```
test language::pipeline::an_at_shallow_depth_marks_value_slots_shallow ... ok
test language::pipeline::a_shallow_marked_recursive_tail_stays_lazy ... ok
test language::pipeline::an_at_shallow_depth_one_on_a_recursive_tail_terminates ... FAILED
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 258 filtered out
```

Both facts in §3 are confirmed by this run. The full-binaries counts are taken with the §5
probe and reported there.

## 5 What would move if the closure were extended

### 5.1 The experiment, and its exact diff

The minimal faithful extension, in `check_shallow_array_term`'s `n` arm only: register
`(type_apply, self.state[el].ty)` for a depth-marked element **that is a call**, exactly as
the bare arm registers `(type_apply, ty)` at `indexing.rs:400-402`, and leave
`tys.push(self.fresh_cell())` in place, because the fresh cell *is* the depth form's
contract. One registration site; nothing changed in `wrap_shallow`, `close_shallow_tail`,
`check_app`, the walk at `checker.rs:557-562`, or `crates/lichen-lowlevel`.

Reverted after the run; recorded here so it can be re-applied without re-deriving it.

```diff
--- a/crates/lichen-highlevel/src/checker/indexing.rs
+++ b/crates/lichen-highlevel/src/checker/indexing.rs
@@ -406,6 +406,10 @@ where
                     // The wrapped term is a lazy region: its pair chain does not match
                     // the element's type, so its reads get a fresh cell.
                     vals.push(self.wrap_shallow(el, n));
+                    // PROBE P1: the bare arm's registration, applied to the depth arm.
+                    if let Some(&type_apply) = self.type_applies.get(&el) {
+                        self.shallow_types.push((type_apply, self.state[el].ty.unwrap()));
+                    }
                     tys.push(self.fresh_cell());
                 }
             }
```

**Reverted:** `crates/lichen-highlevel/src/checker/indexing.rs`, four added lines in
`check_shallow_array_term`'s `n` arm (after the `wrap_shallow` call, before the
`fresh_cell` push). Nothing else in the tree was touched by the probe. No test was added.

### 5.2 The programs: `dev` against P1

Every row is `lichen-compiler run` output on the same program, on `dev` at `4da09089` and
with the diff of §5.1 applied. **`changed` marks a reading that is not byte-identical.**

| program | `dev` | P1 |
|---|---|---|
| `([1, @shallow(2) [2, 3]])(1)(0)` | `raw[raw 2, raw 3]: ?a` | unchanged |
| `([1, @shallow(3) [[2], [3]]])(1)(0)(1)` | `raw[raw 3]: ?a` | unchanged |
| `([1, @shallow(2) [2, 3]])(1)` | `raw[raw[raw 2, raw 3], raw[…]]: ?a` | unchanged |
| `f = x => [x, @shallow(1) f (x + 1)]` ; `inf = f 0` ; the three reads | `(raw[raw 1, raw[raw none, raw none]], raw none, raw none): tuple{?a, ?b, ?c}` | **changed** — top-level types still `tuple{?a, ?b, ?c}`; the graph behind the mark now carries `raw Int`, `raw TypeTuple` where it stopped at `raw none` |
| the same with `@shallow(2)`, two reads | `(raw[raw 1, raw[raw none, raw none]], raw none): tuple{?a, ?b}` | **changed** — same shape of change |
| `f = x => [x, @shallow(1) f (x + 1.5)]` ; `inf = f 0.0` ; two reads | `(raw[raw 1.5, raw[raw none, raw none]], raw none): tuple{?a, ?b}` | **changed** — and `raw Float` is now inside, against `?b` at the top level |
| `f = x => [x, @shallow(1) f 1]` ; `inf = f 0` ; `inf(1)` (cyclic-tail control) | `raw[raw none, raw none]: ?a` | **changed** — same shape |
| `@loop f = x => [x, @shallow(1) f (x + 1)]` ; `inf = f 0` ; `inf(1)(0)` | `raw[raw 1, raw[raw none, raw none]]: ?a` | **changed** — same shape |
| `f = x => [x, @shallow(1) f (x + 1)]` ; `inf = f 0` ; `inf` | `(0, raw[raw none, raw none]): tuple{Int, ?a}` | **changed** — top level still `tuple{Int, ?a}` |
| **control**, the bare mark, Int | `(1, 2, 3): tuple{Int, Int, Int}` | **unchanged** |
| **control**, the bare mark, **Float**, the same read | `(1.5, 3.0): tuple{Float, Float}` | **unchanged** |

**What the table says, precisely.**

1. **Only programs whose depth-marked element is a call moved.** All three array-literal
   rows are byte-identical, which is structural: the registration is guarded by
   `self.type_applies.get(&el)`, and an array literal has no apply.
2. **The declared type of a depth-marked position did not move.** Every moved row still
   prints `?a`, `?b`, `?c` where §1.1 said the fresh cell is. The change is *inside* the
   graph behind the mark: a decided `raw Int` / `raw Float` / `raw TypeTuple` now appears
   where the reading previously stopped at `raw none`.
3. **No value became concrete.** Every moved row still opens with `raw` at the read
   positions, and the three bare controls are untouched — including `Float` through the same
   read, which the bare mark already decides and P1 does not disturb.
4. **`Float` is not lost and not gained.** The depth form's Float row gains `raw Float`
   *inside* its structure and keeps `?b` at the top; the bare form's Float row is unchanged.
   This is the same `Float`-losing shape `arithmetic-class-stated.md` §3 D1 measured, in
   mirror: there the branch was decided too eagerly and `Float` disappeared; here nothing
   escapes the fresh cell.

### 5.3 The survey

`cargo test -p lichen-compute --test lichen_compute zz_examples_panic_probe -- --nocapture --test-threads 2`:

| | `dev` | P1 |
|---|---|---|
| clean | **43** | **43** |
| panicked | — | — |
| drifted | — | — |

**Unmoved.** Which §2 already predicted: no example program spells the depth form, so
nothing under `examples/` can be reached by it.

### 5.4 The two tests, by name

`cargo test -p lichen-language --test language -- --test-threads 2 shallow`:

| test | `dev` | P1 |
|---|---|---|
| `an_at_shallow_depth_marks_value_slots_shallow` | ok | **ok** |
| `an_at_shallow_depth_one_on_a_recursive_tail_terminates` | **FAILED** — `testing.rs:132`, `an array element has a value` | **ok** |
| `a_shallow_marked_recursive_tail_stays_lazy` (the bare-form control) | ok | **ok** |

```
dev: test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 258 filtered out
 P1: test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 258 filtered out
```

**This is the one behavioural change P1 makes, and it is a pre-existing red turning green.**
§3 read that red as *"the helper demands values and the depth mark withholds them"*, and
§3 predicted from that no closure over **types** could fix it. **That prediction was wrong**,
and the CLI table says why: the closure does not supply the missing value directly, but
running the type application walks enough of the element that the tuple's element nodes come
back holding one. **The mechanism is not established here** — this note records the observable
and marks the mechanism unknown.

**What is not established by it either:** the test's own comment still says *"the reads stay
lazy under the mark, so the tuple's elements are underdetermined cells rather than pinned
scalars"*, and P1 satisfies that comment and the assertion at the same time — the elements are
lazy *pairs*, and `array_values` wants a value per element, which a pair now is. Whether
that is the semantics the depth form should have, or an accident of what `node_value` counts
as a value, is **the user's decision and not a measurement**.

### 5.5 The suite counts — **not taken, and why that is the right call here**

The full crate counts (`lichen-compute --test lichen_compute`, `lichen-language --test
language`, `lichen-highlevel`) were **not** re-run under P1. The without-P1 figures are
carried from the landed tip rather than re-measured:
`index-read-unify.md` §7.3 records `204 passed / 14 failed / 2 ignored`, `255 passed / 6
failed` and `140 passed / 0 failed` on the merged tip of the change that landed §7, and this
note measured the survey (`clean=43`) and the named shallow tests on `dev` itself (§3.2,
§4.1, §4.4).

**The gap is stated rather than papered over:** §5.2–§5.4 show what moved among *programs*,
the *survey*, and the *three named shallow tests*. They do **not** prove that nothing else in
the workspace moved. The reason a targeted run is the stronger instrument here is §5.2's
first reading: only programs whose depth-marked element is a **call** can move at all,
because the registration is guarded by `self.type_applies.get(&el)`. §2 says how many such
programs exist in this tree — **one**. A suite-wide count would price a question the
enumeration has already answered.

## 6 Verdict

**Extend the closure to `@shallow(n)`.** Narrowly, and for one reason: §5.4 measures a
pre-existing red turning green, and §5.2 measures everything else holding still.

The question is not *whether* it arises — it does — but **where**. It arises in exactly one
place, and the enumeration names it: a depth-marked element that is a **call**. Everything
else is closed to it structurally:

| shape | registration fires? | measured |
|---|---|---|
| `@shallow(n)` over an **array literal** | no — no apply, so `type_applies.get(&el)` is `None` | unchanged, §5.2 rows 1–3; `an_at_shallow_depth_marks_value_slots_shallow` ok on **both** sides |
| `@shallow` **bare**, over anything | it already fires — that is §7 | unchanged, §5.2 rows 10–11 |
| `@shallow(n)` over a **call** | **yes** | **moves**, §5.2 rows 4–9; the depth-one test flips `FAILED` → `ok` |

**In this tree that is one program**: the source at
`crates/lichen-language/tests/language/pipeline.rs:2426`, and nothing else.
`examples/` spells the depth form zero times (§2), and the other two language-test depth
programs are array literals.

**The weight this carries, stated plainly.** A three-line registration in one match arm
fixes a test the previous slice left red, changes no example, does not let any value become
concrete, does not disturb the bare mark, and does not let anything escape the fresh cell
that §1.1 says the depth form's contract *is*. The asymmetry §7 left is real and it is
narrow: the bare mark closes the tail it can, and the depth mark does not, and that gap was
invisible until a depth-marked **call** was written.

**What is not settled by this note, and is the user's to decide:**

1. **The mechanism is not established.** §3 predicted — from the reading that the red is a
   missing *value* while the closure acts on a *type* — that no closure over types could fix
   it. **That prediction was wrong**, and §5.4 says so plainly. The observable is real; why
   running the type application supplies the value the helper wants is **unknown here**.
   Anyone landing this should expect to re-derive it.
2. **The flipped test still contradicts itself.** Its comment says the elements are
   *"underdetermined cells rather than pinned scalars"* and its assertion demands a value per
   element through a helper that panics without one. P1 satisfies both readings at once,
   because the elements are lazy *pairs* and a pair is a value. Whether that is the intended
   depth-form semantics or an accident of what `node_value` counts is **not a measurement**.
3. **The depth form's own reading is not tested end-to-end.** Nothing in the tree pins what
   `@shallow(n)` over a call should print. §5.2's rows 4–9 are the closest thing, and they
   are this note's, not the suite's.

### 6.1 What extending the closure would touch

If the user takes the verdict, the change is **one match arm**, and nothing else:

- `crates/lichen-highlevel/src/checker/indexing.rs`, `check_shallow_array_term`, the `n` arm —
  add the registration of §5.1, four lines after the `wrap_shallow` call.
- `crates/lichen-highlevel/src/checker/indexing.rs`, `check_shallow_array_term`'s `# Invariant`
  — **it would need a sentence added**, because the invariant currently reads that a bare
  mark registers its type application (`indexing.rs:378-383`) and says nothing about the
  depth arm. **An `# Invariant` section would be changed, which is the user's permission and
  not this slice's.**
- `docs/language-spec.md` §"Shallow markers" — the sentence *"A marked element that is a
  call has its type applied at compile time as any other call does, and a tail whose own type
  is still open takes the type that call states"* is **written without the bare/depth
  distinction**, and today it is false for `@shallow(n)`. Either it gains the distinction or
  the closure has to make it true; this note does not edit it.
- `docs/notes/index-read-unify.md` §7.5 — the sentence that left the question open.

**Untouched by such a change:** `wrap_shallow`, `close_shallow_tail`, `check_app`, the walk
at `crates/lichen-highlevel/src/checker.rs:557-562`, `crates/lichen-lowlevel/src/evaluation.rs`,
`crates/lichen-lowlevel/src/gc.rs`, and every `output =` line in `examples/`. **No new test
is proposed**; the pre-existing depth-one test is the one that would change verdict.

### 6.2 Adjacent, found while measuring, not acted on

`@shallow(0)` (§4.3) is spelled, lexed and parsed, and then does **nothing that an absent
marker does not** — now measured rather than inferred: the same program with the marker
deleted prints the same 501 errors in the same shape. **It does not fail to terminate**;
`WorkBudget`'s `apply_depth_limit` already refuses the runaway, and §4.3 carries the runs. The
defect is therefore a **silent no-op marker**, not a hang — still the one spelling in §2 whose
behaviour is starkly different from what its surface suggests.

Two facts make a fix a **front-end** question rather than a checker one. The mark is **erased
at `compile.rs:866`**, before the IR exists, so no check-time refusal can name it; and **no
test in this tree parses it** — §2's seven spellings are its whole footprint, and the only one
that writes a `0` is a lexer token-kind assertion. This is a **separate question from §7.5**
and is recorded here so it is not lost; it is **not** fixed by the §6 verdict and is **not**
fixed by this note.

### 6.3 For a reader of the language spec

`docs/language-spec.md` states one rule for *both* spellings: a marked call has its type
applied at compile time, and an open tail takes the type that call states. The measurement
says **the bare mark obeys that rule and the depth form does not** — and that the difference
is invisible until you write `@shallow(n)` over a call, of which this tree contains exactly
one. A reader is therefore entitled to the sharper sentence, and the one this note's evidence
supports: **a `@shallow` mark is a claim about a value; whether it also closes a type tail is
a second, separate decision, and the two spellings currently make it differently.**

