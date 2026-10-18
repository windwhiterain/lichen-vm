# The `@gc` trigger: what decides a scope is done

> Status: **landed — the trigger is the read of a scope root, evaluated shallowly.** The
> maintainer's question was whether an **in-block undecided reference count** could trigger
> the collect instead: **no as stated** (§3), because laziness keeps the count permanently
> nonzero for every scope that contains an application, and the root's own **slot** answers
> the same question without forcing anything (§4). What blocked the trigger was never the
> signal but the collector's edges (§5, §6), and what the maintainer decided is §7. One
> collector item is still open (§8).

## 1. What the trigger must decide

`Module::garbage_collect(root)` moves everything **reachable from `root`** into the parent
block — equality-class members, value items, the operand of an operator that still owes an
answer, a reachable function's template and asserts — and then drops the block, releasing
the rest. So the trigger's one question is *"may this scope's unreachable interior go?"*,
and the answer has three parts:

1. **the scope has decided its value** — a later read of the root must not need a released
   node;
2. **no live node outside the block names a released one** — the walk is outgoing-only, so
   an outside reference is invisible to it;
3. **no evaluation frame holds a released node id** — a frame that is on the stack keeps
   using its `NodeId` after the collect.

The retired design answered (1) with `evaluated_deep(root).undecided == false`, i.e. with a
**deep pass**, and that pass is what forces: `evaluate_node_deep` evaluates every position
of the root's value graph. In the checker's definition pass most of those positions are
forced anyway (it deep-evaluates every statement and every body), so the measured defect is
not the extra work but §6: **the old trigger releases a pending assert's condition and
silently unregisters the assert**. (2) and (3) were never answered — the read-path disable
([gc-trigger-disabled](gc-trigger-disabled.md)) is (3) reaching a running frame, and §5
names (2) in the wild.

## 2. The shape of a scope, measured

