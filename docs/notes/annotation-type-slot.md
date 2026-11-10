# The annotation's type slot: position against the spec

`docs/language-spec.md` §Annotation states the rewrite:

```
@raw[value, ann1, ...] : ann2 -> {ann1 = ann2; @raw[value, ann2, ...]}
```

So `e : T` has two parts: the value's type slot expression `ann1` is **unified** with
the annotation expression `ann2`, and the slot is **replaced** by it. The slot
convention in `docs/notes/attributes.md` is the same one: an attribute slot holds the
annotation value's `[value, type]` **term expr**.

## What the checker does

`Checker::check_ann` (`crates/lichen-highlevel/src/checker/annotations.rs`):

```
denotation = type_denotation(type_expr, Some(value));      // the type expression's term
found      = state[value].ty;                              // the value's type slot node
check_unify(found, denotation, loc, DiagKind::Annotation);
state[e].ty = Some(denotation);
```

`found` is a *type node* (often a type value or an open cell), `denotation` is a
*term*. The unify is the positional lowlevel one (`Module::try_unify`), which reads
both operands as exprs.

## Measured behaviour

| program | reading |
|---|---|
| `5 : type_of (5)` | `5: Int` — the unify resolves the `type_of` call through the term |
| `x : Int`, `x : Float`, `: Struct` | accepted |
| `(plrun … : PBuf)` (a struct type value) | **refused**: `expected Struct, found raw[…Struct]` |
| `(plrun … : PBuf)`, unify target changed to the term's value half | `expected raw[Int, ?a], found Int` in `5 : type_of (5)`; 5 language-lib failures |
| unify target = `state[type_expr].val` (the value channel) | 9 language-lib failures |
| replace an undecided `found` with the term (no unify) | `type_of (1) : Type` renders `?a: ?a` — the unify is what evaluates the term |
| replace, but only for a struct type value's value, unify unchanged | language lib back to baseline; `(plrun … : PBuf)` still `expected Struct, found Table` |
| skip `type_denotation`'s element/attribute unifications for a type value | language lib back to baseline (harmless); the refusal is then the annotation's own unify: `expected Struct, found raw[…Struct]` |
| replace an undecided attribute, deep-evaluating the annotated value's term first | 5 language-lib failures (`type_of (1) : Type` renders `?a: ?a`) — the unify is not reproducible by evaluation |

Baseline suite: `lichen-language --lib` is `68 passed, 1 failed` (a pre-existing parse
test, `a_set_and_a_table_value_render_their_own_syntax`); compute `259`, highlevel
`136`, lowlevel `167`.

## The two representations in play

1. A **struct type value** (`PBuf := {…}`) is an array `[name_table, id, fields…]`
   whose *type* is the shared struct template (`struct_value_node`,
   `crates/lichen-highlevel/src/checker/structs.rs`). Its term is that array plus the
   template; `try_unify(found, term)` reads the term positionally and meets the array's
   first item (the name table): `expected Struct, found Table`.
2. A **native call's type slot** (`plrun`) is a fresh open cell: `check_native_call`
   mints it and the operator never states a type (`NativeApply { value, decided }` —
   no type field). The `: T` annotation is the only thing that can fill it.

## Options

- **A — value channel for the annotation**: make `type_denotation` answer the
  *canonical type value* (not the term) and have `check_ann` replace the slot with it;
  then `type_of (1)`'s value must be its canonical type as well (today it is a raw
  application). This is the model `docs/notes/attributes.md` states for slots, applied
  one level higher.
- **B — term channel, readers resolve**: keep the term in the slot (as now), and make
  every reader (`type_is_concrete`, `check_named_field`'s struct guard, the unify of a
  cell against a term) resolve a term to its value. This is where the `: Struct` kind
  marker already lives; the work is to make the resolution uniform.
- **C — operator states its type**: `NativeOp` gains a result-type channel
  (`ParLaunchOp` answers the kernel's codomain). Measured blocker: the codomain is not
  reachable from the operator's arguments (only `value`s, and the kernel's type is an
  open cell); it lives as operand 2 of the kernel's `Parallel` node, which the `Ctx`
  does not expose.

B is the smaller step that keeps the spec's term slot; A is the model the slots note
states and what `(plrun … : PBuf)` needs; C is orthogonal (it would also give the
wrapper a return type without any annotation).
