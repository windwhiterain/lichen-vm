# Proposal v15.1: the unify rule for an expression's attribute positions

> Status: proposal (v15.1, revised per review) — needs the maintainer's ruling.
> Points at: `docs/language-spec.md` §Lowlevel, §Any, §Annotation, §Clone;
> `crates/lichen-highlevel/src/checker.rs` (`slot_expr`, `stated_attributes`,
> `stated_element_slot`, `Checker::build`'s pardon pass), `checker/annotations.rs`
> (`adopt_stated_attributes`, `clear_wildcard`), `checker/templates.rs`
> (`check_unify_expr`), `docs/notes/attributes.md`, `docs/notes/array-attribute-view.md`,
> `docs/notes/handoff-remaining-six.md` rounds 28–30.

## 1. The gap

The SPEC states what an **annotation** does to its own slot (§Annotation), what
`@any` means under unify (§Any: always succeeds, merges only when the other side is
undecided), that a class shares one value (§Lowlevel), and how a clone decides
scope (§Clone). It never states the rule for **two expressions' attribute
positions meeting under unify**. Every implementation choice so far is a deviation
or a workaround, and each was measured:

| representation of an unstated attribute position | why it is insufficient |
|---|---|
| the wildcard `Any` (`slot_expr`'s `self.any()`) | decoupled, but no statement can decide it — this is the defect |
| undecided, `@raw None` (§Undecided's own form) | decidable, but it joins the other side's class: **107/134** with `compute.parallel: a definition cycle …`, and the same **107/134** restricted to value exprs, so the coupling is intrinsic to full-width exprs, not to type values |
| empty it on demand (`Module::clear_wildcard` + `adopt_stated_attributes`) | works, but adds a second writer of an occupied slot (`write_node_value`'s invariant) and cannot tell "not stated" from explicit `@any` |
| carry it on the declaration (`Function::return_attribute`, read first by `check_app`) | **238/3** — the body's own lowlevel unify cannot see a checker-side fact |
| re-point the value's expr instead of erasing (no lowlevel writer) | **238/3**, the same regression |

So no choice of *value* is right: the missing piece is a **rule**, and it needs a
third state that is neither `Any` nor a plain hole — an **open position**.

## 2. The sentence proposed (appended to §Lowlevel)

> Attribute positions pair by their **canonical index** — an array's layout names
> which canonical attribute each position carries — never by dense slot order.
> When two expressions are unified, an attribute position is:
>
> - **compared**, when both sides state an attribute there: the attribute's own
>   order decides (for the perspective, `requirement ⊑ provider`, `declared |
>   provided`), with the **actual** as the provider and the **expected** as the
>   requirement; neither slot joins the other's class;
> - **decided**, when exactly one side states one: that side's attribute
>   *expression* becomes the value's claim. In a statement `actual = expected`,
>   the expected (annotated) side states it, and the actual takes it;
> - **left alone**, when neither side states one: no write, and **no class join**.
>   Such a position is **open**, not undecided: it carries no claim, and it must be
>   distinguishable from both `@any` and a plain hole, because a reader treats only
>   a *stated* position as a certificate (§3).

`@any` keeps §Any's exception unchanged; this clause governs only positions that
are neither stated nor `@any`. Cross table:

| left \ right | stated | open | `@any` |
|---|---|---|---|
| **stated** | compared by the order | the open side takes the stated expression | §Any: success, no merge |
| **open** | the open side takes the stated expression | *left alone*, no class join | §Any |
| **`@any`** | §Any | §Any | §Any |

This **supersedes** `attributes.md` §"An attribute slot is unified like any other
slot" — the recorded verdict "no skip, no second dispatch" is exactly what a
`no class join` clause contradicts. On approval that paragraph and
`array-attribute-view.md`'s §"The isomorphism" must be rewritten to this rule, not
kept beside it.

## 3. The reader rule (part of the same change, not optional)

"States something" must be defined for **readers**, and today it is not:
`Checker::stated_attributes` skips only `Any` and treats a bare hole as stated, and
`stated_element_slot` does the same. If the open state is undecided, every
unstated position would become a certificate. So:

> A reader treats a slot as **stated** when it holds a value, or holds an operation
> whose value is still to be resolved (a reference/lazy certificate). An **open**
> position is undecided *and declares itself open on the expression*; it is not a
> certificate. A bare hole is never a claim.

The distinction cannot be "skip every hole" — that deletes the lazy certificate
the `write` shape needs.

## 4. Templates and clones: one rule, one boundary

The rule is **uniform**; it is not split into "templates" and "clones":

> A definition-time statement may decide a **template's** open position — that is
> what `r = (r # 32)` is. An **apply** binds only the per-call clone's position,
> never the template's own parameter position.

Not pinning a template is therefore §Clone's instantiation boundary, not a second
scope on attribute unify.

## 5. Measured rows

Measured at the tip (`89b325ee`) with `zz_attribute_certificate_probe`; the
"expected" column is what the sentence above requires.

| # | example | expected | measured actual |
|---|---|---|---|
| 1 | `g := x => {r := x; r = (r # 32)}`<br>`(g 0) # 64` | the statement's unify is **accepted and `r` carries 32**, so the program is refused `expected 64, found 32` | refused `expected 64, found 32` — but only because `adopt_stated_attributes` erases the wildcard first; without it, admitted |
| 2 | `f := (x : Int # 32) => x`<br>`f (0 # 64)` | accepted (`32 \| 64`) | **accepted** |
| 3 | `f := (x : Int # 64) => x`<br>`f (0 # 32)` | refused `expected 64, found 32` | **refused** `expected 64, found 32` |
| 4 | `id := x => x + 1`<br>`(id 0) # 32` | a callee that states nothing hands **no** certificate, so the demand is the slot | **accepted** — the control against every call suddenly gaining a certificate |
| 5 | two expressions that state nothing, unified (e.g. `p := {.a @any : Int}` / `k := (x = p) => x` / `k (p{.a 1})`) | no class coupling; both stay open | couples once the wildcard is a hole: **107/134** |

Row 5's number is a suite reading, and its mechanism (a full-width unify joining
the positions) is the reason the clause is about class joining. The "both stated
also do not join a class" half of the rule is **contractual, not yet measured**: no
program writes one side after a unify, so the observable is that a later
contradicting unify is judged against the requirement (row 3) instead of being
silenced — which the current class-joining implementation can also produce, so the
rule is a declaration of intent there.

## 6. Impact if approved

The open state becomes representable instead of the wildcard fill; the erasure
(`clear_wildcard`, `adopt_stated_attributes`) is withdrawn; the two remaining
perspective tests get their certificate path. No persisted shape changes.
Re-measure after the change, **both directions and both paths**:

- template pinning, dynamic **and** frozen: `f (5 # 8)` then `f (5 # 2)`, and the
  reverse order `2 → 8`;
- the markerless parameter (`id (5 # 1)` answers `# 1`) and row 4's silent callee;
- `attributes.md` §Behaviour's compound rows and §"Annotation over an existing
  attribute";
- the whole compute and engine suites.

## 7. Separate slice: the pardon pass

`Checker::build` pardons **any** failed unify whose root is registered in
`Checker::attribute_nodes`, via `ext.is_subtype(root_a, root_b)` — the opposite
direction from the printed acceptance table. Registering an annotation's
constraint slot there made a `32` against `1` failure vanish outright. Until that
pass's direction and root reach are settled, **no v15 implementation may route
through a new `attribute_nodes` entry**, and the pass gets its own measurement.
