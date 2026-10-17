# Graph recording and the re-drive: one graph is one walk

> Status: **open, and this note is the handover.** The recording pass re-drives its own deep pass so a
> structural read that stalled on a placeholder cannot keep the returned value unnameable. That fix works, and
> it has one visible cost: the recording spans **every** round, so a graph now carries two rounds of one body's
> dispathces and a producer's algebra name reads one role further than it should. The cause is measured; the
> fix is not made. Whoever owns `lichen-compute` should decide the shape, because the change is to *which
> dispatches a graph contains*, which is that crate's semantics, not this note's.

## The symptom

`consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched` fails at
`crates/lichen-compute/tests/lichen_compute/consumer_guard.rs:256`:

```text
left:  ["combine", "identity", "element"]
right: ["combine", "identity"]
```

The test's own words: *"the producer's claim declares the node-local algebra and nothing else"*. The extra
`"element"` is not a mis-rendering — it is a dispatch that a second walk of the same body performed.

## The cause, measured

1. `evaluate_until_the_return_is_nameable` — defined at `compute.rs:5360`, called once at `:5201` — deep-evaluates
   the apply in its own body (`crates/lichen-compute/src/compute.rs:5369-5382`), and when
   the returned value is not nameable it calls `Module::reopen_stalled(block)` and evaluates again, up to
   `RECORDING_ROUNDS`.
2. `reopen_stalled` (`crates/lichen-lowlevel/src/module.rs:368-397`) clears the run stamp of every operation
   under the block that ran and could not decide. Its invariant: only an operation whose value slot is empty is
   touched, so an answer is never discarded.
3. The second round therefore resolves the reads that stalled in the first, and the deep walk **continues past
   the point where it previously stopped**. Operators beyond that point run for the first time, and each
   dispatch appends its algebra role to the producer's claim.
4. The recording, however, is opened once around **all** the rounds (`graph::begin` at `compute.rs:5196`,
   `graph::finish` at `:5237`, with the re-drive loop at `:5201` between them), so the appended dispatches of
   every round land in one graph. That is the defect: `graph::is_recording`'s invariant has **the thread** as its
   subject — *"The thread is load-bearing: one graph is built by one evaluation, so a dispatch on another thread
   is not part of it"* (`graph.rs:193-197`) — and that a round **is** an evaluation is an **inference** from that
   invariant, not its literal words. The compute owner confirmed the inference; it is recorded as one so that
   nobody can cite this note as licence to change that invariant.

So the algebra name is the *symptom*; the cause is that a recording outlives the round it belongs to.

## The fix proposed, and why

**Reset the recording per round: `abandon()` before `begin()` for every round after the first, and keep the
last round's recording.**

- The last round is the one whose returned value is nameable — that is the loop's whole purpose — and it is a
  superset of the earlier rounds, since the first round's stalls resolve in it. Discarding the earlier rounds
  loses only duplicates.
- The graph then contains exactly one walk's dispatches, which is what `is_recording`'s invariant states.
- **One detail is load-bearing**: `abandon()` hands back that round's refusals. They must be re-recorded into
  the new recording (`graph::record_refusal`), or the failure information a refusal carries is lost.

Two alternatives were considered and are worse:

- **Snapshot and truncate to the first round.** Keeps the first round's dispatches, which is closer to the
  original design, but if the returned value's producer only ran in a later round, `graph::record_return`
  refuses for want of a producer edge — the "unnameable return" failure comes back.
- **Re-drive only the stalled nodes instead of the whole apply.** Leaves the walk's extent unchanged, but needs
  a second mechanism that can name the returned value without a full walk.

## Acceptance

- `cargo test -p lichen-compute -j 1 --no-fail-fast`: 201 passed / 17 failed / 2 ignored today (see
  [late-writes-map](late-writes-map.md) §15). After the fix: **`consumer_guard` and
  `graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture` should be re-examined** — the second one
  is the same class of defect, a rule that stopped firing because of the wider walk — and the other sixteen
  must not change name.
