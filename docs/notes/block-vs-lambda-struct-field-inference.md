# A block and an applied lambda disagree on a struct field's inferred type

> Status: **root cause found** (§9); **a workaround now fixes the reported
> programs** (§13) while direction (a) — the principled fix — is still owed and
> still handed off (§12).  The
> measurements in §1–§7 stand as the investigation that narrowed the question;
> §9–§11 are the answer, found by tracing every root `unify`/`MERGE`/value
> write during the build of both §1 programs and by three confirming
> experiments (§10).
>
> Worktree `.worktrees/trace-order`, branch `feature/class-value-on-representative`,
> HEAD `06b2b37`.
>
> **The two programs below are both correct and must print the same type.**
> Under the intended semantics a struct instantiation is a macro expansion, so a
> full clone of the template cannot differ from the template's own evaluation —
> and `(K _)(.x T)`'s field type must be inferred from the argument, not
> annotated.

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

Measured with the language's own evaluator (a throwaway test calling
`lichen_language::run::evaluate_raw`):

| program | printed |
|---|---|
| block | `(Int,): struct<.x Type>` |
| lambda | `(Int,): struct<.x raw[?a, ?b]>` |

The block's answer is the wanted one.  `raw[?a, ?b]` is
`type_printer.rs`'s fallback (`elements()`'s last arm) and means the field's
type node is not a readable type at all.

Two smaller programs isolate the same hole without any lambda:

```lichen
h = X => struct<.x _>
h Int
```

prints `struct<.x raw[?a, ?b]>`, while

```lichen
h2 = X => struct<.x X>
h2 Int
```

prints `struct<.x Int>`.  So a struct type function whose field type is a hole
(`_`) does not get that hole filled by its own type argument, and one whose
field type names the parameter does.  The rest of this note is about why the
block nevertheless gets a type out of the same shape.

## 2. What a reader of the instance looks at

A struct type expression compiles to the pair `[shape, kind]`, and the shape is
the array of **field types** (`checker/structs.rs`, `struct_type_type`).  A
field type written `_` is a fresh pair `[value cell, kind cell]` (the
`Placeholder` arm of `checker.rs`, which allocates two fresh cells).

The printer reads a field's type through `type_printer.rs`'s `elements()`: a
two-element array whose second slot is the universe (`is_universe_any`) renders
its **first** slot (`self.any_node(elements[0].node)`), and anything else falls
through to `raw[...]`.  So what decides `Type` versus `raw` is the *first* slot
of the field's type pair — the field's **value cell** — and whether the class it
belongs to has a value at print time.

## 3. What was measured

All node ids below are from the lambda program in §1 unless stated.

### 3.1 The template is identical in both programs

For `K`'s body `struct<.x _>` (the `TypeStruct` expression at index 16 of the
compiled IR; `K`'s own field pair is node 28):

```
field pair 28 = [26, 27]
  26 (the field type's value cell)  class(1) = [26]      — a singleton
  27 (the field type's kind cell)   class(1) = [27]      — a singleton
```

The same holds in the block program.  **The template therefore cannot be where
the two programs differ**, and every later measurement agrees: both programs
reach the same template shape.

### 3.2 The only structural difference is what else is unified with `T`

In the lambda, after `x : T` and the instantiation:

```
x's term        = [42, 43]
x's ty          = 43 = [51, 52]        (T's term)
T (the body's `_`)  term = 53 = [51, 52],  val = 51,  ty = 52
52 (a cell of T's pair)  class(3) = [49, 52, 71]     ← 71 is K's field pair
26 (the field type's value cell)  class(1) = [26]    ← NOT in that class
```

`71` is the field pair `[?]` of the *instantiated* struct type, and it is in the
same class as `T`'s type cell; the field's value cell `26` is not.

In the block, `1 : T` is an ascription, and the ascription's unify descends the
pair elementwise: it writes `1`'s type into **`T`'s term's value slot**, which
is the very cell the field's value cell belongs to.  That is the block's whole
advantage — it is a *write into the value slot*, done at check time.

**The lambda has no such write anywhere.**  The apply's parameter unify reaches
`T`'s chain from the type side (the class above: `49, 52, 71`), and the field's
value cell is a different class that nothing writes.

