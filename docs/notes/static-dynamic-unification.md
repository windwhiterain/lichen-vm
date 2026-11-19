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
