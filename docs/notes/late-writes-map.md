# Late writes under a plain deep gate — the elimination map

> **§1–§10 are the measurement slice and are kept as it was written**: that slice
> changed no behaviour, and its gate was a scratch edit reverted before the commit.
> **§11 records what has since landed** — the concrete-only deep gate, the suites it
> was measured on, and the correction §11.3 makes to this map's own §8. Where §8
> names a first target, read §11.3 with it.
>
> **§13 measures the plain gates and §14 records the one behavioural change this note
> holds**: `compute.graph`'s recording pass no longer waits for a stalled structural
> read to be re-asked by the run gate. It is a local, bounded retry; the gate itself
> stays where §13 left it.
> **§13 runs §1's goal on both gates and does not land it**: the deep gate alone
> costs 4 `lichen-compute` tests, the run gate alone 65, and §13.2 corrects §4.1's
> and §4.4's silence about the run axis. Where §4 or §11 is quoted as the map's
> headline, read §13.2 with it.
>
> Slice: measurement only, as §1–§10 stand. **No behaviour change landed there.** The
> gate below is a scratch edit that is reverted before the commit; what this file holds
> is the per-test trace of every write that arrives *after* the deep verdict, the site
> that wrote the verdict, and the sanctioned site that should have produced the
> write instead.
>
> Worktree: `.worktrees/dlw`, branch `feature/deep-late-writes`, base `dev@f360c72d`.
> Build: private target `C:\resource\lichen-vm\.worktrees\dlw\target`, `-j 1`,
> `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`.
>
> **What this slice did not do**, deliberately: no `settled` bit and no new state of
> any kind, no `# Invariant` touched, no test edited, and no write to
> `lichen-compute`, `lichen-compute-gpu` or `lichen-language-server` (they were
> read). Every code change described here was a scratch probe behind an env var and
> was **reverted**; `git diff dev...HEAD --name-only` on that branch lists exactly one
> file, this one.
>
> **Where the decision itself lives.** This map is the measurement the decision in
> [incremental-evaluation](incremental-evaluation.md) §7 already records —
> that commit (`557a8265`, on `dev` after this branch was cut) names "the three
> lowlevel epoch tests and the four compute late-write tests … as the first map of
> those defects". This file is that map, with the per-test traces; it deliberately
> restates no prose from that note.

## 0. The goal, in the maintainer's terms

`unify` is separated from `evaluate`: the checker does the initial unify
(`expr: expr`), normalization happens at the moment a function is born, apply does
runtime unify, and **beyond those three sites no unify is produced dynamically**.

The payoff wanted: `evaluate` / `evaluate_deep` gate purely on their stamps —
`evaluated` = the value-epoch stamp, `evaluated_deep.is_some()` — so **a node is
evaluated and decided exactly once, never re-decided**.

The `settled` bit of [incremental-evaluation](incremental-evaluation.md)
§4 was **rejected** ("加了 `settled` 等于没重构"). So the late writes are defects to
eliminate at their source, not state to be tolerated.

## 1. The gate under experiment

The experiment: give the deep walk the plain cut that the rejected `settled` design
would have given, but with the gate reading **only** `evaluated_deep.is_some()` —
no second bit, no new state.

Inserted in `crates/lichen-lowlevel/src/evaluation.rs`,
`evaluate_node_deep_inner`, after the static-leaf and cycle-cut arms and before
`deep_depth` is charged (the position
[incremental-evaluation](incremental-evaluation.md) §4.4 names):

```rust
// SCRATCH: the plain deep gate.
if self.nodes[node].evaluated_deep.is_some() {
    return self.nodes[node].value;
}
```

This is the whole of the "decided exactly once" property: once a node holds a
verdict, the deep walk never re-enters it, so no verdict is ever revised.

## 2. The single choke point for the trace

Every value-slot write in the module goes through one function,
`Module::set_node_value` (`crates/lichen-lowlevel/src/equality.rs:287-290`), which
bumps the value epoch and then assigns `Node::value`. Its callers, by
[stalled-rerun-epoch](stalled-rerun-epoch.md) §4, are exactly:

| caller | `file:line` | what it is |
|---|---|---|
| `write_node_value` | `equality.rs:257` | the public slot write; also calls `propagate_class_value` |
| `write_node_value_if_absent` | `equality.rs:293` | the same, refusing an occupied slot |
| `write_node_answer` | `equality.rs:319` | an operator's computed answer, unified against the class |
| `propagate_class_value` | `equality.rs:345` | one value given to **every** member of a class |
| `fill_class_holes` | `equality.rs:111` | the `add_equality` merge giving the value to empty members |
| `relocate_node_value` | `equality.rs:307` | GC's move of a value, not a change of it |

So the trace point is `set_node_value`: a write to a node that already holds
`evaluated_deep.is_some()` **is** a late write, by definition, and the trace records
which of the six callers it arrived through.

### 2.1 The three sanctioned unify producers, and where each one runs

| site | where | what it unifies |
|---|---|---|
| **1. checker** | `lichen-highlevel/src/checker.rs:526-562` — `check_expr` over every statement, then the lambda-value deep pass at `:537-539`, then every written type term at `:540-556`, then the shallow tails at `:557-562` | the initial `expr: expr`; `Checker::check_unify` (`checker/diagnostics.rs:30`) → `Module::try_unify` |
| **2. function-birth normalization** | the same lambda-value loop, `checker.rs:537-539` — *"Deep-evaluate every lambda's value node before the definition pass, so a recursive reference stays in place"* | what the function's template owns before any call |
| **3. apply** | `checker.rs:563-592`, the definition pass: `evaluate_node_deep(ret)` per function, and at run time `static_function_apply` (`lichen-lowlevel/src/static_module/apply.rs:7`) — whose codomain bind is `static_module/apply.rs:90` and whose result wiring is `apply.rs:137` | the parameter check (`apply.rs:78`) and the codomain/result cells |

The order matters and is already stated in the code: the checker and the
function-birth normalization run **before** the definition pass. Every late write in
§6 is one of these three firing **later than that order says it should** — which is
why §8's first move is a reordering inside site 3, not a new writer.

## 3. Baseline, changes absent (`dev@f360c72d`)

Measured in this worktree, private target, `-j 1`:

| suite | recorded | measured | agreement |
|---|---|---|---|
| `cargo test -p lichen-lowlevel --no-fail-fast` | 147 / 13 | **147 / 13** | exact |
| `cargo test -p lichen-highlevel` | 140 / 0 | **140 / 0** | exact |
| `cargo test -p lichen-compute --no-fail-fast` | 204 / 14 / 2 | **204 / 14 / 2** | exact |

The compute baseline's 14 failures, by name (for the `Compare-Object` below):

The 13 `lichen-lowlevel` baseline failures are **all GC/compaction**, and none of
them is one of the three epoch tests:

```
assert::gc_moves_an_assert_condition_with_its_function
assert::gc_prunes_asserts_of_dropped_blocks
compaction::block_run_pulls_outer_and_sibling_blocks
compaction::call_clones_are_compacted_with_the_calling_block
compaction::deep_block_chain_evaluates_stack_safely
compaction::deep_never_run_block_chain_releases_stack_safely
compaction::redundant_nodes_are_not_compacted
compaction::u128_payload_is_relocated_into_parent_and_block_releasable
compaction::unreferenced_child_blocks_are_released
function::function_in_local_block_survives_compaction
function::function_scope_is_dropped_with_its_block
function::mixed_blocks_and_functions_survive_compaction
table::gc_compaction_moves_table_payloads_and_entries
```

(That set is the GC trigger disabled — see
[gc-trigger-disabled](gc-trigger-disabled.md) — and is the same set on
every later run in this slice, so every delta below is measured against it.)

The compute baseline's 14 failures, by name:

```
lichen_compute::automatic_tiling::a_fold_whose_bound_is_a_literal_is_refused_by_name_and_nothing_lands
lichen_compute::automatic_tiling::the_floor_tiles_a_declared_reduction_the_merge_stage_declines
lichen_compute::automatic_tiling::the_pipeline_tiles_a_declared_reduction_and_the_tiled_body_answers
lichen_compute::combine_fold::a_sum_of_squares_tiles_with_the_datum_its_claim_names_and_answers_the_squares
lichen_compute::fold_datum::a_datum_no_body_computes_is_refused_by_name_and_the_body_answers
lichen_compute::fold_datum::a_maximum_tiles_under_the_same_proof_and_answers_the_untiled_body
lichen_compute::graph_structure::a_parameter_is_bound_by_the_position_the_source_names_and_not_by_read_order
lichen_compute::identity_from_body::a_maximum_over_data_below_the_old_seed_answers_what_the_untiled_body_answers
lichen_compute::identity_from_body::a_minimum_over_a_row_of_positives_answers_what_the_untiled_body_answers
lichen_compute::identity_from_body::a_seed_that_is_no_literal_and_a_missing_role_are_refused_by_name
lichen_compute::pack_placement::a_matching_pack_fires_and_the_plan_it_states_is_what_lands
lichen_compute::pack_placement::a_pack_asking_for_a_width_the_target_lacks_is_refused
lichen_compute::pack_placement::a_pack_whose_constraints_do_not_hold_is_inert
lichen_compute::pack_placement::an_unmatched_pack_is_inert
```

