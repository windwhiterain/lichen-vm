# The `@gc` trigger: what decides a scope is done

> Status: **research — measured, not merged.** The maintainer's question: the retired
> read path triggered the collect by deep-evaluating the scope root, which forces work and
> so has side effects; could an **in-block undecided reference count** trigger it instead?
> Answer: **no as stated** (§3), and the forcing is not needed at all — a plain read of the
> root answers the same question without running anything (§4). What blocks the trigger is
> not the signal but **two collector defects** (§5, §6).

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
of the root's value graph. In the checker's definition pass the same positions are forced
anyway (it deep-evaluates every statement and every body), so the measurable defect is not
the extra work but §6: **the deep pass decides the scope's assert conditions, and the collect
that follows then unregisters them**. (2) and (3) were never answered — the read-path
disable ([gc-trigger-disabled](gc-trigger-disabled.md)) is (3) reaching a running frame, and
§5 names (2) in the wild.

## 2. The shape of a scope, measured

Every construct the checker compiles a `@gc` with, and what the block ends up holding
(probe `zz_gc_probe`, `src` from the language's own tests):

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

| suite | disabled (dev) | deep (old) | shallow (proposed) |
|---|---|---|---|
| `lichen-lowlevel` `basic` | 146 / 13 | 155 / 1 | 154 / 2 (153 / 3 with §6's assert fix) |
| `lichen-language` `language` | 271 / 1 | — | 262 / 1 (a research probe) |
| `lichen-highlevel` `highlevel` | 138 / 0 | — | 136 / 0 (the probe adds none here) |
| `lichen-compute` `examples::every_example…` | passes | **panics** | passes |
| `gc_block::*` | 6 / 1 | 7 / 0 | 7 / 0 |

The shallow reds and the deep reds are the same tests seen from two designs:
`compaction::call_clones_are_compacted_with_the_calling_block` fails under **both** (its
call is already deep-cached, so the read path never delegates — a fact of the deep pass's
verdict cache, not of the trigger), `evaluation::a_block_root_the_budget_refuses_yields_an_empty_value`
encodes the deep design's own depth accounting (its premise disappears without the forcing),
and `assert::gc_prunes_asserts_of_dropped_blocks` fails under either design **once §6's fix
is in**, because that test *is* the "asserts die with their scope" contract. All three are
contracts to re-state with the maintainer, not regressions of the proposed trigger.

## 5. What the old trigger's panic actually is — the collector's defect, named

The compute examples run under the deep trigger reproduced the disable note's panic. With
`LICHEN_GC_DEBUG` naming the collect:

```
GC DEBUG mode=deep collect block=BlockId(12v1) root=NodeId(463v1) blocks=15 nodes=698
GC DEBUG released NodeId(464v1) [value=array[4v1, 11v1] operation=None visiting=false block=BlockId(12v1)]
        referenced by [(NodeId(462v1), BlockId(11v1), "array[463v1, 464v1]", ["array item"])]
DEEP DEAD NODE NodeId(464v1) visiting-frame depth=6
```

The released node is **decided and not in flight**; it is named as an array item by a live
node of the **parent** block (11v1). `462v1`'s array holds both the scope root (`463v1`)
and its sibling (`464v1`) — it is the scope expression's `[term, value]` pair, whose value
was propagated into a parent-block cell by a class write (`fill_class_holes` /
`propagate_class_value`), and the payload lives in the child block's arena. The collector
walks the class edge to `462v1`, sees a node outside the vacated subtree, and **returns
without relocating its value**: the array still names `464v1`, which the walk then releases
(the root's class reached only `463v1`). The next deep frame iterating that array dies on
the released id.

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
(`assert::gc_prunes_asserts_of_dropped_blocks`). Measured consequence: under the old deep
trigger, `x = @gc { @assert (1 > 2); 3 }; x` reports **no** error — the failing assert is
silently unregistered, while the same program with the collect disabled reports
`ok=false / assert_errors=1`. The mechanism is the deep pass: it decides the condition, the
collect's walk no longer reaches it (a decided node's operand edge is dead), and
`drop_block` prunes the entry before `check_asserts` runs. `@gc` is documented as an
allocation marker that "computes nothing", so a release that changes a program's verdict is
a semantic hole.

The shallow trigger keeps the assert **only because** the condition is still undecided and
therefore kept as an owed operand — luck, not a rule: as soon as anything reads the
condition before the collect, the same loss happens. Treating pending assert conditions as
collect roots fixes it (measured: the deep trigger's false assert is reported again), at the
cost of the lowlevel test that states the opposite contract.

## 7. Recommendation

1. **No refcount.** The observable the collect needs is the scope's answer (its root slot),
   not a count of the block's unresolved nodes; laziness guarantees the count stays nonzero.
2. **Collect on the read, from a shallow evaluation.** It is the same point the retired
   design collected at, but it forces nothing; measured above.
3. **Repair the collector first**: the class-carried value of §5 and the assert policy of
   §6. Either defect alone makes an enabled trigger unsound or semantically visible.
4. **Decide the template question.** A `@gc` inside a body whose value does not depend on
   the parameters decides during the definition pass, and a trigger on the read then
   collects the *template's* scope block — measured: `f = n => @gc { a = 1; a }` leaves 7
   blocks / 5 declared scopes before the run and 2 / 0 after, so the per-instantiation
   scopes are gone. [gc-block](gc-block.md) states the template is never collected; the
   rule needs to be implemented (skip a block whose parent is a function's template body)
   rather than assumed.

## 8. What the sources do not answer

- Whether an assert inside a `@gc` scope *should* die with it: the lowlevel test says yes,
  the marker's "computes nothing" says no. The maintainer owns the verdict.
- Whether a lazy statement inside a scope should be freed without ever running: with a
  decided root the walk keeps whatever the value reaches, and a statement the value does not
  reach is freed unrun — the same outcome the program has without `@gc`, so it looks right,
  but it is a semantic statement worth stating in the spec.
