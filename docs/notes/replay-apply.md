# A template's clone replays the apply

> Status: **landed at the language layer.** The maintainer's verdict: a clone and
> its template cannot be unified; a clone's undecided cells are linked to a
> **replay** of the apply instead. The replay and the native-call half are in;
> lowlevel 164/1 ignored and language 275 are green with them. `lichen-compute`
> is unchanged by the replay: its dominant refusal was measured to be a
> *compile-time* provenance gap (a read chain ending at `Index(Apply, 0)`), which
> is a separate piece of work, and one half of it — resolving the read's input
> position by equality class — is landed.

## The problem

`f = (i <: I => _)` with `f` still undecided. A direct `Unify(f, template)` is
the undecided-vs-decided write arm: it writes the **template** into `f`'s value
class. A later `$parallel(f)`/`$jit(f)` then compiles the template record, not
the caller's function. Measurements, `lichen-compute` integration:

| shape | passed | failed |
|---|---|---|
| deferred `Unify`, `dev` | 67 | 173 |
| + static re-home capture parity | 75 | 165 |
| eager unify + materialize gate | 66 | 174 |

Eager unify makes the package load, but the first remaining diagnostic is
`compute.parallel: no output ... carries a decided scalar class`: the
codomain/output class is still the template's `_`. Unifying a clone with its
template would specialize the template, so the two must not be unified.

### Measured: the runtime write is not the trigger

The dominant compute failure is **not** the deferred `Unify` write. Disabling the
lowlevel `Unify` operator's `unify_ref` outright changes nothing for
`buf_element_class_direct`: the same
`compute.parallel: read's buffer argument is not an input buffer of the parallel
parameter (the parameter's inputs are [[1, 0]])` is reported. Deleting the
checker's deferred branch (so the unify happens eagerly at check time) does change
it: the read resolves and the refusal becomes
`no output of this parallel index function carries a decided scalar class ... An
undecided class is not an Int`. The graph's shape is therefore decided at **check**
time, by how the parameter binding unifies, not by the `Unify` operator's run.

### Measured: the read's chain ends at a constructor application

