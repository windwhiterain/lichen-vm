# Implementation plan for the template-match proposal (v13)

> Status: plan for [proposal-struct-place-class](proposal-struct-place-class.md) v13. It is
> an implementation order, not a spec text; the proposal stays the reviewed artefact. The
> steps are cut so each one is measurable on its own and the tree stays green (or keeps
> its current count) between them.

## Step 0 — the plan's baseline

- Compute suite 215 / 25 / 1 ignored; the 22 family tests named in the proposal's §6 table.
- `cargo test -p lichen-lowlevel` and the language/serve suites are the controls a change
  to the engine can move; they must be read with each step.

## Step 1 — records, relations and a gate that is **disabled**

- `Module`: `pending_requirements: HashMap<NodeId, Vec<PendingRequirement>>`,
  `attr_relations: HashMap<ExtensionId, Box<dyn AttrRelation<P>>>`, where
  `PendingRequirement` is `Pin { expects: NodeId }` or
  `Attribute { extension: ExtensionId, expects: NodeId }`. The gate carries an explicit
  `enforce: bool`, set `false` in this step: it is a *disabled* phase, not a no-op that
  silently reads an empty set — registration lands in Step 2 and enforcement is switched
  on in the same step, so there is no window in which a populated set is unenforced.
- The gate `install_value(class, value)`: checks the class's records (empty at first, so
  it is a no-op) and returns `Err(UnifyError)` before writing on a refusal. The five
  write sites (`write_node_value`, `write_node_answer`, `propagate_class_value`,
  `fill_class_holes`, `add_equality`'s pre-union fill) call it instead of writing
  directly; GC's `relocate_node_value` and freeze's value write stay licensed movements.
- `AttrRelation::accepts(module, declared, candidate: &P::Value)`, registered by the
  highlevel for each extension; the gate composes `value_eq(expects, candidate)
  || relation.accepts(...)`.
- Measure: compute 215/25 unchanged, lowlevel and language green — the gate is inert
  while the sets are empty.

## Step 2 — the checker resolves the roles, registers the records, and turns the gate on

- Registration **and** enforcement land in this one step: the gate's `enforce` is set
  `true` in the same commit that first populates the sets, so no populated set is ever
  unenforced.
- `check_instantiate` (checker/structs.rs): per field, resolve the type from the field's
  type child and the attributes by their own relation. The decision is **per position,
  not per group**: a field whose value is merely shallow-decided can still hold open
  children (the wrapper shape), so only a **deep-closed** field checks now; a closed pin
  with an open field pushes `Pin`; an open attribute with an open field pushes
  `Attribute`; an open value child stays a fresh cell; a group recurses into its
  children. No value is installed from the declaration.

  The same correction applies to `check_unify_expr`: the whole-term unify is kept only
  for a deep-closed actual; every open position registers its records and stays open.
- The same registration for the parameter templates the apply matches (`check_app`'s
  template path), so the runtime patterns carry their records.
- Measure: this is the first enforcing step, so the **22 tests** are read here. The
  **acceptance cases 1–3** wait for Step 3's dynamic clone copy — they need the records on
  the clone — while case 0 (the checker's initial unify) and 0b/0c (the partially-open
  group and the wildcard) are readable here. A regression is fixed in the registration
  (which positions are registered), never by turning the gate off.

## Step 3 — enforce at the gate

- The records are now checked at every install; a refusal leaves the value unwritten and
  reports the existing `expected <pin>, found <value>` wording.
- Measure: the 22 tests. Expect the chain family (4) to flip first; watch the earlier
  class-fill experiments' two regressions (206/34, 209/31) — the difference is that those
  gated *fills from declarations*, while this gate refuses a fill that violates a
  registered pin, and the chain's declaration fill meets a pin (the codomain's `.length`)
  — if the count falls, the pin set is too wide and the registration is the place to fix.
- The acceptance cases 1–3 (deferred pin, two pins, per-call clone) are testable once the
  dynamic clone walk copies the records; do that copy in this step: the walk copies each
  class's records through its remap (the dynamic clone path in `function.rs`).

## Step 4 — the frozen and static path

- `StaticModule.pending_requirements`; freeze maps the records with the edge remap; the
  container format writes them sorted by class root and bumps `ARTIFACT_FORMAT_VERSION`;
  load restores and rewrites the refs; the static clone walk copies them through its
  remap. **The static wildcard branch is named separately**: like the dynamic branch it
  returns before `ctx.remap.insert`, so the branch itself copies the frozen wildcard
  class's requirements onto the fresh cell, and the occurrence-independence behavior is
  unchanged. Acceptance case 6 (an imported deferred requirement refuses after load, and a
  frozen wildcard's deferred requirement reaches a concrete bind after static
  application).
- Measure: the imported-requirement tests, the static path's tests, and the language
  suite (the container format's round trip).

## Step 5 — the whole objective

- `cargo test --workspace`; the examples survey (the 8 panics should stop being
  collisions once the plrun codomain read answers the recorded buffer); the compute 25;
  `comments-check`; `cargo fmt`.

## What this plan does not do

- It does not touch `a = b`, the annotation rule, §Struct's reduction text, the library
  wrappers, or the value-templates rulings; the proposal keeps them.
- It does not weaken or rewrite any test: the 22 are the measurement, not the target.