## 4. Results

### 4.1 `lichen-lowlevel` — the plain deep gate reproduces exactly

`LICHEN_PLAIN_DEEP_GATE=some` → **144 passed / 16 failed**. Against the 13 baseline
failures, `Compare-Object` on the **names** gives exactly three new:

```
assert::an_untriggered_assert_is_decided_by_a_later_drain
evaluation::a_stalled_operator_reruns_only_after_a_value_changed
function::function_call_operator_preserves_undecided_operand_chain
```

Two further readings of the same experiment, both worth recording because they
separate two different things:

| variant | lowlevel | new failures |
|---|---|---|
| plain run gate — `run_stamp_is_current` reads only `runned != 0` | **144 / 16** | the **same three**, by name |
| plain deep gate — `evaluated_deep.is_some()` | **144 / 16** | the **same three**, by name |
| deep gate cut on **concrete only** — `is_some_and(\|d\| !d.undecided)` | **147 / 13** | **none** |

The last row is the important one and is easy to misread: **the three epoch tests do
not break because the cut reads `undecided`; they break because a cut node is never
re-decided at all.** The `undecided: true` verdict is the one that must be revised —
a node that stalled deep has to be re-descended when the graph moves under it. See
§5.

### 4.2 A measurement trap worth writing down

The first version of the scratch gate carried a stray `self.deep_depth -= 1` on the
cut path, placed before `deep_depth` is charged. That underflowed and reported
**108 / 52** — 39 new failures — which reads as "the plain gate is hopeless" and is
an artefact of the probe, not a finding. The 52-name list included plausible-looking
victims (`table::*`, `host_loop::*`, `recursion::*`) that pass once the decrement is
gone. **A probe that panics in the crate under test manufactures failures that look
exactly like deep design problems.**

### 4.3 `lichen-highlevel` — unaffected

`LICHEN_PLAIN_DEEP_GATE=some` → **140 passed / 0 failed**, byte-identical to the
baseline. No highlevel test pins the deep verdict's revisability.

### 4.4 `lichen-compute` — four late writes, by name

`LICHEN_PLAIN_DEEP_GATE=some` → **200 passed / 18 failed / 2 ignored**. Against the
14 baseline failures, `Compare-Object` on the names gives exactly four new:

```
lichen_compute::graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture
lichen_compute::graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one
lichen_compute::graph_structure::a_body_dispatches_through_an_apply_whose_callee_holds_the_operator
lichen_compute::graph_structure::the_kernel_slot_is_a_field_read_whose_target_is_the_captured_kernel_struct
```

All three suites reproduce the recorded gate numbers **exactly** (144/16, 140/0,
200/18/2), and the delta sets are exactly the ones named in the brief.

### 4.5 How the deep pass is reached from a *shallow* read

One code fact governs all seven failures and is not in the brief:

> `OperatorExt::run_deferred` (`crates/lichen-lowlevel/src/lib.rs:384-407`) calls
> `module.evaluate_node_deep(operand, Some(block))` on **every** operator that does
> not override `run`, then reads `nodes[operand].evaluated_deep` to decide
> undecidedness (`:394-397`).

So an operator's verdict is read off **its operand's** deep verdict. The deep pass is
not only an explicit entry point (`check_asserts`, `table`, `loop_run`, the checker);
it is in the shallow path. That is why a stale verdict on an operand starves the
operator that reads it, and why a test that only calls `evaluate_node` can still be
broken by a *deep* gate.

## 5. Per-test traces — `lichen-lowlevel`

Line numbers below are **pristine `dev@f360c72d`** numbers, not the scratch
instrumented ones.

### 5.1 `evaluation::a_stalled_operator_reruns_only_after_a_value_changed`

`crates/lichen-lowlevel/tests/basic/evaluation.rs:410`

Graph: `left` = **1v1** (empty cell), `right` = **2v1** (u128 2), `operands` = **3v1**
(array `[1v1, 2v1]`), `add` = **4v1**.

**Who wrote the verdict first.** `evaluation.rs:614` — the real write, not the cut at
`:578` — three times, all during the first `m.evaluate_node(add)` at
`evaluation.rs:420`, which reaches the deep pass through `run_deferred`:

```
VERDICT-REAL node=1v1 undecided=true
VERDICT-REAL node=2v1 undecided=false
VERDICT-REAL node=3v1 undecided=true
```

So **3v1 was decided `undecided: true`** — "the array has an open position at 0".

**The late write.** One, on node **1v1** (`left`), `Some`, at epoch 1:

| | |
|---|---|
| path | `write_node_value` (`equality.rs:257`) → `set_node_value` (`equality.rs:266` → `equality.rs:287`) |
| call site | `tests/basic/evaluation.rs:429` — **the test itself** |
| verdict it lands after | 1v1's own `undecided: true` from `:614` |

**What the gate then does.** The third read (`evaluation.rs:433`) re-runs `add`
(the epoch moved, so `run_stamp_is_current` at `evaluation.rs:169` opens), and
`run_deferred` calls `evaluate_node_deep(3v1)`. The plain gate **cuts 3v1** and
returns its cached array; `lib.rs:394-397` then reads 3v1's verdict — still
`undecided: true`, now stale — and the operator returns `None`. Panic at
`evaluation.rs:434`.

**Which sanctioned site owed this write.** *None of the three.* The writer is a
direct slot write from a test, not a unify. The refactor's rule ("beyond checker /
function-birth normalization / apply, no unify is produced dynamically") does not
cover this writer at all: it is a **value write**, which the design still allows.

### 5.2 `assert::an_untriggered_assert_is_decided_by_a_later_drain`

`crates/lichen-lowlevel/tests/basic/assert.rs:217`

Graph: `x` = **1v1** (empty cell), `one` = **2v1**, `operands` = **3v1**, `eq` = **4v1**.

**Who wrote the verdict first.** `evaluation.rs:614`, for 1v1 (`true`), 2v1 (`false`),
3v1 (`true`), and **4v1 (`true`)** — during the first `m.check_asserts()`, which calls
`evaluate_node_deep(condition)` at `src/assert.rs:51`.

**The late write.** One, on node **1v1** (`x`), `Some`, epoch 1:

| | |
|---|---|
| path | `write_node_value` (`equality.rs:257` → `:266` → `:287`) |
| call site | `tests/basic/assert.rs:238` — **the test itself** |
| verdict it lands after | 1v1's `undecided: true` |

**What the gate then does.** The second `check_asserts()` → `evaluate_node_deep(4v1)`
→ **gate cut on 4v1**, value `None` → the drain keeps the entry pending →
`assert!(m.asserts.is_empty())` fails at `assert.rs:242`.

**Which sanctioned site owed this write.** *None of the three.* The test says so
itself: *"The binding happens outside the drain; here it writes the cell directly."*

### 5.3 `function::function_call_operator_preserves_undecided_operand_chain`

`crates/lichen-lowlevel/tests/basic/function.rs:104`

Graph: `call` = **7v1**, `ret`/`mid` = **9v1**, `arg` = **10v1**, `ninety_nine` = 11v1.

**Who wrote the verdict first.** `evaluation.rs:614`, for 10v1 (`true`), 9v1 (`true`)
and **7v1 (`true`)**, during `m.evaluate_node_deep(call)` at `function.rs:125`.

**The late writes.** Two:

| # | node | what | path | call site |
|---|---|---|---|---|
| 1 | **10v1** (`arg`) | `Some`, epoch 4 | `Module::unify` (`equality.rs:393`) → `unify_inner` (`:396`) → the one-sided arm `self.add_equality(ra, rb)` (**`equality.rs:688`**) → `fill_class_holes` (**`:99`** → **`:115`**) → `set_node_value` (`:287`) | `function.rs:143`, `m.unify(arg, ninety_nine)` |
| 2 | **7v1** (`call`) | `None`, epoch 6 | `write_node_value` (`equality.rs:257` → `:266` → `:287`) | `function.rs:144`, "drop the cached marker" |

**What the gate then does.** `m.evaluate_node_deep(7v1)` at `function.rs:145` →
**gate cut on 7v1** → returns `None` → `unwrap()` panics.

**Which sanctioned site owed these writes.** Write **#1 is a `unify`** — one of the
three sanctioned producers. But it is the *test's* unify at test time, not the
checker's, and it is idempotent: the argument was already in the same class as
`ninety_nine`, so the write is `fill_class_holes` giving a value to an **empty
member of a merged class**. Write **#2 is a value clear**, not a unify, and is the
test deliberately invalidating a cached marker.

## 6. Per-test traces — `lichen-compute`

Four failures, and the shape is the same in all of them: the late write is **not** a
stray slot write but a **sanctioned** unify — apply-time in three, checker-time in
one. That is the important reading: these are not "the design forgot a writer", they
are "the sanctioned writers fire **after** the verdict they should have preceded".

The observable consequence is a **diagnostic change**, not a wrong number:

- `graph_jit::*` — the refusal is still filed under `compute.graph`, but it no longer
  names the role, so the message-content assertion fails
  (`graph_jit.rs:263`, `graph_jit.rs:303`).