### 3.3 After the run: two batches of clones, one of them empty

Every template node of `K`'s scope has **two** clones, each pair with the same
`origin`:

```
origin 26 → 72  (own = TypeValue(TypeType))     + 122 (own = Parameterized)
origin 27 → 73                                 + 123
origin 28 → 71                                 + 121
origin 30 → 70                                 + 120
origin 38 → 69                                 + 119
origin 20/21/22 → 76/77/75                     + 125/126/124
```

The printed instance's type tree contains **only** the second column
(`119…126`); the first column is not reachable from it.  The instance reads
`122` as its field type — the empty one.  For comparison, the block produces a
single column (`66, 67, 65, 64, 68`), and `66` carries
`TypeValue(TypeType)` — which is what makes the block print `Type`.

Both columns' nodes carry the **same owner**
(`Some(FunctionId(1v1))`), so they are not two generations in the sense of two
different applied functions; and `122`'s `origin` is `26`, the template node, so
it is a direct clone of the template.

### 3.4 The extra instantiation, and why it happens

`APPLY BEGIN` logging added to `instantiate_stamped` (`function.rs`) shows the
lambda evaluating `K`'s function **eleven** times; the last three are:

```
fn=1 node=63   arg=60    origin=None       owner=Some(FunctionId(2v1))
fn=2 node=99   arg=96    origin=None       owner=None          ← the user's `j 1`
fn=1 node=113  arg=115   origin=Some(63)   owner=Some(FunctionId(2v1))
```

(The first eight are the prelude's own functions, each with a different owner
`FunctionId(2..9)`; the block program has those same eight and then exactly one
`fn=1 node=57 origin=None owner=None` — its own `(K _)`, evaluated once.)

Node `113` is a clone of node `63` — the `(K _)` of the lambda's body — made by
`node_apply` while the `j 1` apply clones its template.  Logging the clone
decision right after `self.nodes[clone].origin = Some(node)` prints:

```
APPLY CLONE template=63 clone=113 operand_rewritten=true carried_value=false runned=false
```

and, for the rest of the same batch:

```
APPLY CLONE template=28 clone=121 operand_rewritten=false carried_value=true runned=true
APPLY CLONE template=30 clone=120 operand_rewritten=false carried_value=true runned=true
… (20, 21, 22, 38 alike)
```

So `(K _)`'s clone is the one node in the batch whose **operand was rewritten**:
its argument is the placeholder `_`, a per-call cell (node `60` → clone `115`),
so `node_apply` remaps it, and the existing rule in `node_apply` — *"A cached
value on an operation node was computed against the body's parameter and is
stale once the argument is mapped in, so such clones are left unevaluated"* —
clears the clone's value.  The clone then recomputes on its first read, i.e.
`K` is instantiated a second time.  That second instantiation is the batch
`119…126`, and it is the one the instance carries.

**The author's position on this step is that it is correct**: an operand that
was rewritten must be recomputed, because a full clone of the template *is* the
macro expansion, and under macro expansion this recomputation is exactly what
happens.  The recomputation is therefore not the defect — the defect is that
its result does not agree with the other evaluation of the same template node.

### 3.5 Where a value would have to arrive, and does not

The field's value cell is written by exactly one thing in the block: the
ascription's elementwise descent.  In the lambda, the only unifications that
mention `T` are the parameter's (type-side) ones.  Measured at the
instantiation site, the argument the field is checked against is `52` — `T`'s
*type* cell — and the field's value cell is a level below it and in another
class.

## 4. What the measurements rule out

* **Not the template.**  Both programs reach the identical template shape
  (§3.1).
