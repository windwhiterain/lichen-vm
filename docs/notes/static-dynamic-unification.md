# Static and dynamic: one clone interface, no materialization

> Status: **in progress.** A1 landed on dev (`8590ea76`); U1 (the parameter-bind unification)
> was measured RED on compute and is parked on `u1-parked` (`f19eae58`); A2-A4 are planned,
> the unify folded into A3. The contract is the maintainer's own list in
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
- **U1 — measured RED on compute, parked** (`u1-parked`, `f19eae58`; see §9): the one bind. `apply.rs:287` calls `apply_parameter_check` with the sibling
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

## 9. U1 measured: the bind cannot change before the walks unify

U1 applied the ruling alone — `apply.rs:287` calls `apply_parameter_check` with the sibling's
error-discard, `copy_parameter_slots`/`copy_slot` deleted, the retired invariant named in the
commit message with `language-spec.md:291`. One file, +12/−54. **Not merged**; parked on
`u1-parked` (`f19eae58`), the working branch reset to the gated A1 tip `8590ea76`.

**Gate**: language external `289 passed; 0 failed` (`template_instances` unmoved),
`frozen_function_type` ok, lowlevel `167/0/1`, highlevel `136/0`, language lib `69/0`,
compute-gpu `94/0`, workspace check / comments-check / fmt exit 0; compute
`203 passed; 80 failed; 1 ignored` in **1286.88s** against the baseline `215/68/1` in 575.67s —
**12 regressions, 0 improvements**, classified by name, 69 over-60s warnings.

**The mechanism, corrected.** "Compute classifies callees by class topology" is wrong as stated:
**no callee predicate reads a class root**. `static_callee` (`compute/body.rs:1621-1640`),
`dynamic_callee` (`:1649-1673`), `is_static_function` (`compute.rs:8180-8192`) and `kernel_id_of`
(`compute.rs:5925-5958`) all read **values**. The bind changes *where a value lands*, through two
rules already in the tree: **write-once** (`lib.rs:936-940`, `equality.rs:357-365` — a value
enters a class once, only a class holding nothing is written, both `debug_assert!`ed) and
**§Merge propagation** (`language-spec.md:108`, `propagate_class_value` `equality.rs:512-521`,
`fill_class_holes` `:160-171`). After the merge `node_value(callee)` stops answering
`ComputeValue::Kernel(kid)`, `kernel_id_of` answers `None` at `body.rs:1913`, and the fallthrough
refusal at `:1940-1944` fires on a callee that *is* a kernel value.

**The direct fact exists and is already used one file over.** `pending_kernel`
(`compute.rs:5968-5995`) reads the callee's **operation** (`ComputeOperator::Jit`), which the bind
does not rewrite, and the JIT's own predicate already composes the two (`compute.rs:1187`:
`kernel_id_of(..).is_some() || pending_kernel(..)`); the lowering uses `kernel_id_of` alone. The
minimal consistency fix is 2-3 lines. Two caveats: `pending_kernel` is a **superset** by its own
invariant (`:5964-5967`), so it can keep a non-kernel callee on the cross-kernel path and fail
later at `body.rs:2723` with a different message — it must be measured against **215/68 with
identical names**, not "no worse"; and if the failing callee is a plain binding with neither
operation nor value, both answer `None` and the fix is inert. **It covers only the
`jit_cross_kernel_*` family**: `an_output_position_that_is_not_a_buffer_is_refused` and
`the_product_is_warp_uniform_not_wider` read the value/class sites, so making compute robust under
the merged topology is a second, A2/A3-sized slice (~20 `class_root`/`node_class` reads in
`body.rs:566..2524`).

**The decision record.** No `wontfix:D` covers this. The refusal is a recorded, deliberately
deferred v1 limitation ([loop-conversion](loop-conversion.md) §99-102 and §1481-1487,
[transcendentals-as-operators](transcendentals-as-operators.md) §70-74,
[wall-claims-audit](wall-claims-audit.md) §83/§306). The message is correct for a genuinely inline
callee; the defect is the **misclassification**, not the refusal's existence.
`compile_order_class` is already recorded as swinging for an unrelated reason (process-global
state, [open-issues](open-issues.md) §359-360, fix at §382), so it must be re-run **alone** before
it is counted.

**The perf cost is the ruling's own.** 2.24× is **data growth**, not recompute cost: merged classes
make every O(members) walk bigger — `propagate_class_value`, `fill_class_holes`,
`evaluate_class_first` (`evaluation.rs:105-140`, which also *runs* more members, §Merge:106's own
rule), `refresh_class_summary`, `class_inside_scope`, the polymorphic fixpoint's class edge.
[upgrade-perf](upgrade-perf.md)'s landed optimization does not help: different amplifier. Perf-only
on the available evidence, but a changed run order is a correctness surface too.

**Consequence for the plan.** The bind cannot change before the two walks unify, because the
value/class-site robustness work *is* A2/A3. The unify therefore lands inside A3, together with
the reference-in-place exit, the tagged operand encoding, compute's classification-by-fact, and a
ruling on whether the O(members) walks must be bounded first. Retracting the ruling to the
reporting paths only would contradict `language-spec.md:291` (the SPEC's apply has no
reporting/non-reporting distinction) and contract rule 2, and would have to be recorded as a
deliberate SPEC deviation.

**Open, each needing a run**: which value mechanism fires (write-once vs §Merge); whether the
failing callee is a `Jit` operation or a binding; `compile_order_class` alone; whether the slowdown
also changes run order; whether a debug build's `equality.rs:357-365` assertion catches a U1 class
state (the suite runs the test profile).

## 10. M1 measured: the classification fix is inert, and the defect is §Clone's detach

The maintainer ruled **"fix or SPEC proposal"** — pursue the fix, and bring a SPEC proposal only
if the fix cannot honour the SPEC; never a silent deviation. M1 is the decisive measurement
(impl, `.worktrees/unified-interface/.scratch-core/M1-measurement.md`, logs `m1-*.txt`; temporary
env-gated instruments, never committed, tree restored to `8590ea76` and recompiling clean).

**Q1 — the 2-3 line classification fix is INERT, measured not inferred.** At `compute/body.rs:1913`
the callee (node 405) is an **Index operation node** — the value-half read `Index(k0_expr, 0)` of
the binding `k0` — neither a `Jit` operation nor a plain binding. Under U1:
`kid=None pending=false vof=Some(368) slot=None classval=None`; under A1, same instruments:
`kid=Some(185383013334871206) pending=true vof=Some(954) slot=Some(Array) classval=Some(Array)`.
`pending_kernel` is **already false** under U1, so `kernel_id_of || pending_kernel` is still false
and the recorded inline-callee refusal still fires. The classification-consistency fix is worth
having on its own merits, but it does not unblock the bind.

**Q2 — neither hypothesized mechanism fires for the Kernel.** Counts over the U1 repro: all value
writes 5180, write-once skips 31, `fill_class_holes` 615, `propagate_class_value` 69; **Kernel**
writes 1, Kernel skips 0, Kernel fills/props 0. The Kernel is written once to the result **clone**
node 1057 (`origin=368`, its own singleton class); the callee reads node **368** (the Apply node,
holding nothing), and 368 and 1057 are **not unified**. The divergence is in `value_of_node`
(`compute.rs:5872`): for `Index(target, 0)`, A1 takes the `array_items(target)` branch (the target
holds the concrete `[value, type]` expr, peeling to the kernel-bearing value slot 954); U1 falls
through to `node_operation(target).is_some()` and returns the empty Apply node 368, because the
Index target **lost its concrete expr Array**. So the bind displaces the frozen jit apply's
**result expr structure / value-half** from the caller's binding — the same family as rework 7's
frozen wildcard-fill value-half, not a Kernel overwrite.

**The refinement M1 leaves open**: the 31 write-once skips were counted in total, not by value
kind, so whether the **expr Array's** write is among them is unmeasured. That is A3's first
instrument: count Array writes and skips separately at the bind.

**Q3 — `compile_order_class` is a genuine regression, not the recorded noise.** Alone on U1 it
fails at the first assertion (`control.len() == 2`, one Float kernel compiled); the recorded
process-global noise (`open-issues.md:359-360`) **passes** alone, and the test passed on A1. It
reads the compiled kernel-fragment count — a sibling consequence of the same displacement.

**The two non-classification failures read checker output, not classification**:
`an_output_position_that_is_not_a_buffer_is_refused` gets the codomain-position diagnostic twice
(`expected Struct, found Int` ×2 where 1 is asserted), and
`the_product_is_warp_uniform_not_wider` finds the warp-uniformity refusal **fails to fire** (the
program checks when it must be refused). Both are correctness consequences of the bind.

**The route conclusion: fix, and the fix is A3 — a SPEC proposal is not indicated.** The SPEC's
unify works on the dynamic path (language stayed `289/0` under U1); what breaks is the frozen
materialization. §Clone's own sentence is the criterion the frozen walk misses: *"Cloned
polymorphic nodes should be detached from it from its class, while placed in new class that has
the same tolopogy"* (`language-spec.md:311-319`). A per-call clone must be **detached into a fresh
class with the same topology** before the argument unifies with it; the frozen walk's image is over-broad: the detach/regroup machinery is SHARED
(`regroup_clones`/`unify_clone_groups`, `apply.rs:473-505`, both walks call it), but the frozen
walk inserts EVERY walked node into the remap while the dynamic walk inserts only per-call clones
(after `!per_call -> return node`), so under unify the remap is over-broad. **The causal claim that
followed here — a write-once collision between the template expr structure and the argument
value — was REFUTED by measurement (§12): no Array write is skipped, and the Array IS written,
just after the lowering reads it.** The fix surface is OPEN pending the ordering investigation in §12; the route (fix rather than a
silent SPEC deviation) stands, but step 2 is held because its premise was the refuted collision.

**Perf is secondary.** Bounding the O(members) walks is necessary for the unify to be affordable
but **not sufficient** to make it work: even with perfectly bounded walks the classification stays
inert until the value-half wiring is restored. Correctness is the primary blocker.

## 11. A2 landed — the shared read view, one openness reader, one claim rule

Dev `0ae75181` (commits `8d1f5658`, `c79cd2c8`, `0ae75181`; 5 files, +263/−78), ff-merged in the
owning checkout. Behaviour-preserving: no `per_call`, `rule.inside`, `copy_parameter_slots`,
`value_holds_foreign_function`, `polymorphic_over_scope` or `ARTIFACT_FORMAT_VERSION` line moved,
and the bind site is untouched.

- **One read view over `AnyNodeId`** (`static_module.rs`, +183): value, operation, owner place,
  flag, `wildcard_fill`, `runned`, and `module_key` — F3, the "which module am I reading"
  parameter — plus `clone_class_root`/`clone_class_members` for A3's unified decision. A frozen
  ref is read through the **`Arc` the walk holds**, never re-resolved through the registry:
  eviction refuses while refs are live, so the held source is the robust read.
- **One openness reader**: the fast path moved INTO `node_value_in_module`, and
  `static_answer_elements_are_undecided` is gone; both walks now use
  `crate::apply::answer_elements_are_undecided` with that one reader. The retired reader's
  invariant migrated with it (each item read where its own slot lives), and `node_value`'s "no
  reader copies it into a dynamic node first" survives verbatim.
- **One claim rule**: `answer_claims_run` (`apply.rs`), since `!undecided` and
  `evaluated_deep.is_some_and(|e| !e.undecided)` are the same predicate.

