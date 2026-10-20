# What a depth-marked element holds, and where the arithmetic went

> **Status:** measurement, **and its fix landed** on the user's decision. It answers
> the question
> [shallow-depth-mechanism](shallow-depth-mechanism.md) §4.1 left open — *"the reads
> still answer with the marked structure rather than 1, 2 and 3; the test asserts
> only a value per element"* — and it answers it **by re-measuring, not by reading
> forward**. That note established *which registration* makes the depth form's cells
> hold anything; this one asks *what they hold*, element by element, and whether the
> numbers are reachable at all. §4 carries the decision the measurement led to:
> **the held test's own program was given the arity its spelling needs and its
> assertion was strengthened from `.len() == 3` to the three numbers**, and no
> production code was touched.
>
> Measured in the worktree `.worktrees/dmev` on `feature/dmev`, branched from
> `feature/shx` at `b96cd332` (which is three commits ahead of `dev` at `7323cf6b`),
> through `lichen-language`'s own `testing::evaluate` and `testing::array_values` —
> the faithful runner and the helper the held test itself uses, per
> [test-harness](test-harness.md) §"Verify" — plus six `lichen-compiler` runs of the
> two programs and three variants. **Every probe was reverted before this commit**
> and **no test was added**: one `crates/lichen-language/tests/dmev_probe.rs`, two
> temporary `if false &&` / `let _ = &tail;` edits in
> `crates/lichen-highlevel/src/checker/indexing.rs` and
> `crates/lichen-highlevel/src/checker.rs`, and a `.probe/` directory of six
> `.lichen` files. Nothing is left behind; `git diff` on both production files is
> empty.
>
> **The rebase, and one thing it is not mine.** The branch was rebased onto `dev`
> after the measurements, which had moved to `077a28fd`. Replaying `feature/shx`'s
> `990094e1` onto it conflicted in `docs/language-spec.md`, **between two other
> sessions' prose**: `dev`'s `@shallow(0)` bound sentence against `990094e1`'s
> bare/depth sentence. The two are compatible, so the resolution keeps **both,
> verbatim** — the shared lead-in, then `990094e1`'s sentence, then `dev`'s bound
> sentence. **That resolution is a rebase artefact and is the user's to review**; no
> word of either session's sentence was changed, and no `# Invariant` or `# Safety`
> sentence was touched by this slice.
>
> Companions, referenced rather than restated:
> [shallow-depth-mechanism](shallow-depth-mechanism.md) §§1–3 (which cell is pointed
> at, which half of the closure does the work, what the three elements held);
> [index-read-unify](index-read-unify.md) §7 (the bare-form change this extends, and
> §7.5's depth-form paragraph); [shallow-depth-closure](shallow-depth-closure.md) §6.3
> (the spec sentence this bears on).
>
> **The answer, stated first:** the arithmetic is reachable under a depth mark
> **today, with no production change**, one level deeper on every read — measured as
> `usize_array == [1, 2, 3]`. What the held program reads is one level too few for
> the depth spelling, and the depth spelling is the only thing that changed.

## 1 The two programs, side by side

The **depth-marked** program is the one under review,
`crates/lichen-language/tests/language/pipeline.rs:2426`, in
`pipeline::an_at_shallow_depth_one_on_a_recursive_tail_terminates`:

```rust
"f = x => [x, @shallow(1) f (x + 1)]; inf = f 0; (inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))"
```

The **bare-mark control** is the neighbouring test that answers the arithmetic,
`pipeline.rs:2391`, in `pipeline::a_shallow_marked_recursive_tail_stays_lazy`:

```rust
"f = x => [x, @shallow f (x + 1)]; inf = f 0; (inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))"
```

The definitions are the same recursive function over the same tail, the same `inf`,
and the same three read suffixes; the difference is the mark's spelling and nothing
else.

**The difference, in one sentence:** the depth form replaces the marked element with
the call's `[value, type]` **pair**, while the bare form leaves the element as the
call's value alone — so the same read suffix walks one level deeper into a structure
that is now a type spine.

Through the CLI, on the same base:

| program | printed |
|---|---|
| bare control | `(1, 2, 3): tuple{Int, Int, Int}` |
| depth-marked | `(raw[raw 1, raw[…]], raw[…slot list…], raw TypeTuple): tuple{?a, ?b, ?c}` |

## 2 What each element holds, re-measured

Read through `testing::array_values`, the helper `pipeline.rs:2431` itself uses: for
each root-tuple item node, `module.node_value(node)`, printed recursively to four
levels. `…` below is the depth cut in the printout, not a value.

### The depth-marked program, `@shallow(1)`

| # | read | holds | class | kind |
|---|---|---|---|---|
| `[0]` | `inf(1)(0)` | `Array[USize(1), Array[NO VALUE, Array[…, …]]]` | a **2-element array** — a pair | the marked call's **value**: the pair `f 1 = [1, <lazy pair>]` |
| `[1]` | `inf(1)(1)(0)` | `Array[Array[NO VALUE, Array[TypeValue(TypeInt), Array[…]], NO VALUE, NO VALUE, NO VALUE], Array[NO VALUE, Array[…], NO VALUE, NO VALUE, NO VALUE]]` | a **2-element array of two 5-item element slots** | the **shape list** of the marked call's tuple type: `shape` of `[shape, kind]`, slot 0 typed `Int`, slot 1 open |
| `[2]` | `inf(1)(1)(1)(0)` | `TypeValue(TypeTuple)` | **not** a `LowValue` at all | the **kind marker** `TypeTuple`, read off the `kind` half `[TypeTuple, Type]` |

The chain that produces them, from the same printout: `inf(1)` is the marked element
pair `[value, type]`; `(0)` reaches its value half — which is `f 1` **itself**, a
pair, not a scalar; `(1)` reaches its type half, whose value the registration decided
to `[shape, kind]`; `(0)` on that is `shape`, a list of two element slots; `(1)` is
`kind = Array[TypeValue(TypeTuple), Array[TypeValue(TypeType), …]]`; `(0)` on the kind
is `TypeValue(TypeTuple)`.

### The bare-mark control, `@shallow`

| # | read | holds | class | kind |
|---|---|---|---|---|
| `[0]` | `inf(1)(0)` | `USize(1)` | scalar | the arithmetic |
| `[1]` | `inf(1)(1)(0)` | `USize(2)` | scalar | the arithmetic |
| `[2]` | `inf(1)(1)(1)(0)` | `USize(3)` | scalar | the arithmetic |

`inf` itself reads `Array[USize(0), Array[USize(1), Array[USize(2), Array[USize(3), NO VALUE]]]]`
in the control — the lazy list, each element the bare mark left unforced — against
`Array[USize(0), Array[Array[USize(1), Array[NO VALUE, …]], Array[Array[…], Array[…]]]]`
under the depth mark, where the tail element is the two-slot pair.

### The author's three-element description, checked

`990094e1`'s message — *"[1, <lazy pair>], a two-slot type structure, and the
`TypeTuple` kind marker"* — is **right on all three counts**, and the middle one can
be sharpened. Element `[0]` is exactly `[USize(1), <a pair whose head holds no
value>]`. Element `[2]` is exactly `TypeValue(TypeTuple)`. Element `[1]` is a
two-element array, so "two-slot" is right; what it holds is **not** a `[shape, kind]`
type pair but the **shape half alone** — the list of two element slots — which is why
its slot 0 is typed `TypeValue(TypeInt)` and its slot 1 holds an undecided cell.

One further measured fact, because it removes the obvious suspect: **the two
programs' read chains are identical.** Walking each root item down through its
`LowOperator::Index` operands, both programs emit the same subscript sequence from
the same apply node — `0, 1, 0` for `[0]`, `0, 1, 1, 0` for `[1]`, `0, 1, 1, 1, 0`
for `[2]`. The reads are not where the two programs differ; the two programs differ
only in **what the tuple's element position holds**.

## 3 The mechanism: the slot, the write, the read

The brief's hypothesis was a **value/type confusion** — *the registration writes a
type cell and the element reads it as a value*. It was tested directly. **It is
rejected: no cell is mistyped.** What follows is the slot, the write and the read,
each quoted, and then where the confusion actually is.

### The slot — the tuple's element position

`crates/lichen-highlevel/src/checker/indexing.rs:405-418`, `check_shallow_array_term`:

```rust
n => {
    // The wrapped term is a lazy region: its pair chain does not match
    // the element's type, so its reads get a fresh cell.
    vals.push(self.wrap_shallow(el, n));
    // A marked call is applied as the bare arm's is, so the mark's
    // tail cell is decided before a read goes through it.
    if let Some(&type_apply) = self.type_applies.get(&el) {
        self.shallow_types
            .push((type_apply, self.state[el].ty.unwrap()));
    }
    tys.push(self.fresh_cell());
}
…
mask.push(depths[i] == usize::MAX);
```

Line 398 is the bare arm's write of the same position, and the contrast is the whole
of §1's difference:

```rust
usize::MAX => {
    vals.push(self.value_of(el));
```

**The bare arm puts `self.value_of(el)` in the position. The depth arm puts
`self.wrap_shallow(el, n)` there**, and `wrap_shallow` is what adds the level —
`indexing.rs:501-507`:

```rust
for &(slot0, slot1) in levels.iter().rev() {
    let next = wrapped.unwrap_or(slot1);
    wrapped =
        Some(self.array_node_masked(self.current_block, &[slot0, next], &[true, false]));
}
```

with level 1 seeded at `indexing.rs:476-477` as `(self.value_of(e),
self.state[e].ty.unwrap())`. So the marked element is a **fresh masked pair
`[value_of(el), state[el].ty]`** — slot 0 the call's value, slot 1 the call's type
node, slot 0 marked shallow — and `mask.push(depths[i] == usize::MAX)` marks the
**array position** for the bare form only.

### The write — into the type cell, with a type

`crates/lichen-highlevel/src/checker.rs:557-562`:

```rust
// A `@shallow` mark is on the value; its element's type is applied here, and an open tail then closes.
let shallow_types = std::mem::take(&mut checker.shallow_types);
for (type_apply, tail) in shallow_types {
    checker.module.evaluate_node_deep(type_apply, None);
    checker.close_shallow_tail(tail);
}
```

The second component is `self.state[el].ty` — the marked call's **own type node**,
which is by construction the element pair's slot 1. `evaluate_node_deep(type_apply)`
decides that node to hold the **type value** `[shape, kind]`. **A type cell, written
with a type: correct.**

### The read — a pure value-array read

`crates/lichen-lowlevel/src/evaluation.rs:146` and `:190-215`:

```rust
Some(LowOperator::Index) => {
…
let element = array[index].node;
match element {
    Dyn(element) => {
        self.alias_read(node, element);
…
        self.evaluate_node(Dyn(element), Some(block))
```

**The read takes the target's item at `index` and evaluates it. It never consults a
type to choose a slot**, so it cannot read a type cell *as* a value: `inf(1)(1)`
selected item 1 of an array whose item 1 genuinely holds a type, and every read below
it is that type's own data. The confusion hypothesis is **not** the mechanism.

### Where the confusion actually is

It is between the **element's shape** and the **program's read arity**, and it is
two facts:

1. `@shallow(n)` puts a **pair** in the element position and `@shallow` puts a
   **value** (`indexing.rs:408` against `:398`). A pair's slot 1 is its type half,
   so `(1)` enters the type — by the project's own core idea
   ([overview](overview.md) §"The core idea"), not by any mistake.
2. The program was **written with the bare spelling's arity**. `inf(1)(0)` asks for
   the marked element's slot 0, which is `f 1`, and `f 1` *is* a pair whose slot 0
   is `1`.

Element `[0]` is the direct evidence that the registration is not implicated: with
the `n` arm's registration disabled by `if false &&`, the probe printed element `[0]`
as `Array[USize(1), Array[NO VALUE, NO VALUE]]` — **the same pair**, only the deeper
type side undecided — while `[1]` and `[2]` both became `NO VALUE`. **The
registration wrote exactly the two type-side elements; the pair in `[0]` predates it
and is `wrap_shallow`'s.**

## 4 Is there a correct fix, and what would it touch?

**For the arithmetic, a correct fix exists and needs no production change at all.**
Measured, through the suite's own `testing::usize_array` on the depth-marked program
with one extra `(0)` per read:

```rust
"f = x => [x, @shallow(1) f (x + 1)]; inf = f 0; \
 (inf(1)(0)(0), inf(1)(0)(1)(0)(0), inf(1)(0)(1)(0)(1)(0)(0))"
```

gives **`[1, 2, 3]`**, and through the CLI, `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}`.
One `(0)` per level undoes the level `wrap_shallow` adds, and the depth form's
contract is intact: the declared slots stay open (`?a, ?b, ?c`), exactly as
[index-read-unify](index-read-unify.md) §7.5 says they must.

**The change is therefore in the held test, not in the checker** — and by the user's
decision, taken on this measurement, that is what was done:

| what | where | state |
|---|---|---|
| the held test's program, one `(0)` deeper per read | `crates/lichen-language/tests/language/pipeline.rs` | **done** |
| the held test's assertion, `.len() == 3` → the three numbers | same test, `common::usize_array(…) == vec![1, 2, 3]` | **done** |
| the held test's comment, which described the pair and not the arity | same test | **done** — it now states the arity rule and points here |
| `docs/language-spec.md` §"Shallow markers" | `docs/language-spec.md` | **untouched** — its declared slot is not meant to become `Int`, so nothing there needed changing |
| `an_at_shallow_depth_marks_value_slots_shallow` | `pipeline.rs` | **untouched by decision**, and see below for why that is the reason the fix belongs in the arity |

**The strengthened assertion was checked for teeth before it was kept.** The old
spelling `(inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))`, run against the new assertion,
**fails** — `usize_of` panics at `crates/lichen-language/src/testing.rs:104` with
`expected a usize value, got LowValue(Array(… metadata: 2))`, which is exactly the
pair §2's table measured for element `[0]`. The new assertion therefore cannot pass on
a type structure or a kind marker, which is what `.len() == 3` could not tell. The
test's **name is unchanged** — `an_at_shallow_depth_one_on_a_recursive_tail_terminates`
still says what it was written to pin, and every companion that names it still
resolves.

**The arity is one extra `(0)` immediately after `inf(1)`, not one at the end of each
read.** Measured both ways: the suffix form `inf(1)(1)(0)(0)` does **not** reach the
arithmetic, because `(1)` is the element pair's **type** half, so a further `(0)` lands
inside the type's shape list. The spelling that works — and the one the test now
carries — takes the element's value half first and then walks it exactly as the bare
mark's reads do.

**If the user instead wants the same spelling to answer the arithmetic**, that is a
semantics change in the checker, and it would touch **`wrap_shallow` and the `n`
arm's `vals.push` at `indexing.rs:408`** — dropping the extra pair so the position
holds `self.value_of(el)` and marking the position in `mask` instead, the way the bare
arm does at `:418`. **That would break a passing test**, and this is why it is not
recommended: `pipeline::an_at_shallow_depth_marks_value_slots_shallow` asserts
`([1, @shallow(2) [2, 3]])(1)(0)` is `[2, 3]` (`pipeline.rs:2406-2411`), which pins
the extra level for the **array-literal** case as firmly as for the call case. The
depth form's wrapping is load-bearing, and the program, not the checker, is what is
one level short.

**And the closure.** [shallow-depth-mechanism](shallow-depth-mechanism.md) §2
measured that `close_shallow_tail` contributes nothing here; re-measured here by
replacing the `checker.rs:561` call with `let _ = &tail;`, all three elements came
back **byte-identical** to the full run — same pair, same shape list, same
`TypeValue(TypeTuple)`. So the contradiction resolves cleanly rather than sharply:
**a correct fix for the arithmetic does not need the closure, and could not be said
to need it.** Had a fix required it, that would have contradicted §2; it does not,
because the numbers live entirely on the **value** side — `inf(1)(0)` — which both
the registration and the closure leave untouched. `close_shallow_tail` is still dead
code for `@shallow(n)`; §2 stands.

## 5 What the held test can and cannot tell

Before this slice's change, `pipeline.rs` asserted, in full:

```rust
assert_eq!(
    common::array_values(&module, &value).len(),
    3,
    "a tuple value"
);
```

**It can tell** that every root-tuple item resolves to *some* value. On `dev` it was
red for a real reason — `array_values` maps each item through
`module.node_value(...).expect("an array element has a value")`
(`crates/lichen-language/src/testing.rs:129-132`), and the depth mark's cells held
nothing, so it panicked rather than failed. The registration fixed exactly that, and
`an_at_shallow_depth_one_on_a_recursive_tail_terminates` is now `ok` (measured).

**It cannot tell** that the three values are `1`, `2` and `3`. Measured: it is green
on `[1, <lazy pair>]`, a shape list, and `TypeTuple`. The author's charge — *"it is
now green on values that are not the arithmetic"* — is **confirmed**, and this note
sharpens it: it is green on values that are not the arithmetic **because the program
reads one level too few for its own mark**, not because the mark answers a type
where a number belongs.

**A stronger assertion** was available with no production change, and **by the user's
decision it is what the test now carries**: `common::usize_array(&module, &value) ==
vec![1, 2, 3]` on the one-level-deeper program of §4. It demands that the
depth-marked reads reach the arithmetic — and it passes, measured, and the old
spelling fails it, measured. What it bought is exactly what `.len() == 3` could not
say: the three elements are the three **numbers**, not a pair, a shape list and a kind
marker that happen to exist.

**Can the program answer the arithmetic at all under a depth mark?** Yes — measured,
`[1, 2, 3]` and `(raw 1, raw 2, raw 3)`. **Is a type-shaped element the mark's
correct answer?** For a read spelled at the bare mark's arity, yes, and the spec says
so: `docs/language-spec.md`, added by `990094e1`, says the depth form's declared slot
"keeps open, **so a read under the depth's mark is answered by the marked levels
themselves**". That sentence is **confirmed by this measurement** — but it is a
statement about the *marked levels*, not a licence for the *test's arity*. **So the
green was not wrong; the test was.** It pinned a mis-spelled program with a
length-only assertion, and its comment — *"an element holds a value the deep pass
never forced: a pair is one"* — described the pair correctly while leaving the missing
`(0)` unnamed. The two readings that comment left open are both right for different
elements, and only one of them is the reason the numbers were missing. The comment now
states the arity rule instead, and the assertion now says the numbers.

## 6 Corrections to the companions

Kept here, in this note's voice, with the history intact — **neither companion is
edited by this slice**, and both remain as they were.

- **[shallow-depth-mechanism](shallow-depth-mechanism.md) §3**, whose table gives
  `[1]` as *"a two-slot type structure"*: **imprecise, corrected here.** It is the
  **shape list** of the marked call's tuple type — two element slots, slot 0 typed
  `TypeValue(TypeInt)`, slot 1 open — i.e. the `shape` half of the `[shape, kind]`
  value the registration wrote, not a `[shape, kind]` pair. Its `[0]` and `[2]` rows
  (`[USize(1), <lazy pair>]` and `TypeValue(TypeTuple)`) are **confirmed exactly**.
- **[shallow-depth-mechanism](shallow-depth-mechanism.md) §4.2**, which says the
  bare mark's *"element slot is the call's own pair and the depth mark's is a rebuilt
  one, whose resolved reads come off the type side"*: **confirmed and now exact.**
  "Off the type side" is not a side effect of the reads — the read chains are
  **identical** in the two programs (§2); the difference is the rebuilt element, whose
  slot 1 is a type by `wrap_shallow`'s construction.
- **[shallow-depth-mechanism](shallow-depth-mechanism.md) §4.1**, which leaves open
  whether the depth mark *should* answer `2` and `3`: **answered — yes, and today.**
  §4 above carries the measurement. §3's `dev`-side column (elements `[1]` and `[2]`
  having no value) is re-confirmed by this slice's `if false &&` probe.