- `graph_structure::*` — the program **no longer checks at all**: the panic is
  `assert!(report.ok())` at `graph_structure.rs:32`.

### 6.1 `graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture`

7 writes landed on nodes the gate had already decided. The last three:

| node | epoch | write path | sanctioned site |
|---|---|---|---|
| **530v1** | 954 | `apply_parameter_check` (`src/apply.rs:78`) → `Module::unify` (`equality.rs:393` → `:396`) → `unify_inner` → `self.add_equality(ra, rb)` (`:688`) → `fill_class_holes` (`:99` → `:115`) → `set_node_value` (`:287`) | **apply** (site 3) |
| **141v1** | 1510 | the same path | **apply** (site 3) |
| **1104v1** | 2065 | `wire_apply_result` (`src/apply.rs:137`) → `write_node_value` (`equality.rs:257`) → `propagate_class_value` (**`:345` → `:348`**) → `set_node_value` — reached from `static_function_apply` (`static_module/apply.rs:92`) | **apply** (site 3) |

**Why the message changed.** The body closes over `held`. The checker's lambda-value deep pass
(`checker.rs:537-539`) already decided the body's verdict; the *capture's* value only arrives when
the apply's parameter check merges it into the call's class, i.e. **after** the verdict the graph
lowering reads. The gate then hands the lowering a decided node whose value is the one the merge
wrote, and `place()` (`lichen-compute/src/compute/graph.rs:388`) no longer sees a
`ComputeValue::Buffer` to refuse by name — so the "would have to hold it … rather than closing over
it" sentence (`graph.rs:392-398`) never fires.

**Corrected later:** "a live buffer" is not what the build sees, and the write paths above are a
record of this gate experiment rather than of that question. The lazy run forces `held` **inside** the
recording window, so its own dispatch becomes the recording's first node and `held.z` reaches the body
as a `GraphValue` — `place` receives an edge, not a buffer, and the program is legitimately accepted
([graph-recording-rounds](graph-recording-rounds.md), last section). The test now closes over a
genuine host buffer (`compute.buffer`), which is the case that sentence refuses.

### 6.2 `graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one`

Same shape; the last consequential write is node **141v1** at epoch 1452, through
`apply_parameter_check` → `unify` → `add_equality` → `fill_class_holes`. Only the
count role is wrong in the program, and the count's own refusal sentence is what the
test asserts; it no longer fires (`graph_jit.rs:303`).

### 6.3 `graph_structure::a_body_dispatches_through_an_apply_whose_callee_holds_the_operator`

6 writes; the two that matter are the last expr, and they are **different sanctioned
sites**:

| node | epoch | write path | sanctioned site |
|---|---|---|---|
| **1085v1** | 250 | `check_app` (`highlevel/src/checker/lambda.rs:253`) → `check_expr` (`checker.rs:1116`) → `check_term` (`:1130`) → `check_instantiate` (`checker/structs.rs:542`) → `Checker::check_unify` (`checker/diagnostics.rs:31`) → `Module::try_unify` → `unify` (`equality.rs:393`) → … → `set_node_value` | **checker** (site 1) |
| **312v1**, **313v1** | 1590, 1593 | `static_module/apply.rs:90` — **`module.unify(cell, codomain)`**, the apply's codomain bind, under `with_apply_frame` (`apply.rs:36`) from `static_function_apply` (`static_module/apply.rs:7`) | **apply** (site 3) |

**Why the program stops checking.** The dispatch's callee is an apply node, and the
callee's *operator* is only written into that node's class by the codomain bind at
`static_module/apply.rs:90` — which happens during the apply's own evaluation, i.e.
after the deep verdict the checker's `check_instantiate` already recorded. Under the
gate the verdict is final, so the checker sees a callee that never becomes an
operator and refuses the whole program.

### 6.4 `graph_structure::the_kernel_slot_is_a_field_read_whose_target_is_the_captured_kernel_struct`

Identical shape: 110v1 (epoch 28), 491v1 (112), 1082v1 (249), 1085v1 (250), and the
last expr **312v1** (epoch 1589) and **313v1** (epoch 1594) from
`static_module/apply.rs:90` — the apply codomain bind. Same `graph_structure.rs:32`
failure.

## 7. The three `lichen-lowlevel` epoch tests: what exactly they pin

Measured, not argued:

- plain deep gate `is_some()` → **144 / 16**, all three fail.
- plain **run** gate `runned != 0` → **144 / 16**, the same three, and **each fails
  at the same assertion line as under the deep gate**
  (`evaluation.rs:434`, `assert.rs:242`, `function.rs:145`).
- deep gate cut on **concrete verdicts only** (`is_some_and(|d| !d.undecided)`) →
  **147 / 13**: **none** of the three fails.

**The judgement, stated plainly.**

All three turn on the same narrow thing: **an `evaluated_deep: Some({ undecided:
true })` verdict being revised to concrete after the graph changed.** Not re-decision
in general — a *concrete* verdict is never revised in any of them, and a gate that
refuses to cut an undecided verdict passes all three.

That narrower thing is still exactly the property the refactor deletes. An
`undecided: true` verdict is the deep pass saying *"I could not decide, ask me
again"*; "a node is evaluated and decided exactly once, never re-decided" makes that
answer final. So **these three pin the design itself, not an incidental behaviour**,
and retiring them is a maintainer decision. This report stops there rather than
proposing edits to them.

Two things the maintainer should know before deciding:

1. `a_stalled_operator_reruns_only_after_a_value_changed` is **mixed**: its first
   half (`:419-426`, stall → stamp → no re-descent) is the **run** gate and survives
   both plain gates; only its second half (`:427-436`) is the re-decision. Retiring
   the whole test retires the run-gate pin too, which is a separate, still-current
   design ([stalled-rerun-epoch](stalled-rerun-epoch.md), status
   **current**).
2. The concrete-only reading is **not** the rejected `settled` bit — it adds no
   state and computes nothing extra. It is weaker than the refactor's goal (an
   undecided node is still re-descended forever) but it is a measurement worth
   having: it shows the whole lowlevel delta is **about the `undecided: true`
   verdict**, not about re-descending decided subtrees.

## 8. What to fix first, and why

**First: `static_module/apply.rs:90` — `module.unify(cell, codomain)`, the apply's
codomain bind.** It is the only late write that appears in **two** of the four
compute failures (both `graph_structure::*`), it is one of the three sanctioned
sites so it can be moved without inventing anything, and its ordering is already
explicit and therefore already negotiable: the bind happens **inside the apply's own
evaluation frame** (`static_function_apply` → `with_apply_frame` →
`static_module/apply.rs:23-93`), i.e. after the verdict the checker's
`check_instantiate` (`checker/structs.rs:542`) recorded. Hoisting it to where the
apply node's template is instantiated — the moment the function is born, which is
sanctioned site #2 and is already where `checker.rs:537-539` deep-evaluates lambda
value nodes — makes verdict and write agree, and takes out half the compute delta in
one move.

> **Superseded by §11.3.** This ordering is right about the *plain* gate and wrong as
> a first move under the concrete-only gate that landed. Measured there: the codomain
> bind produces **no** late write that reaches a decided verdict at all, and hoisting
> it contradicts a landed decision — [eval-before-unify](eval-before-unify.md) §7.1
> records the checker-side bind being deleted for exactly this reason.

**Second: the capture edge (the `graph_jit` expr).** Same mechanism, but the writer
is `apply_parameter_check` (`apply.rs:78`) merging a free variable's class. A
capture is a *free variable*, so under "the checker does the initial unify" it
belongs at check time; the capture edge exists there already — `check_lam`
(`checker/lambda.rs:19`) places the body in the enclosing function's scope
(`lambda.rs:35-39`) — and only its **value** arrives later. So this one needs the
decision "is a capture unified at check time or at apply time?" That is a design
question, not a code question, so it goes after the codomain bind.

**Not first: the three `lichen-lowlevel` epoch tests.** They are the maintainer's
call (§7), and two of the seven late writes they pin are test-local
`write_node_value` calls that no unify-site rule reaches — so "move the write
earlier" has no handle there at all.

## 9. Where the brief's evidence needed correcting

1. **The compute baseline was 204 / 14 / 2 as recorded — no discrepancy.** The brief
   asked for a second measurement if it differed; it did not differ, so the
   `Compare-Object` is baseline-vs-gate, not run-vs-run.
2. **`a_stalled_operator_reruns_only_after_a_value_changed` does not purely pin the
   epoch design.** Measured: its run-gate half passes under **both** plain gates and
   only its re-decision half fails. See §7.
3. **The lowlevel delta is about the `undecided: true` verdict, not about
   re-deciding decided subtrees.** Measured: a concrete-only cut breaks none of the
   three. See §4.1 and §7.
4. **The deep pass is in the shallow path**, through `OperatorExt::run_deferred`
   (`lib.rs:392`) — not in the brief, and it is what lets a test that only calls
   `evaluate_node` break under a *deep* gate. See §4.5.
5. **The four compute late writes are all at sanctioned sites**, not stray slot
   writes — so they are an *ordering* defect inside the sanctioned design, not a
   missing writer. The brief's framing ("a closure capture, or an apply callee,
   arriving after the verdict") is confirmed and sharpened. See §6.

