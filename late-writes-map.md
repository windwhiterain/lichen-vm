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

## 3. Baseline, changes absent (`dev@f360c72d`)

Measured in this worktree, private target, `-j 1`:

| suite | recorded | measured | agreement |
|---|---|---|---|
| `cargo test -p lichen-lowlevel --no-fail-fast` | 147 / 13 | **147 / 13** | exact |
| `cargo test -p lichen-highlevel` | 140 / 0 | **140 / 0** | exact |
| `cargo test -p lichen-compute --no-fail-fast` | 204 / 14 / 2 | *(see §6)* | |

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

