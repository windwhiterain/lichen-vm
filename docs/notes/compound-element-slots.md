# Compound element slots

> Status: **in progress.** `LowValue::Any` has landed (the lowlevel crate's
> wildcard, its codec tag, and the two printers). The element-slot layout of
> §*The element slot is a full term* is what the first step — the **tuple** —
> implements; §*What is open* holds the two questions that layout does not
> answer by itself. The step order is tuple, struct, array, then table/set.
>
> Companion: [function-type-merge](function-type-merge.md) is the same refactor
> for the function kind, and it is the precedent every choice below is read
> against.
>
> Points at: `crates/lichen-lowlevel/src/{lib,codec,equality,evaluation,table}.rs`
> (`Any`), `crates/lichen-highlevel/src/checker/tuples.rs` (both construction
> sites), `crates/lichen-highlevel/src/shape.rs` (the element-slot readers),
> `crates/lichen-render/src/render/{type_printer,value_printer}.rs` (`@any`).

## `Any`, the wildcard

A written compound type is one node that many values are checked against, so its
slots must not bind. A plain placeholder (`struct<_:Int>`) does bind: the cell is
one cell, the first value unified against it is written into its class, and every
later use reads that value — which is why the alternative was a type lambda
(`type = _x => struct<_:Int>; (type _)`) instantiated per use. `Any` is the
constant that removes that burden:

- **Unification.** A class holding `Any` unifies with anything, and *neither
  merges the classes nor writes a value* (`Module::unify_inner`, checked before
  the undecided/decided write arms). The wildcard node therefore stays reusable,
  and nothing is ever bound by it.
- **Every other reader** treats it as **undecided**: evaluation answers no value
  (`evaluate_node_body`), it states no low type, a table key that is `Any` is not
  key content (`table.rs`), and both printers spell it `@any`.
- **Codec.** Tag 9, additive; tag 4 stays reserved for the deleted marker.

## The element slot is a full term

A compound type's element slot holds the element's *type value* alone today, so
an element's own value and attribute have nowhere to sit: `type_denotation`
strips the annotation, and a perspective written on a type's element is dropped.
Measured on the tree before this change:

```text
(1 # 2, 2) : <Int # 2, Int>   =>  (1, 2): <Int, Int>
(1 # 4, 2) : <Int # 2, Int>   =>  (1, 2): <Int, Int>
```

Both check, and neither carries the `# 2` anywhere. The refactor makes every
element slot an expression's term — `[value, type, attrs…]` — and keeps the
kinded wrapper `[ [slot…], [marker, K] ]`, with one construction site per kind.

The **sugar** puts the whole annotated type expression into the *type* slot and
`@any` in the *value* slot; the full syntax writes the slots out:

```text
<Int # 2, string>   ≡   tuple{ @any : (Int # 2), @any : string }
  element 0 = [ @any, [int_marker, Type, <perspective 2>] ]
  element 1 = [ @any, [string_marker, Type] ]
```

A **tuple literal**'s own type takes the literal's values ("unify the literal's
values directly"), so its type is the tuple itself, annotated with each element's
type and attributes:

```text
(1 # 4, "a")
  element 0 = [ 1,   [int_marker, Type, <perspective 4>] ]
  element 1 = [ "a", [string_marker, Type] ]
```

An **array literal**'s element slot is one slot shared by every element, so it is
built with `@any` in the value slot first, and each element's value and type are
unified onto it.

Two consequences follow from the layout, and both are deliberate:

- **A pin is enforced by the lowlevel unify.** `tuple{5 : Int}` against a value
  whose element is `1` is `5` against `1`: the ordinary conflict, no checker
  machinery.
- **An element attribute compares structurally**, because the sugar nests the
  annotated type term whole (see §*What is open*).

The readers follow the slot: a field's type is the slot's element 1
(`shape::field_type`), the low-type decode descends the slot's element 1
(`shape::low_type_of`), and a raw read of a type value (`X<0>`, `X::a`) yields
the slot's element 1 with its own kind as the read's type. The interim is visible
in `shape`/`checker::indexing`: the tuple slots are terms while the struct slots
are still bare type values, so the shared raw-read helper states which layout its
container has until the struct step lands.

## What is open

- **A value's element value slot.** The literal rule above makes a tuple's type
  carry its values, so two tuples with different values have different types:
  `[(1, 2), (3, 4)]` (legal today, `array<<Int, Int>, 2>`) would unify
  `[1, Int], [2, Int]` against `[3, Int], [4, Int]` through the array's element
  cell and conflict, and so would `f = x => (x, 1); [f 1, f 2]`. The function
  precedent answers the same question the other way: a function's type carries
  its return's *type* and never its value, which is exactly why `[f1, f2]`
  unifies. Either accept the restriction, or keep an element's value slot `@any`
  in a value's type — the type is then the compound's shape — and enforce a pin
  by a checker walk over the value's own element terms, the analogue of the
  apply's argument-pair unify.
- **The element attribute comparison.** The sugar's nesting makes it structural:
  `# 4` against a written `# 2` is refused, where the perspective lattice says
  `4 ⊑ 2` (a value uniform over 4 threads satisfies 2). The function precedent,
  and every other attribute check in the checker, goes the other way: the
  declared value is kept out of the pair, the pair's slot is a binding target,
  and `AttrExt::unify_slots` compares the two by marker with the lattice's
  subtype relaxation. Matching it here means the sugar writes `@any` into the
  slots it does not constrain and the requirement lives on the element
  expression, compared by `unify_type_attributes` descending the elements.
