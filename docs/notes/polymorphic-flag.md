# The polymorphic flag: clone selection by reachability from `'`

> Status: **decided; the SPEC is refactored (uncommitted), the code is not yet
> migrated.** The maintainer's 2026-11-16 refactor of
> [language-spec.md](../language-spec.md) (§Polymorphic, §Recursion, §Clone,
> §Merge, §Operation) replaces the clone-selection rule — an undecided node
> whose class's outermost scope is inside the applying function — and the
> side-apply design with one creation-time flag propagated through the recursion
> set. This note records the three rulings that fix the spec's open edges, the
> supersession map, and the measured hazards the migration must respect. The
> rule itself is the spec's; this note does not restate it.
>
> Points at: `crates/lichen-lowlevel/src/function.rs` (`node_apply`, the dynamic
> clone rule; the `# Invariant` at `ApplyCtx::scope_root` is the old selection
> rule's home), `crates/lichen-lowlevel/src/template.rs` (`clone_template`),
> `crates/lichen-lowlevel/src/static_module/apply.rs` (`static_node_apply`, the
> frozen mirror), `crates/lichen-lowlevel/src/evaluation.rs` (the deep pass,
> where the flag propagates), `crates/lichen-language-lex/src/lib.rs` (`Prime`,
> the `'` mark, admitted only adjacent to the name it precedes).

## 1. The three rulings (maintainer, 2026-11-16)

1. **The flag is `Some` at clone time; debug-assert it.** Deep evaluate runs
   before clone, so the clone walk never meets an unset flag and never computes
   reachability on demand. The ordering is an implementation invariant: the
   migration lands it at the clone walk as a `# Invariant` (deep evaluate
   precedes clone; the flag is set on every node the walk reads) beside a
   `debug_assert!`. Until the migration, the code carries the old rule and this
   invariant has no code home yet.
2. **The clone's new class topology is fully preserved** ("完全保持"). The remap
   keeps every unify relation among the clones of one instantiation: cells that
   shared a class share the new class, distinct classes stay distinct. The
   detach moves clones out of the template's class; it does not reshape them.
3. **An unprimed lambda is monomorphic.** `x -> e` creates no `@polymorphic`
   cell; its body is referenced in place across calls. Only `'`-marked binders
   opt into per-call instantiation. The spec's sugar row shows the primed form;
   the unprimed form desugars without `@polymorphic`. The prelude is entirely
   primed (`core.lichen:2-10`, measured), so its semantics do not move; unprimed
   lambdas elsewhere in `examples/`/`lichen-std/` change semantics and the
   migration recon must census them.

   **Surface decision (maintainer, 2026-11-16): (a) defer.** The current surface
   has no unprimed binder — the prime *is* the binder (parser `Formal` vs `Name`,
   recon §2.4) — so the monomorphic form is inexpressible today. The migration
   keeps the flag mechanism general (the tri-state already carries `false`) and
   slice 2 swaps selection only; with every binder primed `true` this is
   behaviour-preserving. Introducing a monomorphic surface (an unprimed binder or
   a new sigil) is a separate, later decision.

## 2. What it replaces

- **Selection** — the undecided test plus the class-wide scope verdict
  ([apply-clone-ownership](apply-clone-ownership.md) §4: the nearest common
  ancestor of owner scopes, the virtual root, the binary-lifting ancestor test,
  the per-class cached verdict) becomes reachability from a `'` birth through
  the recursion set (operands, value references, class members). The scope test
  itself stays: a polymorphic node **within the applying function's scope**
  clones; the flag replaces the *class-summary* machinery, not the scope tree.
  The current invariant "a class is cloned only when every member lies inside
  that scope" (`function.rs`'s `ApplyCtx::scope_root`) is superseded: class-wide
  uniformity is now the flag's propagation, and the scope verdict is per-node.
- **The `deep_decided` efficiency gate** — the spec's old "clone recursive into
  a decided node if deep_decided is false" is gone; selection is the flag, not
  decidedness. **Carriage is untouched**: `evaluated_deep` still governs what a
  clone may carry ([apply-clone-ownership](apply-clone-ownership.md) §2, the
  `template_answer`/`proven_concrete` reads in `function.rs` and
  `static_module/apply.rs`). Selection asks "is this per-call", carriage asks
  "is this a template fact"; the refactor answers the first with the flag and
  leaves the second as measured.
- **The side-apply paragraph** — a spec-level design for inner-apply
  specialization, never implemented under that name (grep finds only type-side
  / caller-side apply comments). The nested-apply problem it targeted was
  repaired by the clone-generation fix
  ([nested-apply-none](nested-apply-none.md) §27, **fixed**, 22 programs); the
  flag rule must keep those 22 answers, and the closure/capture walk
  ([apply-clone-ownership](apply-clone-ownership.md) §1) remains the mechanism
  for inner functions until the migration measures otherwise.
- **Tag classification** — the spec's `@raw['a, 'a, 'b]` section is deleted; it
  had no code implementation. `'` (`Prime`, the formal-reference mark) is now
  the polymorphic mark: the surface the Any-migration landed as the template
  binder gains its lowlevel encoding, `@polymorphic` on the function's
  parameter/body placeholder cells.