## 10. Reproducing this

```
# private target, never the parent's
$env:CARGO_TARGET_DIR = "C:\resource\lichen-vm\.worktrees\dlw\target"
$env:CARGO_INCREMENTAL = "0"; $env:CARGO_PROFILE_DEV_DEBUG = "0"; $env:CARGO_PROFILE_TEST_DEBUG = "0"

# baseline
cargo test -p lichen-lowlevel  -j 1 --no-fail-fast   # 147 / 13
cargo test -p lichen-highlevel -j 1                  # 140 / 0
cargo test -p lichen-compute   -j 1 --no-fail-fast   # 204 / 14 / 2

# the plain deep gate
$env:LICHEN_PLAIN_DEEP_GATE = "some"                 # or "concrete"
cargo test -p lichen-lowlevel  -j 1 --no-fail-fast   # 144 / 16
cargo test -p lichen-highlevel -j 1                  # 140 / 0
cargo test -p lichen-compute   -j 1 --no-fail-fast   # 200 / 18 / 2
```

The gate, the trace and the plain run gate were **scratch edits, reverted before this
file was committed** — see §1 for what they were. A single env var
`LICHEN_PLAIN_RUN_GATE=1` made `run_stamp_is_current` read only `runned != 0`, which
is the other half of §7. `LICHEN_LATE_TRACE=1` made `set_node_value` print every
write to a node that already held a verdict, with a `Backtrace` naming the path, and
printed the two verdict-write sites; under the gate it additionally printed a
`CUTLATEWRITE` for the write that landed on a node the gate had already decided,
which is the per-test signal used above.

## 11. What landed on top of this map

> Slice: `feature/concrete-gate`, worktree `.worktrees/concrete`, base `dev@e3bb7c2b`.
> Build: private target `C:\resource\lichen-vm\.worktrees\concrete\target`, `-j 1`,
> `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`.
> `git diff dev...HEAD --name-only` lists `crates/lichen-lowlevel/src/evaluation.rs`
> and the two notes, and nothing else.

### 11.1 The concrete-only deep gate

`crates/lichen-lowlevel/src/evaluation.rs`, `evaluate_node_deep_inner`, between the
`Dyn(node)` match and the structural cycle-cut arm:

```rust
        // A concrete verdict is final, so its value is the answer; an `undecided` one is asked
        // again (late-writes-map.md §11).
        if let Some(deep) = self.nodes[node].evaluated_deep
            && !deep.undecided
            && let Some(value) = self.nodes[node].value
        {
            return Some(value);
        }
```

This is §4.1's last row and §7's judgement, landed as code: **a decided verdict is
final, an `undecided` one is a request to be asked again.** It is not §4's rejected
`settled` bit — no second bit, no new state, nothing computed that was not computed
before. It is weaker than the refactor's goal, because an undecided node is still
re-descended; §11.3 says what that costs.

### 11.2 The suites, measured against a pristine `dev` in the same worktree

Both sides ran in `.worktrees/concrete`, the baseline before the edit and the gate
after it, so the artifact tree and the commit differ by exactly the one file.

| suite | pristine `dev` | with the gate | name-set delta |
|---|---|---|---|
| `cargo test -p lichen-lowlevel -j 1 --no-fail-fast` | 147 / 13 | **147 / 13** | `Compare-Object`: **empty** |
| `cargo test -p lichen-highlevel -j 1` | 140 / 0 | **140 / 0** | **empty** |
| `cargo test -p lichen-compute -j 1 --no-fail-fast` | 204 / 14 / 2 | **204 / 14 / 2** | `Compare-Object`: **empty** |

The compute name set is §3's fourteen, unchanged. Both `Compare-Object` comparisons
were run on **names**, never on counts.

**The three epoch tests pass, unchanged and by name** (`--exact`, all `ok`):

- `evaluation::a_stalled_operator_reruns_only_after_a_value_changed`
- `assert::an_untriggered_assert_is_decided_by_a_later_drain`
- `function::function_call_operator_preserves_undecided_operand_chain`

So [stalled-rerun-epoch](stalled-rerun-epoch.md) is **not** retired by this gate: the
run stamp it landed keeps its own job, and §7's question about the three tests is
unanswered because the gate does not reach the thing they pin.

### 11.3 What this map's §8 got wrong, measured

**The four compute tests §6 traces were never red under this gate.** All four pass
under the plain gate's *failure* list being the right reading — and under the
concrete-only gate they pass on `dev` too, so they were never in a baseline failure
set to begin with:

| test | on `dev` | plain gate (§4.4) | concrete-only gate |
|---|---|---|---|
| `graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture` | **ok** | new failure | **ok** |
| `graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one` | **ok** | new failure | **ok** |
| `graph_structure::a_body_dispatches_through_an_apply_whose_callee_holds_the_operator` | **ok** | new failure | **ok** |
| `graph_structure::the_kernel_slot_is_a_field_read_whose_target_is_the_captured_kernel_struct` | **ok** | new failure | **ok** |

(Each confirmed by name with `--exact`: `4 passed; 0 failed`.)

The reading is the one §4.1's last row already implied and §7 stated: **the delta is
about the `undecided: true` verdict, not about re-descending decided subtrees.** The
plain gate cuts undecided verdicts too, so it is the seven late writes of §5 and §6
that break. The concrete-only gate cuts only decided ones, and those seven writes all
land on undecided verdicts — so they are writes the gate never made final.

**So §8's first target does not fire under the gate that landed.** Measured with the
same trace §10 describes, re-instrumented behind `LICHEN_LATE_TRACE=1` under the
concrete-only gate and run over all three suites:

| suite | late writes | landing on a **concrete** verdict | landing on an `undecided` verdict |
|---|---|---|---|
| `lichen-lowlevel` | 0 | **0** | 0 |
| `lichen-highlevel` | 0 | **0** | 0 |
| `lichen-compute` | 201 | **0** | 201 |

**Zero** of them is `static_module/apply.rs:90`. Where the 201 come from, by the
frame under `set_node_value`:

| count | path |
|---|---|
| 148 | `unify` → `add_equality` → `fill_class_holes` — a class merge filling an empty member, most through the structural `all::check` walk |
| 16 | `evaluate_node_operation` → `write_node_answer` → `write_node_value_if_absent` → `propagate_class_value` |
| 15 | `unify_inner` → `write_node_value` (the `all::try_fold` / `all::check` walks) |
| 15 | `unify_inner` → `write_node_value` → `propagate_class_value` |
| 6 | `try_unify` → `unify` → `add_equality` → `fill_class_holes` |
| 1 | `static_function_apply` → `wire_apply_result` (`static_module/apply.rs:137`) |

`apply_parameter_check` (`static_module/apply.rs:78`) does not appear either.

**What remains** is therefore one class of write, not seven sites: a **unify that
merges a class and fills an empty member** (`fill_class_holes`), which is where 154
of the 201 arrive. That is the class of write the refactor's own rule — "beyond
checker / function-birth normalization / apply, no unify is produced dynamically" —
already names as illegitimate. It is a real target; it is not §8's.

> **The probe is not free, and this run shows it.** `Backtrace::force_capture` on
> every late write moved compute from 204 / 14 / 2 to 203 / 15 / 2, adding exactly
> `consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched`.
> That failure is the probe's, not the gate's — §4.2's trap, met again. Every suite
> number in §11.2 is from a run with no probe in the crate.

> **Superseded on its counts by §12.** The three-suite table above and the frame
> breakdown under it were re-measured without a `Backtrace` and do not reproduce: §12
> reads **6 268** late writes in `lichen-compute` where this table reads 201, and
> attributes 3 621 of them to `fill_class_holes` where this table attributes 154.
> §12.5 also corrects this section's account of **why** the probe perturbs the suite.
> What this section still stands for is its shape — the delta is about the `undecided`
> verdict, and what remains is one class of write — and its correction to §8.

### 11.4 Why the codomain bind was not hoisted

The brief's Part 2 was to move `module.unify(cell, codomain)` to the moment the
function is born. It is not done, and the reason is not effort:

1. **The codomain is minted by the apply, so at function birth it does not exist.**
   `static_module/apply.rs:84` reads `expr_type_item(applied)` where `applied =
   static_node_apply(r#return, …)` — the **clone** the call minted
   (`static_module/apply.rs:40`, `:286`). Hoisting to the template's `return_type` is
   a *different unify against a different node*.
2. **That different unify is a landed decision, already tried and measured.**
   [eval-before-unify](eval-before-unify.md) §7.1: the checker-side bind that read
   the template's own cells "made the same two comparisons in `check_app`, against
   the template's **own** cells, so a call site *decided* them: with `type_of = x =>
   {t = _; x: t; t}`, dev answers `(Int, Type)` … the checker-side version answered
   `expected Int, found Type` — the template was monomorphic after its first call."
   Commit `1ed5f353` deleted it: "The checker-side domain/codomain unification is
   deleted with it."
3. **It contradicts an `# Invariant`.** `Module::function_cells`
   (`crates/lichen-lowlevel/src/equality.rs:519-522`): "These are the template's own
   cells, so **only a clone may descend them**: a caller that descended the template
   would decide its signature by the first reader."
