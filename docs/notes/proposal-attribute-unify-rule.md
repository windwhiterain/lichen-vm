# Proposal v15: the unify rule for an expression's attribute positions

> Status: proposal — needs a review, then the maintainer's ruling.
> Points at: `docs/language-spec.md` §Lowlevel, §Any, §Annotation;
> `crates/lichen-highlevel/src/checker.rs` (`slot_expr`, `stated_attributes`,
> `Checker::build`'s pardon pass), `checker/annotations.rs`
> (`adopt_stated_attributes`, `clear_wildcard`), `checker/templates.rs`
> (`check_unify_expr`), `docs/notes/handoff-remaining-six.md` rounds 28–30.

## The gap

The SPEC states what an **annotation** does to its own slot (§Annotation), what
`@any` means under unify (§Any: always succeeds, merges only when the other side is
undecided), and that a class shares one value (§Lowlevel). It never states the rule
for an expression's attribute positions when two expressions are unified. Every
implementation choice for that rule so far is a deviation or a workaround:

| representation of an unstated attribute position | where it lives | why it is wrong |
|---|---|---|
| the wildcard `Any` | `slot_expr`'s `self.any()` | decoupled, but a statement can never decide it — the whole defect |
| undecided (`@raw None`) | §Undecided's own form | decidable, but it joins the other side's class, and unrelated values become one |
| empty it on demand | `Module::clear_wildcard` + `adopt_stated_attributes` | works, but adds a second writer of an occupied slot (`write_node_value`'s invariant) and cannot tell "not stated" from explicit `@any` |

## The sentence proposed

Append to §Lowlevel (after the class rule):

> Two expressions' attribute positions do not join a class. An attribute position
> is **compared** when both sides state something there, and **decided** when one
> side states something and the other states nothing; when neither states
> anything it stays undecided and couples nothing.

The three clauses in their measured form:

| # | example | expected | actual |
|---|---|---|---|
| 1 | `g := x => {r := x; r = (r # 32)}`<br>`(g 0) # 64` | admitted — undecided against `32` is the ordinary unify write, no erasure | admitted today **only because** `adopt_stated_attributes` empties the wildcard |
| 2 | `a := (1 # 4)`<br>`b := (2 # 6)`<br>`c := (a, b)`<br>`(c(0) + c(1)) # 5` | refused `expected 5, found 2` (the meet of `4` and `6`, via the combine) | refused — the control, unchanged by this proposal |
| 3 | two expressions that state nothing, unified: `p := {.a @any : Int}`<br>`k := (x = p) => x`<br>`k (p{.a 1})` | no class coupling: `p`'s and `x`'s attribute positions stay unstated | couples once the wildcard is a hole — measured across the suite as **107 / 134** with `compute.parallel: a definition cycle: NodeId(…) is lowered while it is already being lowered` |

Row 3 is why the sentence is about **joining a class**, not about which value
fills the position: nothing in the SPEC's vocabulary expresses "unstated and
decidable but non-joining", so no choice of value can be right.

## Why a value cannot fix it (measured twice)

- Filling every unstated position with a hole instead of `Any`: **107 / 134**, with
  definition cycles, because a full-width expr unifies all its positions.
- The same, restricted to **value** exprs (type values keep the wildcard):
  **107 / 134** — so type values are not the cause; the coupling is intrinsic to
  full-width value exprs.
- Carrying the claim on the declaration instead (`Function::return_attribute`,
  read first by `check_app`): **238 / 3** — the body-requirement test regresses,
  because the *lowlevel* unify that the body needs cannot see a checker-side fact.
- Re-pointing the value's expr at a new one (no lowlevel writer at all): **238 / 3**,
  the same regression.

## Rejected alternatives

- **Enshrine the wildcard and keep erasing.** Makes the deviation the contract,
  and needs `write_node_value`'s invariant restated to license a second writer.
- **Carry the certificate only on the declaration.** Cannot serve a body whose
  claim is a *statement*: the applied clone must materialise the claim for a
  field reference, and measured a call site otherwise reads the argument's meet —
  frozen `compute.write` prints `expected 0, found 1` where `.to` states `32`,
  while the same shape inline with a dynamic callee prints `expected 0, found 32`.

## What the ruling must also settle

Whether the rule applies to the **template's** own positions or only to a
**clone's**. `attributes.md` records that a template must not be pinned by a call
(`f (5 # 8)` then `f (5 # 2)` used to answer `expected 8, found 2`); the per-clone
open position may be bound. The sentence must say which.

## Impact if approved

The wildcard fill can become the SPEC's undecided form, the erasure
(`clear_wildcard`, `adopt_stated_attributes`) is withdrawn, and the two remaining
perspective tests have their certificate path. No persisted shape changes. Re-measure
after the change: the markerless-parameter family (`id (5 # 1)` answers `# 1`), the
template-pinning family (`f (5 # 8)` then `f (5 # 2)`), the compound-annotation rows
in `attributes.md` §Behaviour, and the whole compute/engine suites.

## Side finding, a separate slice

`Checker::build`'s post-pass pardons **any** failed unify whose root is registered
in `Checker::attribute_nodes`, via `ext.is_subtype(root_a, root_b)`. Registering an
annotation's constraint slot there made a `32` against `1` failure vanish outright.
That pass's direction and reach deserve their own measurement; it is not part of
this proposal, but no future `attribute_nodes` entry is safe until it is settled.
