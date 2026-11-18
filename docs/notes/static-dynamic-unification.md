# Static and dynamic: one clone interface, no materialization

> Status: **in progress.** A1 landed on dev (`8590ea76`); U1 (the parameter-bind unification)
> is in flight; A2-A4 are planned. The contract is the maintainer's own list in
> [general-contract-handoff](general-contract-handoff.md) §用户裁定 rules 1-7 — **no
> materialization**, **one interface for dynamic and static, treated equally**, normalize before
> freeze, no cross-module `FunctionId` bridges, writes in the checker only, new tests and
> Invariant changes need permission, small commits even when red. This note carries the
> diagnosis, the rulings and the measurements; the rules themselves live at the code sites it
> names.
>
> Points at: `crates/lichen-lowlevel/src/function.rs` (`node_apply`, the dynamic clone walk),
> `crates/lichen-lowlevel/src/static_module/apply.rs` (`static_node_apply`, the frozen walk),
> `crates/lichen-lowlevel/src/static_module.rs` (`node_value`, `FrozenClassRule`),
> `crates/lichen-lowlevel/src/static_module/freeze.rs`, `crates/lichen-lowlevel/src/apply.rs`
> (`apply_parameter_check`, the one bind), `crates/lichen-language/src/persist.rs` and
> `persist/container.rs` (the artifact codec).
>
> The read-only censuses this work builds on are kept in `.scratch-core/`
> (`unified-interface-census.md`, `-A0.md`, `-unify.md`); their durable findings are below.

## 1. The diagnosis

The two apply paths are not two implementations of one rule; they are two different algorithms.

- **Dynamic** `node_apply` (`function.rs:821`) decides `per_call` (`:858-864`: `in_parameter`,
  the parameter and its slot, `node_polymorphic_over_scope && node_inside_scope`,
  `is_apply_output && inside`, replay) and **references in place** when it is false —
  `return node; // reference as-is` (`:866`). Only a per-call node is minted.
- **Frozen** `static_node_apply` (`static_module/apply.rs:562-711`) has **no such exit**: every
  path returns a freshly minted dynamic clone, or the pass's canonical outer instance (also a
  dynamic clone). `undecided` and `FrozenClassRule::inside` decide owner, sharing and carriage —
  **not whether to clone**. The frozen bool is stamped onto the clone (`:614`) and never selects.
- **The structural reason**: `Operation<P>.operand` was `Option<NodeId>` (`lib.rs:576`), so an
  operation's operand could not name a frozen cell, while value positions already hold
  `AnyNodeId::Static`. A walk whose spine cannot reference a frozen cell must copy it.

So the frozen apply materializes the whole graph and then shares by class, and the seven reworks
of [any-removal-migration](any-removal-migration.md) are symptoms of that one difference rather
than seven independent bugs. [polymorphic-flag-slice3](polymorphic-flag-slice3.md) §6's "the
erasure coincides" claim is about the flag being *carried*, not about selection; **no test pins
it** (the phrase appears once in the repository, in that note, and in no test).

## 2. The positive rule is already in code

`Module::node_value`'s `# Invariant` (`static_module.rs:344-351`) states contract rule 1
positively: a static ref is read **in its own module**, directly, exactly as a dynamic node is,
and **no reader copies it into a dynamic node first**, because a copy gives each frozen position
a fresh class and one value reads as several. `evaluate_node` already routes the Static arm there
(`evaluation.rs:73-79`). Unification makes that the shared read's rule; it does not invent one.
`materialize_leaf` (`static_module.rs:386-389`) is the copying escape hatch — for export binding
and tests, never for a per-call path.

## 3. Rulings

- **Shape A** (maintainer): the full unification — one clone walk that may reference in place on
  both sides, retiring the separate frozen apply path. Chosen over B (one walk, two readers,
  minting kept) and C (share only the predicate). C alone would not stop materialization, because
  `FrozenClassRule::inside` does not gate minting.
- **Parameter binding: `unify`** (maintainer). The SPEC already requires it —
  `docs/language-spec.md:291` (§Function): "unify the argument with the cloned parameter" — and
  §Merge (`:106-108`) gives for free what `propagate_class_value` (`equality.rs:512-521`)
  hand-rolls. The frozen value-copy bind was the deviation, and its only written record was the
  implementation invariant at `static_module/apply.rs:293-295`. **No SPEC sentence needed an
  edit.** The two binds were alternatives on different entry points, not steps: the frozen side
  already unified on two of its three paths (`static_function_apply` at `apply.rs:84`,
  `static_function_type_apply` at `:162`); the copy was the outlier on the third,
  `materialize_static_return_apply` (`:287`, the non-reporting definition pass, selected at
  `function.rs:366-390`).
