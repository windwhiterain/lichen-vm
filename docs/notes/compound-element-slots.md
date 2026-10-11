# Compound element slots

> Status: **in progress.** `LowValue::Any` has landed and is merged
> (`8b2fa7a`); the **tuple** step has landed on `feature/compound-element-pairs`
> — the element slot is a full term, both construction sites, the readers, and
> both printers. The step order is tuple, struct, array, then table/set; the
> element-attribute *comparison* and the surface syntax (`@any`, `tuple{…}`,
> `struct{…}`) are the tuple step's remaining work.
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
- **A value reader** treats it as **undecided**: evaluation answers no value
  (`evaluate_node_body`), it states no low type, a table key that is `Any` is not
  key content (`table.rs`), and both printers spell it `@any`.
- **The concreteness verdict alone** takes it as **decided**
  (`evaluate_node_deep_inner`). That is what lets a structure a wildcard sits in
  be proven concrete: a written compound type is content-complete, so an apply
  clone references it **in place** instead of copying an array whose own value no
  deep pass ever wrote. Without the rule every wildcard-bearing type is unproven,
  and both a printer reading it and an assert condition over it answer nothing.
- **Codec.** Tag 9, additive; tag 4 stays reserved for the deleted marker.

## The element slot is a full term

A compound type's element slot used to hold the element's *type value* alone, so
an element's own value and attribute had nowhere to sit: `type_denotation`
stripped the annotation, and a perspective written on a type's element was
dropped. Measured on the tree before this change:

```text
(1 # 2, 2) : <Int # 2, Int>   =>  (1, 2): <Int, Int>
(1 # 4, 2) : <Int # 2, Int>   =>  (1, 2): <Int, Int>
```

Both checked, and neither carried the `# 2` anywhere. Every element is now an
expression's term — `[value, type, attrs…]` — inside the kinded wrapper
`[ [slot…], [marker, K] ]`, with one construction site per kind.

The slot's **width is dense over the attribute order**: `2 + |P::Attr::ORDER|`
positions, `shape::slot_width`. The syntax stays sparse; the width does not,
because a value's slot list and a written type's are unified position by
position, and no value can know how many attributes a written type will spell.

```text
<Int # 2, string>   ≡   tuple{ @any : (Int # 2), @any : string }   # full syntax
  element 0 = [ @any, int_type,    <perspective 2>, @any, @any ]
  element 1 = [ @any, string_type, @any,            @any, @any ]

(1, "a")   # a literal's type is the compound's shape: every value slot `@any`
  element 0 = [ @any, int_type,    @any, @any, @any ]
  element 1 = [ @any, string_type, @any, @any, @any ]
```

Decisions that shape this layout:

- **A value's element value slot is `@any`.** The type is the compound's
  *shape*, so `[(1, 2), (3, 4)]` and `f = x => (x, 1); [f 1, f 2]` stay legal
  (measured before the change as `array<<Int, Int>, 2>`), and one written type
  stays shareable across values that differ. The function precedent answers the
  same question the same way: a function's type carries its return's *type*, never
  its value. A written **pin** therefore rides in a written type's slot and is
  enforced by a checker walk over the value's own element terms, not by the
  lowlevel structural compare.
- **An element's attributes are carried, not yet compared.** The dense attribute
  positions of a written type's slot hold the element expression's own attribute
  slots (`checker::element_attr_slots`), so a written perspective is no longer
  dropped. The value's side holds `@any`, so the lowlevel compare is vacuous by
  construction; the requirement is compared by marker through `unify_slots`, the
  same way the function kind's declared parameter attribute is — that walk is the
  tuple step's next piece.

The readers follow the slot, and the layout is transitional until the struct step
lands:

- `shape::slot_type` reads an element's type position; `shape::field_type` takes
  a `ShapeElements` its caller states — `Slots` for a tuple type's shape,
  `Types` for a struct's — so no reader infers a layout from a node that may not
  be evaluated yet.
- `shape::low_type_of`'s tuple arm decodes each element through its slot;
  `checker::indexing::element_read` reads a slot container's component one level
  in; `checker::structs::slot_read`'s **lazy** fallback does the same, which is
  what a resolved read and an apply-time one must agree about.
- Both printers render a tuple's elements through their slots, and an unreadable
  slot falls back to the raw mark rather than printing an element pair whole.

## What is open

- **The element-attribute comparison.** See the second decision above: the
  requirement is carried, the comparison is a checker walk
  (`AttrExt::unify_slots` per marker) that is not written yet. Until it is, a
  written `<Int # 2>` accepts `# 4` silently — as it did before the refactor,
  when the attribute was dropped outright.
- **The surface syntax.** `@any` is not a token, and `tuple{…}` is not a
  production, so a slot cannot be written out yet; both construction sites build
  the layout internally. `@any` must be a *fresh* node per spelling whose type
  cell is the annotation's, so `@any : Int` binds through the ordinary annotation
  path.
- **The struct, array, and table/set steps.** Each repeats the tuple's shape: a
  construction site, the readers, the printers, and its own wide tests.