- `cargo test -p lichen-lowlevel -j 1 --no-fail-fast`: 146 passed / 13 failed, the reds the project carries
  deliberately. The lowlevel is untouched by the fix, so this is a control.
- `comments-check --config comments-test.toml . --max-violations 0` before every commit.

## Boundary facts for the compute owner

- The recording lifecycle is `graph::begin` / `graph::abandon` / `graph::finish`
  (`crates/lichen-compute/src/compute/graph.rs:198-269`), and the recording is a **thread-local stack**
  (`recordings.borrow_mut().push(...)`), so `begin`/`abandon` inside `evaluate_until_the_return_is_nameable` is on the right thread
  by construction.
- The re-drive loop is `evaluate_until_the_return_is_nameable` (`compute.rs:5369-5382`); `RECORDING_ROUNDS` bounds it and
  `returned_value_ids` decides whether a round succeeded.
- `reopen_stalled` is lowlevel and is **not** the thing to change: it is what makes a re-drive possible at all,
  and its stamp discipline is pinned by the lowlevel's own tests.
- The other two compute reds of this class are `graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture`
  (a capture rule that stopped firing) and `graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches`
  (a test premise: under a plain run gate the host run performs no dispatch, so plain and recorded cannot
  agree). Only the first is fixed by this note's proposal.

## The re-drive's rounds, measured

An env-gated probe printed, at the end of every round, whether the return was nameable and how many dispatches
the innermost recording held (`graph::probe_node_count`). Over `graph_jit::` the 35 readings fall into two
patterns:

| reading | what it says |
| --- | --- |
| `round=1 recorded=false nodes=0` then `round=2 recorded=true nodes=2` | the first round's deep walk reached **no dispatch at all**; the second produced both of them and named the return |
| `round=3 recorded=false nodes=2` | the re-drive ran out without naming the return — a genuine recording failure, which A′ changes in no way except to make its refusal text clean |

So on this corpus the last round is stronger than a superset of the earlier ones: the first round is usually
**empty**, and discarding it loses nothing. That is the measured ground under A′'s justification, and it is why
the truncate-to-first-round alternative would leave a graph with no dispatch of the body that was named.

`record_refusal` is an acceptance item rather than a detail, and it needs a reading, not a code review: take a
program whose recording refuses inside the round that gets abandoned, and compare the refusal text
`build_graph` reports (`refused_because_of(&refusals)`) before and after the change. It must be the same text.
A refusal that disappears is a lost diagnostic.

## Per-program readings, for the third acceptance item

Each `graph_jit` test was run alone (`--exact`) with the probe, so every line below belongs to the named
program; a program that opens more than one recording shows it twice. `recorded` is `returned_value_ids`
succeeding, `nodes` is the recording's dispatch count at the end of the round.

| program | rounds |
| --- | --- |
| `a_buffer_the_body_closed_over_is_refused_by_the_capture` | `r1 false/0` → `r2 true/2` |
| `a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one` | `r1 false/0` → `r2 false/2` → `r3 false/2` → `r4 false/2` |
| `a_function_that_dispatches_nothing_has_no_backend_to_run_on` | `r1 true/0` (no re-drive) |
| `a_graph_dispatches_exactly_what_the_program_dispatches` | `r1 false/0` → `r2 true/2` |
| `a_graph_recorded_for_one_backend_runs_on_another` | twice: `r1 false/0` → `r2 true/2` |
| `a_graph_runs_at_whichever_extent_its_argument_names` | twice: `r1 false/0` → `r2 true/2` |
| `a_recording_produces_the_same_numbers_as_running_the_dispatches` | `r1 false/0` → `r2 true/2` |
| `one_backend_for_the_whole_graph_is_checked_while_it_is_built` | `r1 false/0` → `r2 false/1` → `r3 false/1` → `r4 false/1` |
| `what_a_recorded_body_may_not_reach_for_is_refused_by_name` | three recordings, all unnamed: `false/1` lines and one `false/0` line |