- **[index-read-unify](index-read-unify.md) §7.5**, which says the depth form's
  element type is a fresh cell and *"§7 changes only the bare form"*: **confirmed on
  every point it makes, and its one open question is now answered** — §7.5's own
  later paragraph already flagged it, and §4 of this note answers it: nothing escapes
  the fresh cell, and the arithmetic is one `(0)` deeper.
- **[shallow-depth-closure](shallow-depth-closure.md) §6.3** and
  **`docs/language-spec.md:286-289`**, the sentence *"a read under the depth's mark is
  answered by the marked levels themselves"*: **confirmed by measurement.** It is the
  accurate description of what §2's table shows.
- **[code-audit](code-audit.md) `D8`** was read before assuming anything of it: it is
  about whether a **budget refusal** is sticky or scoped (`deep_depth` versus
  `apply_depth`), and the "shallow siblings" in its text are sibling *nodes* of a
  depth-limited walk. It constrains nothing in this slice, and no `wontfix:D<n>` over
  `@shallow`, the class channel or the read path was found.

## 7 Measured, inferred, unknown

**Measured on `feature/dmev` at `b96cd332` (base `feature/shx`, three commits ahead
of `dev` at `7323cf6b`).** Every number in §1, §2, §3 and §4 came from one of these
runs:

