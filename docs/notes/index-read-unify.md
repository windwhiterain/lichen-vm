# The index read that is never taken

> **Status:** **route 1 of §4 has landed** (the marker's own semantics, by the
> user's decision, recorded before and after in §7), and **no production change
> remains open here.** The rest of the note is the measurement that chose it.
> [arithmetic-class-stated](arithmetic-class-stated.md) §5 re-homed family B onto
> one cell — `Checker::check_index`'s `lazy_index_path` `elem_cell`
> (`crates/lichen-highlevel/src/checker/indexing.rs:70-83`) — and called the write
> that should fill it "a skipped unify on the index-read path". This note
> instrumented that path and found neither half of that sentence: **the cell is
> never built by any example**, the cell that prints `?a` is a different one, and
> **no unify is skipped** — the chain that would carry a decided type dies one link
> earlier, on a container whose type is undecided *because the mark hides it*.
> §2 gives the measurement, §3 says which shape reaches the read and which does
> not, §4 says what would have to change, §7 is the route that changed, and §5 is
> the status line the companions now carry.
>
> Companions: [arithmetic-class-stated](arithmetic-class-stated.md) §5, the
> recommendation this measures and corrects, and §3, the `arithmetic_class`
> experiment it inherits its guard from; [undecided-type-cell](undecided-type-cell.md)
> §10.2/§10.2b, the family and the two accounts of which cell is undecided;
> [compound-element-slots](compound-element-slots.md) §"The read path", the
> invariant the lazy read is allowed to keep; [examples-panic-diagnosis](examples-panic-diagnosis.md),
> the survey this note's counts come from; [class-channel](class-channel.md), the
> class vocabulary; [eval-before-unify.md](eval-before-unify.md) §2.2, the
> positional read's own note.

Everything in §1–§6 was measured on `dev` at `226bed1e`, in the worktree
`.worktrees/idx` on `feature/idx`, through `cargo run -p lichen-compiler run` — the
language front end, which [test-harness](test-harness.md) §"Verify" calls the
faithful runner for a runtime value — and through
`cargo test -p lichen-compute --test lichen_compute`. The probes are named in §6
and were reverted with `git checkout` before this note was written. §7 was
measured and landed on `dev` at `26f07fa1` in the worktree `.worktrees/shal` on
`feature/shal`; its numbers are its own. The `dev` baseline was `26f07fa1`; the
branch was rebased onto `9ddbbed5` at the end and **every number in §7.3 was
re-measured on that rebased tip** — the survey, the four counts, and all thirteen
control programs, byte for byte as recorded above.

## 1. The question this note answers

§5 of [arithmetic-class-stated](arithmetic-class-stated.md) is explicit about what
it leaves open, and the re-homing is its own paragraph:

> **The missing write is the index read's carry from a container element cell that
> is already decided** (`15 of 15` in `lazy_infinite.lichen`), which is
> [compound-element-slots](compound-element-slots.md) §"The read path", in the same
> `lichen-lowlevel` neighbourhood as the `pair_type_half` fix that closed the other
> three, and **not** a checker decision.

Two claims, and both are load-bearing: (a) the cell that prints `?a` is
`check_index`'s lazy `elem_cell`; (b) the write that should fill it is skipped, and
the container element cell it should come from is already decided. The brief that
sent this slice also records what the previous slice did *not* do: it "did **not**
instrument `lichen-lowlevel`'s index-read path to find *which* unify is skipped".
That is what this note did.

## 2. The measurement

### 2.1 The cell §5 names is never built

`check_index`'s `elem_cell` has two arms. The `Some` arm takes the container's own
element node; the `None` arm calls `lazy_index_path` and builds the nested `Index`
chain §5 quotes. A probe on both arms, run over the **whole examples suite** (all
52 files, through `lichen_compute::examples`):

| arm | calls |
|---|---|
| `Some(element)` — the container's own node | **734** |
| `None => lazy_index_path(...)` — §5's cell | **0** |

Every `check_index` the suite makes is on the concrete arm. So §5's cell is not the
cell that prints `?a`; it is a cell no example builds. The one program this family
is about, `examples/lazy_infinite.lichen`, takes the concrete arm **sixteen times**
— all of them inside the *clone* blocks `BlockId(3v1)` … `BlockId(10v1)`, the eight
per-call copies of `f`'s body — and never once in the user's own block
`BlockId(1v1)`.

### 2.2 The cells that print `?a` are minted by the sibling read

`inf(1)` in `lazy_infinite.lichen` is an `ExprKind::Field`, so it goes to
`Checker::check_field` (`crates/lichen-highlevel/src/checker/structs.rs:46`) and
from there to `slot_read` (`structs.rs:105`) — **not** to `check_index`. Probed, one
line per read:

```
PROBE slot_read LAZY container_ty=NodeId(89v1)  key=NodeId(91v1)  pos=Some(0) chain=(98v1,100v1,102v1) ty_node=102v1
PROBE slot_read LAZY container_ty=NodeId(129v1) key=NodeId(131v1) pos=Some(0) chain=(138v1,140v1,142v1) ty_node=142v1
PROBE slot_read LAZY container_ty=NodeId(182v1) key=NodeId(184v1) pos=Some(0) chain=(191v1,193v1,195v1) ty_node=195v1
```

and the three cells the type printer renders `?a`, `?b`, `?c` are exactly
`102v1`, `142v1`, `195v1`, each its own representative:

```
PROBE undecided-type node=NodeId(102v1) rep=Some(NodeId(102v1))
PROBE undecided-type node=NodeId(142v1) rep=Some(NodeId(142v1))
PROBE undecided-type node=NodeId(195v1) rep=Some(NodeId(195v1))
```

`slot_read`'s lazy arm builds the same three-link chain `check_index` would have
built — `Index(Index(Index(container_ty, 0), key), 1)` — from the same shape
constants. **The two are siblings, and only the sibling is on this program's
path.** `slot_read`'s own `# Invariant` is the sentence §5's re-homing would have
to answer to, and it says the lazy form is the *intended* one: *"A known
`position` makes the read's type decided rather than lazy: it is the field's own
node out of the field list … An undecided container keeps the lazy form, resolved
at the apply."*

### 2.3 No unify is skipped; the chain stops one link early

The write §5 asks for is `Module::alias_read`
(`crates/lichen-lowlevel/src/equality.rs:821`), the index read's own join: *"Join
`reader` into `target`'s class and evaluate the target."* Probed on the whole suite
it fires **13 049** times. On `lazy_infinite.lichen`, in the user's block
`BlockId(1v1)`, **none of the nine chain links ever appears as a reader**:

| the `?a` cell's own chain, in `BlockId(1v1)` | `alias_read` reader? |
|---|---|
| `98v1`, `100v1`, `102v1` | **0 / 0 / 0** |
| `138v1`, `140v1`, `142v1` | **0 / 0 / 0** |
| `191v1`, `193v1`, `195v1` | **0 / 0 / 0** |

The only joins on that path are the **first** link of each chain, and each one
targets a cell with no value:

```
PROBE alias_read reader=NodeId(89v1)  blk=BlockId(1v1) target=NodeId(232v1) target_decided=false
PROBE alias_read reader=NodeId(116v1) blk=BlockId(1v1) target=NodeId(232v1) target_decided=false
PROBE alias_read reader=NodeId(156v1) blk=BlockId(1v1) target=NodeId(232v1) target_decided=false
```

`232v1`'s class, dumped member by member, is `{89v1, 232v1, 116v1, 156v1}` — four
members, **all four undecided**, no member carrying an operator's answer. So:

- `89v1` is the type of `inf(1)`, and it is read as
  `Index(Index(Index(71v1, 0), 1), 1)`: `inf`'s type shape, slot 1, that slot's
  **type position**;
- `232v1` is that type position — the type of `@shallow f (x + 1)`, the recursive
  tail of an infinite structure;
- the next link, `98v1 = Index(89v1, 0)` — the **shape** of `inf(1)`'s type — has
  nothing to read, because `89v1` holds no array. It returns undecided, and
  `100v1` and `102v1` are never reached at all.

**So the write is not skipped: it is never reached.** The join the read needs is
one level *past* the undecided cell, and nothing can place it.

§7 closes the loop on the sentence §2.3 ends with: the container's type cell is
undecided **because the mark keeps it that way**, not because it is
unmaterialisable. Nothing about it is infinite in fact — one level of it is a
plain array, and the level after that is the same cell again.

### 2.4 The "already decided" container element cell is not on this path

§5's premise is that a container element cell already holds a decided type (its
`15 of 15`). The probe agrees that such a class exists — `22v1` has fifteen
decided members — and also measures that it is **not an input to the read**. The
`15 of 15` class is element **0**'s type (the parameter `x`, decided to `Int` in
each of the fifteen per-call clones). The read `inf(1)(0)` is forced through
element **1**'s type, because `inf(1)` *is* the container of that read. Reaching
element 0 therefore requires materialising the whole type of the infinite tail,
which is exactly what the `@shallow` mark forbids.