## 3. The measured hazards the migration must respect

- **D18's budget** ([code-audit](code-audit.md) D18): the rejected engine route
  was "the clone walk carrying class members, then evaluating the codomain's
  deciders", and it hung
  `recursion::deep_evaluating_an_infinite_stream_records_the_deep_budget`. The
  new recursion set includes class members and the flag propagates via deep
  evaluate — but **propagating a boolean flag is not evaluating members**:
  monotone, stops at a fixpoint. The current deep pass
  (`evaluation.rs:880-980`) walks array items, table entries and `External`
  operands only; class members are a new walk, and it must respect
  `evaluate_depth_limit` like the rest of the pass. Implementing flag
  propagation as "deep-evaluate along class members" steps back into D18's
  measured hang.
- **The `runned` stamp stays** ([apply-type-side-check](apply-type-side-check.md),
  "One instantiation: measured, not landed"): sharing one instantiation across
  the check and value passes was measured and rejected because an operation
  that ran undecided must not re-open. Per-call re-evaluation happens through
  fresh clones (unset stamp), never by clearing `runned`. The refactor's
  §Operation keeps the stamp, so it is consistent with that measurement.
- **Merge-time staleness is an implementation item.** The flag propagates via
  deep evaluate, and a class union after the pass can make a previously
  non-polymorphic class polymorphic. Ruling 1's ordering (deep evaluate before
  clone) is the contract; whether propagation must also fire at class union —
  as the old scope summary was maintained at every write
  ([apply-clone-ownership](apply-clone-ownership.md) §4) — is for the migration
  recon to measure, not a semantic open question.

## 4. The migration's shape

A work package, not a comment edit: the flag field and its propagation in
`evaluation.rs`; the selection rule in `function.rs` / `template.rs` /
`static_module/apply.rs` (the frozen mirror reads the same flag); the artifact
format bumps (the flag is frozen state); the old selection invariants move only
when the code they govern moves. Acceptance: the five Ideal API forms and the
wrapper programs of [apply-clone-ownership](apply-clone-ownership.md) §3, the
22 programs of [nested-apply-none](nested-apply-none.md) §27, the
infinite-stream budget test above, and the Any-migration's acceptance probes
([any-removal-migration](any-removal-migration.md)). The unprimed-lambda census
(ruling 3) is part of the recon.

Slice 1 (expand) landed behaviour-preserving — the flag is computed and asserted,
selection unchanged. Its implementation record and slice-2 handoff are
[polymorphic-flag-slice1](polymorphic-flag-slice1.md).

Slice 2 (selection swap) landed — clone selection reads the flag at every walk. Its
record, the upgrade mechanism's measurement, and the one open ratification are
[polymorphic-flag-slice2](polymorphic-flag-slice2.md).

The struct-template freshness continuation (open struct-field cells marked at creation;
`any3`/`any6` fixed, `ideal4`/`any4` open) is
[polymorphic-flag-struct-freshness](polymorphic-flag-struct-freshness.md).

## 5. Superseded decisions

- **M5** of [any-removal-migration](any-removal-migration.md) ("the clone rule
  is the SPEC's. Outermost scope among class members") is superseded at the SPEC
  level; the migration note carries the line. The templates package's
  `5fcc181a` cherry-pick implements the pre-refactor verdict and is the starting
  point the migration replaces, not the target.
- [apply-clone-ownership](apply-clone-ownership.md) §4's selection half is
  superseded; its ownership (§1) and carriage (§2) remain current.