* **Not the clone count or a "generation" split.**  Within one apply the
  dedup table is complete (`ctx.remap`, consulted first in `node_apply`, and
  shared with `value_apply`'s element walk); the two batches carry the same
  owner (§3.3).
* **Not `regroup_clones`' topology rebuild.**  It runs before the parameter
  check and groups the call's clones by *their template's class representative*;
  the template classes it reproduces are reproduced (the two-element groups come
  out right).
* **Not the recomputation itself.**  See §3.4 — a rewritten operand must
  recompute.
* **Not the printer.**  `elements()` is reading the node it is handed; that
  node is empty (§3.3).

## 5. The one difference that held through every measurement

**The block contains a write into the field type's value cell; the lambda
contains none.**  Every attempt to supply that write at check time — unifying
the field's declared type with the argument, its value cell with the argument's
value cell, the argument's term in place of its type slot, "whole to whole" —
was either a no-op against cells that are both still empty, or a *conflict*
(the block then failed with `expected Int, found Type`, because the value being
placed there was `T`'s value — `Int` — while the field's type position expects
the type `Type`).

That failure is the sharpest clue in the note: the two sides of the missing
write disagree about *which* node the field should receive.  A fix has to say
which node a `.name` argument contributes when the field's declared type is a
hole: the argument's term, its value, its type cell, or the type it names — and
the block's own history says it must be whichever node a *type position* reads.

## 6. Where a reader should look

* `crates/lichen-highlevel/src/checker/structs.rs`
  * `check_instantiate` — the field-list check (`check_unify(value_shape, field_list)`)
    and the named path's `value_shape` construction.
  * `named_instantiate` — the definition-order reorder; `order_ty` is built from
    `self.state[argument].ty`, i.e. each argument's **type slot**.
  * `struct_field_types` (added during this investigation, later reverted) — the
    field list is `type_pair[0]`'s items.
* `crates/lichen-highlevel/src/checker/lambda.rs` — `check_lam`'s
  `check_unify(type_cell, denotation)` for an annotated parameter, where
  `type_denotation(parameter_type, Some(parameter))` returns the type
  expression's **term**.
* `crates/lichen-highlevel/src/checker/annotations.rs` — `type_denotation`.
* `crates/lichen-lowlevel/src/function.rs` — `node_apply` (the baking rule, the
  operand remap, the `origin` stamp), `value_apply`'s dynamic-function arm (a
  closure clone maps its whole scope), `instantiate_stamped` (`regroup_clones`
  then `apply_parameter_check`).
* `crates/lichen-lowlevel/src/apply.rs` — `regroup_clones` / `unify_clone_groups`.
* `crates/lichen-render/src/render/type_printer.rs` — `elements()`, the
  `[head, universe]` arm and the `raw[…]` fallback.

## 7. Commits made during the investigation

* `61e6791` — `liche-analyze`: the tool crate's `clones_of` / `source_of`, the
  reads that produced §3.3.
* `06b2b37` — `lowlevel: a clone reruns only when its operand was rewritten`.
  Author-confirmed: whether an operator runs is `runned`, so a clone whose
  operand mapped to itself carries the template's answer and `runned` with it.
  Suites: lowlevel 155, checker 87 — unchanged.

Everything else tried was reverted; the tree is clean at `06b2b37`, and the
suites read **lowlevel 155 passed**, **checker 87 passed**, pipeline 137 of 140,
compute 59 of 62 — the same numbers as before this work.

## 8. Open questions for whoever picks this up — **answered in §9**

1. **Which node does a `.name` argument contribute** when the field's declared
   type is a hole — and is that the same node a *type position* reads?
   (§5's conflict says the current code and the block's history disagree.)
2. **Must two clones of one template node be one class** — the author's stated
   model is that a template's clones are one batch and a class cannot diverge
   across them.  If yes, the merge belongs at the clone's creation
   (`node_apply`), not in the checker; if no, the divergence is intended and the
   fix has to make both batches carry the same value by construction instead.