- **FORMAT bump 33 → 34 approved** (`crates/lichen-language/src/persist.rs:38`). Unavoidable at
  A3: the artifact's operand record is a bare `u64` local index (`persist/container.rs:58-71`
  write, `:483-493` read, validated against `node_count`) with no kind tag, so a cross-module
  operand ref has nowhere to go and an untagged foreign index would **silently alias a local
  node**. Minimal new fact: a discriminant `0 = absent / 1 = local index / 2 = (module key, local
  index)`. The polymorphic antichain / creation-mark bit is an independent fact that may ride the
  same bump: the frozen bool is `!chain.is_empty()` (`freeze.rs:161`), a projection that cannot be
  inverted, because the creation marks gate on `ir.monomorphic_formals` (`checker/lambda.rs:69`),
  a source-level fact the artifact does not carry.
- **Five Invariant changes approved** (restating a sentence at its code site is a migration and
  needs no permission; changing the rule does): `function.rs:815-819` (the ordering rule's scope
  grows to two flag representations), `static_module/apply.rs:554-561` (the frozen half of
  ruling 1 moves to the shared walk), `static_module.rs:66-73` (the class-wide-verdict sentence,
  **already false of the code** — `inside` is per-node), `static_module.rs:81-87` (the canonical
  image's ownership moves to the shared walk's state), `static_module/apply.rs:733-741`
  (rework 7's remap whitelist vs the dynamic "map every item"; the module-keyed half migrates
  verbatim). Plus `apply.rs:293-295`, retired by the unify ruling itself.
- **Not granted**: new tests or probe files. Ask first.

## 4. The slices

- **A1 — landed, dev `8590ea76`** (commits `4dd70c40`, `b2342426`, `8590ea76`): widen
  `Operation.operand` to `Option<AnyNodeId>`, behaviour-preserving. Every writer wraps `Dynamic`;
  every reader reuses the existing static path (`node_value`, `array_items_ref`,
  `evaluate_node_deep_ref`) or refuses by name (`freeze.rs:143`, `resolve.rs` `operands_of`,
  `compute/body.rs:1324`) or skips explicitly (GC keeps a frozen ref by its registry `Arc`,
  propagation walks dynamic nodes). `StaticOperation.operand` stays `LocalNodeId`, the format
  stays 33, selection and carriage are untouched, no new tests. The clone walks now reference a
  Static operand **in place** (`function.rs:897`, `template.rs:345`) — A3's groundwork, laid
  behaviour-preservingly because no writer emits one yet. `freeze.rs:143`'s `unreachable!()` is
  the A3 bump site: A3 replaces it with the tagged encoding, not a panic left behind.
- **U1 — in flight**: the one bind. `apply.rs:287` calls `apply_parameter_check` with the sibling
  path's error-discard (`:160-173`); `copy_parameter_slots` and `copy_slot` are deleted (one
  caller each); the ruling-retired invariant is named in the commit message with
  `language-spec.md:291`. No bump, no operand change, no other Invariant touched.
- **A2**: one shared read interface and one decision/carriage code path, behaviour-preserving.
- **A3**: the frozen walk adopts the shared decision, gains the reference-in-place exit, and the
  artifact learns the tagged operand. Behaviour changes here; the bump and the Invariant
  approvals are for this step.
- **A4**: retire `static_node_apply`, `clone_static_signature_node`,
  `materialize_static_signature_with_classes`, `static_remap_value`, `static_clone_function`;
  keep `wire_apply_result`, `apply_parameter_check` and the entry points
  (`static_function_apply`, `static_function_type_apply`, `materialize_static_return_apply`,
  `materialize_static_signature_expr`, `materialize_leaf`).

## 5. Hazards that must be repaired with A3, not after

- **Silent liveness loss**: `referenced_keys`/`referenced_keys_of` (`static_module.rs:557-583`)
  walk `node.value` only, never `node.operation`. Once an operand can name another module, that
  module is absent from the eviction-refusal set and can be evicted under a live ref. The
  operation edge must be added in the same step. This is the one site that fails silently rather
  than loudly.
