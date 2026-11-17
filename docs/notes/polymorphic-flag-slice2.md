# Polymorphic flag, slice 2 (selection swap): implementation record

> Status: **slice 2 landed** on `feature/polymorphic-flag` — clone selection now reads
> the polymorphic flag. The flag is no longer inert. Commits: `1b64f855`, `c833d7d1`,
> `5c8216d9`, `ac46f8b8`, `71843bfc`, `9a52c5f4`. The rulings are
> [the contract note](polymorphic-flag.md)'s; slice 1's mechanism is
> [polymorphic-flag-slice1](polymorphic-flag-slice1.md)'s; baselines are
> [the recon](polymorphic-flag-recon.md)'s. This note records the choices slice 2 made,
> the upgrade mechanism and its measurement, the one approved test update, and what is open.

## The new selection rule

A node clones iff **its flag is `Some(true)` and its own place lies within the applying
function's scope** (SPEC §Clone/§Polymorphic, per-node). The `deep_decided` efficiency gate
is gone from selection; **carriage is untouched** (`evaluated_deep` still governs what a
clone may carry, [apply-clone-ownership](apply-clone-ownership.md) §2). The scope tree stays;
the verdict is per-node, and class-wide uniformity is the flag's propagation.

- `function.rs` `node_apply`: `per_call = in_parameter || is_parameter || parameter_slot ||
  (flag && node_inside_scope) || replay_operation`. The class-wide `node_class_inside_scope`,
  the `scope_verdicts` cache and `template_root` are removed; the `ApplyCtx::scope_root`
  `# Invariant` now states the per-node rule. `node_inside_scope` reads `owner_scope(node)`
  (plus the closure-capture anchor hit).
- `template.rs` `clone_template`: `template_class_is_cloned` and the frozen
  `template_static_inside_scope` read flag + per-node scope; the class-verdict cache and the
  whole freshness machinery (`template_closure_map`, `template_content_is_closed`,
  `template_class_needs_freshness`, the payload/traced children helpers) are removed.
- `static_module/apply.rs` / `static_module.rs`: `FrozenClassRule.inside` reads the frozen
  flag + `static_owner_scope`; the per-class verdict/representative caches are removed. The
  frozen mirror reads the same flag the freeze propagated.

## Propagation refinements (ratified) and the one new decision

The three refinements the maintainer ratified are in `compute_polymorphic`:
1. **Seed every function's parameter** placeholder (ruling 3(a): every binder on this surface
   is primed; the IR path never goes through the checker's mark). The checker mark stays as
   the language emission.
2. **Function-value descent only for nested functions** (`parent` `Some`): a nested function
   value is a per-call closure; a top-level function value is its identity, referenced in
   place ([apply-clone-ownership](apply-clone-ownership.md) §9).
3. The slice-1 **conservative** owning-function edge (pulled a whole template) is dropped.