`param_path` walks `Index` nodes from the read up to the parameter value and
requires the chain to reach the parameter's class. With constructor templates the
chain now ends at the value half of an application — `Index(Apply, 0)` — or at
`Index(Unify, 0)`; `param_path` answers `None` at the first non-`Index` operation,
so the input position is unreadable. `Index(Apply, 0)` is the field's own
constructor expression (`Read`'s `(Buf p)`), whose class the caller's
`lane.in.d` joins by unification.

- Continuing the walk through a `Unify` (its value is its actual side) was measured
  **worse**: 75 passed, and the read refusal rose 78 → 86. Not kept.
- Resolving the position by **equality class** — walking the parameter value by the
  role path and comparing the field's class root with the read's buffer — landed and
  took the suite 75 → 77. It covers the plain case; the remaining chains still end
  at `Index(Apply, 0)`.

Failure histogram at 77 passed (163 failed):

| diagnostic | count |
|---|---|
| `compute.parallel: read's buffer argument is not an input buffer` | 78 |
| `compute.graph: this function's return is not a value a graph can hand back` | 56 |
| `compute.parallel: a kernel body reached a node with neither a value nor an operation` | 11 |
| the rest (`jit` index placement, frozen refs, definition cycle, cross-kernel arg) | 6 |

### Measured: a class's value is chosen by union-by-size

`add_equality` keeps the **larger** class's value when both sides already hold one
(`disjoint::union` is by size), while `fill_class_holes` states that a merge only
happens where "the two sides are one value or one side is undecided". Compound
unification violates that premise: one test showed **707** two-value merges. Which
side a reader sees is therefore a union-find tie-break, not a stated rule — the
likely path by which a read sees the template's value instead of the caller's.

## The replay

When a function is cloned and the clone's undecided cells include cells whose
owner scope belongs to the cloned function, build one **shared replay node** in
the apply's caller block:

- operator `Apply`, operands `[template function value, the apply's argument]`;
- unify each undecided clone cell with the **corresponding cell of the replay's
  result**, matched through the replay's instantiation remap at arbitrary depth
  (the mechanism is added here; a result-slot-only match is not enough);
- set the replay's **`runned`** to true, so this scope does not expand it;
- when the apply's scope is cloned, `node_apply`'s `stamp_node_ran_if` resets the
  replay to not-run because its carried answer is absent, the actual argument
  unifies `f`, and the replay evaluates to the real applied value.

`runned`, not `evaluated_deep`: the clone copies no deep verdict, and only
`runned` distinguishes "this scope ran it" from "the next clone must run it".
The clone target and the apply's caller block are the same block
(`instantiate`'s `block`).

### History: the first attempt, before the conditions were known

Attempt `13925374` built exactly the node above: for every instantiation whose
cloned open cells are owned by the applied function, an `Apply(callee, argument)`
node in the caller's block, `runned = true`, and each open clone cell unified with
the cell of a **second instantiation** standing for the same template node.
Measurements:

| shape | passed | failed |
|---|---|---|
| without the replay | 77 | 163 |
| replay node + correspondence unify | 50 | 190 |
| replay node, correspondence unify dropped | 77 | 163 |

The correspondence unify is what breaks it: 159 of the failures are
`cannot load package 'compute.lichen': expected Struct, found Struct`, plus three
language failures (one a `SlotMap` panic in `gc_block`). Dropping only the unify
leaves four `expected Struct, found Struct` and three
`this value is not a function — it cannot be applied`, so the node alone is not
free either.

Two reasons, both about the design as written:

1. **The trigger selects almost every application.** With no further condition,
   every template instantiation with an open cloned cell in the applied function's
   scope — including the ones inside `compute.lichen`'s own definitions — gets a
   replay, which perturbs the package's own check.
2. **The correspondence is clone-vs-clone, not clone-vs-template.** A second
   instantiation's cells are distinct struct instances, so unifying an open clone
   cell with the second instantiation's cell compares two values and reports a
   mismatch. The premise "the clone's cells are undecided, so the unify is a
   link" only holds when *both* sides are pure cells, which a second instantiation
   does not guarantee at every depth.

### Landed: the two conditions that make it sound

The design is implemented (`link_replay`, `crates/lichen-lowlevel/src/function.rs`).
Once a call is **one** apply node with a check pass and a value pass
([apply-type-side-check](apply-type-side-check.md)), the replay runs for every
instantiation; its one condition is the template's own:

1. **The applied function's return expr is still open.** A template's own hole is
   what a call has yet to fill; an apply whose return expr is a computation has
   nothing to replay.

The correspondence stays as stated — the replay's own instantiation, every depth —
with two guards: only a template node the replay actually cloned has a cell to link
to, and a node the replay referenced in place is decided content that an open clone
cell must never be unified with. The link is sorted by template node, so the
class representatives do not depend on a map's order.

| shape | lowlevel | language | compute |
|---|---|---|---|
| broad trigger, two apply nodes, no guards | 163/1 | 272/3 | 50/190 |
| value-side only, two nodes | 163/1 | 274/1 | 50/190 |
| + open-return trigger, two nodes | 164/1 | 275 | 77/163 |
| + guards and sort | 164/1 | 275 | 77/163 |
| one apply node, replay on every pass (landed) | 164/1 | 275 | 77/163 |

The language layer is green with the replay in place; `lichen-compute` is
unchanged and stays out of scope for this step — its dominant refusal is the
compile-time provenance gap measured above, not the replay.

## Native calls

`$jit`, `$parallel` and `$graph` return undecided (`None`) when the function's
parameter type is not materialized — a function/wildcard type, or no value —
instead of refusing. After the enclosing scope is cloned the actual argument has
unified `f`, the parameter type is a decided struct, and the op compiles. See
`crates/lichen-compute/src/compute.rs` (`function_materialized`).

## Implementation points

- `crates/lichen-lowlevel/src/function.rs`: `instantiate_stamped` (the replay
  node and the cell correspondence), `node_apply` (the clone's `runned` reset).
- `crates/lichen-lowlevel/src/apply.rs`: `record_applied_expr` /
  `wire_apply_result` own the apply-result edges.
- `crates/lichen-compute/src/compute.rs`: `function_materialized`, the
  `Jit`/`Parallel`/`Graph` arms, and `parallel_buffer_pos`'s equality-class
  fallback for the read's input position.