**Gate, merged tip, parent-run**: comments-check exit 0, `cargo fmt --all -- --check` exit 0,
`cargo check --workspace --tests` exit 0 with zero `Compiling` lines naming another worktree;
lowlevel `167/0/1` (0.21s), highlevel `136/0` (11.51s), language lib `69/0`, language external
`289/0` (**3.41s**), compute-gpu `94/0` (1.70s). Compute verified **by name**: `diff fail-a1.txt
fail-a2.txt` **empty** (68 = 68), `215 passed; 68 failed; 1 ignored` in 554.47s against the
baseline 575.67s; `compile_order_class` alone `1 passed`. **Perf neutral**: two isolated language
runs both 3.42s (the 5.78s in the contended round was the parent building on the main target at
the same time, not the view).

**One more divergence recorded for A3**: the dynamic wildcard-fill mint does NOT enter the remap
(`function.rs:843-849`, fresh per encounter) while the frozen one DOES (`apply.rs:569-578`, one
cell per template per apply, a diamond shares it). The two sides differ on diamond sharing of a
fill; measure before changing either.

## 12. A3 step 1 REFUTED the collision mechanism — it is a read-before-write ordering

impl measured the question M1 left open, with env-gated instruments by value kind (never
committed; tree restored to `6200c92c`, residue 0, workspace check exit 0). Record:
`.worktrees/unified-interface/.scratch-core/A3-step1-finding.md`, logs `a3-step1-run.txt` /
`a3-step1-run2.txt`.

**By-kind counts on the U1 repro**: Array writes **2512**, Array write-once skips **0**; Kernel
writes **1**, Kernel skips **0**; all **31** write-once skips are `kind=other` — scalar type-cells
(prelude positions), not the jit result expr. So the expr Array write is **not** among the skips.

**Event order** (`a3-step1-run2.txt` line numbers): BIND at 3966 (node 368, param 369 / arg 365 /
applied 403; `param_val` and `applied_val` both Array layout 1, unchanged before/after) → the
check-stage lowering reads `value_of_node(index 405, target 368)` with `has_array=FALSE`,
`has_op=true` at **4141** → `kernel_id_of=None`, `pending_kernel=false` → the inline-callee refusal
fires → the concrete expr Array is written to node 368 at **4325** (`A3-SET kind=Array`, no skip)
→ later reads see `has_array=TRUE` (4328+) → the Kernel is written to node 1057 at 4435.

**Verdict**: §10's causal claim is refuted on **both** clauses — no Array write is write-once
skipped, and the Index target does not *lose* its Array; it is written to node 368 **after** the
lowering reads it empty. The mechanism is a **check-stage read-before-write ordering**: the unify
bind defers the result expr Array's materialization past the lowering's classification read, where
A1's copy bind had it present. This is consistent with M1 Q2 (neither write-once-skip nor §Merge
propagation of the Kernel) and now the by-kind delta is measured.

**What stands**: the remap-membership code fact (dynamic inserts after `!per_call -> return node`,
frozen inserts every walked node) — a verified fact about the code, not refuted. **What falls**: the
causal link from it to the Array displacement via a write-once collision. A3 step 2 (narrowing the
remap) was premised on repairing that collision, so it is **held**; narrowing the remap may still
change the target's identity or timing, but it must not be sold as the regression fix.

**Replan in flight**: recon is mapping the ordering read-only — who writes the result expr Array
and in which pass, why the copy bind is eager (`propagate_class_value`) while unify is not
(`evaluate_pattern_argument` to pattern depth, then `unify_ref`), what the dynamic path does to have
the value present in time, the target-identity delta (954 vs 368), **what the SPEC says about when
an apply's result is available** (§Operation/§Merge/§Function/§Clone — if it is silent, that is the
SPEC-proposal candidate), the checker/compute stage boundary under contract rule 5, and the honest
fix surfaces with sizes. The route remains "fix or SPEC proposal", never a silent deviation.

## 13. The ordering mapped: the frozen check path does not run what it checks

recon's read-only map (`.scratch-core/A3-ordering.md`) names the writer, the reader and the stage
boundary, and redirects the fix.

**The asymmetry.** Dynamic `function_apply` (`function.rs:199-270`) instantiates, **evaluates the
body** (`:259`) and **wires the result** (`:267`) during the checker's own signature pass, so the
Apply node holds its expr Array before any later reader looks. The frozen non-reporting path
`materialize_static_return_apply` (`static_module/apply.rs:230-289`) materializes, applies the
return's type side, records the claim, binds and records `applied_exprs` — and **never** calls
`evaluate_node(applied)` or `wire_apply_result`. Its sibling `static_function_apply` does both
(`:94`, `:104`) but is reached only from the evaluation Apply arm (`evaluation.rs:350-351`). The
writer is `wire_apply_result` (`apply.rs:390-430`, Array arm `:403-408`: write the result into the
Apply node, then unify it with the applied expr). **The copy bind was the stand-in for that missing
evaluation step**; the unify ruling removed the stand-in and exposed the gap. This is the same
thesis as §1, one level deeper: the frozen path does not do what the dynamic path does.

**Two corrections to the earlier record.** (1) The bind neither creates nor destroys the Apply
node's Array: step 1's log shows `param_val` and `applied_val` both `Array layout 1` *before* the
bind, so the copy's eagerness is not established as why A1 worked at that node. (2) M1's
"954 under A1 vs 368 under U1" is a **cross-run** comparison and node ids are not stable across
runs, so it is not an identity claim. `value_of_node` (`compute.rs:5872-5901`) has **two exits for
one node** — `:5893` peels to the value slot when an Array is present, `:5897` returns the operator
node when it is not — so both readings can be one node at two moments.

