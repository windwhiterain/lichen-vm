# The deferred instantiation: what the reorder checks, and what it cannot

> Status: **proposed — an owner-level design, no code changed by this note.** Three
> audit items are one missing check: when a struct type reaches an instantiation
> through a **parameter**, the reorder that places the arguments resolves at the
> apply, and only the argument's **type** takes part (`P1-45`'s lookup key). The
> field's **arity** (`P1-46`), its **spelling pin** and its **perspective**
> (`P1-44`), and the argument **names** against the field names (`P1-47`, fixed on
> the sentence side) are not compared. This note states the measured behaviour, why
> the checker cannot check it where it stands, what the engine can and cannot do,
> and the two designs left. It asks the owner the questions at the end.

## 1. What is measured

All at `dev@ca3bbcb2`. The **written** form checks everything the spec says; the
**deferred** form (a parameter callee) checks the type only:

```lichen
S = struct{.a Int # 4, .b Int}
S(.a 1 # 2, .b 2)        -- refused: `expected 4, found 2`      (perspective)
S(.a 5 # 4, .b 2)        -- accepted: `(5, 2)` — the perspective is
                         -- uniformity, not the value
S(.a 1, .b 2, 3)         -- refused: `too many fields ...`      (arity)
S(.a 1 # 4,)             -- refused: `missing field b ...`      (arity)

g = c => c(.a 1 # 2, .b 2)
g S                      -- accepted: the `# 2` is never compared
g = c => c(.a 1, .b 2)
g S                      -- accepted, and `S`'s fields are two: correct
g = c => c(.a 1)
g S                      -- accepted with a one-element instance (P1-46)
g = c => c(.z 1)
g S                      -- the miss names the arguments (`no argument is named a —
                         -- the arguments are named z`), which is P1-47's fix
```

The **pin** (`S = struct{.a 5 : Int}` ; `g = c => c(.a 6)` ; `g S`) is accepted the
same way, and `v = g S; v.a` answers `6` — the pinned `5` is not enforced on this
path.

## 2. Why the checker cannot check it where it stands

- The written form's checks are **syntactic**: `check_element_pair_attributes` and
  `check_element_pair_values` pair `ExprId`s of the written `struct{…}` and the
  argument list. When the callee is a parameter there is no written field list; the
  field slot exists only as a lazy read out of the callee's type, undecided until the
  apply binds it.
- A **check-time unify against an undecided read is vacuous** — it binds the read
  instead of refusing. This was measured twice: a per-field type unify before the
  list unify changed nothing (P4-12's first attempt), and it needed the read **forced**
  (`evaluate_node_deep`) before the unify could refuse (P4-12's fix). At a deferred
  callee the read cannot be forced: nothing decides it until the apply runs.
- So the check must happen **where the field is decided** — at the apply, per clone —
  which is the engine's `apply_parameter_check`, or a node the engine re-evaluates
  per call. The assert channel is that node: an assert registered on the template is
  re-instantiated per clone (`function.rs`) and re-evaluated, and an undecided
  condition is left pending rather than failed. But it fires **only if the value's
  evaluation reaches it**: P1-47's first route (a membership assert over the field
  names) never fired, because the reorder's lookup miss aborts the instance's value
  first.

## 3. What the engine can and cannot do

The engine (`lichen-lowlevel`) is **generic over the program**: it knows `LowValue`,
`LowOperator` and the `[value, type, attribute…]` pair layout, and nothing about
`Perspective`, `Refinement` or any other attribute's semantics — those are
`AttrExt` in `lichen-highlevel`, and the engine may not call them. So an
engine-side check can compare **values** (its `key_eq`/`value_eq`, the wildcard
`LowValue::Any` is known there too), but it cannot apply `declared ⊑ value` or a
pin's type-level rule without help.

## 4. The two designs left

**Design A — the engine validates a struct instance crossing an apply.** In
`apply_parameter_check`, when the parameter's type is a struct kind: walk the
parameter's type shape (the field slots) and the argument's value array by position.
Skip a slot whose value element is the wildcard (`@any` — the rule
`compound-element-slots.md` states). A field whose value element is spelled is a
**pin**: compare it with the argument's element by content (`key_eq`); a difference is
a new `UnifyError`-style record whose sentence names the field and the two values —
the first engine-side diagnostic kind, so the *shape* of this record is the owner's
call. Arity falls out of the same walk (the array's length against the shape's).
Attributes need the checker: the engine can *report* "element j differs" only for the
value elements it is allowed to read, so the perspective half would remain the
checker's.

**Design B — the checker registers a per-call assert that the engine evaluates once
the reorder resolves.** For each argument, build a condition over the *supplied*
argument's attribute slot against the field's attribute slot at the reorder's
resolution (the argument's slot is chosen at run by the reorder's table, so the
condition's operands are lazy). The attribute comparison itself cannot be a plain
`Eq`: `Perspective`'s relation is `declared ⊑ value`, so the condition must be a node
that computes the divisibility test (`Rem` + `Eq` are `TypeOperator`s) or a new
numeric operator. This is more work than A and still needs a sentence; its advantage
is that the attribute semantics stay in the checker layer.

**Recommendation**: A for the pin and the arity (both are pure structural facts the
engine already has), and the **written form only** for the perspective — i.e. state
in the spec that a perspective on a deferred instantiation's argument is not checked,
which is the current behaviour, rather than invent a runtime divisibility grammar.
If the owner wants the perspective checked too, B is the only route that keeps
`Perspective`'s meaning in one place.

## 5. Questions for the owner

1. Does the engine own **structural validation of a struct instance** (Design A), or
   is that the checker's contract to keep, with the deferred spelling left unchecked?
2. Is a **new engine-side diagnostic kind** (naming a field and the two values)
   acceptable? No engine-side diagnostic currently names a field.
3. If the perspective must be checked (Design B), is a runtime divisibility node
   acceptable — and does it belong with `Perspective`'s `AttrExt` as a `constraint`?

## 6. What this note changes

Nothing: it records the design. The three items keep their own sections in
[code-audit](code-audit.md) (`P1-44`, `P1-46`, `P1-47`) and point here for the
shared route; `P1-45` and `P1-47`'s sentence side are fixed.