1. `testing::evaluate` + `testing::array_values` on both programs, printing each
   root item's node value recursively — §2's two tables, §2's `inf` values, §3's
   chain.
2. The same walk with the `n` arm's registration gated off by `if false &&` —
   element `[0]` unchanged, `[1]` and `[2]` `NO VALUE` — §3's "where the confusion
   actually is".
3. The same walk with `checker.rs:561`'s `close_shallow_tail` call replaced by
   `let _ = &tail;` — all three elements byte-identical — §4's closure paragraph.
4. `testing::usize_array` on the one-level-deeper depth program — `[1, 2, 3]` — §4.
5. Six `lichen-compiler run` invocations: the two programs, `inf(1)` under each
   spelling, `inf(1)(0)(0)` under the depth mark, and the one-level-deeper program —
   `(1, 2, 3)`,
   `(raw[raw 1, …], …, raw TypeTuple): tuple{?a, ?b, ?c}`,
   `(1, raw none): tuple{Int, tuple{Int, …}}`,
   `raw 1: ?a`, and `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}`.
6. `cargo test -p lichen-language --test language -j 3 -- --test-threads 2` —
   **260 passed / 5 failed** on the measured base, and neither shallow test is among
   the five. Re-run on the rebased tip (onto `dev` at `077a28fd`, which carries the
   sibling's four `@shallow(0)` tests): **264 passed / 5 failed**, the **same five**,
   and `a_shallow_marked_recursive_tail_stays_lazy`,
   `an_at_shallow_depth_marks_value_slots_shallow` and
   `an_at_shallow_depth_one_on_a_recursive_tail_terminates` all `ok`.
7. `cargo test -p lichen-compute --test lichen_compute -j 3 zz_examples_panic_probe
   -- --nocapture --test-threads 2` on the measured base —
   **`clean=43 panicked=[] drifted=[]`**.

**Inferred from the code, not separately measured:** that `wrap_shallow`'s rebuilt
element is what makes the read enter the type half, and that marking the position
instead of wrapping would break `an_at_shallow_depth_marks_value_slots_shallow`. The
first is the code at `indexing.rs:501-507` plus §2's identical read chains; the
second follows from that test's own assertion at `pipeline.rs:2406-2411` and was not
tried, because it is a production change this slice does not make.

**Unknown here:** what `@shallow(n)` should print for a marked call end-to-end
([shallow-depth-closure](shallow-depth-closure.md) §6.1 item 3), which no test in
the tree pins — the strengthened test reads values through `testing`, not the
printer, so it says nothing about the printed form either way. **Not a question any
longer:** whether the held test should be strengthened, and whether the depth form's
element shape should change. The first was the user's decision and is taken (§4); the
second was declined, because §4 measures that changing it would break
`an_at_shallow_depth_marks_value_slots_shallow`.

## 9. The slot encoding's reading of the two held tests (round eleven)

Measured with temporary prints, then removed; the tree is unchanged by this section.

- **`@shallow(2)`** — `l(1)(0)` fails at check with an **attribute read past a narrow
  pair**: the checker's `element_pair` builds `Index(element, 2..4)` projections, and
  the element's runtime value is a two-item array (the tuple's own value), so the read
  is out of bounds. The projections are built while the element's value is still
  undecided, which is why a static guard in `element_pair` cannot see it.