4. **The clone's codomain carries more than the template's.** [index-read-unify](index-read-unify.md)
   §7.2 measures the clone's tail as "the *clone's own* result cell, in a class of its
   own", which is why `close_shallow_tail` exists as a second step.

Per the brief's own stop condition, this is reported with the measurement rather than
substituted. The site is untouched.

## 12. The `fill_class_holes` slice: where they come from, and what removing them is worth

> Slice: `feature/class-hole-fill`, worktree `.worktrees/chf`, base `dev@c666a372`.
> Build: private target `C:\resource\lichen-vm\.worktrees\chf\target`, `-j 1`,
> `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`.
> **No behaviour change landed.** Every code edit this slice made was a scratch probe
> and was reverted; the branch's code diff against `dev` is empty and it carries
> documentation only.
>
> **Which numbers are probe-free.** Every **suite result** here (§12.6) is from a run
> with no probe in the crate. Every **count** (§12.2, §12.3, §12.4) needs the probe and
> is reported as instrumented — see §12.5 for why that is not a formality.

### 12.1 The question this slice was given, and the reading it came to

The brief authorised changing `fill_class_holes`'s own `# Invariant` if the elimination
required it, on the reading that §11.3's remaining writes are the next elimination. The
reading this slice measured is narrower and it is the one that decides the work: *a late
write onto an `undecided` verdict costs a re-descent only if the verdict stays undecided
longer than it otherwise would.* That is a measurable question, and §12.4 measures it
by running the change rather than arguing it. The answer is that the cost is **one
descent in 929 488**.

### 12.2 Where the late writes come from, by `file:line`

The probe tags every *public* unification entry with the `file:line` that called it —
`Module::unify`, `Module::try_unify`, `write_node_answer`, and each named call site — and
reads that tag inside `set_node_value` and `fill_class_holes`. Nodes are labelled
`<module instance>:<NodeId>`, because `NodeId`s restart with every `Module` and a bare id
is not unique across a suite.

`lichen-compute`, **6 268 late writes** — a write to a node that already holds an
`evaluated_deep` verdict — of which **every one lands on an `undecided` verdict**:

| writer | count |
|---|---|
| `fill_class_holes` (`equality.rs:111`, from `add_equality` `equality.rs:99`) | **3 621** |
| `propagate_class_value` (`equality.rs:345`) | 556 |
| `write_node_value` (`equality.rs:257`) | 147 |
| `relocate_node_value` (`equality.rs:307`) | 0 |

The 3 621 class-hole fills, by the unification that performed the merge:

| origin — `file:line` | what | sanctioned site | fills |
|---|---|---|---|
| `lichen-highlevel/src/checker/diagnostics.rs:31` | `Checker::check_unify` | **checker** (site 1) | **2 738** |
| `lichen-lowlevel/src/apply.rs:139` | `wire_apply_result`'s `unify(node, applied)` | **apply** (site 3) | 653 |
| `lichen-lowlevel/src/apply.rs:78` | `apply_parameter_check` | **apply** (site 3) | 227 |
| `lichen-lowlevel/src/static_module/apply.rs:90` | the apply's codomain bind | **apply** (site 3) | 3 |

Traced instances, one per row, as the probe wrote them:

```
FILL origin=checker/diagnostics.rs:31        rep=NodeId(11v1)  member=2:NodeId(821v1)  verdict=undecided value=array  would_decide=Some(true)
FILL origin=apply.rs:139                      rep=NodeId(248v1) member=28:NodeId(248v1) verdict=undecided value=scalar would_decide=Some(true)
FILL origin=apply.rs:78                       rep=NodeId(11v1)  member=…              verdict=undecided value=array  would_decide=Some(false)
FILL origin=static_module/apply.rs:90         rep=NodeId(321v1) member=884:NodeId(1671v1) verdict=undecided value=scalar would_decide=Some(true)
```

`lichen-lowlevel` has **12** late writes in the whole suite and `lichen-highlevel` has
**0**; one of the lowlevel twelve is a `fill_class_holes` write and it is a flip (below).

**Three readings that matter more than the totals.**

1. **The merges are already at sanctioned sites — all of them.** 76 % of the fills are
   the checker's own `check_unify` (site 1) and the remaining 24 % are apply-time binds
   (site 3). Not one arrives from a fourth, unsanctioned producer. The brief's first
   route — *move the merge to the sanctioned moment it belongs to* — has no handle here,
   because it is already there.
2. **The brief's second route is also already satisfied.** `add_equality` forms the class
   at `disjoint::union` (`equality.rs:94`) and calls `fill_class_holes` two statements
   later in the same function (`equality.rs:99`). The class *is* formed where the fill
   runs; there is no window between them to move the fill into.
3. **So what is left is an ordering inside the checker**, not a site: the verdict was
   written when the checker evaluated the node, and the merge that would have decided it
   is the checker's own later `check_unify`. That ordering — evaluate the operands,
   *then* unify — is a landed decision with its own measurement
   ([eval-before-unify](eval-before-unify.md) §2.1, and `Checker::compute_operands`'s
   comment: *"undecided-against-decided is the one unify arm that writes"*).

### 12.3 The flip: would the verdict be concrete the first time?

For every fill that lands on a member already carrying an `undecided` verdict, the probe
asks the counterfactual the brief names: **had this value been in the slot when the
verdict was computed, would that verdict have been concrete?** It answers with the same
rule `value_is_undecided` uses, reading dynamic verdicts only; a value whose items
include a static ref or a table is reported as unknown rather than guessed.

| | events | distinct nodes |
|---|---|---|
| the written value would decide the member (`Some(true)`) | **2 874** | **2 874** |
| it would not (`Some(false)`) | 614 | 614 |
| unknown — a table, or an item that is a static ref (`None`) | 133 | 133 |
| **total** | **3 621** | **3 621** |

**The flip count is not zero: 2 874 nodes would have been decided concrete the first
time** — 79 % of the class-hole fills, 1 236 of them from the checker's own unify and 507
from apply. Every fill is a distinct node, so there is no double counting to discount.

The lowlevel suite's single `fill_class_holes` late write is also a flip
(`would_decide=Some(true)`), which is the shape
[late-writes-map](late-writes-map.md) §5.3 recorded for
`function_call_operator_preserves_undecided_operand_chain`.

### 12.4 What the flip is worth: one descent in 929 488

The flip being real is not the same as the elimination being worth landing, so this was
measured by running it. The candidate is the smallest thing that could deliver the flip
without moving a merge: **`fill_class_holes` writes the member's verdict when the value
it just wrote decides it**, so the stale `undecided` verdict stops being stale and the
landed gate cuts the node from that moment. It is behind `LICHEN_VERDICT_ON_FILL` for
the A/B, both arms over the same probe, same suites, same binary.

| suite | descents (gate passes) `off` | `on` | delta |
|---|---|---|---|
| `lichen-compute` | **929 488** | **929 487** | **−1** |
| `lichen-lowlevel` | 203 641 | 203 685 | +44 |
| `lichen-highlevel` | 54 080 | 54 114 | +34 |

The compute saving is **one descent in 929 488 — 0.0001 %**. The two smaller suites go
*up* by an amount at the level of run-to-run noise, and no suite changes its failure set
in a way attributable to the change.

The static reading explains the number, and it is the finding. The 2 874 flip nodes are
descended **5 633 times in total** across the whole compute suite — **0.61 %** of all
descents. So even a perfect elimination caps out at removing well under one percent of
the deep walk's work, and the A/B shows the realised saving is three orders of magnitude
below even that cap. The reason is that **these verdicts are already re-decided almost
immediately**: the suite writes 399 589 re-decisions of which 6 852 turn concrete, and a
node whose verdict flips concrete is cut by the landed gate from then on. The late fill
therefore costs the walk one extra ask of a node that was going to be asked again
anyway, and buys nothing that the next ask does not already buy.

**So: the flip is real and the elimination buys nothing for "run once."** By the brief's
own stop condition, that is reported rather than landed.

### 12.5 The probe perturbs, and it is not `Backtrace`

§11.3 recorded that `Backtrace::force_capture` moved `lichen-compute` from 204 / 14 / 2
to 203 / 15 / 2, adding `consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched`,
and attributed the delta to the backtrace. **This slice reproduced that exact delta with
no backtrace anywhere in the crate** — the same 203 / 15 / 2, the same one added name. The
probe here uses no `Backtrace` at all: it is a thread-local origin label, two counters,
and one append-only file.

Two measurements narrow the cause, and both are reported because the correction matters
more than the attribution:

- Removing the probe's hottest allocation — a `String` built on **every** gate pass
  (929 487 times) — did **not** remove the delta. It survived unchanged.
- The `off` and `on` arms differ by that same test's presence in one direction and its
  absence in the other across two runs, while the descent counts move by 1.

So the delta follows **any** instrumentation that shares mutable state across the suite's
**parallel test threads** — a global mutex and a file write per event is enough — and it
is not evidence about `force_capture` in particular. The correct reading of §11.3's note
is the weaker one it already half makes: *"a probe is not free"*, with the mechanism
being shared state across threads rather than stack capture.

