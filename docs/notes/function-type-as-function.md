# A function's type is the function itself

> Status: **planned** — design agreed with superior; Phase 1 in progress on
> `feature/function-type-as-function`.
> Points at: `crates/lichen-highlevel/src/checker/lambda.rs` (`check_lam`'s
> arrow build), `crates/lichen-highlevel/src/shape.rs` (`is_function_type` /
> `class_holds_type`), `crates/lichen-lowlevel/src/equality.rs`
> (`unify_inner`, the `Program::defer_pending` sibling this adds),
> `crates/lichen-lowlevel/src/lib.rs` (`Program`), the printer, the codec,
> `low_type_of`.
> Supersedes the "attribute flow is a non-goal" line in
> [`attributes.md`](attributes.md) (Phase 2).

## The defect, measured

A function `f = x => e`'s type is the **static arrow** `[[dom, cod],
[FunctionType, K]]`, built in `check_lam` from the parameter's type cell and
the return's type cell — **the template's own cells** ([lambda.rs: the
`arrow_parts` call](../../crates/lichen-highlevel/src/checker/lambda.rs)). An
apply clones the body template (fresh param/return cells per call) but **never
clones the arrow**, so the arrow stays `?a -> ?a` across uses — that is
let-polymorphism today.

Two consequences follow from the arrow being an unclonable static term whose
slots are the template's cells:

1. **The arrow cannot unify soundly with undecided slots.** `check_ann` does a
   direct `check_unify(value.ty, T)` with no clone
   ([annotations.rs](../../crates/lichen-highlevel/src/checker/annotations.rs)).
   So `f : Int -> Int` on a polymorphic `f = x => x` binds the template's
   shared `?a := Int`, killing polymorphism and propagating to every alias. It
   only appears to work for monomorphic-in-practice functions (`gcd`). There is
   no per-unify-site clone of the type.
2. **The lowlevel leaks the highlevel's encoding.** `unify_inner` is
   positional (slot 0↔0, 1↔1, …) with no notion of `[value, type, attrs…]]`;
   the highlevel merely relies on slot 1 = type. The function-ness guard
   inspects only the *type* structure (`is_function_type`), and the attribute
   check is run **separately** in `check_app` through `AttrExt::unify_slots`,
   not through unify. So the arrow encodes only `dom`/`cod` — it cannot express
   an attribute relationship (e.g. "the return's perspective is the argument's
   perspective"). Attribute flow through a function is an explicit non-goal
   today ([attributes.md](attributes.md)).

## The design

> **A function's type is the function itself.** `f : f`, and the type chain
> for a function cycles at `f` (`f : f : f : …`), exactly as the universe
> cycles at `Type` (`Type : Type`). The function's existing clone machinery
> becomes the type's clone machinery, and the separate arrow entity collapses
> into the function.

### Representation

A function's type slot holds a **self-referential pair `[Function(fid), ↺]`** —
a node whose value is the function itself and whose type slot points back at
itself, the exact shape of the universe `K = [Type, ↺]`. It is recognised as a
type by the self-cycle (`node_holds_type`'s `is_self_referential` branch
already admits it). The signature (`dom`/`cod`, and in Phase 2 the attribute
slots) is not stored in the type node — it lives in the function **template**
(`Function::parameter` / `Function::r#return`), reached through `fid`.

**Written arrow types stay as terms.** A written `Int -> Int` has no body, so
it is not a function; it remains the arrow term `[[dom, cod], [FunctionType,
K]]`. "Less entity" means a *function value's* type is the function (no arrow
is built for it), not that the arrow spelling is removed. The two forms
coexist, and the unify rule below handles the cross-case.

### Scope: every type-unify clones (decided)

Clone-on-unify is **full**: every type-unification of a function-type clones
the signature and **never binds the template**. An annotation `f : T` becomes a
*check* (clone the signature, unify the clone against `T`), not a
specialization (the template is not bound). Consequence, accepted by superior:
the `gcd` example (`gcd : <Int, Int> -> Int`) currently relies on the annotation
to fix its recursive return type to `Int` (the gcd note: without it gcd reports
`6: ?a`). Under full clone-on-unify the annotation checks a clone, the
template's return cell stays unbound, and the apply clones that unbound cell —
so gcd reports `6: ?a`. This regression is accepted for Phase 1; a separate
mechanism for recursive types comes later.