- **`@shallow(1)`** — `step2(1)` reads a non-container: the lazy-region read of
  `step1(0)` is *not* recognized, so the read takes the normal slot path and yields a
  scalar; the chain then applies `(1)` to it. The recognition fails at check because
  the container's value is a clone that states no value, and the template walk
  (`first_position`/`masked_pair` through `node_origin`) does not yet reach the mask
  across that clone.
- The wrapper is now a **full-width slot** (`array_node_masked_slot`), which removes
  the out-of-bounds class for reads that reach *it*; the two failures above are one
  level outside it.

**The block-local recursion** (`a_recursive_binding_inside_a_block_recurses`) is a
separate reading: the conflict is a bare value `3` against a slot *dereference* whose
value is `2`, at the unify's root. The parameter pairs themselves are fresh per
instantiation (measured: `282` then `319` for one apply), so the sharing is one level
in — the dereference node, not the pair.

## 10. The lazy peel's resolution (round sixteen)

The one-depth chain (`f = x => [x, @shallow(1) f (x + 1)]`) is now read as a sequence
of **constant selections through reads**: `selected_item` resolves `[v0, v1][k]`
through a container that is itself a read, and a call's pair resolves to the callee's
template return pair. Measured verdicts, in chain order: `inf(1)` — not a region (a
plain tuple); `step1(0)` — a region; `step2(1)` — a region; `step3(0)` — **not**, and
the peel's selected node there is a wrapper *value cell* rather than the wrapper, so
the mask is one level further out than the peel currently looks.

