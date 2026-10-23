# The deferred instantiation: what the reorder checks, and what it cannot

> Status: **superseded — decided by the maintainer's ruling, and both designs are
> rejected.** The ruling (recorded in
> [function-normalization](function-normalization.md) §3.7): *"normalize时函数类型检查
> 就应当确定。旧的依赖lazy的检查方式要被淘汰了。"* An instantiation whose callee
> struct type is not determined at normalize — `g = c => c(.a 1, .b 2)` — is
> **refused**; the lazy reorder that resolves at the apply is retired, not extended.
> So the designs below (§4) are moot, the questions (§5) are answered by the ruling,
> and the measured matrix (§0–§3) is kept as the reason: every defect this note
> records exists because a deferred check is absent, not late. The refusal itself is
> filed as `P1-49` in [code-audit](code-audit.md).

## 0. The two spellings at a glance

| fact the written form checks | written | deferred (a parameter callee) |
|---|---|---|
| arity (`too many fields` / `missing field`) | yes | **no** — the reorder iterates the arguments, so a short value is built (`P1-46`) |
| field type | yes | yes, as the lookup key's miss (`P1-45`) |
| pin (`5 : Int`) | yes | **no** — the instance carries the argument's value (`6`) |
| perspective (`# n`) | yes (relaxed, `declared ⊑ value`) | **no** |
| unknown argument name | yes (`no field named z`) | the miss's sentence names the arguments (`P1-47`, fixed) |

The one fact the deferred form does check is the **field type**, and it checks it as
a *key*: the supply table is keyed by `[name|rank, argument type]`, so a type no
supplied argument matches is a miss. Everything else the field states — its arity,
its pin, its perspective — never enters a key.

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

- The written form's checks are **syntactic**: `check_element_expr_attributes` and
  `check_element_expr_values` expr `ExprId`s of the written `struct{…}` and the
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
`LowOperator` and the `[value, type, attribute…]` expr layout, and nothing about
`Perspective`, `Refinement` or any other attribute's semantics — those are
`AttrExt` in `lichen-highlevel`, and the engine may not call them. So an
engine-side check can compare **values** (its `key_eq`/`value_eq`, the wildcard
`LowValue::Any` is known there too), but it cannot apply `declared ⊑ value` or a
pin's type-level rule without help.

## 4. What the ruling decides, and the two designs it removed

The ruling removes the problem rather than repairing the lazy route: the callee's
name table must be decided at normalize, and an instantiation that needs it earlier
is **refused there**. That makes the two designs this note had carried moot:

- **Design A (the engine validates a struct instance crossing an apply)** is not
  needed: no undetermined instance reaches an apply. It would also have asked the
  engine for a semantic role it has never had (a diagnostic naming a field) and it
  could not have checked the perspective, which needs `AttrExt`.
- **Design B (a per-call divisibility assert)** is not needed either: the perspective
  is checked in the written form, and the spelling that would need the runtime node
  is refused.

The work left is the refusal: the checker must diagnose an instantiation whose
callee struct type is undetermined at its normalize, and `lazy_named_instantiate`
(and the lazy reads it builds) must be retired. `P1-49` carries it.

## 5. What this note asked, and the ruling's answer

1. *Does the engine own structural validation of a struct instance?* — No: the
   checker refuses the undetermined spelling, so there is no instance to validate.
2. *Is a new engine-side diagnostic name acceptable?* — Not needed.
3. *Must the perspective be checked at run time?* — No: the written form checks it
   ([attributes](attributes.md)'s `declared ⊑ value`), and the undetermined spelling
   is refused instead of checked lazily.

## 6. What this note changes

Nothing in code; it records the design and now the ruling. The three items keep their
own sections in [code-audit](code-audit.md): `P1-44` and `P1-46` are subsumed by
`P1-49` (the refusal removes the silent acceptances), `P1-45` and `P1-47`'s sentence
side are fixed, and the ruling itself is `P1-49`.