Measured on the shapes, which is what makes the two cases different rather than
accidental:

| program | container of the read | its type cell | printed |
|---|---|---|---|
| `x = 0` ; `xs = [x, @shallow [x + 1, x + 2]]` ; `xs(1)` | a literal | **decided** | `[1, 2]: array{Int, 2}` |
| `f = x => [x, @shallow f (x + 1)]` ; `inf = f 0` ; `inf(1)(0)` | element 1 of a literal | **undecided, by construction** | `raw 1: ?a` |

## 3. Why it is reached for one shape and not another

`slot_read` has two arms, and the discriminator is stated in its own `# Invariant`:
a **decided** container yields *"the field's own node out of the field list"*,
which a class question can see through; an **undecided** one gets the nested
`Index` chain, which resolves *"at the apply"*. Both arms ran in `lazy_infinite`,
and the probe shows which was taken every time:

- `pos` was **`Some(0)`**, `Some(1)` — the position was known on every one of the
  nine reads. The `resolved` arm still declined, because
  `shape::field_type(module, TypeRef::Term(container_ty), at)` answers `None` for a
  container whose type is an **unevaluated `Index` chain**: there are no array
  items to read a slot out of. Known position, undecided container, lazy arm.
- Inside the clone blocks `BlockId(3v1)`…`BlockId(10v1)` the container's type *is* a
  concrete pair, so `check_index` takes its `Some` arm sixteen times. **The same
  program resolves the read in the clones and declines it in the user's own block.**