## 11. The block recursion's conflict (round seventeen)

Measured on `a_recursive_binding_inside_a_block_recurses`, prints since removed:

- The parameter checks are **fresh per instantiation**: the same apply node sees distinct
  parameter pairs (`282`, `319`; `476`, `513`) with five unmarked items, so the
  parameter pair is not what is shared.
- The failing unify compares two **clone chains**: one holds a bare `USize(3)`
  (clones of a value node ending at a node of function 6), the other an `Index` whose
  value is `USize(2)` (clones ending at a node of function 4). So the conflict is
  between a *value clone* and a *read clone*, one function apart — the pair unify is
  comparing a slot value against a dereference, not two values of one call.

## 12. The region answer is a construction fact (round eighteen)

The user's reading: **the type slot's own computation has no value during the check
either**, and nothing complains about it — so an undecided value is not a reason the
attribute side may fail. The region question must therefore be answered the way the
type slot answers: by a fact the check states, not by reading a value.

That is now the case: `shallow_regions` records every wrapper the check builds, and
`lazy_region` asks the set through the node's origin chain (a clone answers for its
template), falling back to the value mask only for a wrapper this check did not build
(a frozen one). The wrapper's own value is not read.

## 13. The pair-form wrapper, measured (round nineteen)

Tried and reverted: the wrapper's first item as the **call's pair** (`state[e].term`)
rather than its extracted value. Measured on the one-depth chain:

- The read's element then is the apply node, whose value is the call's **result** — the
  tuple value, an array of two element slots — so the normal read's `Index(element, 0)`
  double-dereferences, and the attribute projections read `Index(tuple_value, 2..4)`:
  three `IndexOutOfBounds` diagnostics, index 2/3/4 on an array of length 2.
- The pair form therefore still needs the region read (value = the element) and the
  region test to fire for those reads; with the constructional set it did not reach the
  wrapper through the chain, so the form changed the failure without removing it.
The form is the structurally right one (the type slot composes the same way); it needs
the region test to resolve the wrapper through the pair chain first.

## 14. What made the chain work (round twenty)

The one-depth chain is green. The working form is the pair-form of §13 **plus** a
mutually recursive resolution:

- The wrapper's first item is the call's **pair** (the apply node), so a read of the
  wrapper dereferences the pair a call states.
- `array_of` and `selected_item` resolve each other: the array a node's value is is
  the node itself, the item a constant read selects, or the array a call's template
  return pair holds — and a call's template pair is what states the call's value before
  any evaluation decides it, which is the reading the type slot already uses.

## 15. The block recursion's clone chains (round twenty-one)

Dumped with temporary prints, then removed:

- The conflicting nodes are **clones carrying a previous call's value**: one side is a
  clone chain whose value is `USize(3)` (its origin chain crosses function tags 7 → 6 →
  4 → 2), the other a clone of a read whose value is `USize(2)` (tags 6 → 4 → …). So a
  later instantiation cloned an **earlier instantiation's node**, not the template.