**Why §11.3's counts do not reproduce is not established here.** 201 against 6 268 is a
factor of thirty-one, and this slice's probe is materially lighter than a forced capture
on every late write, so the likelier reading is that §11.3's run was truncated or
distorted by the capture it installed — but that is a conjecture, and it is recorded as
one. What is not a conjecture: §12.2's numbers come from one probe over one run and are
internally consistent (every fill is a distinct node; the writers and the origins
partition the total).

### 12.6 The suites, probe-free, and the two name-set comparisons

Measured on this branch's tip with **no probe in the crate**, against a pristine `dev`
run in the same worktree. Name sets compared with `Compare-Object`, never counts.

| suite | pristine `dev` | this branch | name-set delta |
|---|---|---|---|
| `cargo test -p lichen-lowlevel -j 1 --no-fail-fast` | 147 / 13 | **147 / 13** | **empty** |
| `cargo test -p lichen-highlevel -j 1` | 140 / 0 | **140 / 0** | **empty** |
| `cargo test -p lichen-compute -j 1 --no-fail-fast` | 204 / 14 / 2 | **204 / 14 / 2** | **empty** |

The three epoch tests pass by `--exact` name, unchanged:
`evaluation::a_stalled_operator_reruns_only_after_a_value_changed`,
`assert::an_untriggered_assert_is_decided_by_a_later_drain`, and
`function::function_call_operator_preserves_undecided_operand_chain`.

### 12.7 What this leaves open, stated as a question rather than a change

The one thing that would make these 2 874 verdicts concrete *the first time* is for the
merge that carries the value to happen **before** the deep walk first reaches the
member. That is the checker's own evaluation/unify order, and §12.2's third reading says
why that is a decision and not a code change: [eval-before-unify](eval-before-unify.md)
§2.1 measured the other order and it loses. Re-deciding it needs the maintainer, not a
slice. Until then the honest state is that **the remaining late writes are accounted
for, are all at sanctioned sites, and cost one descent.**

## 13. The plain gates, measured together and apart

> Slice: `feature/undecided-final`, worktree `.worktrees/ufin`, base `dev@71adaa1a`.
> Build: private target `C:\resource\lichen-vm\.worktrees\ufin\target`, `-j 1`,
> `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`.
> **No probe and no instrumentation anywhere in the measured crates.** Every suite
> number below is from a run with nothing else in the process, and each arm was run
> **twice** with identical name sets, so §12.5's trap is not in play here.
> `git diff dev...HEAD --name-only` on this branch lists
> `crates/lichen-lowlevel/src/evaluation.rs`, `crates/lichen-lowlevel/src/module.rs`
> and the three notes, and nothing else. **No test was edited.**

### 13.1 What was made plain