**The programs whose recording never names its return must stay exactly as they are.** They are
`a_count_…count_filter…`, `one_backend_…is_built` and `what_a_recorded_body_may_not_reach_for…`, and each is a
test that **asserts a refusal** — the unnamed return is the refusal working, not a defect. A′ must not turn any
of them green, and must not change the reason they name. That is the "these stay red with the same reason"
clause of the acceptance.

The first round is empty in seven of the nine programs (`nodes=0`), and the one program that names its return
without a second round does so with **zero** dispatches (`a_function_that_dispatches_nothing…`). So the
"one graph, one walk" proposal loses nothing measurable on this corpus: the first round's dispatch count is
**0** everywhere the re-drive runs at all.

## Acceptance, settled with the compute owner

Three corrections were settled in the exchange, and one of them corrects this note's own earlier wording.

**Item 3 is per recording, in order.** A program may open more than one recording
(`a_graph_recorded_for_one_backend_runs_on_another` and `a_graph_runs_at_whichever_extent_its_argument_names`
open two each, `what_a_recorded_body_may_not_reach_for_is_refused_by_name` opens three), so "each program's
count is the same" is the wrong sentence. The baseline is the table above read as `(program, which recording,
nodes)`: the same programs, in the same order, with the same counts. One recording fewer or more is a change.

**The never-naming programs are green, and that is the point.** This note's earlier sentence and the exchange's
first draft both said those programs "stay red". That is wrong: they **pass today**, because each of them
asserts that a refusal is issued, and an unnamed return is that refusal working. Corrected, the acceptance is
about the **refusal text**, not about a red status: after the change, each of those programs must fail to name
its return **for the same reason**, and the assertion that pins the reason is the literal in the test —
`"a collect is asked here"` and `"after the graph has run"` in
`what_a_recorded_body_may_not_reach_for_is_refused_by_name` (`graph_jit.rs:342-348`), and the matching pair in
the host-read half of the same test. A reason that changes is a lost diagnostic; a reason that disappears is a
test that turns green, which is also a failure of the acceptance.

**`a_buffer_the_body_closed_over_is_refused_by_the_capture` is not A′'s.** It names its return on round two
(`r1 false/0 → r2 true/2`), so its red is not a recording failure: the program is **accepted** where the capture
rule should refuse it, which is a different defect with a different shape (the wider walk stops a rule firing).
**A′ does not promise to fix it**, and its fix must not be booked against this proposal.

## Replicating the table

The probe is not an artefact and was reverted; the readings are reproduced by re-applying it — `graph::probe_node_count`
returning the innermost recording's `nodes.len()`, and an `eprintln!` at each round's end in `evaluate_until_the_return_is_nameable`
(`compute.rs:5369-5382`) — and running each test **alone**:

```text
cargo test -p lichen-compute --test lichen_compute <full::test::path> -- --exact --nocapture
```

The `--exact` flag needs the **full** path (`lichen_compute::graph_jit::<name>`; a bare name filters to zero
tests and looks like a pass). Running alone is not a convenience: `eprintln!` writes to stderr, which libtest
does not buffer, so a whole-module run interleaves one program's readings into another's and attribution becomes
guesswork — the first attempt at this table did exactly that. With one test per process, every line belongs to
the named program.

### The refusal literals, verbatim

`what_a_recorded_body_may_not_reach_for_is_refused_by_name` is **one** test that pins **six** message fragments
across three programs it refuses, not four. These are the anchors a comparison must use — copied from
`crates/lichen-compute/tests/lichen_compute/graph_jit.rs`, with the line each `contains` call sits on:

| # | literal | line | which program |
| --- | --- | --- | --- |
| 1 | `"a collect is asked here"` | `:345` | `compute.collect` inside a recorded body |
| 2 | `"after the graph has run"` | `:346` | the same message's other half |
| 3 | `"a host read is asked here"` | `:369` | `compute.read` inside a recorded body |
| 4 | `"a graph has no node for one"` | `:393` | a scalar kernel: no node to be |
| 5 | `"nowhere for what it computes"` | `:394` | the same message's other half |
| 6 | `"compute.graph"` | `:401`, `:402`, `:405` | all three: the fault is filed under the graph |

A comparison that finds the same six fragments has found the same reasons. One missing fragment is a lost
diagnostic; the test file itself is the authority, so this table is a pointer with line numbers, not a summary.

### The acceptance is two-way

- **A reason that changes fails the acceptance even if the test still fails**, because the failure would be
  about a different defect.
- **A reason that disappears fails the acceptance even though the test turns green**, because the test asserts
  that the refusal is issued — a silent refusal is a lost diagnostic that looks like success. This is the
  failure mode that a "does it pass?" check cannot see, and it is the reason this clause is written separately.
### The frozen set is (literal, count), read from the source

The acceptance does not freeze "the six strings are still somewhere in the messages". It freezes **six pairs of
(literal, how many messages contain it)**, because two failures are invisible to a presence check:

- a fragment that **disappears** — that message has lost part of its reason;
- a fragment that **gains an occurrence** — an extra refused path now produces the same fragment, which is a
  refusal that did not exist before.

Over the messages `fail()` returns for the three programs:

| literal | messages that must contain it |
| --- | --- |
| `"a collect is asked here"` | 1 |
| `"after the graph has run"` | 1 |
| `"a host read is asked here"` | 1 |
| `"a graph has no node for one"` | 1 |
| `"nowhere for what it computes"` | 1 |
| `"compute.graph"` | **3** — one per program's message, the three checks at `:401`, `:402`, `:405` |

**Six fragments, four blocks.** The count and the block count do not disagree: the first assert block pins one
fragment, the second one, the third two, and the fourth block checks the shared `"compute.graph"` once against
each of the three messages. Six = 1 + 1 + 2 + (1 × 3 messages).
**Read the input from the source, never from this table.** The four `assert` blocks (`:342-348`, `:367-372`,
`:390-397`, `:398-409`) hold the `contains` arguments; the table only says where to look, and if the two ever
disagree the source wins. A table used as the input freezes the acceptance on **old** text the day the source
changes, and nothing reports the divergence.
## Running the acceptance

Four steps, in this order. Nothing here needs a decision, so the comparison cannot drift between whoever runs it.

1. **Re-apply the probe** (the same two edits the table was taken with): `graph::probe_node_count` returning the
   innermost recording's `nodes.len()`, and the `eprintln!("ROUNDPROBE round=… recorded=… nodes=…")` at the end
   of each round in `evaluate_until_the_return_is_nameable`.
2. **Run the nine programs alone**, one process each, and collect the `ROUNDPROBE` lines:
   `cargo test -p lichen-compute --test lichen_compute lichen_compute::graph_jit::<name> -- --exact --nocapture`,
   for each name in the table above. Compare **per recording, in order**, and as a **single-program** comparison — the same program measured on its own commit before and after, never the whole table against the whole table (see "The comparison is single-program" below).
3. **Run the suite and diff the failure names**:
   `cargo test -p lichen-compute -j 1 --no-fail-fast`, then `Compare-Object` the `… FAILED` names against a
   pristine `dev` run. `consumer_guard` must leave the set; the other sixteen must not move.
4. **Read the six refusal fragments from the source**, not from this note: the `contains` arguments of the four
   `assert` blocks at `graph_jit.rs:342-348`, `:367-372`, `:390-397`, `:398-409`. Check each against the
   messages `fail()` collects, with `"compute.graph"` appearing exactly three times.

**A detached checkout needs its own target directory.** Different worktrees of the same commit carry an identical
`path` hash in their fingerprints, so two checkouts can write the *same* `liblichen_lowlevel-<hash>.rlib` and
overwrite each other; this repository has produced "the code compiles but the probe cannot find the symbol" four
times from exactly that. A run on someone else's branch must therefore set
`CARGO_TARGET_DIR` to a target inside its own checkout, and every `Compiling` line it prints must name that
checkout — not the parent and not the branch's own worktree. If a reading disagrees with the owner's, rule out a
mixed build first (`cargo clean -p lichen-compute`, then re-run); only a clean re-run counts.

**Why a derived reading cannot be trusted.** The verdict column of the before readings was produced by a script,
and it was wrong: PowerShell's `-match` is case-insensitive, so `-match 'FAILED'` matched the `0 failed` inside a
passing result line and reported all nine programs as failed. The suite's own `test result:` lines were right all
along. So the rule is not "be careful with scripts" but the stronger one the compute owner stated: **ignore any
table that went through a parser, and read the output the parser read.** That is also why the acceptance anchors
on the suite's name set and the source's literals — both are first-hand — rather than on a summary.
The probe is reverted before any commit, in both directions: before the change, the table above is the record;
after it, the compared lines are the record.
## The before readings, taken on `a4593102`

Requested before the change so the comparison is one run rather than two derivations. Each test was run alone
with the probe; the readings reproduce the table above line for line, which is itself a check that nothing moved
between then and now.

```text
a_buffer_the_body_closed_over_is_refused_by_the_capture          r1 false/0 | r2 true/2
a_count_the_body_closed_over_is_refused_by_the_count_filter…     r1 false/0 | r2 false/2 | r3 false/2 | r4 false/2
a_function_that_dispatches_nothing_has_no_backend_to_run_on      r1 true/0   (no re-drive)
a_graph_dispatches_exactly_what_the_program_dispatches           r1 false/0 | r2 true/2
a_graph_recorded_for_one_backend_runs_on_another                 twice: r1 false/0 | r2 true/2
a_graph_runs_at_whichever_extent_its_argument_names              twice: r1 false/0 | r2 true/2
a_recording_produces_the_same_numbers_as_running_the_dispatches  r1 false/0 | r2 true/2
one_backend_for_the_whole_graph_is_checked_while_it_is_built      r1 false/0 | r2 false/1 | r3 false/1 | r4 false/1
what_a_recorded_body_may_not_reach_for_is_refused_by_name        three recordings: false/1 x3, false/1 x3, false/0 x4
```

The failure-name set is the one measured on the same commit: `lichen-compute` **201 passed / 17 failed /
2 ignored**, and of the nine programs above only two are among the seventeen —
`a_buffer_the_body_closed_over_is_refused_by_the_capture` and
`a_graph_dispatches_exactly_what_the_program_dispatches`. The other seven pass, which is the point the two-way
clause makes: their refusal is expected and an unnamed return is it working.

The six `(literal, messages containing it)` pairs are read from the source as the acceptance requires; the
per-message occurrence counts are the source's own, since a passing assertion cannot be observed from outside
without a probe. What the passing test establishes is presence in every message it checks.

**One caution about the raw run**: the script that took these readings printed every program as `FAILED`, which
is wrong — its per-test verdict parsing was broken (the suite reports 7 passed / 2 failed for the same nine
programs). The `nodes` columns are unaffected, because they come from the probe's own lines; the verdicts are to
be taken from the suite, never from that script. Recorded here because a wrong verdict column in a baseline is
exactly the kind of thing a later reader would copy.

### The comparison is single-program, not table-to-table

The before readings were taken on `a4593102`; the after run happens on whatever commit the compute owner names
(the `fma` slice landed as `c950f1fb`, and between the two commits `dev` also took the scalar-ring rule, the two
`Verify` rules in `AGENTS.md`, and three notes of mine). So a whole-table before/after would put **other people's
commits** on A′'s account. Each reading answers its own question, and only in this way:

- **Did A′ move the walk?** `consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched`'s
  per-round `nodes`, the same program on each commit separately. The other eight programs are controls, read one
  by one — a program whose readings changed is a finding, not a count.
- **Did `consumer_guard` turn green, and the other sixteen stay?** The `lichen-compute` failure-name set,
  measured **on each commit as a full run** (`201 passed / 17 failed / 2 ignored` on both, with `consumer_guard`
  leaving the set). Two full runs are two measurements, not one code's before and after.
- **Did any refusal text change silently?** The six `(literal, count)` pairs, read from the source's own `assert`
  blocks, on the after commit.

One non-fault: `crates/lichen-compute-gpu/examples/fma_probe.rs` stays in the tree (the repository allows a probe
during an investigation and it drives the two device tables), so a detached run compiles one extra example. That
is a few seconds, not a failure.