- The likely seam is the class topology the instantiation re-establishes:
  `wire_apply_result` unifies the template apply with the instantiation's result, and
  `unify_clone_groups` then regroups the next instantiation's clones against a class
  that already holds the first call's values. A computation's clone must not carry a
  previous call's value; the `proven concrete` rule now excludes computations, which is
  necessary but was not sufficient for this case.

## 16. The block recursion's unify site, and one negative (round twenty-two)

- The 3-against-2 conflict is recorded inside **`apply_parameter_check`**, called from
  `instantiate_stamped` on the **type side** (`apply_type_with` → `instantiate`). The
  parameter and argument pairs are both full width there (measured 5/5), so the pair
  branch, not the whole-value branch, does the unify.
- **Negative:** skipping the *value* slot on the type side (unifying types only) breaks
  four highlevel refinement tests (`in_function_assert_*`, `a_body_index_on_a_literal_*`,
  `an_in_function_bounds_constraint_*`): the type side's value merge is what reports a
  refinement mismatch. That change is therefore wrong and was reverted.
- Both pairs are per-instantiation yet the same call site's *argument pair node* is a
  template node shared by every level; the type side's value merge is what joins the
  levels' parameter cells, so the fix has to keep the reporting merge while making it
  per-evaluation — not remove it.

## 17. The recursive call's clones are chained, not independent (round twenty-four)

Measured on `a_recursive_binding_inside_a_block_recurses`, prints since removed. The
user's question — every call clones, so why does one node carry both calls' values? —
is answered by the **origin chains** of the two compared nodes:

- the parameter side is a clone chain `498 → 440 → 252 → 30`; the two templates are
  `30` (a value slot, no value) and `42` (the `z - 1` Sub), and their **classes are
  different** (`class(30) = 30`, `class(42) = 42`, measured).
