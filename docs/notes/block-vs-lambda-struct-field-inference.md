# A block and an applied lambda disagree on a struct field's inferred type

> Status: **current — defect open.** The root cause is known (§7) and a fix
> direction is chosen (§8); the second generation of field cells that the
> clone-ownership rules used to mint is gone
> ([apply-clone-ownership](apply-clone-ownership.md) owns those rules), but the
> field-list constraint is still part of the *check* rather than of the per-call
> *expansion*, which is the open half.
>
> **The two programs below are both correct and must print the same type.** Under
> the intended semantics a struct instantiation is a macro expansion, so a full
> clone of the template cannot differ from the template's own evaluation — and
> `(K _)(.x T)`'s field type must be inferred from the argument, not annotated.

## 1. The disagreement

```lichen
K = _x => struct<.x _>
j1 = {T = _; 1: T; (K _)(.x T)}
j1
```

```lichen
K = _x => struct<.x _>
j = x => {T = _; x: T; (K _)(.x T) }
j 1
```

| program | printed |
|---|---|
| block | `(Int,): struct<.x Type>` |
| lambda | `(Int,): struct<.x raw[?a, ?b]>` |

The block's answer is the wanted one. `raw[?a, ?b]` is the type printer's
fallback and means the field's type node is not a readable type at all.

Two smaller programs isolate the same hole without any lambda:

```lichen
h  = X => struct<.x _>
h Int        -- prints struct<.x raw[?a, ?b]>

h2 = X => struct<.x X>
h2 Int       -- prints struct<.x Int>
```

A struct type function whose field type is a hole (`_`) does not get that hole
filled by its own type argument; one whose field type names the parameter does.
The rest of this note is about why the block nevertheless gets a type out of the
same shape.

## 2. What a reader of the instance looks at

A struct type expression compiles to the pair `[shape, kind]`, and the shape is
the array of **field types**. A field type written `_` is a fresh pair
`[value cell, kind cell]` (the `Placeholder` arm).

The printer reads a field's type through `type_printer`'s `elements()`: a
two-element array whose second slot is the universe renders its **first** slot,
and anything else falls through to `raw[...]`. So what decides `Type` versus
`raw` is the field type pair's **value cell** — and whether the class it belongs
to has a value at print time.

## 3. What was measured

**The template is identical in both programs.** `K`'s body `struct<.x _>` builds
the same field pair (a value cell and a kind cell, each a singleton class) in the
block and in the lambda, so the template cannot be where the two differ.

**The only structural difference is what else is unified with `T`.** In the
lambda, the apply's parameter unify reaches `T`'s chain from the **type** side,
and the field's value cell is a different class that nothing writes. In the
block, `1 : T` is an ascription, and the ascription's unify descends the pair
elementwise and writes into **`T`'s term's value slot** — the very cell the
field's value cell belongs to. That is the block's whole advantage: a write into
the value slot, at check time. The lambda has no such write anywhere.

**After the run there are two batches of clones, and the instance reads the empty
one.** Every template node of `K`'s scope has two clones with the same `origin`;
the instance's type tree contains only the second column, and that column's field
cell is empty. The block produces a single column, whose field cell carries the
`Type` marker — which is what makes the block print `Type`.