3. **Is the instance's type supposed to be the struct type expression's own
   shape** (`IR::ExprKind::Instantiate`'s doc says "the expression's type is the
   struct type itself"), and if so, where does the argument's type enter that
   shape?

## 9. The root cause

> **Superseded, partly.**  The clone-walk rules this section's argument leans on
> ("the apply clone can never carry the cached value") were themselves the
> defect: a clone was stamped with the *callee's* owner instead of the enclosing
> template's, and a carried answer claimed its operator's run even with open
> slots.  Both are fixed, and a one-line reproduction of the same family
> (`id = x => x`, `f = x => id x`, `f 1`) now reads `1: Int`; the commit that
> broke it (`30f1308`, a flat answer write) was found by bisect.  See
> [`apply-clone-ownership.md`](apply-clone-ownership.md) for the measurements.
> The field-list analysis below still describes how the *check* relates to the
> *expansion*, which is why the companion note's direction (a) is still the plan
> for the field-list form of the divergence.

**The field-list check runs once, at check time, against the callee's
check-time evaluation batch — and its effect lives only in template-level
class topology.  A callee that is an apply node embedded in a function
template is recomputed on every call (its result holds unbound field cells, so
the deep pass judges it parameterized and the apply clone can never carry the
cached value); the recomputation mints fresh field cells that no constraint
ever touches, and the instance's type is wired to that recomputation.  The
field constraint is part of the *check*, not part of the *expansion*: macro
expansion re-runs the callee's body per call but never re-checks the fields
against the call's arguments.**

The full write sequence, traced by logging every root `unify`, every
`add_equality` merge and every `write_node_value` during the build of the
lambda program (node ids as in §3):

1. **Check time, `j`'s body.**  The force in `check_instantiate` evaluates the
   `(K _)` apply node — batch 1 is born (`71 = [72, 73]`).  The field-list
   check then issues `UNIFY value_shape(=[52]) field_list(=[71])`, descending
   to `MERGE 52 71` with **both sides unbound** (`x: T` pins nothing).  The
   class `{49, 52, 71}` commits the array `[72, 73]`; `72` and `73` themselves
   stay unbound.  The constraint now exists *only* as this class topology.
2. **`j 1`, clone walk.**  `52`'s clone (`110`) carries the class's committed
   array `[72, 73]` — with `72`/`73` **referenced in place**, because both are
   owned by `K`'s template and fail the walk's membership test.  The `(K _)`
   clone's cached value is dropped (it is parameterized: the struct pair
   contains unbound cells), so it recomputes: **batch 2** (`121 = [122, 123]`).
   From here on, no unify in the whole run ever mentions `121`, `122` or
   `123`.
