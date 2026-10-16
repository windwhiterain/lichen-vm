# The index read that is never taken

> **Status:** **measurement only; no production change, and no fix proposed.**
> [arithmetic-class-stated](arithmetic-class-stated.md) §5 re-homed family B onto
> one cell — `Checker::check_index`'s `lazy_index_path` `elem_cell`
> (`crates/lichen-highlevel/src/checker/indexing.rs:70-83`) — and called the write
> that should fill it "a skipped unify on the index-read path". This note
> instrumented that path and found neither half of that sentence: **the cell is
> never built by any example**, the cell that prints `?a` is a different one, and
> **no unify is skipped** — the chain that would carry a decided type dies one link
> earlier, on a container whose type is undecided *by construction*. §2 gives the
> measurement, §3 says which shape reaches the read and which does not, §4 says
> what would have to change, and §5 is the status line to copy into the companions.
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

Everything below was measured on `dev` at `226bed1e`, in the worktree
`.worktrees/idx` on `feature/idx`, through `cargo run -p lichen-compiler run` — the
language front end, which [test-harness](test-harness.md) §"Verify" calls the
faithful runner for a runtime value — and through
`cargo test -p lichen-compute --test lichen_compute`. The probes are named in §6
and were reverted with `git checkout` before this note was written.

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
2. **The clone carries its tuple type into the template's cell.** `232v1`'s class
   would have to join the *clone's* slot-1 type cell. The clone's own cell is
   equally undecided — it is the clone's recursive tail — so this carries a value
   that does not exist, and it would cost the laziness the mark exists for.
3. **The read is answered from the value side rather than the type side.** The
   value read already resolves (`raw 1` is `1`), so a read that asked the value's
   own element cell would answer `Int`. That is a different rule from
   [compound-element-slots](compound-element-slots.md) §"The read path" — *"A field
   read's type has exactly one source: the element **slot's type position**"* — and
   changing it changes every compound read.

All three are user decisions, and the first two are the ones §5's re-homing would
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
> `{89v1, 232v1, 116v1, 156v1}`) is undecided by construction: it is the type of an
> `@shallow` infinite tail. **No unify is skipped and there is no decided cell to
> carry from**; what is missing is a rule for reading a decided sibling out of a
> container whose type is not materialisable. See
> [index-read-unify](index-read-unify.md).

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
to change, not what to write.

**Not this note's subject** — whether `examples/lazy_infinite.lichen`'s declared
`output = "(1, 2, 3): tuple{Int, Int, Int}"` should be reached by a rule or
re-declared. [undecided-type-cell](undecided-type-cell.md) §5 rules the re-declare
out on the merits, and this note does not reopen it; it says the route §5 chose is
not the one that reaches it.

**No production change.** The probes were six `eprintln!` call sites — one pair in
`crates/lichen-highlevel/src/checker/indexing.rs` (`check_index`'s two `elem_cell`
arms), one in `crates/lichen-highlevel/src/checker/structs.rs` (`slot_read`'s lazy
arm), one in `crates/lichen-highlevel/src/checker.rs` (`check_term`'s tail),
one in `crates/lichen-render/src/render/type_printer.rs` (the undecided-type read),
and one in `crates/lichen-lowlevel/src/equality.rs` (`alias_read`) — reverted with
`git checkout` before this note was committed. `crates/lichen-lowlevel/src/evaluation.rs`
and `crates/lichen-lowlevel/src/gc.rs` were **not** touched, no `Invariant` or
`Safety` section was added or changed, no `output =` line was edited, and no test
was added.