Every construct the checker compiles a `@gc` with, and what the block ends up holding
(measured with a probe that was removed once the investigation closed, `src` from the
language's own tests):

| source | block nodes | undecided after the run | root decided |
|---|---|---|---|
| `x = @gc { a = 2; b = 3; a + b }` | 29 | 0 | deep: decided |
| `x = @gc f 41` | 25 | **3** | deep: decided |
| `x = @gc [1, 2, 3]` | 14 | 0 | deep: decided |
| `x = @gc { a = [1, 2, 3]; 2 }` | 34 | 0 (a value node is decided at allocation) | deep: decided |
| `x = @gc { y = f 0; 7 }` | 31 | **7** (a call the tail never reads) | deep: decided |
| `f = n => @gc { a = n + 1; b = a * 2; b }` (template) | 33 | 6 | deep: undecided |
| the same scope's per-call blocks | 13 | 3 | never deep-passed |
| `@gc { let a = _; a }` | 3 | 2 | deep: undecided |

## 3. The in-block undecided count, measured — it never reaches zero

A block whose count of nodes with no decided value hits zero is what "undecided reference
counting" proposes. The right-hand column of the table above is that count, and it does not
go to zero for **any scope that contains an application**:

- `@gc f 41` keeps three undecided nodes forever: the checker's `undecided_callee` cell, the
  **type-apply** node built over it (`check_app` allocates both per application and nothing
  ever binds the cell), and a clone of the return pair. They are undecided *by
  construction*, not by laziness.
- a scope holding a statement its tail never reads keeps that statement's nodes undecided
  forever when the statement is an operation — measured 7 for `{ y = f 0; 7 }`. The call is
  never run, so its nodes never decide. (A statement that is a plain value node is decided
  at allocation, which is why `{ a = [1, 2, 3]; 2 }` reads 0.) That is **laziness, not
  incompleteness**: those nodes are unreachable, which is exactly the garbage a scope wants
  to free.

So the count answers "has every node of the block a value?", which lazy evaluation makes
*false by design* for every interesting scope; the collect's question is "is the scope's
**answer** known?", which is a different question and is observable without forcing.

The count would also not have caught the defect that made the old trigger panic: the node
the descent died on is a **decided** array node (§5), not an undecided one.

## 4. The signal that works: the root's own slot

`evaluate_block` is only entered when the parent **read** the scope root. A plain
`evaluate_node(root)` then gives the scope's answer, and the answer is the signal:
`root.value` holds a decided value that is not the wildcard. No deep pass, so nothing is
forced and no side effect runs. Measured, with the trigger reading that:

| suite | before (no automatic collect) | deep trigger (retired) | landed (shallow read) |
|---|---|---|---|
| `lichen-language` `language` | 261 / 1 | — | **262 / 0** |
| `lichen-lowlevel` `basic` | 143 / 13 | 155 / 1 | **156 / 0** |
| `lichen-highlevel` `highlevel` | 136 / 0 | — | 136 / 0 |
| `lichen-compute` `examples::every_example…` | passes | **panics** | passes |
| `gc_block::*` | 6 / 1 | 7 / 0 | 7 / 0 |

The deep column was measured before §6's assert root was in place; with it, the retired
trigger reports the false assert too (154 / 2), because that test states the old contract.
The landed column's three suites are green because the three tests whose contracts this
change re-states were updated with it: `call_clones_are_compacted_with_the_calling_block`
(the deep verdict cache used to answer for a scope-root read, so the read never delegated —
`evaluate_node_deep_inner` now asks `is_scope_root_read` first),
`a_block_root_the_budget_refuses_stays_undecided` (the shallow read means the refused
operand leaves the root undecided, not `Error`), and
`gc_moves_a_pending_assert_condition_out_of_a_dropped_block` (D19).

## 5. What the old trigger's panic actually is — the collector's defect, named

The compute examples run under the deep trigger reproduced the disable note's panic. With a
research-only trace naming each collect (removed with the scaffolding, `LICHEN_GC_DEBUG`):

```
GC DEBUG mode=deep collect block=BlockId(12v1) root=NodeId(463v1) blocks=15 nodes=698
GC DEBUG released NodeId(464v1) [value=array[4v1, 11v1] operation=None visiting=false block=BlockId(12v1)]
        referenced by [(NodeId(462v1), BlockId(11v1), "array[463v1, 464v1]", ["array item"])]
DEEP DEAD NODE NodeId(464v1) visiting-frame depth=6
```

The released node is **decided and not in flight**; it is named as an array item by a live
node of the **parent** block (11v1). `462v1`'s array holds both the scope root (`463v1`)
and its sibling (`464v1`), so the parent names a node the walk does not keep. The walk does
reach `462v1` (its class edge from the root), sees a node outside the vacated subtree, and
**returns without walking or relocating its value**: the array keeps naming `464v1`, and the
next deep frame iterating it dies on the released id. How the parent cell came to hold a
child-block array is not established by this trace — a class write (`fill_class_holes`
copies a class's value into every member) is the likely path — and it does not matter: the
hole is that a node outside the block can name a node inside it and the walk never looks.

That is the same hole in the other direction: the walk relocates the values of nodes it
**moves**, but a value that reached a node **outside** the block by a class write is a
reference the walk neither follows into nor rewrites. Fixing it is the collector owner's
job, and it is a prerequisite of *any* trigger: the trigger's signal was never the reason
the read path was unsafe. Two shapes of repair:

- **walk the outside member's value**: its items are roots (so `464v1` is kept) and its
  payload is re-allocated into the target arena (so the handle stays live) — the same two
  steps the inside case takes;
- or **forbid the case**: a class may not hold a node outside the block whose value names a
  node inside it — a checker-level invariant, and then the walk needs no change.

## 6. The second defect: a released scope loses its asserts

The collector's contract today is that a pending assert's condition **dies with its block**
(`assert::gc_prunes_asserts_of_dropped_blocks`). Measured consequence: with the collect
disabled, `x = @gc { @assert (1 > 2); 3 }; x` reports `ok=false / assert_errors=1`; **under
the old deep trigger the same program reports no error at all** — the condition is released
before the assert pass drains, and `drop_block` prunes its entry. `@gc` is documented as an
allocation marker that "computes nothing", so a release that changes a program's verdict is
a semantic hole.

The shallow trigger keeps the assert **only because** the condition is still undecided there
and therefore kept by an owed edge — luck, not a rule: as soon as anything reads the
condition before the collect, the same loss happens. Treating pending assert conditions as
collect roots fixes it (measured: the deep trigger's false assert is reported again), at the
cost of the lowlevel test that states the opposite contract.

## 7. What landed

The maintainer ruled on the two policy questions — assert conditions are roots (`1.a`) and a
template's scope may be collected (`2.b`) — and the trigger is **on**:

| piece | where |
|---|---|
| the trigger: shallow read, root's slot decides, marker kept while undecided | `evaluation.rs` (`evaluate_block`, `is_scope_root_read`) |
| the verdict cache does not answer for a scope-root read | `evaluate_node_deep_inner` |
| a pending assert's condition is a collect root | `gc.rs` (`garbage_collect`) |
| the walk goes into a node outside the vacated subtree, and a class member's payload moves into its own block | `gc.rs` (`garbage_collect_node`, `relocate_class_value`) |
| the new contract tests | `assert::gc_moves_a_pending_assert_condition_out_of_a_dropped_block`, `evaluation::a_block_root_the_budget_refuses_stays_undecided`, `basic::gc_safety` (the safety battery), `gc_block::a_closure_*_is_still_callable` |

Measured on the merged change: `lichen-language` **264 / 0**, `lichen-lowlevel` **162 / 0**
plus one ignored repro (`P1-52`), `lichen-highlevel` **136 / 0**, `lichen-compute`'s
`examples::every_example_runs_and_prints_what_it_declares` passes (the run that panicked
under the old trigger). The corpus' two collects released 23 and 8 nodes, and no live node
named a released one through its operand, array or table edge; after the function-template
fix (§8) neither leaves a dangling function node.

The template decision (`2.b`) is measured too: `f = n => @gc { a = 1; a }` leaves 7 blocks /
5 declared scopes after compile and 2 / 0 after the run — a definition-pass read decides the
template's scope, so it is released there and the calls have none of their own. Skipping a
block whose parent is a function's template body was the alternative; it would keep a scope
alive for the module's lifetime to serve calls that may never come.

## 8. The collector items around the release

**Fixed — a surviving function's template list named released nodes** (`P1-51`). Measured
before the fix on the example corpus: `FunctionId(3v1)`, homed in block `4v1`, listed **23
nodes of the released block `5v1`**. The cause is that a function's home and its nodes
diverge (a per-call closure is homed at the call site while its nodes land in the call's
fresh scope block). A node's owner records which function it belongs to, so the collect now
keeps every node of a function whose home outlives the block — `owned_by_surviving_function`
walks the block subtree and re-walks those nodes, which moves them into the target. After the
fix the corpus' two collects leave **no** dangling function node, and the closure-escape
programs are unit tests (`gc_block::a_closure_released_from_a_scope_is_still_callable`).

**Open — a parent-block payload naming a non-root scope node** (`P1-52`). The block boundary
says only a scope's root may be named from outside, and nothing enforces it: a parent-block
array can name a sibling inside the scope, and the walk cannot see that reference from its
root, so the release leaves a live payload naming a dead id (the retired deep trigger panicked
on exactly this). It is a decision, not a patch: the candidate repairs are the block-boundary
discipline at the checker (the shape is generated there, in `check_gc`'s pair), a per-block
incoming-reference count, or a parent-block scan at the collect — the last two cost what this
collector is built to avoid. The repro is `#[ignore]`d in `gc_safety.rs`.

## 9. What the sources do not answer

- Whether a lazy statement inside a scope should be freed without ever running: with a
  decided root the walk keeps whatever the value reaches, and a statement the value does not
  reach is freed unrun — the same outcome the program has without `@gc`, so it looks right,
  but it is a semantic statement worth stating in the spec.
