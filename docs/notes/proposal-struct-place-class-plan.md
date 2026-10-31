# Implementation plan for the template-match proposal (v7)

> Status: plan for [proposal-struct-place-class](proposal-struct-place-class.md) v7. It is
> an implementation order, not a spec text; the proposal stays the reviewed artefact. The
> steps are cut so each one is measurable on its own and the tree stays green (or keeps
> its current count) between them.

## Step 0 — the plan's baseline

- Compute suite 215 / 25 / 1 ignored; the 22 family tests named in the proposal's §6 table.
- `cargo test -p lichen-lowlevel` and the language/serve suites are the controls a change
  to the engine can move; they must be read with each step.

## Step 1 — records, relations and a no-op gate

- `Module`: `pending_requirements: HashMap<NodeId, Vec<PendingRequirement>>`,
  `attr_relations: HashMap<ExtensionId, Box<dyn AttrRelation<P>>>`, where
  `PendingRequirement` is `Pin { expects: NodeId }` or
  `Attribute { extension: ExtensionId, expects: NodeId }`.
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

## Step 2 — the checker resolves the roles and registers the records

- `check_instantiate` (checker/structs.rs): per field, resolve the type from the field's
  type child and the attributes by their own relation; a decided actual checks now (as
  today); a **closed pin with an open actual** pushes `Pin`; an **open attribute with an
  open actual** pushes `Attribute`; an open value child stays a fresh cell. No value is
  installed from the declaration.
- The same registration for the parameter templates the apply matches (`check_app`'s
  template path), so the runtime patterns carry their records.
- Measure: still 215/25 (records are checked only by the gate, and the gate does not yet
  see them), but the registration itself is observable in the probe.

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
  remap. Acceptance case 6 (an imported deferred requirement refuses after load).
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