Both gates of §0's goal at once: `evaluate_node_deep_inner` cuts **any** verdict
(§11.1's `is_some_and(|deep| !deep.undecided)` becomes `evaluated_deep.is_some()`),
and the run gate stops comparing the stamp with the value epoch
(`Module::run_stamp_is_current` becomes `run_stamp_is_set`, reading `runned != 0`).

### 13.2 The arms, and what each one costs

Probe-free, in this worktree, so the `dev` row and every arm differ by exactly the
one file. **Name sets compared with `Compare-Object`, never counts.**

| arm | `lichen-lowlevel` | `lichen-highlevel` | `lichen-compute` | new failures by name |
|---|---|---|---|---|
| `dev@71adaa1a` | 147 / 13 | 140 / 0 | 204 / 14 / 2 | — |
| deep gate plain alone | 145 / 15 | 140 / 0 | 200 / 18 / 2 | 2 lowlevel, **4** compute |
| run gate plain alone | 144 / 16 | 140 / 0 | 139 / 79 / 2 | 3 lowlevel, **65** compute |
| both | 144 / 16 | 140 / 0 | 139 / 79 / 2 | 3 lowlevel, 65 compute |

**§4.1 and §4.4 were right about what they measured and silent about the other
half.** §4.1 recorded the run gate's `lichen-lowlevel` row ("the same three, by
name"); §4.4 recorded the deep gate's `lichen-compute` row (four new). Neither ran
the **run** gate over `lichen-compute`, so this map's own headline claim — that the
delta is about the `undecided` verdict and not about re-descending decided
subtrees — was never tested against the axis that turned out to carry 61 of the 65.
The deep gate alone still reproduces §4.4 exactly (200 / 18 / 2, the same four
names), so the base changes since then moved nothing on that arm.

**The two arms are not additive.** Under the run gate alone and under both gates the
three suites' failure name sets are identical, name for name. Two of the deep gate's
four — `graph_structure::a_body_dispatches_through_an_apply_whose_callee_holds_the_operator`
and `graph_structure::the_kernel_slot_is_a_field_read_whose_target_is_the_captured_kernel_struct`
— do not fail at all once the run gate is also plain, and the other two are among the
run gate's sixty-five. So the deep cut adds nothing on top of the run gate's, and it
is the run gate that carries the whole delta.

The `lichen-highlevel` suite is unaffected by either arm: 140 / 0 throughout.

### 13.3 `lichen-lowlevel`: three new, all of them design-pins

Against the thirteen GC/compaction baseline failures of §3, by name:

> **This slice edited no test.** §15 records what became of all three: the epoch test
> retired, the other two rewritten to the narrower true statement.

```
assert::an_untriggered_assert_is_decided_by_a_later_drain          broken by both arms
evaluation::a_stalled_operator_reruns_only_after_a_value_changed   broken by both arms
function::function_call_operator_preserves_undecided_operand_chain broken by the run gate alone
```

All three assert the retired behaviour itself and nothing else, and
[stalled-rerun-epoch](stalled-rerun-epoch.md) §8.5 gives each one's assertion line,
its observed value and its proposed disposition. No test was edited.

### 13.4 `lichen-compute`: sixty-five new, classified

One cause behind all sixty-five: `compute.graph` records a function by applying it
to its own placeholder arguments, and under the plain run gate the operators that
stalled during that first pass never run again, so the body's return is not
resolvable to a value-table entry. The refusal is `compute.rs:5222-5231`:

> compute.graph: this function's return is not a value a graph can hand back, so
> there is no graph to record: a graph's value table holds what its dispatches
> produced and the arguments it was run with, and this body's own value is not one
> of those

**57 wrong-value — the program no longer checks.** Fifty-six fail at the testing
helper's own `assert!(report.ok(), …)` (`crates/lichen-language/src/testing.rs:70`)
and one at `crates/lichen-compute/tests/lichen_compute/composed_chain.rs:26`
(`compile(SOURCE).expect("the chain compiles")`). Each has exactly that one
diagnostic where it had none: a kernel that merged, tiled, recorded and answered on
the device does not build at all.

```
lichen_compute::chain_merge::a_chain_of_three_merges_into_one_node_under_the_floor
lichen_compute::chain_merge::a_consumer_that_reads_two_producers_is_a_fan_in_and_merges
lichen_compute::chain_merge::a_returned_middle_output_is_refused_by_name_and_nothing_merges
lichen_compute::chain_merge::the_merged_chain_answers_the_un_passed_program_and_a_moved_bias_moves_it
lichen_compute::composed_chain::the_composed_chain_answers_the_two_pass_reference_and_the_recorded_graph
lichen_compute::consumer_guard::a_combine_only_consumer_is_refused_by_the_edge_and_the_differential_holds
lichen_compute::consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched
lichen_compute::consumer_guard::the_program_answers_the_same_with_the_pass_and_without_it
lichen_compute::elementwise_chain::an_elementwise_chain_derives_a_schema_the_floor_declines_by_name
lichen_compute::elementwise_chain::two_elementwise_kernels_derive_a_region_and_the_floor_declines_it_by_name
lichen_compute::fan_in_merge::a_consumer_of_two_producers_merges_into_one_node_under_the_floor
lichen_compute::fan_in_merge::a_fan_in_whose_element_does_not_name_its_second_producer_is_refused_by_name
lichen_compute::fan_out_merge::a_fan_out_derives_a_region_a_pack_can_name
lichen_compute::fan_out_merge::a_pack_naming_the_fan_out_merges_all_three_nodes_into_one
lichen_compute::fan_out_merge::under_a_pack_the_device_answers_the_program_and_the_counterfactual_moves
lichen_compute::fan_out_merge::without_a_pack_the_fan_out_is_refused_by_name_and_nothing_merges
lichen_compute::fuse_licence::a_lane_dependent_value_read_at_a_computed_address_is_recomputed
lichen_compute::fuse_licence::a_pack_naming_another_region_is_inert
lichen_compute::fuse_licence::a_pack_naming_the_region_merges_it_and_the_report_names_the_pack
lichen_compute::fuse_licence::a_read_at_a_computed_address_runs_on_the_device_and_the_counterfactual_moves
lichen_compute::fuse_licence::a_read_at_a_conditional_address_runs_on_the_device_and_the_counterfactual_moves
lichen_compute::fuse_licence::an_elementwise_producer_feeding_a_reduction_consumer_merges_and_the_device_agrees
lichen_compute::fuse_licence::the_floor_still_merges_a_reduction_chain_with_no_pack_installed
lichen_compute::fuse_licence::the_fuse_directive_is_read_as_the_format_spells_it
lichen_compute::fuse_licence::with_the_pack_the_device_answers_the_program_and_the_counterfactual_moves
lichen_compute::fuse_licence::without_a_pack_the_floor_declines_by_name_and_the_graph_is_untouched
lichen_compute::fused_region::a_composed_node_is_named_by_a_second_merge_pass_and_left_alone
lichen_compute::fused_region::a_fan_out_region_with_no_pack_is_refused_by_name
lichen_compute::fused_region::a_returned_producer_output_is_refused_by_name_and_nothing_merges
lichen_compute::fused_region::an_address_the_consumer_computes_is_answered_by_the_producer_s_own_cone
lichen_compute::fused_region::extents_that_do_not_correspond_are_refused_by_name_and_nothing_merges
lichen_compute::fused_region::the_floor_reports_the_merge_and_then_the_tiling_pass_on_the_merged_node
lichen_compute::fused_region::the_floor_run_reports_both_stages_and_answers_the_un_passed_program
lichen_compute::fused_region::the_region_merges_into_one_node_and_the_surviving_node_holds_the_composed_fragment
lichen_compute::graph_argument::a_step_whose_argument_names_two_buffers_records_three_inputs
lichen_compute::graph_argument::the_two_buffer_chain_runs_on_the_device_and_answers_over_both
lichen_compute::graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches
lichen_compute::graph_jit::a_graph_runs_at_whichever_extent_its_argument_names
lichen_compute::graph_jit::a_recording_produces_the_same_numbers_as_running_the_dispatches
lichen_compute::mark_anchor::a_mark_inside_a_loop_still_names_its_whole_expression
lichen_compute::mark_anchor::a_mark_on_a_top_level_operator_names_its_whole_expression
lichen_compute::recompute_reads::the_merge_copies_the_producer_cone_once_per_read_at_that_read_s_address
lichen_compute::recompute_reads::the_recording_reads_one_buffer_at_two_addresses
lichen_compute::recompute_reads::two_reads_merge_and_the_device_answers_the_program
lichen_compute::rmsnorm_denominator::a_fixed_size_and_a_runtime_extent_both_fuse
lichen_compute::runtime_scalar_leaf::a_normalise_reading_a_runtime_scalar_merges_into_one_node
lichen_compute::runtime_scalar_leaf::a_producer_declaring_a_runtime_scalar_is_refused_by_name
lichen_compute::runtime_scalar_leaf::the_merged_diamond_answers_the_body_and_a_moved_bias_moves_it
lichen_compute::sibling_diamond::a_sibling_diamond_merges_into_one_node_under_the_floor
lichen_compute::sibling_diamond::a_sibling_diamond_reading_the_launch_extent_is_one_region_and_merges
lichen_compute::sibling_diamond::a_sibling_diamond_whose_element_names_one_producer_is_refused_by_name
lichen_compute::sibling_diamond::an_element_reading_the_invocation_index_is_one_region_and_merges
lichen_compute::sparse_input_classes::the_consumer_that_reads_only_its_second_input_runs_and_answers_the_closed_form
lichen_compute::tiling_visibility::a_composed_node_is_reported_by_name_and_nothing_lands_in_it
lichen_compute::tiling_visibility::an_ordinary_graph_reports_one_line_per_node
lichen_compute::union_ports::a_consumer_naming_only_the_absorbed_buffer_merges_and_the_device_agrees
lichen_compute::union_ports::a_node_whose_ordinals_disagree_with_its_body_is_refused_by_name
```

**2 wrong-value — nothing is recorded where the test asserts a recorded fragment.**
Both in `crates/lichen-compute/tests/lichen_compute/graph_argument.rs`, and both are
`assert!(records(&chain(…)))`: `:275` (the control) and `:368` (the accepted half of
the rule). Observed `false` where the baseline observed a fused fragment.

```
lichen_compute::graph_argument::a_second_buffer_records_under_the_in_group_and_does_not_beside_it
lichen_compute::graph_argument::a_declared_buffer_beside_the_in_group_fails_the_build_and_never_runs
```

**6 diagnostic-only — the refusal stands and its cause changes.** The program is
still refused, so the outcome is the one the test asks for, but the sentence is the
graph's fallback rule instead of the rule each test is about — which means the rule
under test no longer fires and the coverage is silently lost even where the outcome
matches.

```
lichen_compute::graph_argument::an_unannotated_step_is_never_refused_by_the_rule   graph_argument.rs:416
lichen_compute::graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture graph_jit.rs:263
lichen_compute::graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one graph_jit.rs:303
lichen_compute::graph_jit::what_a_recorded_body_may_not_reach_for_is_refused_by_name graph_jit.rs:342
lichen_compute::graph_jit::one_backend_for_the_whole_graph_is_checked_while_it_is_built graph_jit.rs:471
lichen_compute::graph_jit::a_graph_recorded_for_one_backend_runs_on_another graph_jit.rs:572
```

### 13.5 The counts, and what decides the question

| bucket | `lichen-lowlevel` | `lichen-compute` |
|---|---|---|
| design-pin | 3 | **0** |
| diagnostic-only | 0 | 6 |
| **wrong-value** | **0** | **59** |

`lichen-highlevel` has none of the three: 140 / 0 in every arm.

**The `wrong-value` bucket is not empty**, so by
[incremental-evaluation](incremental-evaluation.md) §5 step 1's own condition the
plain run gate does not land. Sixty-five `lichen-compute` tests, none of them a
pin of the retired behaviour, and fifty-nine of them asserting that the compiler
accepts a program it now refuses.

### 13.6 The epoch is unread

`Module::value_epoch` has no production reader after the run gate stops comparing
against it. What is left is `bump_value_epoch`'s own write (`equality.rs:288`),
`stamp_node_ran`'s read of it to stamp with (`module.rs:338`), and the public
accessor `Module::value_epoch()`, whose only reader in the tree is
`crates/lichen-lowlevel/tests/basic/evaluation.rs:417` — a test this slice is not
allowed to edit. `Node::runned` is read only for `!= 0`, so the `u32` is a `bool`
with room to spare. **Nothing was removed.**

> **Superseded by §15.** That test has since been retired, and with it all three items:
> `Module::value_epoch`, `bump_value_epoch` and `Module::value_epoch()` are gone.

### 13.7 What this leaves open

§12.4 measured that *repairing* a late write buys one descent in 929 488. This slice
measures something §12.4 never touched: **removing the re-run itself** costs 65
programs that stop building. Those are different quantities, and together they say
the elimination path is not only cheap — the re-run it would remove is load-bearing.
The lever §12.7 named is unchanged and is still a maintainer's decision: the
checker's evaluate-before-unify order ([eval-before-unify](eval-before-unify.md)
§2.1).

**§14 revises the second of those two sentences, and only for the recording.** The
re-run is load-bearing *because the recording was waiting on it*, and the recording
no longer is; §14.5 measures that as 62 of these 65 going away without the gate
moving. Read this section with §14 beside it: the "sixty-five" in §13.5 and in
§13.7 is the count **before** the recording was made independent of the re-run.

## 14. Graph recording, and the operator it had to wait for

> Worktree `.worktrees/grec`, branch `feature/graph-recording`. Every number below is
> from a probe-free run: the probe that located the stalls was removed before any of
> them was taken, as §12.5 warns.
>
> **This is the one behavioural change this note records**, and it is the answer to
> §13's sixty-five — not a removal of the run gate, which §8 of
> [stalled-rerun-epoch](stalled-rerun-epoch.md) measures separately.

### 14.1 The cause

`compute.graph` records a body by **applying it to its own input placeholders**
(`build_graph`, `crates/lichen-compute/src/compute.rs:5038`) and then reads the
body's returned value tree. That read is `place_of_node` → `Module::node_value`, and
`node_value` is the **raw slot** — it evaluates nothing
(`crates/lichen-lowlevel/src/static_module.rs:169`). So the tree is readable only
if every operator along it **has already run**: a position the recording's deep pass
did not manage to run is an empty slot, not a lazy one.

The operators it did not manage to run are structural reads, and only structural
reads. Measured with a `TRACE-STALL` line per stall (the probe, removed before any
reported number): **204 `Index` and 31 `TableGet`, and nothing else** under the
plain run gate; **889 and 142** under the epoch gate. That is the whole population,
not a sample.

A stall is not the defect. Under the **landed** gate the recording works, because a
write elsewhere in the same pass moves the epoch and the stalled read is asked again.
That is an accident of ordering: the recording did not ask for a second attempt, it
received one. Under a plain run gate — `runned != 0` with no epoch comparison — the
second attempt never comes, and §13's sixty-five is what that costs.

Traced on `graph_argument::a_second_buffer_records_under_the_in_group_and_does_not_beside_it`,
the returned value is an array of **two** items and under the plain gate the **first
one's slot is empty**; under the epoch gate both hold values. The stall chain above it
is an `Index` whose target is another stalled `Index`, whose target is a `TableGet`,
and so on — each link re-asked, each re-refused by the run gate.

### 14.2 The two shapes that were not available

