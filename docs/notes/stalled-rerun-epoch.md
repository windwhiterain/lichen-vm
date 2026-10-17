# A stalled operator re-runs only when a value changed

> Status: **retired, and landed.** The run stamp no longer carries the value epoch:
> `Module::run_stamp_is_set` reads `runned != 0`, so an operator that stalled has had its one
> attempt and a later value change does not re-open it. `Module::value_epoch`,
> `bump_value_epoch` and the public accessor are gone with the comparison, and the three
> tests that pinned them are retired or rewritten to the narrower true statement. §8 records
> what the retirement was taken over and what it costs: **three** `lichen-compute` tests, named
> in [late-writes-map](late-writes-map.md) §15, none of which is a recording failure. The
> ordering half of §3 still stands — reordering evaluation against unification is not the fix.
>
> Points at: `crates/lichen-lowlevel/src/evaluation.rs` (`evaluate_node_body`'s
> run gate, `evaluate_node_operation`'s stall postlude, `ran_undecided`),
> `crates/lichen-lowlevel/src/equality.rs` (`set_node_value` and its three
> callers), `crates/lichen-lowlevel/src/module.rs` (`stamp_node_ran`,
> `run_stamp_is_set`), `crates/lichen-lowlevel/src/function.rs`
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

## 4. What the gate needed, and what it could not be

> **§4 and §5 are the design as §8 retired it.** They are kept as the reasoning behind the
> decision, not as a description of the code: the stamp is no longer a version counter,
> and no epoch exists for it to be compared against.

A stalled node had to know whether the graph it read was still the graph it would
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

> **Historical — §8 retired this form.** What survives it is the stamp's width,
> `has_no_result_yet`'s zero test, the clone walks' expression and the `bool` freeze mirror;
> the epoch, the comparison and the stamping at the current epoch are gone, and
> `run_stamp_is_current` is `run_stamp_is_set`.

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

## 8. Retired: what it cost

**The retirement has been taken.** `Module::run_stamp_is_current` is now
`Module::run_stamp_is_set`: the stamp is read as "this operator has run", and the epoch it used
to record is compared with nothing. [incremental-evaluation](incremental-evaluation.md) §5's
step 1 asked for both halves — `evaluate` and `evaluate_deep` gate on their stamps and re-decide
nothing, neither a concrete verdict nor an `undecided` one. **The run half is what landed.**
The deep half did not: `evaluation.rs:552-559` still cuts a **concrete** verdict only, because
§8.2's arms measure it adding nothing on top of the run half.

### 8.1 The two gates, and which one was taken

Branch `feature/undecided-final`, worktree `.worktrees/ufin`, base `dev@71adaa1a`,
private target, `-j 1`, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`.

| gate | site | before | after | taken |
|---|---|---|---|---|
| deep | `crates/lichen-lowlevel/src/evaluation.rs`, `evaluate_node_deep_inner` | cut a **concrete** verdict only (`is_some_and(\|deep\| !deep.undecided)`) | cut **any** verdict (`evaluated_deep.is_some()`) | **no** — `evaluation.rs:552-559` is untouched, because the arms measure this half adding nothing on top of the run half |
| run | `crates/lichen-lowlevel/src/evaluation.rs`, the empty-slot read, through `Module::run_stamp_is_set` (was `run_stamp_is_current`, `module.rs:358-362`) | stamp non-zero **and** equal to the epoch | stamp non-zero | **yes** — this is the landed retirement |

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

### 8.3 What the sixty-five were

**Historical.** §9 removed the cause for sixty-two of them before the retirement was taken, so
what follows is the shape of the cost as it was measured, not the cost as it was paid. One
refusal, everywhere. `compute.graph` records a function by applying it to its
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

### 8.4 The epoch is removed

Once the comparison goes, nothing reads the module's value clock, so it is deleted rather than
left as dead state. Each item was verified to have no reader left after the change:

| item | site | why it went |
|---|---|---|
| `Module::value_epoch`, the field | `crates/lichen-lowlevel/src/lib.rs` | the clock the run stamps compared themselves against |
| `Module::bump_value_epoch` | `module.rs` | the field's only writer, and `set_node_value` its only caller |
| `Module::value_epoch()`, the accessor | `module.rs` | public reader; its only in-tree reader was `crates/lichen-lowlevel/tests/basic/evaluation.rs:417`, the test §8.5 retires |

`set_node_value` and `run_stamp_is_set` each keep an `# Invariant`, and each is **replaced**
rather than deleted: `set_node_value`'s was the epoch's ("the epoch moves exactly when some
node's value changes") and the part that survives is that it is the one value-slot assignment
every caller's contract is stated against; `run_stamp_is_current`'s was the retired gate's
soundness argument — "the epoch moves on every value write, so equality is proof that every
read this operator made would answer the same now" — and what it says now is the fact the gate
rests on, that the stamp is written the once an operator runs and cleared only by the walks
that re-open a stalled subtree.

`Node::runned` stays a `u32` and `stamp_node_ran` writes a non-zero constant, so every `!= 0`
reading is unchanged and the field is the size it has been since §5 widened the original `bool`.
The freeze mirror was **already** set semantics (`node.runned != 0`) and is untouched, as is the
codec in `crates/lichen-language/src/persist/container.rs`. `Module::node_run_stamp` stays as the
stamp's public reader; after §8.5 it has no in-tree caller, which is the state
`Module::value_epoch()` was in one step earlier and is why it was kept rather than deleted.

### 8.5 The three tests that pin this note, and their disposition as taken

All three are in `crates/lichen-lowlevel/tests/basic/`, and they are the only tests this
retirement touched. No `lichen-compute` test was edited.

| test | assertion that no longer holds | observed under the plain gate | disposition |
|---|---|---|---|---|
| `evaluation::a_stalled_operator_reruns_only_after_a_value_changed` (`evaluation.rs:433-436`) | `assert_eq!(u128_of(m.evaluate_node(add, None).unwrap()), 5)` | panics at `:434` — `unwrap()` on `None` | **retired**, deleted outright: it is this note's own test of the retired gate, and §8.2 measures what that gate was worth (2.7 % of operation attempts). Its first half was the *run* gate's pin and goes with the rest |
| `assert::an_untriggered_assert_is_decided_by_a_later_drain` (`assert.rs:242`) | `assert!(m.asserts.is_empty(), "the pending entry got decided")` | still pending | **rewritten**, and renamed `an_untriggered_assert_stays_pending_and_records_no_error` because the old name asserts what no longer holds. The first drain's assertions (`assert.rs:229-234`) are kept verbatim; the second drain now asserts the entry is *still* pending and still records no error |
| `function::function_call_operator_preserves_undecided_operand_chain` (`function.rs:145`) | `assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 99)` | panics at `:145` — `unwrap()` on `None` | **rewritten in place under the same name**, which already said what it asserts: the undecided argument stays lazy and the template body is untouched (`function.rs:126-134`, kept verbatim), and the re-decision is dropped |

The first drain of the second and the head of the third already passed, so each rewrite keeps
the half that is still true and drops the re-decision. The first has no such half: it is the
retired design's own test.

**What the retirement costs in this suite is one test.** `lichen-lowlevel` goes from
147 / 13 to **146 / 13**, and `Compare-Object` over the full name sets reports exactly the
retired name and the renamed one — no failure moves.

### 8.6 What it costs, as landed

**Three `lichen-compute` tests, all of them red now, and all of them accepted as the price.**
The sixty-five are gone: §9's bounded retry took sixty-two of them away before the retirement
was taken, and these three are what the retirement itself costs. None is a design-pin — none
asserts a re-decision — and each asserts something the plain gate changes. §15 of
[late-writes-map](late-writes-map.md) has the by-name comparison and the per-test reading.

Branch `feature/retire-epoch`, worktree `.worktrees/re`, base `dev@12425166`, private target,
`-j 1`, probe-free.

| suite | pristine `dev` | after the retirement | name-set delta by `Compare-Object` |
|---|---|---|---|
| `cargo test -p lichen-lowlevel -j 1 --no-fail-fast` | 147 / 13 | **146 / 13** | the retired test leaves, the rewritten assert test changes name — no failure moves |
| `cargo test -p lichen-highlevel -j 1` | 140 / 0 | **140 / 0** | **empty** |
| `cargo test -p lichen-compute -j 1 --no-fail-fast` | 223 / 14 / 2 | **220 / 17 / 2** | three new failures, the three below |

| test | what it observes | reading |
|---|---|---|
| `consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched` | `consumer_guard.rs:256` — `left: ["combine","identity","element"]` against `right: ["combine","identity"]`, on the **producer's** claim | a recording failure: the extra round `reopen_stalled` drives forces operand edges the first pass never reached, so the producer's algebra name reads one role further than the rule expects |
| `graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture` | `graph_jit.rs:135` — `expected this program to fail: "raw[raw 2, raw 2, raw 2]: ?a"` | the capture rule stops firing where it must: the program is accepted rather than refused, so the rule the test is about no longer fires |
| `graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches` | `graph_jit.rs:514` — `left` (the recorded graph's two dispatches) against `right: []` | **a premise mismatch, not a recording failure**: `right` is the *host* run's trace, and the plain gate makes the program itself perform no dispatch at all. Plain and recorded cannot agree however the recording is driven |

The third is why this is no longer the `wrong-value` bucket §8.2 used to hold the decision:
the retirement does not change a *checked* outcome. Fifty-seven programs stopped building
before §9's fix and none of them does now. What remains is a claim that reads one role too
far, a capture refusal that no longer fires, and a host run that no longer dispatches.

The lever §12.7 of [late-writes-map](late-writes-map.md) names is unchanged and is still a
maintainer's question: the 6 268 late writes are all at sanctioned sites, and the checker's
evaluate-before-unify order is a landed decision ([eval-before-unify](eval-before-unify.md)
§2.1). It is no longer what the retirement waits on.

## 9. The recording stopped waiting for it

`compute.graph`'s recording pass was the one caller that **depended** on the epoch
moving rather than on asking again: it applied a body to its own input placeholders
and read the body's returned value tree out of **raw node slots**
(`Module::node_value` evaluates nothing), so a structural read that stalled once
during the recording left its slot empty and the recording refused. It worked only
because some other write in the same pass moved the epoch — an accident of ordering,
not a second attempt it asked for.

That dependency is gone. The recording now drives its own bounded retry
(`evaluate_until_the_return_is_nameable` in `lichen-compute`'s `compute.rs`, over
`Module::reopen_stalled`), and a plain run gate — `runned != 0` with no epoch
comparison — is **62 `lichen-compute` failures lighter** with it and **zero**
failures heavier. §14.4 of [late-writes-map](late-writes-map.md) gives the arms by
name, the two narrowings that were tried and rejected, and the three that remain.

So §8.6's "sixty-five programs that stop building" is **three**, and the gate
was still not free — but the reason had moved. Of the three, one is the recording reading
an algebra name further than §8.3's refusal implies it should, and two are the plain
gate breaking `lichen-compute` outside the recording altogether: in one of them the
*host* run performs no dispatch at all. Those two were the last thing standing between the
measurement and the decision; §8.6 now records what the retirement costs, and they are its price.