That is the whole asymmetry: the read resolves where the container's type was
built, and declines where the container's type is still a chain over an undecided
cell.

## 4. What would have to change — and why it is not this slice's

For `lazy_infinite.lichen` to print `(1, 2, 3): tuple{Int, Int, Int}`, one of these
has to happen, and **none of them is a skipped unify**:

1. **`@shallow` stops marking the type side.** The mark is
   [`check_shallow_array_term`](../../crates/lichen-highlevel/src/checker/indexing.rs)'s
   `# Invariant`: *"a bare `@shallow` position's whole subtree stays lazy in the deep
   pass"*. The type of `@shallow f (x + 1)` is infinite; materialising it to make a
   sibling slot readable is a semantics change to the mark, not a bind.
   **Landed — §7.** The user took this decision; it was measured, not assumed.
2. **The clone carries its tuple type into the template's cell.** `232v1`'s class
   would have to join the *clone's* slot-1 type cell. The clone's own cell is
   equally undecided — it is the clone's recursive tail — so this carries a value
   that does not exist, and it would cost the laziness the mark exists for.
   **Refuted by the same measurement (§7): the clone's cell is closed, not carried.**
3. **The read is answered from the value side rather than the type side.** The
   value read already resolves (`raw 1` is `1`), so a read that asked the value's
   own element cell would answer `Int`. That is a different rule from
   [compound-element-slots](compound-element-slots.md) §"The read path" — *"A field
   read's type has exactly one source: the element **slot's type position**"* — and
   changing it changes every compound read. **Not taken: the read path is unchanged.**

All three were user decisions, and the first two are the ones §5's re-homing would
have reached by accident: "decide `arithmetic_class`'s branch earlier" is the
`Float`-losing, twelve-example route already measured and rejected in
[arithmetic-class-stated](arithmetic-class-stated.md) §3.

## 5. Status, for the companions to copy

> **Family B is not `arithmetic_class`'s open class, and it is not the index read's
> carry.** Measured on `dev` at `226bed1e`: `check_index`'s `lazy_index_path`
> `elem_cell` is built **zero** times across the examples suite, and the `?a` cells
> of `lazy_infinite.lichen` are minted by `checker/structs.rs`'s `slot_read` lazy
> arm. `Module::alias_read` — the index read's own write — never fires for any of
> those chains, because the container's type cell (`232v1`, class
> `{89v1, 232v1, 116v1, 156v1}`) is undecided: it is the type of a `@shallow`
> infinite tail, and the mark is what kept it undecided. **No unify is skipped and
> there is no decided cell to carry from**; what was missing is a rule for reading
> a decided sibling past a container type the mark hides. See
> [index-read-unify](index-read-unify.md).
>
> **And the mark is what kept it undecided — that is now the landed fix.** Route 1 of
> §4, decided by the user and measured in §7: `@shallow` marks the element's
> **value** and says nothing about its type, so a marked call's type is applied at
> compile time as any other call's is, and a tail whose own type is still open
> closes on the type that call states. `examples/lazy_infinite.lichen` now prints
> its declared `(1, 2, 3): tuple{Int, Int, Int}`, and the survey is
> `clean=43 panicked=[] drifted=[]`. The read path itself is untouched. See
> [index-read-unify](index-read-unify.md) §7.