**Making the placeholders concrete enough.** Measured and unavailable. The chain
bottoms in cells that are **value-free *and* class-free** — `equality_representative`
is the node itself and its class has no parent — so no placeholder and no
unification ever reaches them. Decisively, `is_a_graph_placeholder`
(`compute.rs:425`) asks whether the node **holds** a `GraphInput`/`GraphValue`, so an
empty cell is not a placeholder by that rule at all: the two existing placeholder
rules (`ComputeOperator::Read`, `compute.rs:509`, and `ComputeOperator::BufferCollect`,
`compute.rs:516`) cannot see the thing that stalls, let alone refuse it. The stall is
a value nobody has written yet, not a placeholder that is too coarse.

**Recording where the callee's types are known.** Measured, and already the case.
`build_graph` reads the parameter's declared type at `compute.rs:5076`
(`parallel_roles`) and builds the placeholder from the paths it decodes, falling back
to a flat tuple at a ceiling only when the roles come back `RoleUnread::Undecided`.
The recording is not short of a type; it is short of a **slot write**.

### 14.3 The landed shape, and what it costs

`build_graph` now drives its own bounded re-evaluation
(`evaluate_until_the_return_is_nameable`, `compute.rs:5351`). When the return cannot
be named, `Module::reopen_stalled` (`crates/lichen-lowlevel/src/module.rs:367`)
clears the run stamp of the operators under the recording's block that have an
operation, an **empty** value slot and a stamp, and the pass is re-driven — at most
`RECORDING_ROUNDS` (4) times, and a round that re-opens nothing ends the loop, so a
body that can never decide still refuses after one wasted round.

Three properties are what make the retry sound rather than merely effective:

- **A dispatch that answered is not re-opened**, so a retry records no second
  dispatch: the graph a body ends with is the graph its first round recorded.
- **The retry is the recording's, not the module's.** It asks for one subtree, so it
  moves no epoch for anyone else. The epoch run gate, `Module::value_epoch` and
  §11.1's concrete-only deep gate are all untouched — §14.5 measures a plain gate
  *over* the fix, which is the only way this can be told apart from §13.
- **A round re-opens only what stalled**, so the cost is bounded by the recording's
  own graph and not by the program's.

Two narrowings were tried and **rejected by measurement**:

| narrowing | arm result | why it does not work |
|---|---|---|
| re-open only operators whose deep verdict is `undecided` | 139 / 79 / 2 — the arm unchanged | an operator an `Index` forces through its **operand** edge is evaluated *shallowly* and never gets a deep verdict at all |
| re-open only nodes in child blocks, not the scope's own | 139 / 79 / 2 — the arm unchanged | the chains live in the recording's **own** scope, not in a child block |

The landed numbers are also a trap worth naming: **the gated suites do not tell the two
narrowings apart.** Both read 147 / 13, 140 / 0 and 204 / 14 / 2. Only the plain-gate
arm distinguishes them, which is the whole reason §8.2's arms exist.

### 14.4 The experiment arm, by name

The gate was temporarily plain (`LICHEN_PLAIN_RUN_GATE=1` on `run_stamp_is_current`,
making it `runned != 0` with no epoch comparison), reverted before the commit, on top
of the landed fix.

| run | `lichen_compute` integration binary |
|---|---|
| plain gate, no fix (§13's arm) | 139 passed / **79** failed / 2 ignored |
| plain gate, **with the fix** | **201** passed / **17** failed / 2 ignored |
| epoch gate (landed), with the fix | 220 passed / 14 failed / 2 ignored |

`Compare-Object` on names, never counts: **62 fixed, 0 newly failing**. The 17 that
remain are the 14 of `base-compute-names.txt` — which fail identically on a pristine
`dev` — plus three:

| name | why it remains |
|---|---|
| `consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched` | the extra round reads one algebra name further than the first (`["combine","identity","element"]` against `["combine","identity"]`): re-driving a pass forces operand edges the first pass did not reach |
| `graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture` | the program is **accepted** (`expected this program to fail`) — of §13's six "diagnostic-only" tests this is the one the retry changes rather than merely preserves. The cause was measured later as *no capture in the program at all*: the lazy run records the closed-over launch, so the refusal's precondition never occurs (`graph-recording-rounds.md`, last section) |
| `graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches` | the **host** run is the broken half: under the plain gate the program itself performs no dispatch (`right: []`), so recording and running cannot agree however the recording is driven |

The last one is not a recording failure at all — it is the plain gate breaking
`lichen-compute` outside the recording.

### 14.5 The suites, probe-free

Gates untouched, against the pristine `dev` measured in this same worktree
(`base-compute-names.txt`, `base-lowlevel-names.txt`), name sets compared with
`Compare-Object`:

| suite | pristine `dev` | this branch | name-set delta |
|---|---|---|---|
| `cargo test -p lichen-lowlevel -j 1 --no-fail-fast` | 147 / 13 | **147 / 13** | **empty** |
| `cargo test -p lichen-highlevel -j 1` | 140 / 0 | **140 / 0** | **empty** |
| `cargo test -p lichen-compute -j 1 --no-fail-fast` | 204 / 14 / 2 | **204 / 14 / 2** | **empty** |

### 14.6 What this leaves for the gate

The recording was the run gate's last **recorded** dependent, and it no longer is:
the dependency was an accident of the epoch happening to move, and it is now an
explicit, bounded, local retry. §8.5's three lowlevel design-pins and
§14.4's three compute failures were what the epoch comparison was waiting
on, and both lists have since been disposed of: [stalled-rerun-epoch](stalled-rerun-epoch.md)
§8 records the retirement and §15 below records its cost.

## 15. The retirement landed: three names, by comparison

> Slice: `feature/retire-epoch`, worktree `.worktrees/re`, base `dev@12425166`. Private
> target `C:\resource\lichen-vm\.worktrees\re\target`, `-j 1`, `CARGO_INCREMENTAL=0`,
> `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`. **Probe-free**: no
> instrumentation is in any measured crate, so §12.5's trap is not in play.
> `git diff dev...HEAD --name-only` lists four `lichen-lowlevel` sources, three
> `lichen-lowlevel` tests and the notes; no `lichen-compute` file is touched.

### 15.1 The suites, against a pristine `dev` measured in the same worktree

Name sets compared with `Compare-Object`, never counts.

| suite | pristine `dev` | after the retirement | name-set delta |
|---|---|---|---|
| `cargo test -p lichen-lowlevel -j 1 --no-fail-fast` | 147 / 13 | **146 / 13** | the retired test leaves and the rewritten assert test changes name; **no failure moves** |
| `cargo test -p lichen-highlevel -j 1` | 140 / 0 | **140 / 0** | **empty** |
| `cargo test -p lichen-compute -j 1 --no-fail-fast` | 223 / 14 / 2 | **220 / 17 / 2** | three new failures, §15.2 |

The `lichen-compute` integration binary alone reads 204 / 14 / 2 before and **201 / 17 / 2**
after; the package figure adds that binary's 19 unit tests, which are green either way.

**§13's sixty-five are gone and §14.4's three are what landed.** The comparison confirms
§14.4's list by name rather than by count: exactly the three it names, and nothing else moves
in either direction — no name stops failing, and no name starts.

### 15.2 The three, and what each one is

| name | assertion site and observation | what it is |
|---|---|---|
| `consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched` | `consumer_guard.rs:256` — the **producer's** claim reads `left: ["combine","identity","element"]` against `right: ["combine","identity"]` | **the recording, reading one algebra name further.** §14.4's reading holds: re-driving a pass (`Module::reopen_stalled`) forces operand edges the first pass did not reach, so the role the producer is not supposed to claim is now in its claim |
| `graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture` | `graph_jit.rs:135` — `expected this program to fail: "raw[raw 2, raw 2, raw 2]: ?a"` | **the program is accepted, with no capture behind it.** Of §13's six "diagnostic-only" tests this is the one the retry changes rather than merely preserves; the rule's precondition is a live buffer, and the lazy run forces the closed-over launch inside the recording window, so `held.z` arrives as an edge (`graph-recording-rounds.md`, last section) |
| `graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches` | `graph_jit.rs:514` — `left` is the recorded graph's two dispatch lines, `right` is `[]` | **a premise mismatch, not a recording failure.** `right` is the trace of the **host** run, and the plain gate makes the program itself perform no dispatch at all — so the premise "a graph is a transcript of the run" has nothing to be a transcript of. No way of driving the recording repairs this |

### 15.3 What is no longer true of §13 and §14

§13.5's `wrong-value` bucket held the retirement back: fifty-seven programs stopped
checking. **None of them does now** — §14's bounded retry took that bucket to zero, and what
remains is a claim one role too wide, a refusal that does not fire, and a host run with no
dispatch in it. §13.7's "the re-run is load-bearing" was true only for the recording, and
§13's own closing note already said so; §15 measures the consequence: with the recording
fixed, the re-run it removed was worth **three tests, and not one of the three is a
recording failure**.

[stalled-rerun-epoch](stalled-rerun-epoch.md) §8 holds the retirement's own account, §8.5 the
three `lichen-lowlevel` test dispositions, and §8.6 the same three costs read from that side.
