# The read-path collection was disabled — the ruling and the trace

> **Superseded.** The collection is enabled again with a repaired trigger: the read is
> shallow and the root's own slot decides, a pending assert's condition is a collect root,
> and the walk goes on into a node outside the vacated subtree. See
> [gc-trigger-design](gc-trigger-design.md) for the design and the readings, and
> [gc-block](gc-block.md) for the shipped contract. This note keeps the ruling and the trace
> that motivated it.

**The maintainer's ruling.** The engine-side trigger of garbage collection has a defect, so it is off until
the person who owns the collector repairs it: a scope read no longer compacts its scope away. The collector
itself — `Module::garbage_collect`, `Module::drop_block`, and the tests that call them directly — is
untouched; only the automatic call from the read path (`evaluation.rs`'s scope read) is gone.

## What was measured

`crates/lichen-compute`'s `examples::every_example_runs_and_prints_what_it_declares` panicked with
**`invalid SlotMap key used`** at `evaluation.rs`'s deep-pass entry (`self.nodes[node].visiting`), and the
trap with `LICHEN_GC_TRACE` set shows the panic in the frame **immediately after the only collection of the
run**:

```
DEEP enter node=NodeId(454v1) alive=true
DEEP enter node=NodeId(455v1) alive=true
DEEP enter node=NodeId(455v1) alive=true
GC drop_block block=BlockId(12v1) nodes=2
thread '…' panicked at crates/lichen-lowlevel/src/evaluation.rs:517:22:
invalid SlotMap key used
```

Two facts make the shape unambiguous:

- **no entry of a dead node was ever seen**: across 65,262 `DEEP enter` lines the check
  `DEEP enter … alive=false` counted **zero**, so the freed key is not a node the pass walked to — it is
  **the running frame's own node**, freed while that frame was still on the stack;
- the collection freed a block of **two nodes**, i.e. the very scope the descent was reading, because the
  trigger collects the root's own block right after deep-evaluating it.

So the walk's liveness does not include the descent that is in flight through it. Making the *readers*
lenient is not a fix: adding a "collected node reads as nothing to compute" guard at the deep-pass entry
and an "undecided" guard in `ref_is_undecided` only moved the panic from `evaluation.rs`'s entry to
`evaluation.rs`'s `ref_is_undecided` and then to `disjoint::find` (`lichen-utils`), with the failing set
unchanged at 203 passed / 15 failed / 2 ignored. Those guards were reverted.

## What re-enabling needs

Either the in-flight descent frames (`visiting`) are roots for the walk, or the read path stops collecting
at all and a host collects explicitly. The measurement above is the acceptance test: with collection back
on, `examples::every_example_runs_and_prints_what_it_declares` must not panic, and the compute suite's
failing set must not grow.

## The cost of the disable, measured

`lichen-lowlevel`'s own suite goes from **160 passed / 0 failed** to **147 passed / 13 failed**, and the
thirteen are the compaction contract, one test per claim: `compaction::block_run_pulls_outer_and_sibling_blocks`,
`compaction::call_clones_are_compacted_with_the_calling_block`, `compaction::redundant_nodes_are_not_compacted`,
`compaction::u128_payload_is_relocated_into_parent_and_block_releasable`,
`compaction::unreferenced_child_blocks_are_released`,
`compaction::deep_block_chain_evaluates_stack_safely`,
`compaction::deep_never_run_block_chain_releases_stack_safely`,
`assert::gc_moves_an_assert_condition_with_its_function`, `assert::gc_prunes_asserts_of_dropped_blocks`,
`function::function_in_local_block_survives_compaction`, `function::function_scope_is_dropped_with_its_block`,
`function::mixed_blocks_and_functions_survive_compaction`,
`table::gc_compaction_moves_table_payloads_and_entries`, and `lichen-language`'s `gc_block::a_gc_block_releases_its_scope` (that suite goes from 9 to 10 failures; the other nine are pre-existing reds).

They are red because those tests read a scope **from inside a descent** and require it compacted — which is
exactly the case that frees the running frame's block. Narrowing the disable to "no collect while
`deep_depth > 0`" was measured and does **not** help: the same thirteen fail, so the contract these tests
state and the repair the panic needs cannot be reconciled from the read path. `crates/lichen-compute` is
unchanged at 203 passed / 15 failed / 2 ignored, and the engine's `invalid SlotMap key used` panic is gone
from its output.
