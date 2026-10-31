# Proposal (v5): the template match fills fresh cells; pins are deferred to the bind; attributes keep their own direction

> Status: **proposal — for review, not applied.** Supersedes v4 (`f45c6a6b`). It
> answers the three remaining blockers: a deferred pin with a later-bind recheck, the
> attribute relation's actual direction, and how the runtime match gets its roles. The
> value-templates confirmed rulings are kept. The spec file is untouched.

## 1. The match rule: text proposed for §Function, after the apply reduction

> A **template match** is `a = t` where `t` is a template (a direct template value, a
> parameter pattern, or a call-result template). The match reads `t`'s expression
> children, and for each position:
>
> - the **type** is `t`'s type child's denotation; it is shared and checked;
> - the **attributes** are `t`'s attribute children; they are shared, and the match
>   applies **the attribute's own relation with the declaration on the left**
>   (`declared ⊑ value` for `Perspective`), never a plain `a <: t`;
> - the **value** child: a **closed** value is a **pin** — `a`'s value must equal it; an
>   **open** value (`@any`, an empty cell) is a fresh empty cell for this use, filled by
>   `a`; a **type expression** states the type requirement and carries no value;
> - a **group** child is matched position by position by the same rule.
>
> The match's answer is `a`; `t` is never specialized and is never a place's value. A
> read of a matched place answers `a`'s value for that part.

## 2. The deferred pin: text proposed after the match rule

> A pin whose `a` is **not yet decided** is a **deferred requirement**: the match records
> it on the place's class and does not install the pin's value. When the class binds a
> value, the pin is checked against it and a mismatch is refused with the same
> `expected <pin>, found <value>` the decided case reports. An instance never pre-fills a
> place from its declaration.

Acceptance semantics (the review's counterexample):

```lichen
S := {.a 5 : Int}
x := _
s := S{.a x}     // x stays open; 5 is recorded as the place's pending requirement
x = 5            // accepted; s.a reads 5
```

```lichen
S := {.a 5 : Int}
x := _
s := S{.a x}
x = 6            // refused: expected 5, found 6
```

## 3. Where the deferred requirement lives, and who checks it

The record is **data the checker places on the class**, not a lowlevel guess:

- The checker knows the roles (a type expression, a closed pin, an open cell, a group)
  and the attribute relations, so it resolves the type and attribute aspects and, for an
  open actual, registers the pin on the place's class: `pending_pins: class → value`.
- The runtime's bind (`write_node_answer`/the class write) consults `pending_pins` for
  the class it fills and reports a mismatch through the existing unify-error channel.
  The comparison is **closed-value equality**, which the lowlevel already has
  (`value_expr_equal`); no `Perspective`/`Refinement` knowledge is required at run time
  because the attribute relation was checked at the checker.
- No lowlevel predicate for "type expression vs pin" is needed: the checker classified
  each child and registered only the pins.

## 4. §Struct: the reduction text is unchanged

`raw v[...] = e1` is the match; `e = raw v[...]` fills the place. The rule above states
what the match does with the template's value; no symbolic change is needed.

## 5. Paths

| path | what happens |
|---|---|
| `check_instantiate` (`checker/structs.rs`) | per field, resolve the type from the type child and the attributes by their own relation; a closed value child pins; an open child registers a pending pin and leaves the place open; a group recurses. The decided branch's whole-term `check_unify` is replaced by this resolution; the undecided branch's type/attribute work stays and the pin becomes its deferred case |
| runtime apply (`apply.rs`) | the parameter pattern's slots: a closed slot registered as a pending pin is checked at the bind by value equality; an open slot is filled; groups recurse. The type/attribute work is the checker's and unchanged |
| library wrappers | no edit: `r = declaration` is a match, and the value-templates rulings are kept |
| reads | unchanged: a place answers the matched value's part; an open part is undecided until its bind |

## 6. Acceptance examples

1. **Deferred pin** (§2): `x = 5` accepted, `x = 6` refused with `expected 5, found 6`,
   the instance never pre-filled.
2. **Attribute direction**: requirement `# 4` against actual `# 8` is **accepted**
   (`declared ⊑ value`); against `# 2` it is **refused** — the checker and the runtime
   apply agree, and the existing test (`struct_element_attributes`) keeps its outcome.
3. **Type parameter**: `id = (T : Type) => T; id Int` fills `T` with `Int`.
4. **`@any : Int` config**: `.scale`'s type child `Float` is checked, its `@any` is a
   fresh cell, `s.scale` fills it; the declaration adds no value.
5. **`plrun` codomain**: the codomain's open buffer fields are fresh cells filled by the
   recorded result; its closed `.length` pins check; `first.r` answers the recorded
   wrapper. `r : type_of (k.O); r = k.O` stays as the ruling states.

## 7. Rulings kept

`=` is the only template match; `:` is not a match; a direct template is instantiated per
use (open slots fresh; types, attributes, name table, nominal id shared); the call-result
template stays `r : type_of (k.O); r = k.O`; the annotation rule is untouched; the
attribute extensions keep their own relations (the match applies them, it does not
replace them).
