# A stalled operator re-runs only when a value changed

> Status: **retired**, measured. The gate §5 landed is superseded by the decision
> that a node is evaluated and decided exactly once: the run stamp is read as "this
> operator has run", and the epoch it recorded is compared with nothing. **§8**
> records the retirement, what it costs, and who owns the three tests that pin it.
> §1–§7 are kept as the record of the design as it stood — §2 the measurement
> behind it, §3 why the order of evaluation and unification is not the fix, §5 the
> landed form, §6 the two dependency-edge designs that were rejected. The ordering
> conclusion of §3 survives the retirement; the memoization it names does not.

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

## 8. Retired: the stamp no longer compares against the epoch

**Status: retired, not landed.** [incremental-evaluation](incremental-evaluation.md)
§5's step 1 puts the other half of this note's question: `evaluate` and
`evaluate_deep` gate on their stamps and re-decide nothing — neither a concrete
verdict nor an `undecided` one. That decides §5's run gate against its own form.
The stamp is read as "this operator has run", and the epoch it recorded is compared
with nothing.

### 8.1 The two gates, and where each sits

Branch `feature/undecided-final`, worktree `.worktrees/ufin`, base `dev@71adaa1a`,
private target, `-j 1`, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`.

| gate | site | before | after |
|---|---|---|---|
| deep | `crates/lichen-lowlevel/src/evaluation.rs`, `evaluate_node_deep_inner` | cut a **concrete** verdict only (`is_some_and(\|deep\| !deep.undecided)`) | cut **any** verdict (`evaluated_deep.is_some()`) |
| run | `crates/lichen-lowlevel/src/evaluation.rs`, the empty-slot read, through `Module::run_stamp_is_set` (was `run_stamp_is_current`, `module.rs:358-362`) | stamp non-zero **and** equal to the epoch | stamp non-zero |

### 8.2 The cost, and the attribution between the two halves

Probe-free runs, every number from a run with no instrumentation in the crate, name
sets compared with `Compare-Object`, never counts. The `dev` row and every arm ran
in the same worktree, so the artifact tree differs by exactly the one file.

| arm | `lichen-lowlevel` | `lichen-highlevel` | `lichen-compute` | new failures by name |
|---|---|---|---|---|
| `dev@71adaa1a` | 147 / 13 | 140 / 0 | 204 / 14 / 2 | — |
| deep gate plain alone | 145 / 15 | 140 / 0 | 200 / 18 / 2 | 2 lowlevel, **4** compute |
| run gate plain alone | 144 / 16 | 140 / 0 | 139 / 79 / 2 | 3 lowlevel, **65** compute |
| both | 144 / 16 | 140 / 0 | 139 / 79 / 2 | 3 lowlevel, 65 compute |

The arms are the finding. **The deep half costs four `lichen-compute` tests and the
run half costs sixty-five**, and the two together cost exactly what the run half
costs alone: the failure name sets under the run gate alone and under both gates are
identical, name for name. Two of the deep gate's four do not fail at all once the
run gate is also plain, and the other two are among the sixty-five — so the deep cut
adds nothing on top of the run gate's, and this note's own gate is what carries the
delta.

That corrects [late-writes-map](late-writes-map.md) §4.1 and §4.4 by omission.
§4.1 recorded the **run** gate's row for `lichen-lowlevel` ("the same three, by
name") and §4.4 recorded the **deep** gate's row for `lichen-compute` (four new) —
so the run gate was never run over `lichen-compute`, and the price of this note's
own gate was never measured. The deep gate alone reproduces §4.4 exactly
(200 / 18 / 2, the same four names), which is why the base changes since then moved
nothing on that arm: they did not touch the run axis.

**§3's closing sentence named this note's own gate** — "the fix has to be a
memoization on the undecided node, not a schedule". The *ordering* half of §3 stands
and is unaffected: reordering evaluation against unification is still not the fix,
and that measurement is untouched. What is retired is the memoization it answered
with.

### 8.3 What the sixty-five are

One refusal, everywhere. `compute.graph` records a function by applying it to its
own placeholder arguments and reading what came back; under the plain run gate the
operators that stalled during that first pass never run again, so the body's return
is not resolvable to a value-table entry and the recording reports *"this
function's return is not a value a graph can hand back, so there is no graph to
record"*.

| how it surfaces | count |
|---|---|
| the program no longer checks — the checker's own `report.ok()`, or a `compile(...).expect("the chain compiles")` on the same source | **57** |
| the test asserts a graph **was recorded**, and none is | 2 |
| the test asserts the wording of a refusal, and gets this sentence instead | 6 |

Fifty-seven of the sixty-five are a **checked outcome changing**: a kernel that
merged, tiled, recorded and answered on the device no longer builds, and the single
diagnostic in all fifty-seven is that one sentence. The six refusals keep their
outcome — the program is still refused — and lose their cause, so the rule each of
them is about is no longer the rule that fires. §13 of
[late-writes-map](late-writes-map.md) holds the per-test classification.

### 8.4 The epoch is now unread

After this change nothing in `lichen-lowlevel` reads `Module::value_epoch`. What
remains is: `bump_value_epoch`'s own write, `stamp_node_ran`'s read of it to stamp
with, and the public accessor `Module::value_epoch()` — whose only reader in the
tree is `crates/lichen-lowlevel/tests/basic/evaluation.rs:417`, one of the three
tests §8.5 lists. `Node::runned` is read only for its zero test (`!= 0`), so the
`u32` is a `bool` with room to spare, and `value_epoch`, `bump_value_epoch` and
`value_epoch()` are dead state once that test goes.

**Nothing was removed.** The brief for this step says to report the dead machinery
rather than delete it, and deleting it would require editing a test.

### 8.5 The three tests that pin this note, and their proposed disposition

They are red and they stay red: no test was edited. All three are in
`crates/lichen-lowlevel/tests/basic/`.

| test | assertion | observed | what it pins | proposed |
|---|---|---|---|---|
| `evaluation::a_stalled_operator_reruns_only_after_a_value_changed` (`evaluation.rs:433-436`) | `assert_eq!(u128_of(m.evaluate_node(add, None).unwrap()), 5)` | panics at `:434` — `unwrap()` on `None` | a stalled operator re-deciding after a value changed | **retire** — it is this note's own test of the retired gate, and §8.2 measures what the gate was worth (2.7 % of operation attempts) |
| `assert::an_untriggered_assert_is_decided_by_a_later_drain` (`assert.rs:242`) | `assert!(m.asserts.is_empty(), "the pending entry got decided")` | still pending | a later drain deciding an entry a first drain could not | **rewrite** to assert what remains true: the first drain leaves the entry pending and records no error (`assert.rs:229-234`, which passes unchanged) |
| `function::function_call_operator_preserves_undecided_operand_chain` (`function.rs:145`) | `assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 99)` | panics at `:145` — `unwrap()` on `None` | an `undecided` verdict being revised after the argument unifies | **rewrite** to assert what remains true: the undecided argument stays lazy and the template body is untouched (`function.rs:126-134`, which passes unchanged) |

The first drain of the second and the head of the third already pass, so each
rewrite keeps the half that is still true and drops the re-decision. The first has
no such half — it is the retired design's own test.

### 8.6 What this leaves as the decision

The step does not land on this measurement. The sixty-five `lichen-compute`
failures are not design-pins: none of them asserts a re-decision, and each asserts
that the compiler accepts a program it now refuses. That is a checked/unchecked
outcome changing, which is
[incremental-evaluation](incremental-evaluation.md)'s `wrong-value` bucket and the
one its §5 step 1 says decides the question.

The remaining lever is the one §12.7 of [late-writes-map](late-writes-map.md) names
as a question: the 6 268 late writes are all at sanctioned sites, and the checker's
evaluate-before-unify order is a landed decision
([eval-before-unify](eval-before-unify.md) §2.1). Until that order is re-decided,
the plain run gate is not free — it is sixty-five programs that stop building.