**The SPEC is silent on the ordering.** §Operation (`language-spec.md:111-115`) says an operation
is resolved when it has runned and that a produced value is unified with the class representative;
§Merge (`:106-108`) is a *forcing* rule (evaluating any member runs the class's operations). Neither
says when an application's result becomes **observable to a reader**, and §Clone constrains which
nodes clone, not when a result is readable. The book is absent from this checkout. **The exact gap**:
§Function/§Operation never relate *checking* an application to *its result being observable*. A
sentence would be needed in one of two directions — "an application's result is observable from the
moment the application is checked", or "a reader that needs an application's result must force the
application" — and that choice is the same choice as fix surfaces (i) vs (iii).

**The stage contract.** Compute reads purely, so contract rule 5 is not violated by compute; but the
jit compiles its body **before** the applies inside it are evaluated, so the read is timed against
an incomplete write set *within one pass*. The lowlevel's own rule is run-then-read
(`function.rs:259` then `:267`; `static_module/apply.rs:94` then `:104`). Three read tiers exist —
raw `node_value`, the `value_of_node`/`kernel_id_of` peel, and the forcing
`evaluate_node`/`evaluate_node_deep_ref` — and the lowering uses the weakest.

**Fix surfaces** (recon §7): (i) compute forces the callee's value before classifying, ~2-5 lines ×
3 sites (`body.rs:1913`, `:1757-1759`, `:1926-1928`), a check-stage *policy* change, inert if the
cause is structural; (ii) the frozen tail evaluates and wires like the dynamic path, ~5-10 lines,
**symmetry** but it changes the invariant at `apply.rs:8-14` ("an apply is not a normalization …
the callee's own cells are not settled") and may double-run bodies; (iii) a checker stage-ordering
fix, the largest blast radius, needing an Invariant change and likely a SPEC sentence; (iv) the
selection exit — **orthogonal to the ordering**, able to change only which node the callee's value
half names, so not to be assumed as the fix.

**Next**: the discriminator run (impl, temporary instruments, one run per side, ids compared only
within a run) decides cause 1 (read timing) from cause 2 (structural misnaming via
`expr_value_half`'s own state dependency, `resolve.rs:412-414`), and prints the checker pass at both
the read and the write. Step 2 stays held until it lands.

## 14. D1 decided it: the bind changes WHEN the jit runs (cause 1, pass ordering)

impl's discriminator (`.worktrees/unified-interface/.scratch-core/D1-discriminator.md`, logs
`d1-u1.txt` FAILED exit 101 / `d1-a1.txt` ok; one run per tree, ids compared only within a run;
six files instrumented and restored, residue 0, tree at `6200c92c`, workspace check exit 0).

- **Naming is identical in both trees**: the callee `Index` 405 targets node 368, the bind's Apply
  node, and reads slot 0; `target_eq_apply=false` (the apply being lowered is 471, k1's jit),
  `target_eq_applied=false`. `expr_value_half(368)` at BUILD is `None` in **both** trees, and A1
  works despite it — so the structural cause (cause 2) **falls**, and with it any claim that the
  selection exit repairs this defect.
- **The decisive delta is the pass**: U1's lowering read is in **CHECK** (`d1-u1:777`),
  `kernel_id_of=None`, and 368's Array is written **later**, still in check (`:789`) —
  read-before-write, refusal `stage:Check` (`:826`). A1's lowering read is in **DEF-STMT**
  (`d1-a1:803`), `kernel_id_of=Some`, and 368's Array was written **earlier** in **def-fn**
  (`:776`) — read-after-write. **A1 has only one lowering read: under the copy bind the jit does
  not lower during check at all.**
- **Mechanism**: `apply_parameter_check` → `evaluate_pattern_argument` →
  `evaluate_node(argument)` (`function.rs:756`, "one force answers both reads") **forces evaluation
  during check**, so k1's jit compiles before the sibling frozen apply 368 has produced its Array.
  `copy_parameter_slots` → `copy_slot` only writes/propagates a class value and never evaluates, so
  the jit stayed lazy until def-stmt, after 368 ran in def-fn. At read time `value_of_node` takes
  `:5897` (fallback, no Array) under U1 and `:5893` (peel) under A1 — one function, two exits,
  decided by Array presence.

**So the defect is a latent ordering dependency in the kernel lowering**: it reads a sibling
apply's result without anything guaranteeing that apply has run, and the copy's laziness happened
to satisfy it. The unify's forcing (which SPEC §Merge:106 itself sanctions) exposes it.

**The permission story for (ii) is better than first read.** The `# Invariant` on
`static_function_apply` (`static_module/apply.rs:9-12`) already sanctions *"the callee's body is
cloned into the caller and the third sanctioned unify runs (the parameter check and the return
wiring); no new callee is minted and the callee's own cells are not settled"* — and that function
**already** evaluates (`:94`) and wires (`:104`) under it. Option (ii) extends the same sanctioned
action to the sibling non-reporting path, evaluating the **clone in the caller**, not the callee's
own cells; and SPEC §Operation:115 ("one operation only run once, gated by runned flag") is what
prevents a double run. So (ii) may be a MIGRATION of that sentence to the shared site rather than a
rule change — the maintainer decides which reading holds. Option (i) would make compute trigger
evaluation, straining contract rule 5 (compute only reads); (ii) keeps compute purely reading and
repairs the language layer's asymmetry, which is the unification thesis itself.

## 15. E1 closed (ii): the kernel value is a compute-lowering product

impl measured both (ii) variants as uncommitted edits on the U1 bind over the A2 tree, with
env-gated instruments, then restored all nine touched files (residue 0, tree `6200c92c`, workspace
check exit 0). Record: `.worktrees/unified-interface/.scratch-core/E1-efficacy.md`, logs
`e1-{a,b}-*.txt`. **Verdict: neither variant works; (ii) is inert for this defect and 0 of the 12
regressions move toward passing.**

**The decisive reading.** Both variants DO make the Index target's expr Array present at the
check-stage lowering read — the `:5893` peel exit fires (A: `target368 has_array=true
evh=Some(399)`; B: `target378 evh=Some(415)`) — **but `kernel_id_of` stays `None` and the refusal
still fires**. The value-half (399/415) is the **fresh empty hole**, not the kernel, and the
`ComputeValue::Kernel` value (A:1055 / B:1231) first appears in the trace **after** the refusal. So
the kernel value is a **compute-lowering product available only at a later stage**, not a lowlevel
`evaluate_node` product at check. Forcing the frozen tail to evaluate and wire writes the expr
**structure** faithfully and still cannot produce the value the classifier reads. The §13 node-shape
subtlety is confirmed and extends to variant B: even the body-result clone
(`static_node_apply(r#return)`) evaluates to an Array whose value-half is empty at check.
**The MIGRATE-vs-REWRITE choice for (ii)'s invariant is therefore moot for this defect.**

**Side effects, measured**: `compile_order_class` alone gets **worse** under A (zero kernels, left
0 right 2) and stays at one under B; `an_output_position_that_is_not_a_buffer_is_refused` keeps two
diagnostics (the wire's unify records Struct-vs-Int twice at check — the parked duplicate-diagnostic
family); `the_product_is_warp_uniform_not_wider` is disturbed differently rather than fixed (A: "not
64-lane uniform"; B: a wrong-shape diagnostic). **No double run** was observed (the bind node's
operation runs once in check and is stamped there; no def re-run) — with the caveat that inert
variants would not expose the wire-writes-without-stamping risk. **Laziness canaries survive**:
lowlevel `167 passed; 0 failed; 1 ignored` under both. Errors added on the jit repro: eval 0, unify 0;
the exception is `an_output_position` (unify 2, check_failed true), so an efficacious (ii) would have
needed the error-discard widening.

**The defect, reframed.** Compute compiles k1's jit **before** k0's, although k1's body depends on
k0's kernel identity. Under the copy bind the jit stayed lazy until def-stmt, by which time k0 had
lowered in def-fn — compilation happened to run in **dependency order**. The unify's check-stage
forcing (`evaluate_pattern_argument` → `evaluate_node`) breaks that accidental order. This is a
**compute-internal ordering dependency**, not a language-layer wiring gap, which is why no amount of
symmetry in the frozen tail repairs it.

**Route**: (i) — dependency-ordered kernel compilation, the jit lowering forcing the kernel
production it reads before classifying — or (iii) a SPEC proposal fixing the observability stage.
(ii) is closed. A read-only change-map for (i) is being prepared while the maintainer rules.

## 16. F1: (i)-A works but is partial and costs 2.1×; (v) is the smaller candidate

impl built the dependency pre-pass as an uncommitted edit on the U1 bind over A2, measured, and
restored the tree (`6200c92c`, residue 0, workspace check exit 0). Record:
`.worktrees/unified-interface/.scratch-core/F1-force-efficacy.md`, logs `f1-*.txt`, `fail-f1.txt`.

**Efficacy is real, not inert.** The force runs the Jit arm **at check**: the sibling lambda's
domain TYPE value is a decided `Array layout 1`, so `function_materialized` (`compute.rs:3163`,
gate `:1271`) passes, the Kernel is interned, `kernel_id_of(callee)` becomes `Some`, the `:5893`
peel fires and the refusal stops — `jit_cross_kernel_call` `ok. 1 passed`. **No double run**:
`evaluate_node_deep_ref` stamps `runned`, so the wire-writes-without-stamping risk did not bite.
Zero added eval/unify errors; lowlevel laziness canaries `167/0/1`.

**But partial and expensive.** Compute sweep `208 passed; 75 failed` in **1167.09s** against A2's
`215/68` in ~550s: **5 of the 12 U1 regressions fixed, 7 remain, 0 baseline regressions** (the 68
untouched), 0 improvements, and a **~2.1× wall-time regression** — a stop condition under the gate.
Two measured reasons: (1) the pre-pass's callee shape is **too narrow** — in
`jit_cross_kernel_wrapper` (`compute.launch k0 x`) the Apply's callee is `compute.launch` and k0 is
nested as an **argument**, so forcing the callee's fallback target yields no kernel; reaching the
other four ordering shapes needs `Lower::apply`'s full routing (routed operator / frozen residual /
launch-wrapper / subexpr / multi-value). (2) **read-first-then-force cannot bound the cost**:
`kernel_id_of` is None for most callees at check, so the pre-pass forces inline lichen calls and
routed operators too, running many sibling applies eagerly across all 284 tests. The three
non-ordering failures are unmoved (`compile_order_class` alone still 1 kernel,
`an_output_position` still two diagnostics, `the_product` still not 64-lane uniform). Cyclic kernel
dependency stays OPEN (no new test permitted); termination is code-reasoned from the runned stamp,
`visiting`/`VisitGuard`, the deep cycle cut and the depth limits.

**(v), assessed read-only**: make jit compilation a **definition-pass activity by design** — the
Jit arm returns None at check and compiles at def, turning A1's accidental laziness into a rule.
A phase signal plus a return, **no new graph walk**; it aligns better with contract rule 5 (no eager
check forcing; the jit runs through the normal language apply at def, so writes stay in the language
layer), and it may repair **more** than (i)-A because A1's timing had `an_output_position` passing.
Its cost is that it is a **design ruling** — "jit is a definition-pass operation" — changing when jit
runs program-wide, with a check-None / def-Kernel double evaluation that must be shown benign
(`ran_undecided` stamps on None, value-None keeps `has_no_result_yet` true). The suite is the
evidence that check does not need kernel identities: A1 passed every `jit_cross_kernel_*` with the
jit lowering at def. A measure-first run of (v) is authorized; the permanent fix awaits the ruling.

## 17. V1: the 12 regressions have TWO independent causes, and the ~2× is the unify's own

impl measured (v) as uncommitted edits and restored the tree (`6200c92c`, residue 0, workspace check
exit 0). Record: `.worktrees/unified-interface/.scratch-core/V1-defer-efficacy.md`, logs `v1-*.txt`,
`fail-v1.txt` (71).

**The signal search, measured.** `report_type_side_failures` is a BUILD-level flag (Check vs
Evaluate build), and the language pipeline and the compute tests both run a single `Mode::Evaluate`
build, so it is false at the premature and the good lowering alike — it does **not** distinguish
check-stage from def-stage. `deep_depth` does: U1's premature k1 lowering is `deep=0 phase=check`
(the bind's shallow `evaluate_node`), A1's good lowering is `deep=4 phase=def-stmt`
(`evaluate_node_deep`). So the existing signal is `deep_depth`, needing a read-only accessor.

**The stamping discovery — the simple gate is broken.** A None operator result **stamps
`runned=true`** (`evaluation.rs:698-702`), and `evaluate_node_body` then returns None at the
run-stamp gate (`:197`) **without re-running**, even though `has_no_result_yet` is true. So the
premise that a value-None node naturally re-runs at def is **refuted by measurement**: with only the
`deep==0` gate, k1 is deferred and stamped at check, k0 compiles at def, and k1 never compiles —
FAILED. No checker-side reopen exists between check and def (`reopen_stalled`/`reset_run_stamps`
live in the compute graph/loop machinery, `compute.rs:7538`/`:7793`). **(v) therefore needs TWO
changes**: the gate plus a new call to the existing `reopen_stalled(root_block)` before the def
pass. With both, k0 and k1 compile at def-stmt and `jit_cross_kernel_call` is `ok. 1 passed`.

**The decisive gate.** Compute `212 passed; 71 failed; 1 ignored` in **1092.52s**; the failing-name
diff vs `fail-a2.txt` (68) is **exactly the three non-ordering tests**, the 68 baseline untouched.
Against U1's 80, (v) fixes **9 of 12** — every ordering `jit_cross_kernel_*` — and leaves
`compile_order_class`, `an_output_position_that_is_not_a_buffer_is_refused` and
`the_product_is_warp_uniform_not_wider` failing **identically to U1**. Every other suite is green and
perf-neutral (language external `289/0` 3.37s, lib `69/0`, lowlevel `167/0/1`, highlevel `136/0`
10.95s, compute-gpu `94/0` 1.72s). Nothing at check needs a kernel identity: the nine pass with the
jit None at check.

**THE TWO-CAUSE SPLIT.** U1's 12 regressions are (1) **jit timing, 9** — the bind forces the jit's
evaluation at check before the sibling kernels exist, so `kernel_id_of` is None; fixed by deferring
the jit (v) or forcing the sibling (i). And (2) **the unify bind's class-merging / diagnostic face,
3** — class contamination in `compile_order_class`, the duplicate diagnostic in
`an_output_position`, the missing warp-uniformity refusal in `the_product`. These are **not** jit
timing: they fail identically with and without the gate, so **neither (i) nor (v) addresses them**.

**The ~2× is the unify's own cost, not the fix's.** U1 with no gate at all was already **1286.88s**
against A2's 554.47s; (v) is 1092.52s and (i)-A 1167.09s — both *faster* than U1 alone (they also
remove nine failing tests' extra work, so the comparison is not perfectly controlled, but the order
is clear). The dominant axis is the class growth §9 measured: merged classes make every O(members)
walk bigger. So bounding those walks is a **prerequisite for the ruling being affordable at all**,
not an optional extra, and the broad `reopen_stalled` is not the main cost.

**Where that leaves the fix.** Neither candidate alone restores `215/68`. A complete fix needs the
jit-timing face, the class-merging/diagnostic face, and the walk bounding. The one untested
combination that could address the second face is **the selection exit (A3 step 2) together with the
unify bind**: the three are class-merging consequences, and the frozen walk's over-broad remap
(§11) is what decides which clones share a class — so narrowing it is directly relevant to them,
even though it is orthogonal to the ordering cause. That is the architectural goal anyway.

## 19. Q2 closed by falsification, and the parked test it rested on cannot run

Q2 (the duplicate codomain diagnostic) was briefed as a pre-existing dev defect whose fix
suppresses the runtime apply's join. **Both halves are wrong, measured** (impl,
`.scratch-core/Q2-premise-falsified.md`, logs `q2-*.txt`, probe kept):

- **No duplicate on dev.** `an_output_position_that_is_not_a_buffer_is_refused` has `count=1` on
  dev — one diary-owned check-time error, **no orphan**. Under U1 it is `count=2`, and the two are
  **two check-time diary recordings** (owners `Some(0)` + `Some(1)`), not a diary + runtime-orphan
  pair. Valid-syntax variants and a decided `Int` input are 1 on dev too.
- **The named parked test cannot run.** `compute.rs:1291`
  `a_program_value_where_a_buffer_belongs_is_refused` carries `#[ignore]` but **no `#[test]`** —
  dead code with a "never used" warning, so `--ignored` matches nothing — and its program uses
  **rotted array syntax**: the language parses `[template, length]` as a template and
  `Template[e1..en]` as an instance (`parser ast.rs:264-273`), so a bare eight-element array is a
  parse error. Its own doc comment says "the run-time join never sees it".
  `graph_structure.rs:319` is a real `#[test] #[ignore]`, but it fails on a **cfg-structure**
  assertion, not a diagnostic count.

So the prose I briefed from ("the apply's join is not yet suppressed") describes a prescription with
no observable dev target. **Q2 is U1-only**, and its real mechanism is a second check-time diary
recording induced by the unify bind. A lesson for the record: a parked test's prose is not a
measurement, and a test that cannot run is worse than none — it claims a guard it does not hold.

**This closes the last dev-landable face.** All four — the jit compile timing, Q1 (the lowering reads
the class's current value instead of the declaration's class), Q2 (the second check-time diary
recording), Q3 (the warp-uniform demand not firing) — are **unify-only**: unobservable on dev, and
only observable on `u1-parked` / `stage2-parked`. The unification ruling therefore cannot proceed as
independent dev slices: it needs **one integration branch** carrying the unify plus the four faces
plus the O(members) walk bounding, each slice gated by the compute surface shrinking from 80 toward
the accepted 68, with the final gate A2's parity **and** the ~2× clawed back. That, or the ruling is
shelved with everything parked and recorded.

**A separate, permission-gated hygiene fix is proposed**: give `compute.rs:1291` a `#[test]` and
valid array syntax so "one disagreement, one report" has a runnable guard. That is a test edit and
needs the maintainer, but until it lands the defect has no guard at all — the prescription lives in
a sentence that is never executed.

## 20. compile_order decomposed: the loop's entering-state element, not the membership rule

Three measurement rounds took Q1 from recon's verdict to the exact site. Each round falsified the
round before it, and the sequence is worth keeping because it shows how far a plausible reading can
be from the mechanism.

- **The verdict was wrong.** recon's "the lowering reads the class's current value instead of the
declaration's class" was drawn from the test file's own `//!` header — which describes the defect
the file was **written against**, not the U1 failure. What fails under U1 is a **count**
(`left: 1, right: 2`) because `attend`'s compile **refuses**: "a kernel body reached a node with
neither a value nor an operation". Neither test in that file carries `#[ignore]`. The same trap had
already cost Q2, where a parked test's prose described a prescription whose target does not exist.
**Rule: a test header or a parked comment describes history; read the assertion text of the actual
run.**
- **Not filledness.** Dev carries **more** bare placeholders than U1 (scores 21/500 vs 17/500,
attend 29/638 vs 24/638) and compiles both kernels, so the defect is not an empty cell — it is
**which cell the walk enters**.
- **Not the operand edge, not the membership answer** (item 8, one process, an env-gated bind
toggle). The apply's four operand items match position by position in op/val/value-half/class-root;
`parameter_path` refuses **both** the dev cell (an Apply, own class root) and the U1 cell (a bare
cell merged into a projection class). So the rule is not what flipped — it is **handed a different
kind of cell**. A declaration-anchored `parameter_path`, a class snapshot, or a provenance
membership rule would all miss it.
- **The exact edge.** `Lower::loop_state`'s entry-value map — `body.rs:2192`, whose value comes from
`Module::element_value_node` (`resolve.rs:504-517`), a reader that **peels an element's Array to its
position-0 value half**. The deciding element is the `@loop` call's **third state slot**: dev's
element **is the Apply** (holds no value, so the reader takes its identity branch) and the
computation is lowered; under the bind the element **holds an Array** (layout != 0), the peel
returns the bare requirement cell `24755`, and the compile refuses. Slots 0/1 behave identically in
both modes — both peel to bare domain leaves that `parameter_path` resolves — so **"bare" is not
the defect; peeling to a bare cell instead of the computation is.**
- **Ruled out by the same runs**: `resolve_entry` (the fatal backtrace never passes through it),
`loop_owner`'s class clause (never a sole decider), `parameter_path` membership, the operand edge,
`apply_argument`.

**It is the value-half wiring family again** — as in [any-removal-migration](any-removal-migration.md)
rework 7 and in M1 (§9) — now with an exact site. The reader does not choose wrongly; it faithfully
reports position 0 of what the element holds. **What changed is what the element holds.** The open
question is **which write puts that expr Array on the slot-2 element**, and then the repair is
either (a) the bind not displacing that element's value half (the bind's own correctness face) or
(b) `loop_state` lowering the element's computation when the peel lands on an open cell (which
touches `loop_state`'s quoted invariant, "the entering call's own element list" — the maintainer's
call).