- Silent `None`s: `array_items_of` / `item_of` (`resolve.rs:308-323`) answer `None` for Static.
  The loud refusals (`operand_expr`, `value_leaves`, `resolve.rs:104-140`) are the good failure
  mode and become resolutions.
- `retain_node`/`VisitGuard` (`evaluation.rs:83-92`) guard dynamic re-entry only; a cycle through
  a frozen payload is invisible to it (open: can a frozen payload be cyclic?).
- `refresh_class_summary` (`scope.rs:233-261`) walks dynamic members only, while a class can hold
  a frozen member (`same_class(AnyNodeId, AnyNodeId)`, `resolve.rs:441`). Under U1's unify a
  merged class spans the caller's scope, so every summary reader answers differently — silently.
  A frozen cell never enters the union-find (`unify_ref` reads a Static side by value,
  `equality.rs:622-639`), so the member walk stays all-dynamic. The loud failure of unify is an
  `ApplyError` (`apply.rs:117-128`), which is the good mode.

## 6. Frozen-only facts: none is a semantic fork

F1 the canonical outer image per pass (`static_module.rs:81-87`), F2 one block per pass
(`apply.rs:986-988`), F5 `ReadOnly` drops a decided node's operation (`freeze.rs:138`): walk
parameters. F3 "which module am I reading" (`registry.rs:92`, `lib.rs:744`): the one genuinely new
parameter. F4 the un-mintable id space (`Arc<StaticModule>`): constrains the **write** half only —
mint into the caller's module, which is what `static_function_apply` already does. F6 the
bool+place predicate: not frozen-only at all, it is the erasure. Careful with F1: rework 3
measured the remap theories around it wrong, so no new remap theory without a measurement.

## 7. The gate, and A1's measured acceptance

Accepted baseline: language external **289/0**, language lib **69/0**, lowlevel **167/0/1**,
highlevel **136/0**, compute **215/68** (0 regressions / 80 pure improvements against baseline
135/148; further improvements allowed, regressions not), compute-gpu **94/0**;
`comments-check --config comments-test.toml . --max-violations 0` and `cargo fmt --all -- --check`
clean; and **`cargo check --workspace --tests`** (AGENTS.md Verify, after the measurement below).
Only `lichen-compute-gpu` may start a backend. Cite the tool's own `test result:` lines.

A1, on the merged dev tip `8590ea76`, parent-run: workspace check exit 0 (0 errors, zero
`Compiling` lines naming another worktree), comments-check exit 0, fmt exit 0, and
`94 passed; 0 failed` (compute-gpu), `136 passed; 0 failed` (highlevel), `69 passed; 0 failed`
(language lib), `289 passed; 0 failed` (language external), `167 passed; 0 failed; 1 ignored`
(lowlevel). Compute was verified by **name**, not by count: the base `eac092f3` was run in a
throwaway worktree with its own target and `diff fail-base.txt fail-a1.txt` was **empty** —
identical failing set, 0 regressions and 0 improvements (`215 passed; 68 failed; 1 ignored`,
575.67s). The dev-c tip compute sweep is deferred to U1's merge, where compute must run anyway
to classify every moved test; the rebase between the verified tip and the merged tip added only
the LSP fix and a documentation rule.

## 8. The workspace-check lesson

`c02b8b46` (polymorphic-flag slice 2) added `ast::Binding.primed`
(`lichen-language-parser/src/ast.rs:343`) and left the two `RecordBlock` binding syntheses in
`lichen-language-server/src/analysis.rs` (`:2074`, `:2413`) without it, so the workspace check was
red on dev from that commit until `3b683e6d` while **every per-crate suite stayed green**. The
fix is `primed: false` at both sites, and that is the faithful value rather than a convenience
default: the parser's struct-field prime only abbreviates the field's own value
(`parse.rs` `struct_field_value` / `primed_field_value`; `StructFieldValue` carries
`{name, value}` alone), so a field's polymorphism lives in its value's `Formal` and never in a
flag on the field — `RecordField` has no `primed` field — and the synthesized `Binding` exists
only so a named field resolves in the LSP's own scope walk. AGENTS.md's Verify rule now names
`cargo check --workspace --tests` explicitly, with this measurement.
