# A stalled operator re-runs only when a value changed

> Status: **current.** The epoch gate is landed on `feature/stall-epoch-gate`
> (§5): a node whose operator ran and could not decide records the value epoch it
> ran at, and a read of it while the epoch has not moved answers `None` without
> re-descending. §2 is the measurement the decision rests on. §3 records why the
> order of evaluation and unification is **not** the fix, and §6 the two
> dependency-edge designs that were considered and rejected.
>
> Points at: `crates/lichen-lowlevel/src/evaluation.rs` (`evaluate_node_body`'s
> run gate, `evaluate_node_operation`'s stall postlude, `ran_undecided`),
> `crates/lichen-lowlevel/src/equality.rs` (`set_node_value` and its three
> callers), `crates/lichen-lowlevel/src/module.rs` (`value_epoch`,
> `stamp_node_ran`, `run_stamp_is_current`), `crates/lichen-lowlevel/src/function.rs`
> and `static_module/apply.rs` (the clone walks), `static_module/freeze.rs` and
> `crates/lichen-language/src/persist/container.rs` (the static mirror).
>
> Companions: [lowlevel-vm](lowlevel-vm.md) (the evaluation and write rules this
> changes the read side of), [eval-before-unify](eval-before-unify.md) (the
> staleness this answers, and the phase-ordering question §3 closes),
> [incremental-evaluation](incremental-evaluation.md) (the deep pass's own
> redundancy, which is the larger share and is *not* this note's),
> [class-channel](class-channel.md) (the run axis this extends).

## 1. The question

An operation node whose operator runs and cannot decide caches **no** answer:
`Node::value` stays empty so a later read may try again, because the value that
blocked it may arrive from a unification that has not happened yet. The run axis
is what keeps "ran, undecided" apart from "never ran", and the read side of
`evaluate_node` re-runs the operator whenever the slot is empty.

So a read of an undecided node re-descends it — and with it every operand the
operator reads — every time somebody asks, even when **nothing changed since the
last attempt**. The question this note answers: is that re-descent removable, and
is reordering evaluation against unification the cheaper way to do it?

## 2. The measurement

Temporary counters on `dev@b605dee` (a scratch worktree, since deleted), one
`run_directory` sweep over `examples/` — 25 programs. `stall_again` counts a
node that stalls more than once; `repeat_stale` counts a re-attempt of a stalled
node **after which no value slot had changed** — the waste a gate can remove.

| program | op attempts | stalls | stalled nodes | stalled again | re-attempts | of them stale | deep revisits |
|---|---|---|---|---|---|---|---|
| `array` | 163 | 98 | 58 | 40 | 40 | **9** | 632 / 938 |
| `assert_in_function` | 200 | 112 | 63 | 49 | 49 | **11** | 625 / 953 |
| `compute_jit` | 542 | 207 | 139 | 68 | 65 | **16** | 1957 / 2824 |
| `mutual_recursion` | 313 | 118 | 65 | 53 | 52 | **13** | 730 / 1178 |
| `recursion` (`fib 10`) | 6545 | 117 | 64 | 53 | 52 | **12** | 2315 / 7492 |
| `struct_recursion` | 165 | 98 | 58 | 40 | 40 | **9** | 1583 / 1926 |
| `operators` | 249 | 98 | 58 | 40 | 40 | **9** | 904 / 1335 |

Two readings:

- **The stall-and-retry shape is real but small.** Roughly 60% of operation
  attempts stall (the open-class `Index`/`Apply` `?a` reads dominate), a quarter
  of all attempts are re-attempts of an already-stalled node, and **a quarter of
  *those* — ~5% of all attempts — are stale**: the same question asked of an
  unchanged graph.
- **The deep pass is the larger redundancy**, and it is not this note's:
  `deep_revisit` is 31–82% of the deep walk's node visits, and each entry point
  re-descends its whole reachable graph. That is
  [incremental-evaluation](incremental-evaluation.md) §4.1's `settled` cut.

The measured take of the gate (§5) matches the stale column exactly: 310 of
11 687 operation attempts across the sweep, i.e. **2.7%**, and it removes the
descent each one triggered (in `array` a blocking read is worth ~7 further node
descents, so the descent saving is several times the attempt saving).

## 3. Reordering evaluation and unification is not the fix

Three forms of "adjust the order", and why each fails:

- **Separate the phases entirely** (unify the whole graph, then evaluate). The
  checker decides as it builds: `check_binop` asks a class question, `slot_read`
  reads a type cell, `check_named_field` judges a container's kind, and
  `lichen-compute` forces a template's parameter type. Deferring every one of
  them to run time means checks that need a source span and a tier the runtime
  cannot express ([eval-before-unify](eval-before-unify.md) §3.2 is the site
  table, §3.4 why recorded decisions are permanent).
- **A second evaluation pass after unification.** A node stalls on a class that
  holds nothing; re-asking later is exactly what the runtime already does through
  the class channel. The blocker is not the *number* of passes but that a write
  between two reads is not observable to the reader — which is the invalidation
  signal this note's gate supplies.
- **Force more eagerly at check time** (let a read push a value into the cell it
  waits on). That trades a re-run for the laziness the language is built on, and
  turns a stall into non-termination at the apply.

So the ordering question is closed: the fix has to be a **memoization on the
undecided node**, not a schedule. `eval-before-unify` §6.4 records the same
answer from the checker side.

## 4. What the gate needs, and what it cannot be

A stalled node has to know whether the graph it read is still the graph it would
read. The cheap form of that is a **version counter**, not a dependency edge:
every value write moves a module-wide epoch, and the node records the epoch it
ran at.

Two facts make the epoch sufficient, and both are already invariants of the
graph:

- **The operator's inputs are fixed.** An operation's operand is written once
  ([`Module::close_operation_cycle`] is the only late edge, and it clears the
  deep verdict), and the operator reads the module only through
  `evaluate_node`/`static_read`/`key_state`, all of which answer from node slots.
- **Every value change goes through one slot write.** `Node::value` is assigned
  in exactly three places — `write_node_value`, `propagate_class_value`,
  `fill_class_holes` — and all three now call `set_node_value`, which bumps the
  epoch. A merge that carries a value (`add_equality`), a class write, and a
  computed answer therefore all move the clock.

And one thing the stamp must **not** be: a `bool`. "Ran and stalled" and "ran and
answered" are the same bit but different futures — the second must never re-run
(`write_node_answer`'s reconcile is what makes the class's asserted value and the
operator's produced value meet), while the first must re-run as soon as the epoch
moves. The stamp is `0 = never ran`, `e > 0 = ran at epoch e`, and the gate reads
it only in the empty-slot branch; the cached-value branch keeps its own test
(`has_no_result_yet && !visiting`), which the epoch must not gate.

Stated as an invariant, the gate is sound by construction: the operator is a
function of the operand graph and the class values, and no class value changed
since the attempt that stamped the epoch — so the attempt would decide exactly
what it decided then, record exactly the same (no) errors and spend no budget.

**Falsifications to keep checking.** A stall records nothing (the error paths are
all on decided values), so skipping one cannot hide a diagnostic; an attempt does
not spend `apply_total` (only `function_apply` does), so skipping one cannot hide
a `NonTerminating`; a saturated epoch only reads as *stale*, which re-runs an
operator rather than skipping one.

## 5. The landed form

- `Node::runned: bool` became the run stamp `runned: u32`; the field is the same
  size, so nothing grew. `Module::value_epoch` starts at **1** (zero is the
  stamp's "never ran") and saturates.
- `Module::has_no_result_yet` reads the stamp's zero test — unchanged in meaning.
- `evaluate_node_body`'s empty-slot branch gained one test,
  `run_stamp_is_current`: stamp non-zero and equal to the epoch ⇒ `None`, before
  the visit guard is taken.
- `evaluate_node_operation`'s stall postlude and `ran_undecided` stamp at the
  current epoch. `TableGet`'s undecided-key arm deliberately does **not** stamp:
  the lookup never happened, so the next read must try again.
- The clone walks keep their expression verbatim
  (`mapped.is_some_and(|v| !answer_elements_are_undecided(v))`) through
  `stamp_node_ran_if`, so a clone of an operator's answer still owes nothing and a
  clone of a stalled template still re-runs against the call's argument.
- The freeze mirror stays a `bool` (`node.runned != 0`): an artifact carries no
  clock to compare a stamp against. The codec is unchanged.

Measured after landing (same sweep, a temporary probe that only counts): the
gate fires 9–24 times per program, **310 operation attempts across the corpus are
skipped** — byte-identical output for all 25 programs, `lichen-lowlevel` 156/156,
`lichen-highlevel` all targets, `lichen-language` green.

## 6. The dependency-edge forms, and why they were rejected

- **A per-node dependency list** (the edges that would let a write wake exactly
  its readers). The graph already holds the dependency: an operator reaches its
  inputs through its operand array and every node is referenced only from its own
  block, so the reads are re-derivable at the moment of the stall. Recording them
  costs a variable-length list per stalled node (this was the superior's
  "compact storage" idea), and it buys precision the epoch does not have: the
  epoch re-opens a stall when **any** value changed. Measured, that precision is
  worth ~5% of attempts, so the list is not earned; and the signal it must carry
  is a *write* anyway (a merge rewrites an array's items and can re-point a cell
  without a structural change), which is what the epoch counts.
- **A blocked list on the class** — the operations that stalled on it, drained
  when it commits ([eval-before-unify](eval-before-unify.md) §5.1's sketch). It
  is the precise form, and it carries that note's three blockers: a drain runs
  evaluation inside a bind that may itself be inside a unify (the visit-mark
  invariant and the depth budget both apply), a waiter nobody else references is
  what "undecided forever" currently means (draining would resurrect semantics by
  keeping dead nodes alive), and a template's parameters never bind, so its list
  must never fire. At a ~5% take, none of that is justified.

## 7. What this note does not fix

The re-descent **between** attempts. A read still walks the path from the entry
point to the stalled node, and every node on it answers from its own slot; that
path is O(depth) per read regardless of the gate. Removing the per-entry
re-descent of already-decided subgraphs is
[incremental-evaluation](incremental-evaluation.md) §4's `settled` bit, whose
measured headroom (34–72% of deep-pass visits) is an order of magnitude larger
than this note's. The gate is orthogonal to it, and both can land.