### The unify rule (the core)

A new `Program` policy hook — a sibling of `defer_pending`, consulted from
`unify_inner` when one side is a function-type node `[Function(fid), ↺]` and
the other is a type — **clones the function template and unifies the clone's
signature against the counterpart** (the template is never bound):

- counterpart is a **written arrow term** → read its `[dom, cod]`; unify
  clone.param.type ↔ dom, clone.return.type ↔ cod.
- counterpart is **another function-type** → clone it too; unify the two
  clones' param.type ↔ param.type, return.type ↔ return.type.
- counterpart is an **unbound cell** → the function-type is a concrete value;
  the cell binds to it (the ordinary `bind` path, no clone). The clone fires
  later, when that bound cell meets a signature.
- counterpart is a **concrete non-function type** → fail (the function-ness
  guard's static job).

The clone yields **fresh signature cells per unify site**, so unifying `f`'s
type binds the *clone's* cells, never the template's. That is let-polymorphism
*for the type itself* — `f : Int -> Int` no longer touches `f`'s template, and
every alias of `f` stays polymorphic. For `f = x => x` the template shares one
cell between param and return; the clone preserves that sharing, so
`f : Int -> Float` still conflicts (one cell, `Int` vs `Float`) and
`f : Int -> Int` passes. Sound.

The apply path is **unchanged**: it already clones the body template, and the
type slot referencing `fid` stays put. The clone-on-unify is a *type-level*
operation, orthogonal to the value-level apply clone.

## Phasing

- **Phase 1 — `f : f` with dom/cod clone-on-unify.** Build the
  `[Function(fid), ↺]` type node in `check_lam`; add the `Program` unify hook;
  update `is_function_type` / the function-ness guard; render a function-type
  as `dom -> cod` in the printer; persist it in the codec; read its signature
  in `low_type_of`. Fixes defect 1 with no behaviour change for monomorphic
  code. Verifiable by `cargo check` and the existing lambda/annotation/apply
  tests.
- **Phase 2 — the signature carries attribute slots.** Extend the clone to
  carry the parameter and return **pairs** `[value, type, attrs…]`, and route
  the apply's separate `AttrExt::unify_slots` check through the clone-unify, so
  a signature can share an attribute cell between param and return (e.g.
  "return's perspective = argument's perspective"). Delivers defect 2 —
  attribute relationships in a signature.

## Hazards on the record

- **Clone mechanism (decided): reuse the apply clone walk on the signature
  nodes only.** `Module::clone_signature` drives the existing `node_apply` /
  `value_apply` over just the template's parameter and return pairs with a
  synthetic `ApplyCtx` (a fresh scratch block, no argument, no body
  evaluation, no assert re-registration), then re-establishes the template's
  internal class topology among the clones with the existing
  `unify_clone_groups`. This reuses the proven `Function` re-homing and
  sharing primitives verbatim, scoped to the type-level need.
- **Re-homing under an enclosing clone — already handled.**
  `value_contains_foreign_function` (function.rs) recursively scans an
  array/table value for any foreign `Function`, so the function-type node
  `[Function(fid), ↺]` is caught: under an enclosing apply clone it is cloned
  (not shared) and its `Function` value is re-homed to the clone's `fid'`. No
  new exception is needed.
- **The `Type : Type` invariant is revised.** The README states "the type
  chain closes in a cycle at `Type`". Under `f : f` a function's chain closes
  at itself. This is intended; the README, `overview.md`, and any test pinning
  the chain terminal are updated in Phase 1.
- **Codec.** Persisting a function-type node in a type slot changes the
  artifact format. Per the no-forward-compatibility rule, existing artifacts
  are not supported; the codec tag for a function-in-type-slot is added.
- **Compute signatures.** The kernel `.sig` field and the `LaunchOp`/`ParLaunch`
  signature readers read `dom`/`cod` from an arrow term; they must also read a
  function-type's signature from its template (Phase 1, since the host may now
  hand a function-type).
- **`gcd` regression (accepted).** See *Scope* above: recursive functions
  report `?a` until a separate recursive-type mechanism lands.