3. **`j 1`, parameter check.**  The descent into `x`'s type-cell clone — which
   also carries the committed `[72, 73]` — unifies `Int`'s type pair against
   it and binds **batch 1**: `MERGE 72 4` (`72 := Type`), `MERGE 73 11`.  The
   right value arrives on the wrong generation: batch 1 is by then unreachable
   from the result, because `wire_apply_result` wired the instance's type to
   the recomputation (`MERGE 97 113`, `MERGE 113 119` — the root type *is*
   batch 2's struct pair).
4. The printer reads `122` — a cell nothing ever wrote — and falls through to
   `raw[?a, ?b]`.

The block's whole advantage, restated in these terms: there is **no enclosing
template**, so the `(K _)` apply is evaluated exactly once — the batch the
field check ran against is the batch the printer reads.  And because `1: T`
precedes the instantiate in the same check pass, the field check's descent
meets an already-bound `T` kind cell and writes `Type` into the field's value
cell immediately.

## 10. Confirming experiments

* `j = x => {T = _; x: T; struct<.x _>(.x T) }; j 1` (no `K` at all) —
  **works** (`struct<.x Type>`).  The field pair is then a member of `j`'s own
  template: `unify_clone_groups` re-establishes the check-time merge among the
  per-call clones, and the parameter check's descent binds them per call.
  *The constraint survives per call exactly when its unify partner is a
  template member.*
* `j = x => {T = _; x: T; 1: T; (K _)(.x T) }; j 1` (pin `T` before the
  instantiate, inside the same body) — **works**.  Batch 1's field cells are
  bound at check time, so the `(K _)` node's value is concrete, the deep pass
  proves it, `node_apply` bakes it (referenced in place, never recomputed),
  and the instance reads batch 1.  Measured: exactly one field-pair batch,
  `deep=concrete`.  (That a syntactic *ordering* inside the body decides
  whether the program works is itself a symptom of the constraint living at
  check time.)
* `(K 0)(.x T)` in the lambda — still `raw`.  The argument does not decide the
  recompute; the unbound field cells do (they keep the result parameterized,
  so the clone is always dropped and re-run).  Two batches measured.

## 11. Answers and fix directions

**Answers to §8:**

1. The argument's **type slot** — what `named_instantiate`'s `order_ty`
   already pairs with the field pair.  The pairing is right; the timing and
   scope are what is wrong.  (§5's conflicts came from attempts to pair the
   argument's *value* with the field's *value cell* — a level error, correctly
   rejected.)
2. **No.**  The two batches are two macro expansions of `K`'s body under — in
   general — different arguments (the second expansion's argument is the
   call's own placeholder, bound per call), so merging them at clone creation
   would be wrong.  The divergence is intended; what must hold is that the
   field constraint applies to whichever expansion the instance reads.
3. **Yes** — the instance's type is the per-call expansion.  The argument's
   type therefore has to enter that shape *per call, at the apply*.

**A fix has to make the field-list constraint part of the per-call expansion.
Directions, not a decision:**

* **(a) Encode the instantiate as a per-call computation.**  A struct
  instantiation whose callee is not statically concrete compiles to an
  operation (like `Apply`) whose evaluation unifies the just-evaluated
  callee's shape against the argument's type slots — the operand-rewrite rule
  then re-runs the check exactly when the expansion re-runs.  Matches the
  macro-expansion semantics one for one; costs moving the field-check policy
  (which nodes pair, and the diagnostics' attribution) into the runtime graph
  or behind a callback, while keeping the check-time behaviour for concrete
  callees unchanged.
* **(b) Register the field constraint as a per-call obligation**, re-
  instantiated by the apply the way the body's asserts already are
  (`instantiate_stamped`'s assert walk).  Reuses existing re-registration
  machinery, but asserts are evaluated predicates, not unifies: a pending-
  unify channel is new, and the constraint's partner (the nested apply's fresh
  batch) exists only *after* that apply runs, so an ordering has to be
  defined.
* **(c) Unify against a lazy read of the shape** (`Index(callee, 0)`) instead
  of the forced batch — measured and rejected: the unify still runs once at
  check time against the *template* read node, whose per-call clone is a
  different class, so the constraint still never reaches batch 2.  Recorded so
  it is not retried.

Any of them must keep the two accidentally-working paths working: the block
(single batch, bound at check time) and the ascribe-before variant (batch
baked concrete by the deep pass).

## 12. Implementation plan for direction (a) — handed off

> Chosen by the author; implementation handed off.  This section is
> self-contained: §9 is the why, this is the what.

### 12.1 The change in one paragraph

A struct instantiation whose field check is structurally valid compiles its
**term to an operation node** (a new `LowOperator`, tentatively
`Instantiate`) instead of the plain constant pair `pair_of(value_node,
type_pair)` it builds today.  The operator's evaluation forces the callee,
reads the struct shape out of the callee's fresh value, and **unifies the
shape against the argument's type-slot array** — the same pairing the
check-time `check_unify(value_shape, field_list)` already establishes, now
re-established against whichever batch the call actually computed.  The
operand-rewrite rule (`node_apply`, author-confirmed) re-runs the operator
exactly when the expansion re-runs, so the field constraint becomes part of
the per-call expansion.  The check-time check and everything it feeds
(diagnostics, nominality, the name-table reorder) stay exactly as they are.

### 12.2 Why this is safe to add beside the existing check

* The check-time `check_unify` and the operator's per-call unify are
  **idempotent against each other**: both are class merges over the same
  pairing; the second merge binds nothing new at check time and everything
  that matters per call.
* Ordering is a non-issue: the unify is a class merge, so it is correct
  whether it runs before or after the apply's parameter check binds the
  argument's cells — replication carries a later write to the merged class.
  (Traced: batch 1's field cell was bound *during* the parameter check, and
  the merge still delivered it.)
* A *declared* field type is already enforced per call through the parameter
  channel (measured: `K = _x => struct<.x Int>; j = x => (K _)(.x x); j "s"`
  fails with `expected Int, found string` today).  The operator adds the
  missing *inference write* for hole fields; enforcement for declared fields
  keeps working through the existing channel, with the operator as a second
  opinion against the recomputed batch.

### 12.3 The operator's contract

Operand (one array node, three items):

| slot | node | role |
|---|---|---|
| 0 | `callee_pair` | the struct type expression's term (today's `type_pair`) |
| 1 | `value_node` | the instance's field values, definition order (today's `named_instantiate`/`positional` value) |
| 2 | `value_shape` | the arguments' type slots, definition order (today's `order_ty` array) |