**New decision — the precise owning-function edge (needs the maintainer's ratification).**
Dropping the edge outright under-marked function bodies: `type_of := 'x -> {t := _; x: t; t}`
and the `@in`/render programs left undecided body cells referenced in place and unfilled
(the old gate cloned them by decidedness). Re-adding the *conservative* edge over-marked
decided body constants and broke `function_call_operator_clones_array_body` (pins the constant
`7` referenced in place). The resolution that preserves behaviour: an **undecided** body cell
of a function references that function's parameter (a seed) — a recursion-set edge — so it is
polymorphic; a **decided constant** and a **function value** are excluded. This keeps selection
reading the flag, but the flag's propagation now encodes *undecidedness* for body cells, which
deviates from pure reachability-from-marks (SPEC §Polymorphic). It is the behaviour-preserving
reading on this all-primed surface (ruling 3(a)); a future monomorphic surface would revisit it.

## The upgrade-staleness mechanism (contract §3) and its measurement

Slice 1's O(1) counter skip guaranteed *totality*, not monotone *value* correctness. Slice 2
adds `Module.polymorphic_dirty`:
- `mark_polymorphic` sets it (a new `true` seed must spread).
- `add_equality` sets it when a union joins a polymorphic class to a non-polymorphic one
  (`(poly_a == Some(true)) != (poly_b == Some(true))`), and **propagates synchronously** there.
  Synchronous propagation is required: a union during a clone walk must upgrade the flag
  *before* a later `node_apply` in the same walk reads it (deferred dirty was too late —
  `type_of`'s root stayed undecided until this became synchronous).
- `propagate_polymorphic` skips in O(1) only when `polymorphic_unflagged == 0 && !dirty`;
  otherwise it recomputes the global fixpoint and clears `dirty`.

**Measured:** an upgrading union fires **13,294** times across the language external suite
and **97** times across lowlevel basic; with synchronous propagation the suites read the
upgraded flag and stay green (language 284/5, lowlevel 167/0/1). Before the synchronous fix,
`membership::a_membership_test_matches_a_type_value_against_a_class_domain` and the four
`render::tests::type_of*` lib tests failed (root undecided); after, they pass.

## The one approved test update

`recursion::undefined_recursive_function_clones_a_function_per_level` pinned the removed
`deep_decided` gate ("with `evaluated_deep` still `None`, the clone rule copies the function
per level", `assert_ne!(cloned, f_id)`, `functions.len() == 3`). Maintainer permission granted
for this test only. Renamed to `undefined_recursive_function_references_itself_in_place`: a
top-level recursive function value is referenced in place regardless of decidedness
(`referenced == f_id`, `functions.len() == 1`); the lazy-stream structural assertions
(`ids.len() == 2`, the argument representative, the per-level `[5, f(5)]` shape, fresh apply
clone per level) are kept.

## Acceptance (the tool's own lines)

- `cargo test -p lichen-lowlevel`: `test result: ok. 167 passed; 0 failed; 1 ignored` (basic,
  0.21s) and `ok. 0 passed` (lib).
- `cargo test -p lichen-highlevel --test highlevel`: `test result: ok. 136 passed; 0 failed`
  — the capture gap did **not** re-open (`an_open_struct_template_lets_its_field_be_read` passes).
- `cargo test -p lichen-language --test language`: `test result: FAILED. 284 passed; 5 failed`.
  The five are a **subset** of the recon's twelve pre-existing names — **seven of the twelve
  are now fixed**, five remain, **zero new**: `field_read_kinds::a_named_read_of_a_deferred_array_is_refused_at_the_read`,
  `field_read_kinds::a_named_read_of_a_deferred_non_struct_is_refused`,
  `frozen_function_type::a_function_template_in_a_frozen_module_is_a_function`,
  `template_instances::a_struct_types_wildcard_fields_stay_open_across_instances`,
  `template_instances::an_inline_literal_domain_is_fresh_per_application`. They still fail for
  their pre-existing reasons (output template `none` vs `2`; the `expected 1, found 1.0`
  Int/Float collide), not a flag regression.
- `cargo test -p lichen-language --lib`: `test result: ok. 69 passed; 0 failed`.
- `recursion::deep_evaluating_an_infinite_stream_records_the_deep_budget ... ok`, fast.
- Five Ideal API forms (front end): `6: Int`, `2: Int`, `2: Int`, `0.5: Float`, `Int: Type`.
- Any-migration probes 1–7 (front end): `6: Int`, `2: Int`, `(2, 2)`, `(2, 0.5)`, nested checks,
  `((1, 2), (1.5, 2.5))`, `(43, 44)`.
- nested-apply-none §27: the corpus is gone (recon §0); the recon's twenty reconstructed
  programs (recon §1.2) all answer exactly their recorded values (`m4`/`v4`/`t1`/`z1`/`s1`/`c1`/
  `n1e`/`w4`/`w6`/`w8` = `3: Int`, `v3`/`b6` = `4: Int`, `h1`/`h2` = `6: Int`, `b7`/`g3` = `8: Int`,
  `i7` = `7: Int`, `w2` = `7: Int`, `w7` = `4: Int`, `r4` = `[?a]: Array` instrument difference) —
  no answer moved.
- `comments-check --config comments-test.toml . --max-violations 0`: no violations.
  `cargo fmt --all -- --check`: clean.

## Open

- **The precise owning-function edge was REJECTED** (maintainer ruling: pure
  reachability-from-marks stands, no decidedness in propagation); it was reverted in
  `318fa358`. The struct-template freshness it left open is taken up in
  [polymorphic-flag-struct-freshness](polymorphic-flag-struct-freshness.md): open struct-field
  cells are marked at creation (`any3`/`any6` fixed); `ideal4`/`any4` remain open.
- **Perf: resolved** by the incremental reverse-edge class upgrade
  ([upgrade-perf](upgrade-perf.md)): an upgrading union spreads from the merged class along
  reverse edges instead of a whole-module recompute (language suite 9.13s → 3.53s). Caching
  the reverse edges was ruled out (correctness risk).
- The five remaining language failures are pre-existing Any-migration gaps, out of scope for
  the flag migration (the contract's gate is "no new failures", met).