## 6. Measured, inferred, unknown

**Measured** — `check_index`'s two arms over the whole examples suite (734
concrete, **0** lazy) and over `lazy_infinite.lichen` alone (16 calls, all in
`BlockId(3v1)`…`BlockId(10v1)`, none in the user's `BlockId(1v1)`); the three
`slot_read` lazy chains and the identity of `102v1`/`142v1`/`195v1` with the cells
the type printer renders `?a`/`?b`/`?c`; `alias_read`'s 13 049 suite-wide calls
and the **zero** with any chain link as reader in `BlockId(1v1)`; the three
first-link joins with `target_decided=false`; `232v1`'s class membership, four
undecided members; `pos = Some(0)`/`Some(1)` on all nine reads; the `decided` and
`undecided` control rows in §2.4; `clean=42 panicked=[] drifted=["lazy_infinite.lichen"]`
on `dev`; `203 passed / 15 failed / 2 ignored` for `lichen-compute` and
`255 passed / 6 failed` for `lichen-language` on `dev` at `226bed1e`.

**Inferred** — that the container's type cell cannot be decided without collapsing
`@shallow`'s laziness, from the mark's own `# Invariant` and from `232v1`'s class
being four undecided members with no computational member; that
`arithmetic-class-stated` §5's re-homing would have reached option 2 of §4, from
the two cells being the same node read one link apart.

**Unknown** — whether a rule that reads a decided sibling out of an unmaterialised
container type exists at all. Nothing here measures one; §4 lists what would have
to change, not what to write. **§7 answers it for the one shape this note is
about**, and does so by removing the question rather than adding a rule: the mark
never made the type unmaterialisable, it only made it unvisited.

**Not this note's subject** — whether `examples/lazy_infinite.lichen`'s declared
`output = "(1, 2, 3): tuple{Int, Int, Int}"` should be reached by a rule or
re-declared. [undecided-type-cell](undecided-type-cell.md) §5 rules the re-declare
out on the merits, and this note does not reopen it; it says the route §5 chose is
not the one that reaches it.

**No production change** — for §1–§6, which is what that sentence covers. The
probes were six `eprintln!` call sites — one pair in
`crates/lichen-highlevel/src/checker/indexing.rs` (`check_index`'s two `elem_cell`
arms), one in `crates/lichen-highlevel/src/checker/structs.rs` (`slot_read`'s lazy
arm), one in `crates/lichen-highlevel/src/checker.rs` (`check_term`'s tail),
one in `crates/lichen-render/src/render/type_printer.rs` (the undecided-type read),
and one in `crates/lichen-lowlevel/src/equality.rs` (`alias_read`) — reverted with
`git checkout` before this note was committed. `crates/lichen-lowlevel/src/evaluation.rs`
and `crates/lichen-lowlevel/src/gc.rs` were **not** touched, no `Invariant` or
`Safety` section was added or changed, no `output =` line was edited, and no test
was added. **§7 is the exception, and it is named there.**

## 7. Route 1, measured and landed: the mark is on the value

§4 route 1 is a **user decision**, and this section is the record of it: the
documented semantics of `@shallow` changed, and the edit below is that decision,
not an agent tidying a contract to match its code. The `# Invariant` it changed is
[`check_shallow_array_term`](../../crates/lichen-highlevel/src/checker/indexing.rs)'s
own, and `docs/language-spec.md` §"Shallow markers" is the prose form of the same
sentence. Every other `# Invariant` and `# Safety` section is untouched, and the
history of §§1–§6 above is intact.

### 7.1 What marks the type side today, and what the change is

There are **two** mask sites in the whole workspace, both in
`crates/lichen-highlevel/src/checker/indexing.rs`, and both are read by one place,
`Module::evaluate_node_deep_inner` in `crates/lichen-lowlevel/src/evaluation.rs`:

| site | what it marks | who reads it | what it does to a type cell today |
|---|---|---|---|
| `check_shallow_array_term`, the bare arm: `mask.push(depths[i] == usize::MAX)` | the **array position** of a bare `@shallow` | `evaluation.rs`'s `if item.shallow { continue; }` and `value_is_undecided`'s *"An array holding a shallow position can never be proven concrete"* | the position's `Index(apply, 0)` is never walked, so the apply never runs, so **neither** of the apply's two applications runs |
| `wrap_shallow`, `array_node_masked(…, &[true, false])` | the **value slot** of each level of a `@shallow(n)` wrap | the same two reads | none: `@shallow(n)` already replaces the element's type with `self.fresh_cell()` in `check_shallow_array_term`, and its wrapped pair is `[value, type]` |

So the type side is marked **indirectly and only for the bare form**: the mark
rides on the value position, and an apply's *type* application
(`evaluation.rs`'s `apply_callee_type`) is reached only when the apply's *value*
runs — `crates/lichen-lowlevel/src/evaluation.rs:283` puts both under the `Function`
arm. A bare `@shallow` therefore hides the type as a side effect of hiding the
value, which is exactly what "a lazy *value* marker should keep the value lazy, not
hide the type" names.

**The change**, in three places, all in `lichen-highlevel`:

1. `Checker::check_app` builds a **type-only** application — the same four operand
   elements over an *undecided* callee value, which is the one shape the apply arm
   turns into `apply_callee_type` without a body — and records it. It replaces the
   `type_apply` node that was built and immediately dropped (`let _ = type_apply;`,
   a dead node since `e0e5ae09`); the replacement is the same intent, reachable.
2. `check_shallow_array_term`'s bare arm registers `(that application, the
   element's result type cell)` for every marked element that is a call.
3. After the written-type pass and before the definition pass — once every type
   expression is decided and every function's `func_node` holds its `Function` —
   each registration runs, then `Checker::close_shallow_tail` closes the tail.

### 7.2 Why a second step is needed, and what it is

Running the type application alone decides **one** level. Measured on
`lazy_infinite.lichen`: `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}` becomes
`(1, raw 2, raw 3): tuple{Int, ?a, ?b}`. The reason is structural, and it is worth
stating because it is what no `lichen-highlevel` change can walk around:

- `Module::apply_type` (`crates/lichen-lowlevel/src/function.rs:139`) clones the
  template's return pair and unifies the result cell with **the clone's** codomain.
  The clone's tail is the *clone's own* result cell, in a class of its own.
- `regroup_clones` / `unify_clone_groups`
  (`crates/lichen-lowlevel/src/apply.rs:182`, `:201`) re-establish the template's
  class topology **among the clones only** — the group is the clones of one
  template representative, and the template node is never unified into it.

So the decided head is `[shape, kind]` with a **fresh, unbound** cell at its tail's
type position, and the second read follows exactly that cell. `Checker::close_shallow_tail`
closes it: when the tail's own type position is **undecided** after the type
application ran, the tail takes the type the head states. One level suffices —
the closed cell *is* the head's tail, so `inf(1)(1)`, `inf(1)(1)(1)` and every
further read follow the same closed cell rather than a fresh one.

**The guard is what keeps this from being an assertion.** Only an *undecided* tail
position is closed, and only after the type application decided the head: a tail
that already names a type states it, and an undecided head has nothing to close.
So `f = x => [x, @shallow f 1]` — where the tail's type is `Int` and is decided —
is left alone.

### 7.3 The blast radius, measured separately

Each change alone first, through `cargo run -p lichen-compiler`, then the whole
survey and the suite counts.

| program | `dev` | type application alone | + `close_shallow_tail` |
|---|---|---|---|
| `examples/lazy_infinite.lichen` | `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}` | `(1, raw 2, raw 3): tuple{Int, ?a, ?b}` | **`(1, 2, 3): tuple{Int, Int, Int}`** |
| `f = x => [x, @shallow f (x + 1.5)]` ; `inf = f 0.0` ; `(inf(1)(0), inf(1)(1)(0))` | `(raw 1.5, raw 3.0): tuple{?a, ?b}` | `(1.5, raw 3.0): tuple{Float, ?a}` | **`(1.5, 3.0): tuple{Float, Float}`** — `Float` through the same read, decided as `Float` |
| `f = x => [x, @shallow f (x + 1.0)]` ; `inf = f 0.0` ; `inf` | `(0.0, raw none): tuple{Float, ?a}` | `(0.0, raw none): tuple{Float, tuple{Float, ?a}}` | `(0.0, raw none): tuple{Float, tuple{Float, tuple{Float, …}}}` — the coinductive type, rendered with its own `…` (§7.6) |
| `@loop f = x => [x, @shallow f (x + 1)]` ; `inf = f 0` ; `inf(1)(0)` | `raw 1: ?a` | `1: Int` | `1: Int` |
| `f = x => [x, @shallow [x + 1, x + 2]]` ; `g = f 0` ; `g(1)` | `[raw 1, raw 2]: array{?a, 2}` | unchanged | **unchanged** — see §7.4 |
| the same with `(x : Int)` | `[raw 1, raw 2]: array{?a, 2}` | unchanged | unchanged |
| `f = x => [x, @shallow [x, x]]` ; `g = f 0` ; `g(1)` | `[0, 0]: array{Int, 2}` | unchanged | unchanged |
| `f = x => [x, @shallow [x + 1, x + 2]]` ; `g = f 0` ; `g` | `(0, [raw none, raw none]): tuple{Int, array{?a, 2}}` | unchanged | unchanged |
| `x = 0` ; `xs = [x, @shallow [x + 1, x + 2]]` ; `xs(1)` | `[1, 2]: array{Int, 2}` | unchanged | unchanged |
| `xs = [1, @shallow [2, 3]]` ; `xs(0)` / `xs(1)` | `1: Int` / `[2, 3]: array{Int, 2}` | unchanged | unchanged |
| `f = x => [x, [x + 1, x + 2]]` ; `g = f 0` ; `g(1)` (no marker) | `error: expected Int, found array{?a, 2}` | unchanged | unchanged |
| `f = x => [x, @shallow(1) f (x + 1)]` ; `inf = f 0` ; the three reads | `(raw[raw 1, raw[raw none, raw none]], raw none, raw none): tuple{?a, ?b, ?c}` | unchanged | **unchanged** — `@shallow(n)` is a different mark, §7.5 |
| `([1, @shallow(2) [2, 3]])(1)(0)` | `raw[raw 2, raw 3]: ?a` | unchanged | unchanged |

The **survey**, `cargo test -p lichen-compute --test lichen_compute
zz_examples_panic_probe -- --nocapture --test-threads 2`:

| | `dev` | with §7 |
|---|---|---|
| clean | 42 | **43** |
| panicked | — | — |
| drifted | `lazy_infinite.lichen` | **—** |

And the counts, on the merged tip of this branch:

| | `dev` at `26f07fa1` | with §7 |
|---|---|---|
| `lichen-compute --test lichen_compute` | 203 passed / 15 failed / 2 ignored | **204 passed / 14 failed / 2 ignored** |
| `lichen-language --test language` | 255 passed / 6 failed | **255 passed / 6 failed** — the same six |
| `lichen-language` lib | 72 passed / 0 failed | **72 passed / 0 failed** |
| `lichen-highlevel` | 140 passed / 0 failed | **140 passed / 0 failed** |

`pipeline::an_at_shallow_depth_one_on_a_recursive_tail_terminates` is one of those
six and **does not move**: it is still failing, for the same reason — it spells
`@shallow(1)`, and §7.5 is about the bare mark. The `examples` test itself was
already failing on `dev` (it is among the fifteen) and is fixed by this change.

**Nothing refused a working program, nothing panicked, and nothing outside the
intended case moved.** `lazy_infinite.lichen`'s `output =` line was **not**
edited; the language came to match it.

### 7.4 The control that correctly stays undecided

`f = x => [x, @shallow [x + 1, x + 2]]` ; `g = f 0` ; `g(1)` still prints
`[raw 1, raw 2]: array{?a, 2}`, and that is the **right** answer under the new
semantics, not an omission. The marked element there is an array **literal**: there
is no apply, so there is no type application to run and no tail cell to close, and
the `?a` is `Checker::arithmetic_class`'s open class — family **A**, a different
question with a different owner. What `@shallow` states is that the value is lazy.
It says nothing about whether `x + 1`'s class is determined, and that depends on
`x`'s type, which `f`'s caller supplies. Deciding it is `arithmetic-class-stated`
§3's D1, measured there as `Float`-losing and twelve examples wide, and this
change does not touch it. The survey agrees from the other side: the two
`import` examples that **declare** `raw[?a, ?b]` did not move.

### 7.5 What is deliberately still lazy

`@shallow(n)` keeps both halves of its own semantics: the element's type is a
**fresh cell** (`check_shallow_array_term`'s `n` arm) and its value is wrapped for
`n` levels. It is a different mark with a different contract — the depth form says
*how deep* the laziness reaches — and `an_at_shallow_depth_marks_value_slots_shallow`
asserts that its read's type stays underdetermined. §7 changes only the bare form.

**Settled by measurement, later: the same closure should apply to `@shallow(n)`, and the
question was narrower than it looked.** [shallow-depth-closure](shallow-depth-closure.md)
§5 ran the minimal extension — the bare arm's registration, added to the `n` arm alone — and
measured it. The paragraph above holds on every point it makes: the element's type is still a
**fresh cell**, nothing escapes it, and `an_at_shallow_depth_marks_value_slots_shallow` still
passes. What does not hold is "§7 changes only the bare form" **as an open question**: over a
depth-marked **call** the closure has something to close, and
`an_at_shallow_depth_one_on_a_recursive_tail_terminates` — red here, and red because
`array_values` wants a value per element that the depth mark withholds — **turns green**. The
examples survey is unmoved (`clean=43 panicked=[] drifted=[]` on both sides), the bare mark
is unmoved, and no example program spells the depth form at all, which is why §7 could land
without meeting the counterexample. **Not landed here**; the change, the `# Invariant`
sentence it needs, and the spec's unmarked-both-spellings sentence are listed in that note's
§6.1 as the user's decision. The mechanism by which the closure supplies the missing value is
**not** established in that note, and §6 says so.

### 7.6 The rendering's spec line

The `…` in that table row now has one: `docs/language-spec.md` §5
["Checker spellings"](../../docs/language-spec.md) states the cut, and why a cut
type is a **finite notation for an infinite type** rather than a truncation of
one, and which of the **three** readings that share the `…` and `?` vocabulary a
given `…` is.  Two of the three remain; the third — an undecided field list —
is **displaced by the two-sided probe** recorded below, which measured this
section's own alternating-pair row as a cut and left the arm reachable from IR
only.  [raw-rendering-mark](raw-rendering-mark.md) §1 already owned the
same cut for the raw fallback and the universe's self-loop, so the spec line
cites it rather than restating it.

**Which programs reach it, measured on `dev@4da09089`.** The same probe run at
`9ddbbed5` (the commit before §7) and at `b8444708` (§7 itself).  A `—` is a program not run at that commit:

| program | `9ddbbed5` | `b8444708` and after |
|---|---|---|
| `f = x => [x, @shallow f (x + 1.0)]` ; `inf = f 0.0` ; `inf` | `tuple{Float, ?a}` | `tuple{Float, tuple{Float, tuple{Float, …}}}` |
| `xs = [@shallow xs]` ; `xs` | — | `tuple{tuple{…}}` — the same cut on a one-element list spine |
| `g = f => [f, @shallow g f]` ; `inf = g g` ; `inf` | `tuple{?a -> tuple{?a, ?b}, ?c}` | `?a -> tuple{?a, tuple{?a, tuple{?a, …}}}` — the cut inside an **arrow** |
| `f = x => [x, @shallow g x]` ; `g = y => [y, @shallow f y]` ; `inf = f 0.0` ; `inf` | `tuple{Float, ?a}` | `tuple{Float, tuple{Float, tuple{Float, tuple{?a, …}}}}` — the alternating pair ends on the **cut**, beside a decided field list (see "The `…` in `tuple{?a, …}`" below) |
| `A = struct{.f Int, .g A}` ; `A` | `struct{.f Int, .g struct{.f Int, .g …}}` | **unchanged** — pre-existing, and pinned by `examples/struct_recursion.lichen`'s declared `output` |
| `A = struct{.g B}; B = struct{.g C}; C = struct{.g A}` ; `A` | — | `struct{.g struct{.g struct{.g struct{.g …}}}}` — a 3-cycle, cut after four levels |
| the same over `A…E` (a 5-cycle) | — | `struct{.g struct{.g struct{.g struct{.g struct{.g struct{.g …}}}}}}` |
| `f = x => [@shallow f x]` ; `inf = f 0.0` ; `inf` | `tuple{?a}` | `tuple{tuple{?a}}` — **finite**, no cut |
| `f = x => [[x, @shallow f x]]` ; `inf = f 0.0` ; `inf` | `array{tuple{Float, ?a}, 1}` | `array{tuple{Float, array{tuple{Float, ?a}, 1}}, 1}` — **finite**, no cut |
| `is_even`/`is_old` (`examples/mutual_recursion.lichen`) | `(0, 1): tuple{Int, Int}` | unchanged — a mutual *value* cycle whose reads decide the type |
| `f = x => x` ; `f` | `?a => ?a: ?a -> ?a` | unchanged — a function's own type prints `?a -> ?a`, with no cut |
| `(Type, Type : Type, (1, Type))` | no `…` | unchanged — the universe's self-loop is cut by the head collapse, so `Type` prints `Type` |

So **the cut itself is pre-existing** and the §7 change is not new rendering
machinery: the cyclic spelling was already reachable at `9ddbbed5`, through a
recursive struct type, and `examples/struct_recursion.lichen` has declared it in
its `output =` line since `772ad9ff`. What §7 changed is that a **coinductive
value's** spine now reaches it — at `9ddbbed5` that program's type was
`tuple{Float, ?a}`, with no cyclic structure to show at all.

**Every cyclic shape above terminates**, at the first revisited node.  Each row
is one `lichen-compiler` run of one file; the sweep is fourteen files (the table
holds twelve), all exited 0, and at `dev` they took 57–93 ms each.  The first
run after each build was 2.2–2.4 s, which is the binary's pages being faulted
in, not the printer.  The printer's own limit is a nesting depth, not a cycle
([code-audit](code-audit.md) `P4-10`, closed `wontfix:D9`).

#### The `…` in `tuple{?a, …}`: the alternating pair's spelling, re-read

**What this section used to say, and why it was wrong.** The row above used to
read "the alternating pair ends on the **undecided-field** `…`, not the cut" —
it took the `…` in `tuple{?a, …}` to be a field list the check had not fixed.
That is the **cut**: measured on `feature/trl`, the shape slot at the level
this program prints as `tuple{?a, …}` (depth 3 of 3, after three
`tuple{Float, …}`) **holds an array of two fields**, so the printer's
undecided-field arm does not apply to it at all.  The two fields read as an
undecided cell (`?a`) and a node the walk has already met (`…`).

**The two readings print the same string, which is why the row was written the
way it was.** A tuple type whose shape slot holds **no array at all** takes the
printer's second arm and prints `tuple{?a, …}` as well.  Measured on the same
build, by hand: a module holding `[shape, [TypeTuple, K]]` with a value-less
`shape` node prints `tuple{?a, …}`; the same term with a two-element array for
`shape` prints `tuple{?a, ?a}`.  The spelling is therefore ambiguous between
"the field list is undecided" and "a cell beside a cut", and this note resolved
it wrongly.

**The arm is orphaned, not dead.** Its only construction site was the pin
`f4c2b93a` ("checker: a positional read requires the tuple *kind*, never binds
the type") replaced; every front-end site passes an `array_node` now.  Measured:
zero hits across four whole test binaries and 55 candidate programs, so **no
language program can make it true** — the arm is reachable from IR alone, and a
hand-built module does render it.  `docs/language-spec.md` §5 "Checker
spellings" now says so, and
`crates/lichen-language/tests/language/tuple_open_field_list.rs` pins the
rendering.  Nothing under `crates/lichen-render/` changed: the arm stays.

### 7.7 The files

`crates/lichen-highlevel/src/checker.rs` (the registration and the walk entry
point), `crates/lichen-highlevel/src/checker/indexing.rs`
(`check_shallow_array_term`'s registration, `close_shallow_tail`, and that one
`# Invariant`), `crates/lichen-highlevel/src/checker/lambda.rs` (`check_app`'s
type-only application), `docs/language-spec.md` §"Shallow markers", and this note
with its companions. `crates/lichen-lowlevel/src/evaluation.rs` and
`crates/lichen-lowlevel/src/gc.rs` were **not** touched, no new test was added,
and no `output =` line was edited.

**The rendering this section could not finish, finished afterwards.** The `…`
that row needed a spec line for now has one: §7.6 and `docs/language-spec.md`
§5 "Checker spellings".  That follow-on was **docs only** — no code, no test, no
`output =` line, and **no `# Invariant` or `# Safety` sentence added or
changed**; the printer's own (`print_type`'s, in
`crates/lichen-render/src/render.rs`) already stated the cut and was left
alone.

**Is `sync-readme` safe now?** The survey is `clean=43 panicked=[] drifted=[]`,
so re-deriving every `output =` from a live run finds nothing to rewrite — and it
is **not safe to run as part of this change**, because it would commit a rewrite
across the whole tree on top of a semantics change; the declarations are already
what the language prints. It is a separate, now-safe run.