**The second batch comes from a recomputation, and the recomputation is correct.**
`(K _)`'s clone does not claim the template's answer: the result holds undecided
field cells, so the clone's answer has an open element and the operator still owes
its own answer, which re-runs it on the first read. Under macro expansion that
recomputation is exactly what happens — the operator's attempt was against the
template's operand, and this clone's operand is this call's — so it is not the
defect. The defect is that its result does not agree with the other evaluation of
the same template node. (The clone-ownership rules that decide what a clone may
carry are [apply-clone-ownership](apply-clone-ownership.md)'s subject.)

**Where a value would have to arrive, and does not.** The field's value cell is
written by exactly one thing in the block: the ascription's elementwise descent.
In the lambda, the only unifications that mention `T` are the parameter's
type-side ones, and the argument the field is checked against is `T`'s *type*
cell, a level above the field's value cell and in another class.

## 4. What the measurements rule out

- **Not the template** — both programs reach the identical template shape.
- **Not the clone count or a generation split** — within one apply the dedup
  table is complete and shared between the node walk and the value walk, and the
  two batches carry the same owner.
- **Not the topology rebuild** (`regroup_clones`) — the template classes it
  reproduces come out right.
- **Not the recomputation itself** — a rewritten operand must recompute.
- **Not the printer** — it reads the node it is handed, and that node is empty.

## 5. The one difference that held

**The block contains a write into the field type's value cell; the lambda
contains none.** Every attempt to supply that write at check time — unifying the
field's declared type with the argument, its value cell with the argument's value
cell, the argument's term in place of its type slot, whole to whole — was either a
no-op against two still-empty cells or a *conflict*: the block then failed with
`expected Int, found Type`, because the value being placed there was `T`'s value
(`Int`) while the field's type position expects the type `Type`.

That failure is the sharpest clue: the two sides of the missing write disagree
about *which* node the field should receive. A fix has to say which node a
`.name` argument contributes when the field's declared type is a hole — the
argument's term, its value, its type cell, or the type it names — and the block's
own history says it must be whichever node a *type position* reads.

## 6. Where a reader should look

- `crates/lichen-highlevel/src/checker/structs.rs` — `check_instantiate` (the
  field-list check `check_unify(value_shape, field_list)`, and the named path's
  `value_shape` construction); `named_instantiate` (the definition-order reorder;
  `order_ty` is built from each argument's **type slot**).
- `crates/lichen-highlevel/src/checker/lambda.rs` — `check_lam`'s
  `check_unify(type_cell, denotation)` for an annotated parameter, where
  `type_denotation` returns the type expression's **term**.
- `crates/lichen-highlevel/src/checker/annotations.rs` — `type_denotation`.
- `crates/lichen-lowlevel/src/function.rs` — the clone walk (the baking rule, the
  operand remap, the `origin` stamp) and the per-call instantiation.
- `crates/lichen-lowlevel/src/apply.rs` — `regroup_clones` /
  `unify_clone_groups`.
- `crates/lichen-render/src/render/type_printer.rs` — `elements()`, the
  `[head, universe]` arm and the `raw[…]` fallback.

## 7. The root cause

**The field-list check runs once, at check time, against the callee's check-time
evaluation batch — and its effect lives only in template-level class topology. A
callee that is an apply node embedded in a function template is recomputed on
every call (its result holds undecided field cells, so the deep pass judges it
undecided and the clone cannot carry the cached value); the recomputation mints
fresh field cells that no constraint ever touches, and the instance's type is
wired to that recomputation. The field constraint is part of the *check*, not
part of the *expansion*: macro expansion re-runs the callee's body per call but
never re-checks the fields against the call's arguments.**

The write sequence, in order:

1. **Check time, the body.** The force in `check_instantiate` evaluates the
   `(K _)` apply node, and the field-list check unifies the value shape against
   the field list with **both sides undecided** (`x: T` pins nothing). The class
   commits the array, but its two cells stay undecided; the constraint now exists
   *only* as this class topology.
2. **The call's clone walk.** The committed array carries, with its cells
   referenced in place because they are owned by `K`'s template. The `(K _)`
   clone's cached value is dropped (it is undecided), so it recomputes: the
   second batch. From here on, no unify in the run ever mentions it.
3. **The parameter check.** The descent into `x`'s type-cell clone — which also
   carries the committed array — unifies the argument's type pair against it and
   binds **batch 1**. The right value arrives on the wrong generation: the result
   type was already wired to the recomputation.
4. The printer reads the second batch's empty field cell and falls through to
   `raw[?a, ?b]`.

The block's whole advantage, restated: there is **no enclosing template**, so the
`(K _)` apply is evaluated exactly once — the batch the field check ran against
is the batch the printer reads. And because `1: T` precedes the instantiate in the
same check pass, the field check's descent meets an already-bound `T` kind cell
and writes `Type` into the field's value cell immediately.

**The constraint survives per call exactly when its unify partner is a template
member.** That is why the same body with no `K` at all works:
`j = x => {T = _; x: T; struct<.x _>(.x T) }; j 1` prints `struct<.x Type>`,
because the field pair is a member of `j`'s own template and the topology rebuild
re-establishes the check-time merge among the per-call clones.

## 8. The chosen direction: make the instantiate a per-call computation

A struct instantiation whose field check is structurally valid compiles its
**term to an operation node** (a `LowOperator::Instantiate`) instead of the plain
constant pair the checker builds today. The operator's evaluation forces the
callee, reads the struct shape out of the callee's fresh value, and **unifies the
shape against the argument's type-slot array** — the same pairing the check-time
`check_unify(value_shape, field_list)` already establishes, now re-established
against whichever batch the call actually computed. The operand-rewrite rule
re-runs the operator exactly when the expansion re-runs, so the field constraint
becomes part of the per-call expansion. The check-time check and everything it
feeds (diagnostics, nominality, the name-table reorder) stay as they are.

**Why it is safe beside the existing check.** The check-time unify and the
operator's per-call unify are idempotent against each other — both are class
merges over the same pairing — and ordering is a non-issue, because a class merge
is correct whether it runs before or after the parameter check binds the
argument's cells (replication carries a later write to the merged class). A
declared field type is already enforced per call through the parameter channel;
the operator adds the missing *inference write* for hole fields.

**The operator's contract.** Operand: one array, three items — the struct type
expression's term, the instance's field values in definition order, and the
arguments' type slots in definition order. Evaluation mirrors the `Apply` arm:
evaluate the operand array and propagate `Parameterized`/`Error` markers exactly
as `Apply`/`Index` do (**never** answer `Error` for an undecided callee — an
`Error` caches as a decided value and certifies the node concrete); evaluate slot
0 and read element 0 of its value pair; unify that shape against the argument's
type slots; answer the pair `[field values, struct type]` through the write path
so the node *is* the instance pair the checker used to build.

**The checker change.** In `check_instantiate`, keep everything up to and
including the `check_unify(value_shape, field_list)` call; when `valid`, build the
term as the operation node instead of the plain pair, and leave `state[e].ty` and
`state[e].val` as they are (the expression's type is the struct type itself).
When `!valid`, keep today's plain pair: a mismatch was already recorded and there
is no constraint to enforce per call.

**Pitfalls, all measured.** The constraint must be on the read path — the
instantiate's `term` must *be* the operation node, or the constraint node is never
evaluated. Do not delete the check-time unify: it owns the diagnostic attribution
for check-time field mismatches, while the operator's unify records ownerless
errors that surface as orphan mismatches after every attributed diagnostic. Do
not merge the two batches: they are expansions under, in general, different
arguments. An undecided callee must yield `Parameterized`, never `Error`. The two
accidentally-working paths (§9) depend on the deep pass judging the operation node
concrete when its field cells bound at check time.

**Verification.** The note's own programs, all through the language's evaluator:

| program | expected |
|---|---|
| `K = _x => struct<.x _>` / `j1 = {T = _; 1: T; (K _)(.x T)}` / `j1` | `(Int,): struct<.x Type>` (unchanged) |
| the same body inside `j = x => {…}` / `j 1` | `(Int,): struct<.x Type>` (**the fix**) |
| no `K`: `j = x => {T = _; x: T; struct<.x _>(.x T) }` / `j 1` | `(Int,): struct<.x Type>` (unchanged) |
| `K = _x => struct<.x _>` / `j = x => {T = _; x: T; 1: T; (K _)(.x T) }` / `j 1` | `(Int,): struct<.x Type>` (unchanged) |
| `K = _x => struct<.x Int>` / `j = x => (K _)(.x x)` / `j 1` | `(1,): struct<.x Int>` (unchanged) |
| the same / `j "s"` | `expected Int, found string` (unchanged) |
| `h = X => struct<.x _>` / `h Int` | `struct<.x raw[?a, ?b]>: TypeStruct` (unchanged — a genuinely unconstrained hole stays raw) |
| `h2 = X => struct<.x X>` / `h2 Int` | `struct<.x Int>: TypeStruct` (unchanged) |

## 9. The alternatives, and what must keep working

- **(b) Register the field constraint as a per-call obligation**, re-instantiated
  by the apply the way the body's asserts already are. It reuses existing
  re-registration machinery, but asserts are evaluated predicates, not unifies: a
  pending-unify channel is new, and the constraint's partner (the nested apply's
  fresh batch) exists only *after* that apply runs, so an ordering has to be
  defined.
- **(c) Unify against a lazy read of the shape** (`Index(callee, 0)`) instead of
  the forced batch — **measured and rejected**: the unify still runs once at check
  time against the *template* read node, whose per-call clone is a different
  class, so the constraint still never reaches the recomputed batch. Recorded so
  it is not retried.

Whichever way it goes, two accidentally-working paths must keep working: the
block (a single batch, bound at check time), and the ascribe-before variant
(`1: T` before the instantiate, inside the same body), where the check-time batch
is bound, the deep pass proves the instantiate's value concrete, and the clone is
baked rather than recomputed. And a genuinely unconstrained hole
(`h = X => struct<.x _>; h Int`) must stay raw: nothing decides it, and the
printer's mark is the honest answer.
