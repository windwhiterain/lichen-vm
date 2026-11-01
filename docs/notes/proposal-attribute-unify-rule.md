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
> In all three cases below, the attribute positions themselves **do not join a
> class**; "decided" adopts the stated expression without a class union. When two
> expressions are unified, an attribute position is:
>
> - **compared**, when both sides state an attribute there: the attribute's own
>   order decides (for the perspective, `requirement ⊑ provider`, `declared |
>   provided`), with the **actual** as the provider and the **expected** as the
>   requirement;
> - **decided**, when exactly one side states one: that side's attribute
>   *expression* becomes the value's claim, adopted without a class union. In a
>   statement `actual = expected`, the expected (annotated) side states it, and the
>   actual takes it;
> - **left alone**, when neither side states one: no write, no class union.
>   Such a position is **open**: it is *marked undecided*, not a plain hole, and it
>   carries no claim, because a reader treats only a *stated* position as a
>   certificate (§3).

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

> A reader reads three states, never two:
>
> - **stated** — a decided value that is not `@any`, or an operation whose value is
>   still to be resolved (a reference / lazy certificate);
> - **open** — *marked undecided* on the expression: not a certificate;
> - **no claim** — a plain hole, or the wildcard `@any`. `@any`'s unify behaviour
>   stays entirely §Any's; it is only excluded from being read as a certificate.

"Skip every hole" is not the rule: it deletes the lazy certificate the `write`
shape needs. And "a plain hole is not a claim" is what keeps a certificate from
being invented where the body states nothing.

**The marker's carrier is not yet decided.** An open position must be
distinguishable from a plain hole, and no existing representation says which is
which: an expr's attribute layout names canonical positions, not which of them are
open. Candidates are (a) an entry in the expression's attribute layout registry, or
(b) a checker-side table beside `Module::struct_identity_items`. (a) is persisted and
would bump `ARTIFACT_FORMAT_VERSION`; (b) is not persisted, so a loaded artifact
would lose it. This must be settled before implementation, and §6's "no persisted
shape changes" is therefore a **goal**, not a guarantee.

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
| 4 | `id := x => x + 1`<br>`(id 0) # 32` | *sanity only* — a callee that states nothing hands no certificate | **accepted**; but this row does **not** discriminate: the correct path and the misreading both land on the argument's top (`0`) |
| 4b | `id := x => x + 1`<br>`((id # 1) 0) # 32` | the reader gate: with no certificate the call's provider is the callee's own claim, so it should refuse `expected 32, found 1` | **accepted** — it does not discriminate either, and that is a **new finding**: the annotation on the callee's reference never reaches the call's provider, so the provider is the argument's `0`. See §7 |
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
perspective tests get their certificate path. Persisted shape changes **only if**
§3's carrier is option (a); with a checker-side table (b) nothing persisted moves,
at the cost of a loaded artifact losing the marker. Re-measure after the change,
**both directions and both paths**:

- template pinning, dynamic **and** frozen: `f (5 # 8)` then `f (5 # 2)`, and the
  reverse order `2 → 8`;
- the markerless parameter (`id (5 # 1)` answers `# 1`) and row 4/4b's silent callee;
- `attributes.md` §Behaviour's compound rows and §"Annotation over an existing
  attribute";
- the whole compute and engine suites.

## 7. Separate slice: two deviations the rows exposed

**The pardon pass.** `Checker::build` pardons **any** failed unify whose root is
registered in `Checker::attribute_nodes`, via `ext.is_subtype(root_a, root_b)` — the
opposite direction from the printed acceptance table. Registering an annotation's
constraint slot there made a `32` against `1` failure vanish outright. Until that
pass's direction and root reach are settled, **no v15 implementation may route
through a new `attribute_nodes` entry**, and the pass gets its own measurement.

**A callee's own claim never reaches the call's provider (measured, row 4b).**
`((id # 1) 0) # 32` is admitted, at the tip, while the design says a call's provider
is its own certified attribute or the meet of what it is made of — `gcd(1, 0) = 1`,
which `# 32` must refuse. So either the annotation on a callee's *reference* is
dropped before the provider combine, or the combine for an apply does not reach the
function operand. This is not part of v15's sentence, but it is the row the reader
gate was supposed to be guarded by, so it needs its own reading before §5 row 4b
can serve as a gate.