Evaluation (mirror the `LowOperator::Apply` arm in
`crates/lichen-lowlevel/src/evaluation.rs`):

1. Evaluate the operand array; propagate `Parameterized`/`Error` markers
   exactly as the `Apply`/`Index` arms do — **never** answer `Error` for an
   unbound callee (the apply-frame note in `apply.rs` says why: an `Error`
   caches as a decided value and certifies the node concrete).
2. Evaluate slot 0 (the callee).  Read element 0 of its value pair — the
   field-type shape.  A callee that is not a readable struct pair was
   already refused at check time (the `!concrete` pin path re-checks per
   call through this same unify).
3. `self.unify(shape, value_shape)` — the per-call field check.  Arrays
   descend elementwise; each field pair meets the argument's type slot.
4. Answer the pair `[slot 1, slot 0]` (fresh two-element array in the node's
   block) through `write_node_answer`, so the node *is* the instance pair
   `[value, struct type]` exactly as today's `pair_of` makes it.

### 12.4 The checker change

In `check_instantiate` (`crates/lichen-highlevel/src/checker/structs.rs`):

* Keep everything up to and including the `check_unify(value_shape,
  field_list)` call unchanged.
* When `valid` (the structural field checks passed), build the term as the
  operation node (`op_node` with `LowOperator::Instantiate` and the §12.3
  operand) instead of `pair_of(value_node, type_pair)`.
  `state[e].ty = type_pair` and `state[e].val = value_node` stay as they
  are — the `IR::ExprKind::Instantiate` contract ("the expression's type is
  the struct type itself") is untouched.
* When `!valid`, keep today's plain `pair_of` form: a mismatch was already
  recorded, and there is no constraint to enforce per call.

Nothing else in the checker changes: `named_instantiate`'s reorder, the
`!concrete` pin, the mid-recursion probe cell, and the lazy-`Index`
`field_list` all stay.

### 12.5 Touch list

* `crates/lichen-lowlevel/src/lib.rs` — the `LowOperator` variant (enum at
  `:426`).
* `crates/lichen-lowlevel/src/evaluation.rs` — the dispatch arm (`:178`),
  beside `Apply`.
* `crates/lichen-lowlevel/src/codec.rs` — the static-module serialization
  discriminant (`:328`/`:337`); assign the next `u8`.
* `crates/lichen-lowlevel/src/low_type.rs` — the operator's low-type
  projection (`:106`); the answer is a pair, so the pair rule applies.
* `crates/lichen-lowlevel/src/resolve.rs` — operand arity validation
  (`:108`); three slots.
* `crates/lichen-highlevel/src/checker/structs.rs` — the `check_instantiate`
  term construction (§12.4).
* `loop_conversion.rs:223/250` matches specific operators — check the arms
  still cover what they must; no semantic change expected.

### 12.6 Pitfalls (all measured, all avoidable)

1. **The constraint must be on the read path.**  The instantiate's `term`
   must *be* the operation node.  A constraint node on the side that nothing
   references is never evaluated — evaluation is demand-driven.
2. **Do not delete the check-time `check_unify`.**  It owns the diary
   attribution for check-time field mismatches; the operator's unify records
   ownerless errors, which surface as *orphan mismatches* positioned after
   every attributed diagnostic (`diagnostic.rs:421-423`).  Keeping both
   preserves today's diagnostics exactly.
3. **Do not merge batch 1 and batch 2** (§11 answer 2): they are expansions
   under, in general, different arguments.
4. **Marker discipline**: an unbound callee must yield `Parameterized`, never
   `Error` (§12.3.1).
5. The deep pass's concreteness verdicts are load-bearing for the two
   accidentally-working paths (§10): when the field cells bind at check
   time, the instantiate operation node must come out *concrete* so
   `node_apply` bakes it — it will, because its answer's cells are the same
   bound ones the constant pair carried.

### 12.7 Verification recipe

Programs (all through `lichen_language::run::evaluate`; the first four are
the note's own):

| program | expected |
|---|---|
| `K = _x => struct<.x _>\nj1 = {T = _; 1: T; (K _)(.x T)}\nj1` | `(Int,): struct<.x Type>` (unchanged) |
| `K = _x => struct<.x _>\nj = x => {T = _; x: T; (K _)(.x T) }\nj 1` | `(Int,): struct<.x Type>` (**the fix**) |
| `j = x => {T = _; x: T; struct<.x _>(.x T) }\nj 1` | `(Int,): struct<.x Type>` (unchanged) |
| `K = _x => struct<.x _>\nj = x => {T = _; x: T; 1: T; (K _)(.x T) }\nj 1` | `(Int,): struct<.x Type>` (unchanged) |
| `K = _x => struct<.x Int>\nj = x => (K _)(.x x)\nj 1` | `(1,): struct<.x Int>` (unchanged) |
| `K = _x => struct<.x Int>\nj = x => (K _)(.x x)\nj "s"` | `expected Int, found string` (unchanged) |
| `h = X => struct<.x _>\nh Int` | `struct<.x raw[?a, ?b]>: TypeStruct` (unchanged — a genuinely unconstrained hole stays raw) |
| `h2 = X => struct<.x X>\nh2 Int` | `struct<.x Int>: TypeStruct` (unchanged) |

Suites (§7's baseline): lowlevel 155, checker 87, pipeline 137 of 140,
compute 59 of 62 — unchanged except where the fix deliberately adds
coverage.  A regression test belongs with the render tests
(`crates/lichen-language/src/tests/render_tests.rs`'s `output()` helper);
per team rule, add tests only with the author's say-so.

## 13. The workaround that landed meanwhile (author's call)

§9's mechanism is the apply clone walk's **operand-rewrite rule**: an
operation whose operand this call maps to different nodes had its answer
dropped, so an embedded callee apply re-ran per call and minted a second
generation of field cells (§3.4).  The author's rule replaces that rule in
`node_apply`:

> **Whether an operator runs is `runned`, and nothing else.**  A clone
> carries the template's answer, mapped recursively so every node the answer
> names is this call's node, and the operator does not run again.

Three qualifications, each measured rather than chosen:

1. **The answer has to be a template fact.**  Two node axes decide, and they
   are not to be conflated: `runned` says the source's *own* operator produced
   the value (`false` with a value present means a unification wrote it), and
   `evaluated_deep` says the **deep pass** evaluated the node, which is what
   makes its answer a fact about the template rather than about whichever call
   ran last.  Both must hold.  A clone that fails either carries **no value at
   all** — a slot holding a value is a slot a static reader (a backend
   compiling from the graph) reads as decided, and this was measured: with the
   value written anyway, `graph_jit` lost 8 of 9 and `graph_structure` 1 of 5.
2. **A function id is never carried.**  It is a per-call allocation, not a
   value the operator computed from its operand, and mapping it mints a second
   per-call closure beside the one the element walk already cloned — measured
   as `expected Function, found Function` on every block-with-capture closure.
3. **One closure per call.**  The walk reaches a template closure from several
   nodes (its value node, and any carried answer that names it — a recursive
   call's result pair), and each reach mints a fresh closure under the old
   code.  Two closures where the body means one meet in a unification as two
   different functions; measured as 5 functions where `recursion` expects 3.
   `ApplyCtx` now carries the walk's `minted` table (source `FunctionId` →
   fresh `FunctionId`), the closure-level counterpart of the node-level
   `remap`.

**What it fixes:** the second generation is never minted, so the field check's
class binding lands on the batch the instance actually reads, and §1's lambda
program prints `struct<.x Type>` — the note's whole §12.7 table passes.

**What it does not fix:** the root cause.  A callee that *is* re-run per call
(a program that binds its argument before the instantiate, say) still mints a
fresh generation, and the field constraint still does not follow it; §12 is
still owed.  This workaround only removes the generation in the case where the
template's own answer is the one that was being recomputed.

Suites after the change, against the same baseline: lowlevel 155, checker 87,
language 70, pipeline 137 of 140, compute 59, graph_jit 9, graph_structure 5 —
**identical to the numbers before it**, plus the reported program fixed.