- But the clone made for the second call has origin `252` — **the first call's clone**
  — not the template `30`. So the second call's clone was made *from the first call's
  clone*, and it carries that call's value `3`; the argument clone carries `2`; the
  parameter check unifies them and reports `3` against `2`.

So the levels are not independent: the walk hands an earlier instantiation's clone to
the next instantiation. The next measurement is *who references `252` in the template
graph* — the walk's source, which decides whether the fix belongs in the clone rule's
reference-in-place decision or in what the block's record field holds.

## 18. The mint shares the enclosing instantiation's remap (round twenty-four, after 查)

Measured with temporary prints, then removed. The closure mint walk
(`value_apply`'s function arm) reuses the enclosing pass's `remap`, so:

- when the block's `g` value is cloned during a call of `g`, the *minted* closure's
  template parameter is **the same clone node** as the enclosing instantiation's
  parameter pair (`template_parameter 257 → cloned 442` for both);
- that node is then bound by the enclosing call (measured: a `DBG u440` unify from
  `apply_parameter_check`, `440` against a class holding `USize(3)`);
- the next mint clones the bound node and carries `3` into the fresh template, where
  the inner level's argument (`2`) meets it.

So the fix belongs at the mint: a minted closure's parameter must be **its own clone**
in its own template image, not the enclosing call's bound clone. The remap is shared so
captures rewrite to the fresh clones; the parameter is the one node that must not be
shared.

## 19. The mint now owns its template (round twenty-five)

Fixed: the closure mint shared the enclosing instantiation's `remap`, so the minted
template's parameter was the very node the enclosing call binds, and its scope's clones
joined that call's clone groups. The mint now

- removes the nested function's parameter from the shared memo before its walk,
- drops the memo entries its own walk inserted **for the nested scope** (a capture's
  entry stays, so the enclosing pass and the minted body name the same outer clone).

Measured: the stale-value conflict (`3` against `2`) is **gone**. A first, broader form
that dropped every inserted key also broke two capture tests
(`a_nested_function_captures_the_applied_outer_parameter`,
`a_closure_released_from_a_scope_is_still_callable`) — the scope filter is what keeps
them green. The block recursion now unrolls past the **apply-depth budget** instead:
the trace shows every recursive application passing the **same argument node** (`566`),
so `z` never decreases. That shared argument is the next seam.

## 20. The recursion's shared argument, and a measured trade (round twenty-six)

The trace after round 25 shows, for every recursive level, a fresh apply node and a
fresh operand array, but the operand's **items 1..** are the same nodes: item 0 (the
callee) is cloned per level, item 1 (the argument pair) is not. So `z` never decreases
and the check stops on the apply-depth budget.

Root cause measured: the argument pair is a *minted* template node whose value was
copied from the enclosing call (`z - 1` with `z = 3` gives `2`), and the mint's `carried`
rule copies it into every later template.

Two rules were added and kept (measured unchanged: language 274/275, highlevel 136/136,
lowlevel 164):

- **scoped-ness is judged on the template root**, because a clone's block can be
  parentless while its template lives in a function's scope;
- **a clone is proven only when its template is**, because a value this instantiation
  computed belongs to that call.

**Negative, measured:** suppressing the carry for a mint's own scope
(`closure_scope.contains(node)`) makes the block recursion **pass** (273/275 with it in
place the recursion is green) but loses the enclosing call's applied values for
captures — the nested function's scope list *holds* the captures (`a`, `b`, `x`), so
`a_nested_function_captures_the_applied_outer_parameter` and
`a_closure_released_from_a_scope_is_still_callable` fail. A capture is therefore not
distinguishable from the scope's own by the scope list, the function tag, or the block
chain (all three tried and measured). The next seam is that distinction.

## 21. Why the carry gate cannot be scoped (round twenty-seven)

The gate's failure was traced further:

- The block recursion's instantiation **succeeds** with the gate in place; it is the
  *result* that comes back empty — the applied body's array elements lose their decided
  copies, and the root's deep evaluation is undecided (the capture test's `unwrap`).
- The capture probe shows the nested function's scope list carries **both** node
  families: the captures as the enclosing function's nodes (`tag = f1`, `in_anchor =
  true`) **and** decided copies under the nested function's own tag (`tag = f2`,
  `in_anchor = false`). Suppressing the carry for the latter removes the values the
  applied body needs, so even the anchor-based exclusion cannot separate "stale
  recursion value" from "captured fact".

The next distinction must be a *value-level* one — the recursion's stale value is a
slot's answer produced by a previous *call*, while a capture is a fact the enclosing
call holds — not a scope/tag/block property.
