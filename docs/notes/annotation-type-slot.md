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

## The guard step that landed

`check_named_field`'s struct guard (`check_unify(container_ty, struct_template)`) is now
skipped when the container's type is a **struct type value** (`struct_identity_items`'s
name table at its item 0): a type value already proves the layout, and unifying it with
the template is what produced `expected Struct, found raw[…Struct]`. With the guard
skipped and the probe annotating its two runs with their concrete codomains
(`: PBuf`, `: Out`), the named contract's refusals move to two other places:

- `c.c` on the contract call's result: `UndecidedContainer` — the function's return type
  is the annotated run's type, and the caller's read does not see it decided;
- a runtime unify `expected raw[Table, TypeId(51), …], found Struct` at the program's
  `Meta`/`In` declarations — the contract kernel's argument path.

Both are outside the annotation itself.

## The curried call step

The first of those was the *curried* call: a partial application's value is a runtime
cell, so `function_apply_function` answered `None` for the outer applies and their result
types stayed open (`(f 1 2).x` refused). The fallback now walks the IR
(`Checker::curried_callee`): the inner application's own function — recursing through the
chain — then that function's return expression as the next function. Measured: the
minimal curried probe answers `3`, and the named contract's call resolves every apply, so
its only remaining refusal is the runtime unify on the plan type (`build_outputs`'s
declared-codomain assembly, `expected raw[Table, TypeId(51), …], found Struct`).

## The runtime unify, against the spec's struct rule

The spec (`docs/language-spec.md` §Struct) is explicit: an instance's type slot **is**
`Struct`, and the named type is the value's identity (`name_table` + `nominal_id`):

```
s := @raw[@raw[name_table, nominal_id, e1, e2], Struct, ...]
s.<name> := {type_of s = Struct; @raw s.value[s.value.name_table[<name>]+2]}
```

So the failing engine-side unify compares two things that are correct at their own levels:
the **declared codomain** (`PBuf`, a named struct *type value*: `raw[Table, TypeId(51), …]`)
and the **result's type** (the `Struct` marker, as an instance's type must be). The
declared codomain is a *layout to assemble by identity*, which `build_outputs` /
`struct_value_node` already do (name table + id + fields, no unify). The runtime should
therefore not unify the two: the diagnostic is the engine-side apply/normalize unify
(a `DiagKind::Runtime` with no diary entry) meeting the declared codomain.

Next: find which engine unify (definition-pass apply or a clone normalize) receives the
declared codomain as one side, and make the codomain a layout input rather than a unify
operand — the spec's rule is identity, not a type-level equality.

### Measured alternative: the marker in the type slot

`check_ann` was changed so that a **named struct type** in a type position sets the slot
(and the check) to the struct **marker**, per the spec's instance rule. Measured:

- the named contract now **evaluates** (`raw none: Array`; the runtime unify is gone) —
  but the collect answers `none`, so the assembled result needs a look;
- highlevel's `an_annotation_with_a_struct_value_rejects_a_literal_at_apply_time` fails:
  it asserts the diagnostic's expected value is the struct **value** (an Array), and the
  marker makes it a `TypeValue(TypeStruct)`;
- compute 260 and language lib stay green.

So the two spec rules pull apart for a struct type annotation: the annotation rewrite
puts the annotation *expression* in the type slot (the struct value), while the instance
rule says an instance's type *is* `Struct`. Which one the annotation slot follows decides
whether the named-type use is a type slot or an identity — a ruling.

### Measured: the spec's own unify already accepts it

`S := {.x 1}; r := (S = Struct)` answers **1** (`zz_dynamic_contract_probe`,
`probe_a_struct_kind_unify`): the spec's directed rule `a = b` is `a <: b`, and a struct
value's type is `Struct`, so the language-level `=` accepts the struct value against its
kind. The runtime refusal is therefore **not a missing language rule**: it is the engine's
own binding in `crates/lichen-lowlevel/src/function.rs` (`apply`):

```rust
// The call's own result type is the applied return's type cell.
if let Some(cell) = cell
    && let Some(codomain) = self.expr_type_item(instantiation.applied)
{
    self.evaluate_node(codomain, Some(block));
    self.unify_ref(cell, codomain);
}
```

The call's result cell holds the annotation's type value (a named struct type) and the
applied return's type item is the instance's marker (`Struct`), so the structural
`unify_ref` fails: `expected raw[Table, TypeId(51), …], found Struct`. Skipping the
binding when the cell is decided removes the refusal but fails
`a_call_result_annotation_is_checked_against_the_return_type` and its direct variant — the
binding **is** the annotation check. So the engine's `unify_ref` must follow the same
directed subtype the language's `=` follows; that is a correction of the engine's unify,
not a new mechanism.
