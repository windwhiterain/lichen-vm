# An informal soundness argument

*2026-08-23.  Related: [`type-type-crisis.md`](type-type-crisis.md) (the
`Type : Type` paradox).  This is an argument by construction and inspection,
**not a proof** — the unproven edges are called out at the end.*

The question: can the checker certify a program whose evaluation *cleanly
produces a value that contradicts its claimed type* — no diagnostic, no
panic, just a wrong value?  The answer this document argues is **no**: every
accepted program's evaluation ends in exactly one of three ways, and none of
them is a silently-wrong value.

## The three outcomes

For any program the checker accepts (`ok` — no `unify_errors`, no
`eval_errors`), evaluating the root does one of:

1. **Terminate with a value that matches the claimed type.**
2. **Record a diagnostic** (a `UnifyError` or `EvalError`) — the program is
   then *rejected* (`ok = false`), so a "certified wrong value" never
   surfaces.
3. **Diverge**, capped by a VM guard panic
   (`recursion depth exceeded …` / `too many function applications …`).

There is no fourth outcome "terminate cleanly with a value that does not
match the claimed type."  Note the asymmetry the probes made concrete: a
latent mismatch ends in outcome 2 (a diagnostic), while the `Type : Type`
paradox ends in outcome 3 (divergence) — the two never combine into a
cleanly-produced wrong value.

## Why: the pair discipline

Every expression compiles to a recursive pair `[value, type]`.  The invariant
that drives soundness is:

> **An *evaluated* pair is internally consistent** — its value has the shape
> its type slot claims.

By induction over the checker's construction, each production site preserves
it:

- **Literals** `[5, [int, K]]`, **the `Type` constants** (whose value is the
  marker, type the universe), **lambdas** `[f, [[in, out], [FunctionType, K]]]`
  — consistent by construction.
- **Tuples / arrays** — the value's elements and the type's element-type list
  are built from the same subexpressions; the array element check unifies each
  element's type with the shared element cell.
- **Annotations** `e : T` — the annotation unify binds `ty[e]` to `T`, so the
  pair `[value_of(e), T]` is consistent *if* the subexpression's value matches
  its own type (the induction hypothesis).
- **Parameters** — the parameter pair `[value_cell, type_cell]` is bound at
  apply time to the argument's pair, elementwise (value-with-value,
  type-with-type), so it inherits the argument's consistency.

The one site where consistency is *not* established by construction is the
**apply**: the result's pair is the applied function's *return* pair, which is
consistent (by induction) — but the call's **claimed** result type (the
checker-wired result cell, operand element 2) is a separate node that could
disagree with it.  That gap is closed at runtime by the **result-cell sync**
in the lowlevel apply arm (`crates/lichen-lowlevel/src/function.rs`): on every
evaluated application it unifies the result cell with the return pair's type
element, recording a conflict if they differ.  So a mismatched claim on an
*evaluated* apply is outcome 2.

## The boundary: lazy branches

The result-cell sync only fires when the apply **runs**.  The one place a value
stays unevaluated is an unselected `if`/`Index` branch — `[then, else][c]`
evaluates only the selected element.  There, a call's result cell (unbound at
check time) can be unified into a *wrong* type claim with no conflict — e.g.
the array `[5, f (y => y)]` in an unselected branch is certified `Int<2>` even
though the second element's value is the identity lambda.  This is the one
false certification the checker will stand behind.

But it never reaches a produced value: reading such an element forces the
apply (the `Index` arm evaluates the selected element), and forcing runs the
sync, which records the conflict — outcome 2.  So the latent mismatch is
reachable in the graph only as an *unevaluated* node, and evaluating it turns
it into a diagnostic.  This is why the naive, *evaluated* form of the same
program is rejected outright.

## What is *not* covered

- **This is not totality.**  Soundness here is conditioned on the definition
  pass finishing; the `Type : Type` paradox (see `type-type-crisis.md`) is a
  checker-certified pure-λ term whose evaluation diverges (outcome 3).  "The
  checker accepts it" therefore does not imply "it has a value."
- **The clone / `bind` replication path is the least-proven part.**  The
  argument above leans on value-slots and type-slots staying role-aligned
  through the per-apply clone and the class-merge replication
  (`Module::bind`).  If a value/type pollution exists, it is most likely
  there — a misaligned class merge writing a type into a value slot — not in
  the sync.  A real proof would pin this down with an operational semantics
  and a logical relation over the runtime graph; the present argument does
  not.
- **The lazy-branch hole is a property of the current semantics, not a
  structural guarantee.**  It is unexploitable only because the runtime never
  *trusts* a certified type without forcing the value.  Any future feature
  that consumes a certified type claim without evaluation (an unchecked cast,
  a dependent type computed on the claim, type-directed codegen) would turn
  the latent hole into an observable one.
