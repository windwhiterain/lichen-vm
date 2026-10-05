# A block and an applied lambda disagree on a struct field's inferred type

> Status: **open — root cause not found.**  This note is a handoff: it records
> every measurement that was taken, what each one rules out, and the one
> difference that held through all of them.  Nothing here is a fix, and no
> behaviour change accompanies it except the two commits named in §7.
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

## 8. Open questions for whoever picks this up

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
