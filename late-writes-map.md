# Late writes under a plain deep gate — the elimination map

> Slice: measurement only. **No behaviour change lands here.** The gate below is a
> scratch edit that is reverted before the commit; what this file holds is the
> per-test trace of every write that arrives *after* the deep verdict, the site
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
> was **reverted**; `git diff dev...HEAD --name-only` on this branch lists exactly one
> file, this one.

## 0. The goal, in the maintainer's terms

`unify` is separated from `evaluate`: the checker does the initial unify
(`expr: expr`), normalization happens at the moment a function is born, apply does
runtime unify, and **beyond those three sites no unify is produced dynamically**.

The payoff wanted: `evaluate` / `evaluate_deep` gate purely on their stamps —
`evaluated` = the value-epoch stamp, `evaluated_deep.is_some()` — so **a node is
evaluated and decided exactly once, never re-decided**.

The `settled` bit of [incremental-evaluation](docs/notes/incremental-evaluation.md)
§4 was **rejected** ("加了 `settled` 等于没重构"). So the late writes are defects to
eliminate at their source, not state to be tolerated.

## 1. The gate under experiment

The experiment: give the deep walk the plain cut that the rejected `settled` design
would have given, but with the gate reading **only** `evaluated_deep.is_some()` —
no second bit, no new state.

Inserted in `crates/lichen-lowlevel/src/evaluation.rs`,
`evaluate_node_deep_inner`, after the static-leaf and cycle-cut arms and before
`deep_depth` is charged (the position
[incremental-evaluation](docs/notes/incremental-evaluation.md) §4.4 names):

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
[stalled-rerun-epoch](docs/notes/stalled-rerun-epoch.md) §4, are exactly:

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
[gc-trigger-disabled](docs/notes/gc-trigger-disabled.md) — and is the same set on
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

**Why the message changed.** The body closes over `held`, a live buffer. The
checker's lambda-value deep pass (`checker.rs:537-539`) already decided the body's
verdict; the *capture's* value only arrives when the apply's parameter check merges
it into the call's class, i.e. **after** the verdict the graph lowering reads. The
gate then hands the lowering a decided node whose value is the one the merge wrote,
and `place()` (`lichen-compute/src/compute/graph.rs:388`) no longer sees a
`ComputeValue::Buffer` to refuse by name — so the "would have to hold it … rather
than closing over it" sentence (`graph.rs:392-398`) never fires.

### 6.2 `graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one`

Same shape; the last consequential write is node **141v1** at epoch 1452, through
`apply_parameter_check` → `unify` → `add_equality` → `fill_class_holes`. Only the
count role is wrong in the program, and the count's own refusal sentence is what the
test asserts; it no longer fires (`graph_jit.rs:303`).

### 6.3 `graph_structure::a_body_dispatches_through_an_apply_whose_callee_holds_the_operator`

6 writes; the two that matter are the last pair, and they are **different sanctioned
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
last pair **312v1** (epoch 1589) and **313v1** (epoch 1594) from
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
   design ([stalled-rerun-epoch](docs/notes/stalled-rerun-epoch.md), status
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

**Second: the capture edge (the `graph_jit` pair).** Same mechanism, but the writer
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