## 21. The compile_order write is named, and the repair is over-determined (so only the merge can fix it)

Two further rounds close §20's open question and re-aim the repair.

**The write is `fill_class_holes` (`equality.rs:168`, `node.value = Some(value)`), reached from
`add_equality`'s `(Some(value), None)` arm (`:127`), driven by the bind's own per-position parameter
check** (`apply.rs:77` `unify_ref` inside `for slot in 0..width`). Deciding pair: `ADD a=24754
b=20693 a_val=true b_val=false a_vh=Some(24755)` → `FILL node=20693 rep=20693 Array(layout 1)`.
`b=20693` is the `@loop` idx-2 element and `24755` is the bare cell; `rep == node` proves a **hole
fill**, not an overwrite. Dev's counterpart pair is `a_val=TRUE b_val=TRUE` → `_ => {}`, no fill.

**A trap that cost a run**: `set_node_value`'s `# Invariant` calls itself "the one writer of the
slot", but `fill_class_holes:168` and `propagate_class_value:519` write member slots **directly**.
Watching only `set_node_value` + `unify` gives a **false negative**. That invariant sentence is
false as written and should be corrected at its site (an Invariant change, so the maintainer's).

**Repair (a)-as-normalize is refuted.** `evaluate_node_deep_ref(argument)` before the bind is
**inert** on the unify tree (len stays 1; the bare-fill census 338 → 338) because all 644 deciding
empty cells have **no operation**, are their own class root, and are nobody's operand — there is
**nothing to run**, so normalizing cannot fill them. And on the **dev copy bind** the same one-liner
**regresses dev**: `213 passed; 70 failed`, exactly two new names — `compile_order_class` and
`an_output_position` — with the exact unify symptoms.

**The inverse (force less) is refuted too.** Skipping the check-time force on the unify tree leaves
**both** symptoms (len 1 / len 2) while materially changing the graph (fills 1144 → 818, bare
338 → 320). Its dev-side form (skipping the type-side force) is parity-identical by name,
`215/68` in 527.08s, all suites green — a keeper fact that buys nothing for this defect.

**The key new fact: the symptom is OVER-DETERMINED.** Early forcing **alone suffices** (proved on
the copy bind, no unify and no merge) **and the class merge alone suffices** (proved with the force
removed). So neither "force more" nor "force less" can repair it — **only the merge side can**.
What remains: (1) the state element must not be empty when the merge runs, or (2) **the merge must
not fill a state-element slot with a value whose value half is an open requirement cell** — (2)
touches `fill_class_holes`' invariant and `add_equality`'s caller contract, so it is the maintainer's
call. **Q3 (`the_product`) moved under none of these** and stays a separate cause.

One device worth keeping: the forcing one-liner **reproduces Q1 and Q2 on dev without the unify**
(two new names, same symptoms), so those two faces now have a dev-side fixture that needs no parked
bind — useful for any fix's regression check, and it is the measurement that proved forcing is
sufficient.

## 22. Q3 closed: the bind's discard is incomplete, not the certificate's order

The ref-tolerant read landed as `c0e3a26f` (one file, +7/−1) — the dynamic arm still
`class_value`, a frozen ref answers through `node_value`, and the site's invariant keeps its
sentence verbatim while gaining the rule the new arm implements. Gate on dev: every suite green,
compute `215/68` **identical by name**, wall neutral. Its census is the reason it was worth
landing on its own merit: **656 of 656** static hops on dev and **654 of 654** on the unify tree
were **abandoning a decided value that `node_value` answers** — a read losing an answer every
single time, inert on dev only because no dev test reaches those slots.

**Q3 is then closed, and not by an operand order.** The losing comparison carries the same values
in the same order in both trees (`va=USize(32) vb=USize(64)`); what differs is the **kind** of
diagnostic. `render.rs` renders `Attribute` as expected=value_b/found=value_a (correct) and
`Runtime` as expected=value_a/found=value_b (inverted), and `Diagnostic::mismatch` decides the kind
by checking `index.apply[i]` **before** the diary owner. Under the bind the apply path wins, and
the apply error that claims that index is a **different** error — a structural Buffer mismatch —
because the bind rolls back only `unify_errors`:

```
let recorded = self.unify_errors.len(); apply_parameter_check(...); self.unify_errors.truncate(recorded);
```

`apply_parameter_check` also pushes an `ApplyError{error_index: pre_unify_errors}` into
`module.apply_errors` (and `apply_error_nodes`), and **that survives the truncate**, still pointing
at index 0 — which after the rollback is a *later, unrelated* clash. Gated intervention
(truncating `apply_errors` and dropping the nodes) makes `apply=None`, restores the diary path,
renders `Attribute` with the correct order, and turns `the_product_is_warp_uniform_not_wider`
**green** while `an_output_position` and `compile_order_class` stay where they were.

**So the last Q3 gap is the bind's INCOMPLETE DISCARD** — `unify_errors` rolled back, `apply_errors`
and `apply_error_nodes` not — the **same family as Q2's duplicate report, one field over**. The
repair lives in the bind, so it is a no-op on dev by construction (dev's bind records no apply
error) and belongs to the unify slice, under the usual full parity by name. Open: whether Q2's
second report is the same field or a neighbour (it did not move under this rollback), and whether
`assert_errors` / extension records need the same treatment.

**A measurement rule earned the hard way**: **libtest captures a passing test's stderr and
discards it**, so an `eprintln!` instrument reads as "never fires" on a passing test — four rounds
were spent chasing producers that did not exist until `--nocapture` made the lines appear. Per-test
traces need `--nocapture`; suite totals need the thread-exit Drop printer. Every earlier census in
this note used the Drop printer and is unaffected, but any future "it never fires" claim drawn from
a passing test must be re-read with `--nocapture`.

## 23. Q2 is not a stale record: it is one slot checked twice, both failing

The incomplete-discard lens that closed Q3 does **not** apply to Q2, measured three ways in one
build: `apply_errors` at report time is **0 in both trees** (the Q3 mechanism is absent here), the
diary overlap census is **0 in both** (the truncate created no stale range — that hypothesis was
the diary's own "disjoint slices" invariant suggesting it, and it is refuted), and diary pushes with
`in_bind=true` are **0 in both**. The two entries share the expression and the slot
(`Loc{expr: ExprId(76), path: One(Type)}`, kind `Annotation`, ranges `0..1` and `1..2`, both
Int-vs-Struct) — but they are **two check sites**:

- `checker/structs.rs:1131` — the argument's **type cell** (`state[argument_expr].ty`) against
  `value_item(field, 1)`, deep-evaluated first;
- `checker/structs.rs:1177` — the argument's **compiled term position 1**, inside
  `for position in 1..width`, which **starts at 1 = the Type slot** and therefore re-checks the
  same slot through a different reader.

Dev fires **only** `:1131`, and the loop **is** reached on dev with its `pos=1` arm **succeeding**
(`actual=4831 expected=7458 ok=true`); under the bind the same arm fails
(`actual=4503 expected=4502 ok=false`). So the second check is not newly reached — **its answer
changed**, because the bind changed what the argument's term holds. The two arms read **different
cells** and on dev they disagree about which one fails, so they are not literally redundant.

**A complete discard does not close both**: restoring `unify_errors` + `apply_errors` +
`apply_error_nodes` turns `the_product` **green** and leaves `an_output_position` **byte-identical**.
So Q3 is a surviving record and Q2 is a second check that genuinely fails — different fields.

Q2's repair has two surfaces, both dev-visible and both needing their own measurement:
(1) **the double check** — skip `position 1` in the loop or drop the explicit `:1131`; but the arms
read different cells and disagree on dev, so **which reader is authoritative is a checker rule and
the maintainer's call**; (2) **why the argument's term position 1 holds Int under the bind** while
its type cell says otherwise — the value-half family again, one position over.

## 24. Q2 confirmed: reader B judges the RESULT of a call, and the bind is what exposes it

**This corrects §23's sentence** "the loop is reached on dev and its `pos-1` arm succeeds". That reading
group belonged to a **different argument** (`ExprId(131)`); the instrument did not print `arg_expr`.
With `arg_expr` printed, **dev runs no `pos=1` check at all for the failing argument**. The accurate
statement: for a non-call argument the `pos=1` check runs and passes **by identity** (after reader A's
merge); for the call argument it does not run on dev.

**The term of a call argument is the apply node and its array value IS the apply's result expr.**
Measured in both trees: `term=578 op=Some(Apply) tlen=Some(5) applied=Some(3758) cell=Some(576)`, and
`item 0 of the term's array = 3759` while `expr_value_half(applied_exprs[578]) = 3759` — **the same
node**. So reader B's `position 1` on a call argument judges the **result's type slot**
(`actual=4503 Int`, `expected=4502 Struct`, `cell=576` -> `same_actual_cell=FALSE`), not the
argument's type slot that reader A already compared from the cell. A **non-call** argument measured
for contrast has `item 1 == the cell`. **Nothing holds a wrong value**: `Int` is the correct type of
`i` and of its result.

**Why dev never reports twice** — measured at the guards: the same term is visited three times per
run; on dev **all three** visits have `tlen=None`, so `value_item(term,1)=None` and the loop
continues; under the bind the first two are identical to dev and the **third** visits with
`tlen=Some(5)`, so the loop proceeds and re-judges. The difference is **whether the apply's result
array is in the term's value slot when the loop reads it** — the bind materialises it during check;
dev never does.

**A second reason the double check is normally invisible**: reader A's `check_unify` **merges the two
classes when it succeeds**, so reader B then compares a class **with itself** and passes trivially.
Only when reader A **fails** (no merge) can reader B re-judge the same disagreement — and then only
if the term exposes an array.

**Three repair surfaces, none chosen**: (a) the loop's start index — it starts at `1`, the TYPE slot
reader A owns, while attributes begin at `EXPR_ATTR_BASE = 2`; starting at `2` removes the
re-judgement, but on dev some `pos=1` checks pass with `same_expected_cell=FALSE` (655/684, 45/1316),
so it is **not obviously free** and is under census; (b) reader B reading the **cell** for a call
argument instead of indexing the result's type slot; (c) the bind leaving the result array in the
term's slot at check time — the forcing family already measured out. **Which reader is authoritative
for a call argument's type is a checker rule and the maintainer's call.**

## 25. (a) start-at-2 is semantically safe; the open risk is only the diagnostic count

Census in one build, both binds from one binary: starting the term loop at 2 instead of 1 drops
**3544** `pos=1` checks (compute external 3539, language external 5; lowlevel, highlevel,
compute-gpu, language lib and compute lib **never enter the branch** — measured zeros under
`--nocapture`, not missing-line inferences). **Not one of them catches anything reader A did not**:
`new_a_ok = 0` and `new_a_absent = 0` on every suite, and reader A ran for **every** dropped check
(`a_failed` was `Some` in all 3544). Of the 3544, **99 fail and all 99 are duplicates** of a failure
A already recorded for the same argument; the other 3445 pass, and A passed them too. Extra
texture: 1221 of the 3539 compare the **literal same class pair** as A; among the 99 failures
`same_pair` is **0** — every failing `pos=1` check compares a *different* pair (the result's type
slot, §24) and still reaches A's verdict, across 10 distinct argument ExprIds. The census read dev
and did not change it: compute still `215/68`, `comm` both ways empty, every suite at baseline.

On the unify tree the same test shows `pos1 = 7` against dev's 6, and the extra one is **exactly the
`.y i` call argument** (`arg=ExprId(76) actual=4503 expected=4502 failed=true a_failed=Some(true)
same=(false,false)`), so **start-at-2 drops precisely the duplicate that gives that test two
messages**.

**What the census cannot answer**: the 99 dropped failures are *redundant `unify_errors` entries*, so
removing them **changes the error count** in those tests, and a test asserting a message count could
move. Only running start-at-2 can answer that — which is what a gated candidate does. For Q2 itself
the count change **is** the fix.

**Correction to §22's rule.** §22 said suite totals need the thread-exit Drop printer. In this build
that printer produced **zero lines on every suite** — the rule did **not** hold here. The numbers
above come from per-check verbose lines under `--nocapture`, which are capture-independent, and
that is the reliable route. One more: those counters are process-global and monotonic, so exit lines
must be **MAXed, not summed**.

## 26. The GC segfault is a PRE-EXISTING violation of D7's arena obligation

`zz_gc_panic_probe::gc_variants` segfaults on the parked unify tree (41% of runs in light
builds) and never on dev. The characterisation shows **the bind did not create the unsoundness**:

- **The read is of a released arena, and dev does MORE of them.** Full-slotmap-key census of
dangling reads per run: **dev 1616, stable**; unify 630-680, varying. Same sites on both.
**Dev reads freed payloads inside `apply_parameter_check` 218 times per run**, and dev performs the
same collects (10 triggers, all `deep>0`; 40 drops; 30 with recorded slices into the released
block, the collect's own root among them 5 times). Dev survives by **heap-layout luck**. The rate is
a property of the *build*, not of the bind: light builds crash 11/27, builds carrying an
allocation-provenance map (a HashMap insert per `alloc_array`) crash **0/22**.

- **It is a use-after-free, not a borrow spanning the free.** An RAII guard marking blocks whose
slices `apply_parameter_check` holds fired **zero** times, so no release happened under a live
borrow: **a node's payload handle outlives its arena** and is read later. Death is a
`STATUS_ACCESS_VIOLATION` (0xC0000005), not a stack overflow.

- **Keeping the arena alive makes the fault vanish and restores dev's five outcomes exactly**
(15 runs, 0 crashes) — which confirms the diagnosis and is explicitly **not** a repair.

**The contract being violated is written down and decided.** `array_items`' `# Safety`
(`utils.rs:11-18`) requires the caller to keep the node reachable for as long as the slice is read;
`equality.rs:688` asserts "nothing here drops a block", which the read-path collect can break; and
**`code-audit` `D7` decided exactly this shape**: accessors stay unbounded but `unsafe` behind **one
written contract**, with *"inside the crate the one-clause invariant is enforced by discipline and
review rather than by the type system"*. So this is **not a `wontfix` — it is a D7-contract
violation**, and the measurement shows the discipline is being broken routinely on dev.

**Consequences for this work.** (1) The parked tree's compute numbers were **never fully trustworthy
run to run** — the observed spread in dangling counts and the 41% crash rate sit under every parked
reading. (2) A UAF is P0-class and deserves its own decision-record entry, separate from the unify
work. (3) The unify integration should not be judged on parked-tree parity until it is fixed.

Two measurement rules earned here: **a slotmap key is index + version** — masking `as_ffi()` to the
index conflated `5v1` with `5v2` and manufactured 1148 fake dangling reads; and libtest writes
status to stdout and counts to stderr, which interleave mid-line in one file, so `[0-9]+` and
separate streams are required (§25).

## 27. Narrowing §26: what the P0 does and does not invalidate

§26 consequence (1) over-stated the blast radius. Measured from the sweeps already on disk, with
no new runs:

- **The parked tree's FAILING-NAME SET is stable.** Four full unify sweeps across **three different
  instrument builds** all printed `213 passed; 70 failed; 1 ignored` (848.31 / 831.61 / 831.74 /
  829.72 s), and all **six pairwise diffs of the four 70-name sets are empty** — one identical set,
  4/4. Dev likewise: four sweeps, `215/68/1` every time, names identical to `fail-a2`.
- **`zz_gc_panic_probe::gc_variants` is in none of the four failing sets.** It asserts nothing, so it
  can only fail by dying, and it **passed in every full sweep**; the ~41% rate was measured running
  it **alone** in a light build. The rate depends on the build *and* the execution history, which is
  exactly the heisenbug shape.
- **No landing gate ever used parked-tree parity.** N2, rule C and the ref-tolerant read were each
  gated on **dev** — identical names, every suite green — and dev showed **0 crashes in 39 runs with
  a stable 1616 dangling reads per run**. Those gates stand.
- **What is genuinely unstable on the parked tree**: the *dangling-read count* (630-680), the
  *crash rate of the assert-free GC probe run alone in a light build*, and that probe's report count
  (9 vs 6). Nothing else measured.
- **The one claim still resting on a single sweep**: start-at-2's unify-side "fixes Q2"
  (one ON sweep, `214/69`). Its dev-side parity is solid (two sweeps plus a same-build OFF); a repeat
  ON sweep is the bounded follow-up, not a wholesale re-run.

The policy from §26 consequence (3) still stands on its own merits: the defect is real and a future
run could differ, so **unify landability should not be judged on parked-tree parity until the
arena-lifetime violation is fixed**.

## 28. What start-at-2 withholds: 2218 unions of throwaway copies, not facts

The tautology hypothesis ("the withheld merges are already no-ops") is **refuted as stated** — but the
split names the shape exactly. All 3539 `pos=1` checks on dev compute, zero exceptions:

| count | already one class | outcome | field ref |
|---|---|---|---|
| 2218 | no | passes | **Static** |
| 1222 | yes | passes | Dynamic |
| 99 | no | fails | Static |

**Perfect biconditional: `already=false` <=> `field_ref_static=true`.** On the **language surface all
five checks are `already=true`, so start-at-2 withholds nothing at all there.**

**Why**, code read then measured: `Module::as_dynamic` (`static_module.rs:557`) returns a Dynamic node
as-is but for a Static ref calls `materialize_leaf` (`:550`), which **mints a fresh leaf**. Both
readers call the same pure `shape::value_item(field,1)` and then `as_dynamic`, so a **Dynamic** ref
gives both readers the **same** node (reader A already merged it — a tautology), while a **Static** ref
gives each reader its **own copy**, and reader B's copy is a different singleton — a real union.

**So what is withheld is a merge of a throwaway copy** of the frozen field type into a class that
**already holds the same fact** from reader A's merge with its own value-identical copy. The minted
leaf is bound to a local, passed to `check_unify`, then dropped; nothing stores it, so afterwards it
is reachable only through the class membership the merge itself created (read from code, not
instrumented). **Also: `same_cell=true` in all 3539** — reader B's actual side is *always* already in
reader B's type-cell class, so it never carries independent information.

Withholding therefore removes **redundant class members, not facts** — which also explains the
measured wall times (unify ON 820-822s vs OFF 829.72s; dev ON 539.52s vs OFF 541.77s: fewer members
in the O(members) walks). The 2218 are carried by 463 distinct argument ExprIds;
`.scratch-core/q2m-real.txt` names every check.

**Inertness**: the instrument changed nothing — compute `215/68` in both rounds, zero new and zero
gone, langext `289/0`, line integrity `raw = strict = 3539`.

**So decision #1 is not "withholds no merge" but "every withheld union merges a throwaway copy of a
fact already recorded, and the actual side is always already in the cell's class"** — a narrower
claim, still a checker-rule call rather than a proof of equivalence.

**A withdrawn draft worth the record**: an intermediate pass counted "776 distinct expected ids across
the 2218" and reasoned from it. Invalid — **node ids are per-module and repeat across the 284
 tests**, so cross-test id identity means nothing. Withdrawn; every identity claim here is within a
check.

## 29. WITHDRAWING §26-§27's framing: the dangling reads are the BIND's, and the holder is the root list

Two corrections, both from measurement, and the P0 draft must be rewritten to match.

**The holder is the polymorphic walk's own root list** — `propagate_polymorphic`'s
`let roots: Vec<NodeId> = self.nodes.keys().collect()` (`polymorphic.rs:87`, also `:193`).
`HELD_BY=root` in **4 of 4** attributed reads in every parked run; every other push site in
`polymorphic_referrers` (operand, array item, table key/value, function parameter/return/
return_type/assert/nodes, class root, class member) contributed **zero**. No structure holds an id
*into* a released block: every dangling id names a **live** node (`home_live=true`), and
**`home != alloc_block`** — **the node was re-homed to a surviving block while its PAYLOAD HANDLE
still points into the released one.** The dangling thing is the **handle inside a live node**, not
the id, and there is no side table (§26 said there might be; there is not).

**Withdrawing §26/§27.** "Dev performs MORE dangling reads (1616/run) and survives by heap luck; the
bind only changes the layout" is **wrong, and it was a measurement artefact of the parent probe**:
**`BlockId` keys are per-module** and the provenance maps were **thread-global**, so a block released
in one module condemned another module's *live* block with the same key (the probe builds five
variants and nine module constructions per run). The tell was a read with `home == alloc_block` and
`home_live=true` while the released set held that key — **impossible inside one module**, since
`drop_block` removes the key permanently. Re-measured with provenance keyed to a module construction
epoch: **dev 5 runs = 0, 0, 0, 0, 0; parked unify 5 runs = 460, 305, 18, 18, 314.** **The bind
introduces these reads; dev performs none on this repro.** The `# Safety` obligation is real and the
fault is real, but it is **the bind's face**, not a pre-existing dev violation.

**One reader path**: all 22 backtraces are
`apply -> instantiate -> instantiate_stamped -> propagate_polymorphic -> compute_polymorphic ->
polymorphic_referrers -> AnyHandle::items`, all `kind=array`, epochs 2 and 4. That walk's own
comment — "this walk releases no block" — is true; the hazard is a payload released **earlier** whose
handle a surviving node still holds. The earlier detector sat in `Module::array_items` and this path
**bypasses that helper** (it calls `array.items()` directly), which is why it read as zero: a false
negative. The detector now sits at `AnyHandle<[ArrayItem]>::items` / `<[TableItem]>::items`
(`lib.rs:263`/`:281`), the one choke point every payload read passes through.

**The mechanism, located by code read and not yet instrumented**: `gc.rs:165` re-homes
`node.block = target`; `gc.rs:288-291` relocates the payload only `if inside && let Some(value) =
value` — so **a node with no value at collect time is re-homed but its payload is not relocated**,
and a later class write hands it a shared payload handle into the released arena.
**`relocate_class_value`'s own `# Invariant` already names exactly this hazard**: "a class write gives
every member the same payload, so a member outside the vacated subtree can hold a payload the collect
is about to release." So the engine has a latent relocation gap that **the bind's class merging
reaches on this repro** — still a P0, but as the bind's face rather than a dev regression.

## 30. The ruled scoping is refuted: GC's roots are per-collect, not a module-wide live set

The maintainer ruled that no walk may root at the whole node table. Implemented faithfully — roots
mirroring `garbage_collect`'s three (every live block's `scope_root`, every `asserts[].condition`,
and every node in a live block's list whose function owner is live), applied to both
`propagate_polymorphic` (`:87`) and `upgrade_polymorphic_class` (`:182`) — it fails all three
measurements.

- **Dev parity fails: 10 regressions** (highlevel `134/2`, langlib `68/1`, langext `282/7`). The
  control on the **same build** with whole-table roots reproduces the baseline exactly, walls
  included, so the instrument is inert and **the scoping is the cause**.
- **The flag does not stay `Some`**: the counts of nodes left unflagged equal the failure counts
  exactly (2/1/7) and every one is `site=dynamic` — the dynamic clone walk's `debug_assert`
  (`function.rs:827`). Unflagged after the walk: 13 / 4832 / 141 / 449 / 338 by suite, versus **0
  everywhere** with whole-table roots. **The scoped root set is 5 nodes over a 40006-node table.**
- **The parked fault is not removed, and the walk is 40x slower**: `gc_variants` on the parked bind,
  five runs each — scoped roots crash **2/5** at ~115 s per run; whole-table roots crash 1/5 at
  2-3 s.

**Why, from the code.** GC's three roots are **per-collect roots for ONE vacated subtree** — they
decide what to move out of `source`; they are not a claim that everything else is dead. Read
module-wide they select almost nothing, because **most nodes in the table are unowned caller
cells**: `add_node` sets `function: None` and the owner is written later by `set_node_owner`, so root
3 picks only tagged clones; only `mark_scope_root`ed blocks have a `scope_root`; asserts are few.
Separately, `propagate_polymorphic` early-returns only while `polymorphic_unflagged == 0 &&
!polymorphic_dirty`; nodes permanently outside the roots' reach leave that counter **never
draining**, so the walk re-runs at every clone-walk entry — 40x on the probe, **92x** on langext
(326.01 s vs 3.55 s).

**What this settles**: the root set the ruling names cannot substitute for the whole table — it
breaks the clone walks' own invariant **on dev**, with an inert control, and does not remove the
fault. **What it does not settle**: whether the direction can be realised at all. Two shapes remain:
(a) a root notion that is both GC-guaranteed and covers every node a clone walk reads — the clone
walks read **unowned caller cells**, which the module does not record as rooted today; (b) keep the
whole-table walk and make its guarantee **true** by repairing the relocation — `relocate_class_value`
repairs one member's slot, while `relocate_node_value` is `set_node_value` with no propagation and
its `# Invariant` claims "the collect visits every class member itself".

The whole-table walk is what the clone walks' invariant already assumes, so **(b) is the shorter
path**; (a) needs a root the module does not have.

## 31. The origin: the node that ENTERS its own class is the one nobody repairs

§29-§30's timing story is refuted on both clauses. At the end of `garbage_collect`, the live nodes
still holding a payload into the just-released arena are **outside** the vacated subtree
(`rehomed=false`, `had_value_at_entry=true`) — not valueless, not re-homed.

**The measured biconditional, 24 of 24 rows across three parked runs**:

- a node reached as a **returned member** of a class walk -> `relocate_class_value` runs -> it does
  **not** dangle;
- a node that **enters its own class** -> `repair_times=0` -> it **dangles**.

`class_walk` shows it directly (`GCXW-CW node=…388 rep=…388 fresh=true members=2
self_excluded=true`): it hands back one or two **other** members, which do get repaired, and **excludes
the node that asked**. Three measured facts make that fatal for an outside node: `class_walk` builds
its list with `if member != node`; the repair branch runs **only over that list**; and the node's own
value arm ends `if inside { … } else { value }` with the final rewrite `if inside && …` — and an
outside node fails both guards. **So the first node walked in a class, when it lives outside the
vacated subtree, keeps a payload allocated inside it.** Not the member loop (it is excluded), not its
own arm (it is outside). The `gc_classes` dedupe candidate is **refuted as the cause**: that path is
busy but its nodes are repaired members.

**Dev contrast, with a denominator**: ten collects, **zero dangling**, while the self-excluding entry
is exercised **hundreds of times per collect** (`cw_calls 407/876`, `fresh 308/501`, repair_nodes
90/101, scanned 1474-1852, handles 479-593). So the exclusion strands nothing on dev; what differs is
the **trigger** — an outside member holding a payload **allocated inside** the vacated subtree, which
is the sharing the bind's class merging creates. The reconciling statement: **the code hole is
pre-existing in GC; the condition that reaches it is introduced by the bind.**

**Repair face, decided by the maintainer**: the entering node needs what a returned member gets —
when `!inside`, re-allocate its payload into its own block. Either `class_walk` includes the entering
node, or `garbage_collect_node` calls `relocate_class_value(node)` for itself when `!inside`. One
line shaped, but it changes what `class_walk`'s member-list contract and `relocate_class_value`'s
coverage promise. `class_walk`'s "the list is headed by the representative, so the walk starts there"
invariant is about **traversal order** and is satisfied — the gap is **repair coverage**, which no
invariant currently claims for the entering node.

Also measured: the fault **persists across collects**. A later collect releasing a different block
still reports the same two handles (`into_source=false`), so they are not re-created each time.

## 32. The origin is fixed: the entering node gets the member repair (dev `bc8aed2d`)

The guarded self-repair landed and was ff-merged. Two changes, `+8/−1` in two files:

- `garbage_collect_node` counts the class members **inside** `source` **before** the walk (the walk
  re-homes them, so the verdict is unreadable afterwards), and after the member loop
  `if !inside && members_inside > 0 { self.relocate_class_value(node); }`, placed before
  `let current = self.nodes[node].value;` — so the entering outside node gets what a returned member
  already got.
- `relocate_node_value`'s sentence: *"so each is relocated **once**"* -> *"so each is relocated"*,
  because the entering node was not relocated at all and "once" was not literally true either (a
  node listed as a member by several parents is relocated once per listing).

**Both halves of the gate closed**, same build, repair toggled:

- **No dangling handle at collect end**: five ON runs, every collect reporting
  `dangling=0, into_source=0, node rows=0, read-time events=0` — and **the zero has a denominator**
  (`scanned=1822`, `handles=577` per collect).
- **Cross-collect persistence is gone**: OFF reported `into_source=8`, of which **2 rows were handles
  from an earlier release**; ON reports 0.
- **The fault is real in this build**: one OFF run died `exit 132` (SIGILL) — the repair, not the
  instrument's layout, is what removes it.
- **Dev parity identical by name**: compute `215/68` with 0 new and 0 gone, wall neutral (511.42 s vs
  the 554.47 s baseline), every other suite green, comments-check and fmt clean.

**A tooling fact worth keeping**: `comments-check`'s budget counts a **BLOCK, not a line** —
consecutive `//` lines form one section, so a wording that is fine per line can still trip
`section-too-long` (153 and 232 characters against 120). The landed comments are two single-line
comments, and the diff's non-comment lines are exactly the five code lines of the repair.

**Scope, unchanged**: the completeness argument covers **class-mediated copies**.
`loop_run.rs:72` — the non-class route through the loop runner — **stays open**, with the nested-`@gc`
base-arm shape named and expressible, and its deciding test still needing permission.

## 33. start-at-2 landed: eleven of the twelve regressions are fixed on dev

`structs.rs`' `for position in 1..width` became `for position in shape::EXPR_ATTR_BASE..width`
with one rule line at the site (dev **`15573b16`**, one file `+2/−1`, ff-merged). The loop started at
`1`, which is the TYPE slot reader A has already compared from the cell; attributes begin at
`EXPR_ATTR_BASE = 2`.

**Why it is safe was measured before it landed**: of the 3539 dropped `pos=1` checks on dev compute,
**none catches anything reader A did not** (`new_a_ok = 0`, `new_a_absent = 0`, reader A ran for
**every** dropped check), and the only failures among them are duplicates of A's. On the unify tree it
removes exactly the second diary entry `an_output_position` reported — confirmed with the candidate
permanent, not toggled: `an_output_position` now reads `counts 0,0,1` under **both** binds (it was 2).

**What it withholds, stated rather than hidden**: 2218 unions of **throwaway copies**, not zero —
`same_cell=true` in all 3539 rows so reader B's actual side is always already in the type cell's
class, and the withheld merges unite a per-check `materialize_leaf` copy of a fact reader A already
recorded. Twelve message counts move by exactly the −99 duplicates. **Landed as a checker rule on
the maintainer's approval, not as a proof of equivalence.**

Gates on the merged tip: compute `215/68` with 0 new and 0 gone, every suite green (language external
`293/0`, which now includes the four `loop_gc_exit` tests), comments-check and fmt clean, wall
514.23 s against the 554.47 s baseline — slightly faster, consistent with fewer class members in the
O(members) walks.

**The account**: of the twelve regressions, **eleven are fixed and merged** — nine jit-timing (N2's
normalize), `compile_order_class` (rule C), and `an_output_position` (start-at-2). **`the_product` is
the last one**, and its repair (the complete discard: `unify_errors` + `apply_errors` +
`apply_error_nodes`) is measured sufficient and belongs to the unify slice.

## 34. Slice 2 measured: the non-empty antichain defers too, and the wall moves 676.42 s → 614.22 s

`equality.rs`'s `add_equality` now states `self.polymorphic_dirty = true` on **both** arms: the arm
that had a non-empty `seed_scopes` no longer calls `upgrade_polymorphic_class` inline, but defers the
fixpoint to the clone walk that reads it, exactly as slice 1 did for the empty arm. One line, branch
commit **`c7235946`**, on top of `e06b7607`.

**The name gate, measured by name and not by count**: 68 failing tests before, 68 after, and an
exact-name comparison of the two `failures:` blocks reports **zero regressions and zero fixes**.
Counts alone would not have been enough — an equal count can hide a swap.

**The wall, with its confound stated**: compute finished in **614.22 s** against slice 1's **676.42 s**
on the same branch, same worktree, same machine — a 9% drop from the one line. Against dev the
residual is **not a single ratio**: dev itself has been measured at **511.42 s, 514.23 s and 554.47 s**
(§32, §33, baseline), a spread of 8%, and no dev run was taken in this session to pair with this one.
The honest statement is *614.22 s against a dev population of 511–554 s* — somewhere between 1.11x and
1.20x — and the earlier "1.22x" was computed against the slowest dev reading.

**Every other gate on the same commit**: workspace `cargo check --tests` clean, `cargo fmt --all
-- --check` clean, `comments-check --max-violations 0` reports no violations, and ll 167, hl 136,
lang 69+293, gpu 94 all pass with zero failures.

**What it costs, stated rather than hidden**: `upgrade_polymorphic_class` now has **no caller**
(`warning: method ... is never used`), so the accumulating path is unreachable and the recompute path
is the only one. The method is **kept, not deleted**, because the batching fallback would build on it.

**The uncertainty that remains**: deferral discards the antichain `upgrade_polymorphic_class`
accumulated and re-derives scope from current flags. The named loss route — a scope held in an
antichain by a seed whose flag is later cleared (`write_polymorphic(..., Some(false))`), or by a
released block or a vanished edge — **is not exercised by any suite run here**. Identical names across
these suites is evidence, not proof, that the two paths agree on every program.

## 35. The unify gate, complete: every red face on this branch is red on dev too

The branch at **`04965de3`** was gated across the whole workspace, and each red reading was then
A/B'd against dev **`71e901a5`** rather than assumed to be pre-existing.

**Green**: `cargo check --workspace --tests` exit 0; `cargo fmt --all -- --check` exit 0;
`comments-check --max-violations 0` no violations; `test-layout-check` 30 packages, 0 violations.

**Suites**: lowlevel 167/0/1, highlevel 136/0, language 69/0 + 293/0, compute-gpu 94/0, compute lib
14/0, graph-ir 2/0 + 10/0 + 17/0 + 46/0, lex 22/0, parser 62/0, package 1/0, utils 22/0 + 1 doc,
registry 8/0, serve 11/0, line_model 13/0, directive_span 1/0, basic 167/0/1.

**Red, and identical on dev**:

- `lichen-compute` external: `215 passed; 68 failed; 1 ignored`, 68 failing names identical to the
  dev baseline, zero new and zero gone, in 614.22 s and again in 624.66 s.
- `lichen-language-server`: lib `31/1`, external `19/3`. At dev `71e901a5` — detached in the same
  worktree, warm target — the same crate reads `31/1` and `19/3` with the **same four names**:
  `analysis::tests::a_prelude_name_use_jumps_into_the_builtins_own_file`,
  `language_server::lsp_incremental::a_burst_of_edits_runs_the_frontend_once`,
  `language_server::lsp_incremental::a_prelude_name_jumps_into_the_materialized_builtin_file`,
  `language_server::statement_values::compute_kernel_bindings_render_by_name_not_raw_layout`.
- `lichen-tools`: its external binary **does not finish** — 180 s bound, exit 124, on the branch and
  on dev after a forced rebuild that printed `Compiling lichen-lowlevel` and `Compiling
  lichen-tools`. Its output ends in the example probes (`convolution2d.lichen: PANICKED … expected
  ?, found ?`, `decode.lichen: DRIFT …`), so it hangs in a test that walks `examples/`.

**Two traps met while gating, both worth keeping**: a `cargo test --workspace` sweep cut off by an
SSH reset left its test binary running, and the next build died with `LNK1104: cannot open
…tools-….exe` — a link error that reads like a code failure; and a test binary's filename hash does
**not** change when its sources change, so "the same exe name in both runs" is not evidence that
nothing was rebuilt. Only a `Compiling` line shows that.

## 36. The wall, paired in one session: 1.15x, not 1.22x

Dev's recorded walls (511.42 / 514.23 / 554.47 s) came from other sessions on other days, so §34
could only state a range. Measured back to back in this session — same worktree, same machine, same
suite — dev `71e901a5` reads **536.38 s** and the branch at `8af89b30` reads **614.22 s** and
**624.66 s**. That is **1.145x and 1.164x**: about **1.15x**, or +78 to +88 s on a 284-test suite.

The failing names were compared against **today's** dev run with the same exact-name pattern, not
only against the older baseline file: **zero regressions** — every name dev fails today is in the
branch's set, and both sets hold 68.

Slice 1 measured 676.42 s, so the two bounding slices together moved the branch from 1.26x to
1.15x. What remains is `propagate_polymorphic` itself: it still seeds every run from
`self.nodes.keys()` — the whole node table — no matter how few classes changed. That is the next
defect if the remaining 1.15x is to be removed, and it is **not** addressed here.

## 37. Correction: §36's wall was cross-day, and the paired number is 1.143x

§36 says the wall was "measured back to back in this session". It was not, and the error is mine:
`s4-ab.sh` ran cargo **once**, on dev's commit, then checked out the branch without running it. The
614.22 s §36 compared against is the reading from `2026-11-20T17:16:35Z`–`17:27:44Z` — a session that
ended **ten hours** before dev's run began at `2026-11-21T03:53:09Z`. A script's comment promised both
sides; its body delivered one.

The trap that made it read as same-session is worth keeping: a run's session comes from the `date -u`
markers inside its own log, never from a file's mtime — `ls --time-style=+%FT%TZ` appends a literal
`Z` to **local** time, and on a UTC+8 machine that renders yesterday evening as today's small hours.

**The genuine pair — both today, same machine, same suite, same invocation**: dev `71e901a5`
**536.38 s** (`03:53:09Z`–`04:02:31Z`) and the merged tip `09f3b422` **612.97 s** (script start
`04:04:09Z`, 98 s after dev's run ended). That is **1.143x**, a gap of **76.59 s**.

The name result survives and now holds on the merged tip too: `215 passed; 68 failed; 1 ignored`,
the same 68 names as dev's, zero new and zero gone.

**Limits, stated**: one run per side, not a median. The two used different `CARGO_TARGET_DIR`s, which
does not enter libtest's `finished in Ns`; both use the identical invocation and
`CARGO_BUILD_JOBS=4`, and neither passes `--test-threads`.

**The ladder, so superseded figures are not requoted**: 1.22x (slice 1 vs A2), 1.108x (slice 2 vs A2)
and 1.145x (yesterday's branch vs today's dev) are all cross-day. **1.143x is the number.** That the
cross-day 1.145 and the paired 1.143 differ by 0.002 is worth recording: the conclusion was right
while the evidence for it was not.

**Landed and verified on the merged tip**: the branch fast-forwarded into dev in the main checkout —
`dev` and `feature/unify-integration` are both `09f3b422`, seven commits on from `71e901a5`. In that
checkout, whose target was cleaned: `cargo fmt --all -- --check` 0, `comments-check
--max-violations 0` no violations, `cargo check --workspace --tests` 0 (58.62 s), compute
`215 passed; 68 failed; 1 ignored` in 612.97 s, and the rest of the workspace green except the same
four `lichen-language-server` reds of §35 that dev already carries.

**A confounder, checked rather than assumed**: free space did fall around this run — 15 GB at `04:11Z`
and `04:13Z`, then 13 GB at `04:19Z`, 12 GB at `04:20Z`, 9.8 GB at `04:24Z`. But the compute
measurement is pinned to **~04:07:04Z–04:17:17Z** from both ends — start plus fmt, comments, the
58.62 s check and a 1m52s build; end minus the rest phase's 5m01s build and 36.76 s of test
execution — and the two derivations agree to 9 s. Only the 15 GB readings fall inside that window;
the steep part is the **rest phase's own build**, which ran after the compute window closed, and
dev's window had 17 GB inside it at `04:01:55Z`. So the slide is not the mechanism. **76.59 s stays
an upper bound for the one reason left standing: one run each, ambient load unmeasured.**

## 38. The accumulate path is gone, and narrowing the walk's roots is a refuted alternative

Slice 2 left `upgrade_polymorphic_class` with no caller, so it was deleted: `polymorphic.rs`, 77
lines, commit **`5dfa5a59`**. Measured unchanged where it counts cheapest — ll 167/0/1, hl 136/0,
lang 69/0 + 293/0 — and the `never used` warning is gone. Dead code by construction, so the same
verdicts are the only acceptable reading.

**Do not re-propose shrinking `propagate_polymorphic`'s root set.** That was tried and measured, and
it fails on three counts at once:

- **10 regressions**: highlevel 134/2, langlib 68/1, langext 282/7 — against a whole-table control on
  the *same build* that reproduced the baseline exactly, so the scoping was the cause and not the
  instrument.
- **the flag invariant breaks**: 4832 / 141 / 449 nodes left with no verdict, against **0** for the
  whole table, every failure at the same `debug_assert!(read.polymorphic.is_some())` — with a root
  set of 5 over a 40006-node table.
- **it is far slower, not faster**: 111–115 s against 1.8–3.0 s, and 326.01 s against 3.55 s.

The mechanism is the part worth keeping: the O(1) skip is `if polymorphic_unflagged == 0 &&
!polymorphic_dirty`. Nodes left permanently outside the roots' reach mean that counter **never
drains**, so the early return is lost and the walk re-runs at every clone-walk entry. And a root set
cannot stand in for a liveness set here: `add_node` sets `function: None`, so most table nodes are
unowned caller cells that the clone walks do read.

That refutation does **not** cover a walk made incremental from the changed classes — the direction
`upgrade_polymorphic_class` and the parked `b5-batching.pl` took. Any such design must still satisfy
both constraints above: every node a clone walk reads ends with a verdict, and `polymorphic_unflagged`
still reaches 0.

## 39. The fixpoint forwarded each node's whole chain on every pop; it now forwards each admitted scope once

`compute_polymorphic` re-sent a node's **entire** antichain to every referrer on every pop. A census of
the merged tip (77 runs, one target test) measured the waste exactly: **22,825,172 fixpoint sends for
1,345,906 genuine admissions — 94.1% of inserts changed nothing**, 745,522 pops for 179,234 distinct
nodes, and 58.8% of pops added no scope anywhere. Redundancy rose with table size (6.76x under 1k
nodes, 17.71x above 8k), which is the O(k²) signature: a chain that grows one scope at a time was
re-sent k times.

**A cheaper antichain was ruled out by the same census**: chains are small — mean **10.8** at pop, max
**25**, nothing above 50 — so the count came from re-sending, not from chain length.

**The change** (`polymorphic.rs`, commit **`06f4ad6f`**): the work item carries the scope that was just
admitted rather than the node, so each admission crosses an edge once and only an admission re-queues a
referent. **Why the chains are unchanged, not merely equivalent**: a rejected scope always has an
ancestor already in the referrer's chain, and that ancestor was itself admitted earlier and therefore
already forwarded along every out-edge — by induction every subsuming scope has crossed the edge. And
the chain is the set of **maximal** scopes of the covered set, which is unique, so the result is
identical element for element whatever the insertion order.

**Measured** (same machine, same day, same invocation):

| | wall | failing names |
| --- | --- | --- |
| dev `71e901a5` | 536.38 s | 68 |
| merged tip `09f3b422` | 612.97 s | 68 |
| **delta `06f4ad6f`** | **169.83 s** | **68, zero new, zero gone** |

That is **3.16x faster than dev** and **3.61x faster than the tip the 1.143x regression was measured
against** — the unify regression is not cancelled, it is reversed. The census test itself went from
**7.67 s to 1.03 s**. Every other suite is unchanged: ll 167/0/1, hl 136/0, lang 69/0 + 293/0, and
`cargo fmt --check` and `comments-check --max-violations 0` are clean.

**What the new distribution says about a 10-second target**: 284 tests in 1435 core-seconds against
12 cores is 119.6 s of ideal packing, and the slowest single test is **112.49 s**
(`zz_dynamic_contract_probe::probe_a_function_local_contract`), so the suite is now **floor-dominated**
— one test is 66% of the wall, and 58 tests are still ≥10 s. Reaching 10 s therefore needs both ~14x
less total work and the top test under 10 s. The top three are all `zz_dynamic_contract_probe`, which
are probe tests that assert nothing and exist only while their investigation is open.

## 40. Probes leave the default run — this is a scope reduction, not a speedup

74 probe tests are now `#[ignore]`d across 26 files, commit **`9514fbb7`**. A second test binary was
not an option: `test-layout-check` allows one `test` target per package, so a probe binary would fail
the layout gate. `#[ignore]` keeps them compiled — they cannot rot — and `cargo test … -- --ignored`
still runs them.

**Label this honestly.** The suite now runs **212 of 284 tests** and finishes in **63.60 s**, against
169.83 s for all 283 before. That is **less work, not faster work**: 71 tests were moved out. The one
legitimate *speedup* claim remains §39's **536.38 s → 169.83 s on the same 283 tests (3.16x)**. A
number quoted from this section without saying how many tests were excluded is the filtered-run trap.

**The canary stays.** `zz_gc_panic_probe::gc_variants` asserts nothing, so it can only fail by dying —
it is the only default-run detector for the **open** P0-9 arena route, and it produced the SIGILL and
SIGSEGV readings that located P0-8. Ignoring it would have removed a crash detector from the gate while
the investigation is unresolved, so it remains in the default run. AGENTS.md says a probe stays in the
tree while its investigation is open; it must also stay *runnable* then.

No probe is among the 68 baseline failures (`0` of 68), so the failing-name gate is unchanged by the
move — verified, not assumed.

**What reaching 10 s still needs.** 213 tests at ~63.6 s wall on 12 cores, with a slowest single test of
**17.02 s** (`pack_placement::the_landed_fragment_reads_the_class_the_pack_states`) and the next few at
16.8 / 16.0 / 14.6 / 14.5 s. Two things must both happen: the per-test floor has to fall below 10 s, and
the total work has to fall by roughly another six times. Those tests are real verification —
`pack_placement`, `fuse_licence`, `tiled_reduction`, `graph_jit`, `recompute_reads`, `fused_region` —
so they cannot be excluded the way probes were. The next slice is therefore another shared-cost
reduction inside the checker, not a test-selection change.

**Two ignores in the compute area were already there and are stale — recorded, not changed.** Both
carry the reason "one disagreement, two reports: the apply's join is not yet suppressed", and that
mechanism was measured wrong (two check-time diary entries, `apply_errors` was 0 in both trees) while
the defect itself was fixed by start-at-2, after which `an_output_position` reports once on both binds.
`graph_structure.rs:319` is a real test whose stated blocker no longer exists; reviving it is a test
edit and needs the maintainer, and its assertion is a cfg-structure count, so it may fail for an
unrelated reason. `compute.rs:1290` carries `#[ignore]` with **no `#[test]`**: it is not a test, libtest
never sees it, it is the source of the `never used` warning, and it cannot be reached by `--ignored`
either — it should gain `#[test]` or be deleted rather than sit in the tree looking like coverage.

## 41. The suite's wall was half a mutex: the backend now installs per thread

86.5% of the wall was a serial chain, not work. `tests/common.rs` held `BACKEND_SLOT`, a
**process-wide** `Mutex<()>` taken by every test that installs a backend, because
`lichen-kernel-ir`'s `BACKEND` is process-global by stated invariant. Forty-one tests were serialized
behind it, and the suite's 12-core speedup was 3.29x against an ideal of 12x.

**The change is additive, so no existing invariant was reworded** (commit **`d54554be`**, plus
`47d50f27` for the two text sites):

- `kernel-ir` gains `install_thread_backend` / `clear_thread_backend`, and `parallel_backend` prefers
  this thread's override before falling back to the process slot. `BACKEND`,
  `install_parallel_backend` and `clear_parallel_backend` are untouched, and the `None`-versus-
  *declines* sentence is kept verbatim — the precedence rule was **prepended**, so this is an
  addition, not a reword. Production hosts never install an override and take today's path.
- The harness installs on the installing thread and `SlotGuard` **clears the override when it drops**.
  That drop is load-bearing: libtest reuses worker threads, so 212 tests over 12 threads means ~18
  tests per thread, and without it one test's stub would serve the next test on that thread — a leak
  the failing-name gate cannot see, because a test can fail for a new reason under the same name.
- `graph_jit` mints a fresh stub per call instead of a `thread_local!` one, and the doc blocks that
  described the removed lock now state what holds.

**Measured, one guarded sweep, every point clean** (68 names, zero new, zero gone, at each):

| threads | before | after |
| --- | --- | --- |
| 2 | 119.10 s | 111.22 s |
| 4 | 79.43 s | 63.70 s |
| 8 | 66.32 s | 39.41 s |
| 12 | 63.57 s | **34.84 s** |

The T=8 before-row is the **flake run** (`143 passed; 69 failed`), not a clean baseline like the other
three: its extra failure panicked early, so the clean before-wall is **≥66.32 s** and the improvement
there is understated, not overstated. The after-rows are all `144 passed; 68 failed; 72 ignored`.

The improvement grows with thread count, which is the signature of removing a serial section. The
**T=8 flake is gone at every count**: `identity_from_body::a_seed_that_is_no_literal…` no longer
appears as FAILED anywhere. Its mechanism is closed rather than papered over — no test installs into
the process slot now, so a test that dispatches without installing reads `None` and gets the built-in
backend instead of a neighbour's stub.

**What did NOT improve: the work.** This slice removes a mutex and deletes no checking work. The
uncontended total was 209.41 s before and is unmeasured after (T=1 was not re-run); the T=2 per-test
sum fell 238.0 → 222.2, which is *lock wait* leaving the `--report-time` figures, not work leaving the
suite. **The ~1.75x work cut is still required**: even perfect packing of today's work is 209.5/12 =
17.46 s, which is above 10 s on its own.

**Where the 10-second target stands**: 34.84 s at T=12 against 17.46 s of ideal packing is still
**2.0x of residual contention**, plus the ≥1.75x work term that is 87–93% `propagate_polymorphic`.
Both are needed; neither is sufficient. The one thing that stopped being an obstacle is the per-test
floor: the slowest single test fell from **17.268 s to 6.781 s** in the same sweep, so no test must be split or excluded
to fit under 10 s.

**Not claimed**: that a host outside this repository does not rely on the backend slot being
process-global across threads; and four `#[test]`s in `crates/lichen-compute/src/compute.rs` still
install into the process slot with no clear — pre-existing, green (14/0), and left named rather than
fixed here, since converting them without a guard would just move the leak to the thread slot.

## 42. The ancestor test cloned the Arc it was only reading: 34.84 s to 18.52 s

The post-delta census inside the fixpoint found the cost was **not** the number of sends, the HashMap,
or the worklist. It was the **antichain scan**, and the scan's cost was **atomic refcount traffic
inside `is_ancestor_or_self`** — `current = other.clone()` at entry and one more `clone()` per set bit
of the depth difference. On the two measured bodies that came to **52.7M / 203.7M internal Arc
clone-drop pairs against 6.3M / 25.7M at the call sites: 8x more inside the predicate than around it.**

The function only reads. It now holds `&Arc<Scope<F>>` through the lifting walk and clones nothing
(`scope.rs`, commit **`ae3ffb10`**, two lines changed). **No decision, ordering or invariant moved.**

| | wall | slowest test |
| --- | --- | --- |
| before | 34.84 s | 6.781 s |
| **after** | **18.52 s** | **4.236 s** |

1.88x from one predicate, with the failing names unchanged at 68 (zero new, zero gone), and ll 167/0/1,
hl 135/0/1, lang 69/0 + 293/0, gpu 94/0, compute lib 14/0, `fmt`, `comments-check` and
`check --workspace --tests` all clean.

**Counted from the start of this work, the suite has gone 612.97 s → 18.52 s.** That figure spans
several different scopes and must be read with §39–§41: the delta slice was 3.16x on the same 283
tests; the probe move **removed work rather than speeding it** (§40); the harness change removed a
mutex (§41); this one is the only other true speedup.

**The larger finding, measured but not acted on**: admissions per distinct node were 151 and 218 while
the chain stays at ~8.6, so **94–96% of admissions are transient** — admitted, forwarded along every
out-edge, then deleted by a later `retain`. If the fixpoint admitted each scope once in its final
position, attempts would fall **17.5x / 26.5x**. The candidate that attacks it is a **shallow-first
worklist**: admitting an ancestor before its descendants makes a descendant **reject on arrival**
instead of being admitted and later retained. That is an ordering change on a monotone fixpoint, so the
least fixpoint is unchanged, but it has **not been measured** and is not done here.
