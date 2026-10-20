# Graph recording and the re-drive: one graph is one walk

> Status: **landed and accepted.** The recording pass re-drives its own deep pass so a structural read that
> stalled on a placeholder cannot keep the returned value unnameable. That fix worked, and its one visible cost
> was that the recording spanned **every** round. The compute owner landed the per-round reset — A′, `e659607e` —
> and it is accepted: the suite's seventeen names are unchanged, the refusal text is untouched, and three of the
> nine `graph_jit` controls moved, one of them from *never naming* its return to naming it on the third round,
> which is the change being proved. `consumer_guard`'s extra role is **not** this defect and was re-attributed to
> the test's kernel order. The sections below are the before readings, the after readings, the acceptance, and
> the corrections. One more correction follows them: the capture diagnosis this note then settled was refuted by
> measurement — that program closed over a launch, not a live buffer — and its test was corrected instead of a
> rule repaired (last section).

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
  [late-writes-map](late-writes-map.md) §15). After the fix: **`consumer_guard` should be re-examined** — it is
  not this defect, as the correction below measures — and the other sixteen must not change name.
  (`a_buffer_…capture` was re-examined later and has no capture behind it; see the last section.)
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
  (later measured to have no capture behind it — see the last section) and
  `graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches` (a test premise: under a plain run gate the
  host run performs no dispatch, so plain and recorded cannot agree).

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
(`r1 false/0 → r2 true/2`), so its red is not a recording failure: the program is **accepted**. That is not a
capture rule that stopped firing either — measured later, the recording contains the closed-over launch's own
dispatch, so there is no live buffer for the rule to see (last section). **A′ does not promise to fix it**, and
its fix must not be booked against this proposal.

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
   pristine `dev` run. The sixteen other failures must not move, and `consumer_guard` is **not** expected to leave the set — its role is a separate defect, as the correction at the end of this note measures.
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

**The `consumer_guard` before reading**, taken the same way on `a4593102`:

```text
consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched   r1 false/0 | r2 true/2
```

So A′'s verdict is a two-part reading on the after commit: the per-round `nodes` must be **`0` then `2`** — the
walk's extent must not move — *while* the producer's roles stop growing and the test turns green. A change in
those two numbers is A′ moving the walk, which the name-set reading cannot see; a green test with different
numbers is a green that came from somewhere else.
One non-fault: `crates/lichen-compute-gpu/examples/fma_probe.rs` stays in the tree (the repository allows a probe
during an investigation and it drives the two device tables), so a detached run compiles one extra example. That
is a few seconds, not a failure.

## Corrected by measurement: `consumer_guard` is a separate defect

This note's chain was written for the nine `graph_jit` programs and then **attributed to `consumer_guard`
without measuring that program's rounds** — the mistake both sides of this exchange kept repeating, partial
readings read as the whole. Measured, it does not hold:

- `consumer_guard` **does** drive the loop once (two evaluations: the before reading on `a4593102` is
  `r1 false/0 → r2 true/2`, and the compute owner's own probe reports the loop iterating once).
- With the per-round reset **in place** — so the graph holds the last round's dispatches only — its producer's
  roles are still `["combine", "identity", "element"]` and the test still fails, while the suite's name set is
  unchanged at `201 passed / 17 failed / 2 ignored`.

So the extra `"element"` is produced **inside one round's own dispatches**, not by a recording that spans two.
A′ therefore changes nothing on this program: it is a no-op there by measurement, not by argument.

**What this corrects:**

- Rings 3 and 4 of "The cause, measured" explain the **multi-round path** — the nine `graph_jit` programs. They
  do **not** explain `consumer_guard`.
- A′'s acceptance is about the multi-round path (the nine readings unchanged) and about **not disturbing** the
  suite's name set. It is **not** required to turn `consumer_guard` green, and a green `consumer_guard` would not
  be A′'s evidence.
- `consumer_guard`'s own defect is **open** with an unknown shape. Its next step is the role collection point the
  test asserts on, `the test's kernel attribution — see "The `consumer_guard` shape, measured" below, which refutes the role-collection-point reading`: the roles must be found there from a single round's dispatches,
  which is a different question from which recording they land in.

**The `consumer_guard` before reading** stands as recorded below (`r1 false/0 | r2 true/2`); what was wrong was
the cause attached to it, not the reading.

## The `consumer_guard` shape, measured: it is the test's kernel order, not a collection point

The section above names `the test's kernel attribution — see "The `consumer_guard` shape, measured" below, which refutes the role-collection-point reading`'s role collection point as the next step. There is no such
point in the source, and the shape is now measured.

**The role names are the annotation's own words.** `"combine"`, `"identity"` and `"element"` appear nowhere as
literals in `crates/lichen-compute/src`; they are what a source writes in
`~ "claim:Reduction(combine,identity)"`. This test's producer declares `Reduction(combine,identity)` and its
consumer declares `Reduction(combine,identity,element)`, so **both claims are read correctly** — the failing
assertion is not about a claim's content.

**The test identifies its kernels by id order.** `run` (`consumer_guard.rs:162-178`) collects every kernel id
its module names, then `ids.sort_unstable(); ids.dedup();`, and builds `kernels` in that order. So `kernels[0]`
is the **lowest id**, not the source's first kernel, while the assertions at `:256` and `:261` treat `[0]` as the
producer and `[1]` as the consumer.

**The defect appears when registration order and source order disagree.** Then the producer's expected
`["combine","identity"]` is compared against the **consumer's** actual `["combine","identity","element"]` — the
failure this note opens with. The test's own invariant says the registry is **global** and that a program's
kernels are read from its own module "rather than from what the registry gained"; it did not go on to say that
**id order is source order**, which is the assumption that broke.

The parallel kernel's own `parallel_roles` (`compute.rs:1932`) is the other place roles are read and it is
**not** involved: it walks the parameter's **type**, and its `LeafRole` has no `element` at all.

**What this changes:** the next step is not a collection point but the test's identification of its two kernels —
by name, or by the roles each declares, instead of by id. That is a **test** change, and it is the compute
owner's decision rather than an unattributed fix. Recorded here because this note is where the attribution lives.

## The after readings, taken on `e659607e`

A detached checkout of the A′ commit, its own `CARGO_TARGET_DIR`, the probe re-applied and reverted, each program
run alone. The four readings the acceptance asks for:

**`consumer_guard` is unchanged**: `r1 false/0 → r2 true/2`, still failing. A′ cannot help it, and the note's
earlier expectation that it would is corrected above.

**The name set is unchanged**: `lichen-compute` 201 passed / 17 failed / 2 ignored, the seventeen names identical
to the before reading, `consumer_guard` still among them; the unit suite 19/0.

**The six refusal fragments are unchanged**: the three tests that assert them all pass, and A′'s six lines touch
the recording lifecycle only, never a `refuse` call.

**Three of the nine controls changed**, so A′ is not a no-op on the multi-round path:

| program | before | after |
| --- | --- | --- |
| `one_backend_for_the_whole_graph_is_checked_while_it_is_built` | `r1 false/0 → r2 false/1 → r3 false/1 → r4 false/1` (never named) | `r1 false/0 → r2 false/1 → **r3 true/1**` |
| `a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one` | `r1 false/0 → r2 false/2 → r3 false/2 → r4 false/2` | `r1 false/0 → r2 false/2 → **r3 false/0 → r4 false/0**` |
| `what_a_recorded_body_may_not_reach_for_is_refused_by_name` | three recordings, `false/1` runs then `false/0` | three recordings, `false/0, false/1, false/0, false/0` and one all-`false/0` |

**This is A′'s only evidence on the multi-round path.** The nine controls are otherwise line-for-line unchanged;
the three rows above are the whole of what the change moved.
The other six are line-for-line unchanged, including both that still fail (`a_buffer_…capture` and
`a_graph_dispatches_…`).

**The fifth acceptance item, and it is narrower than "an empty graph".** A kept round with zero dispatches is not
itself a defect: `a_function_that_dispatches_nothing_…` is a supported, tested outcome, and `abandon()` putting an
abandoned round's dispatches into no graph is the definition of one graph per round rather than a cost of it. The
failure to watch for is this: **a program that is not meant to be refused names its return, yet all of its
dispatches sit in the abandoned round** — the kept graph then lacks them, while the name and the return are still
there and can keep the test green. So the reading is "the kept round's dispatch set is non-empty, or the program
genuinely dispatches nothing".

**What the change fixes, stated causally.** `one_backend_…` went from *never naming* to *naming on the third
round*, which means the graph shared across rounds was **blocking the naming**. That is what A′ repaired. The two
zeroed readings are the same fact seen from the other side: those rounds produced nothing, so keeping only the
last one keeps nothing — and their tests assert refusals, which is why the name set does not move.

## `a_buffer_the_body_closed_over_is_refused_by_the_capture`: the determination, refuted

The sections that stood here settled a conjunction diagnostic for this test and handed five
implementation steps to a new session. **Measurement refuted the premise before any of it landed**,
so the material is replaced rather than kept with a "superseded" label: the readings it rested on are
real, the conclusion drawn from them was not, and a reader who followed the old text would build a
guard that can never fire. What follows is the refutation, the corrected reading, and what landed.

### The premise, and the reading it came from

The probe printed, at the recording path's role match, **how many input roles** the decoded table
held and nothing else. Over the suite, on `dev@c9ba66ac`: 144 `Ok`, 4 `Undecided`; of the `Ok` ones
133 carried at least one input role and **11 carried none**, and the capture program was one of the
eleven. That was read as "the role table is empty", then as "the parameter's names hold neither `in`
nor `out`", and the settled fix became a conjunction: fire when the names hold neither group **and**
a dispatch in this round's recording read a live buffer.

**The input count is not the table.** The capture program's decode names both groups —
`names=[Some("n"), Some("in"), Some("out")]`, `inputs: []`, `outputs: [[2, 0]]`,
`scalars: [[0], [1, 0]]` — and `inputs` is empty only because `InS.a` is a runtime scalar, not a
buffer. The same reading takes away the second half: the recording contains **no dispatch that read a
live buffer at all**. `roles=0` also never meant "the walk did not run": two of the three `roles=0`
programs attributed by order are passing tests that record and run normally
(`a_graph_recorded_for_one_backend_runs_on_another`,
`a_graph_runs_at_whichever_extent_its_argument_names`), which is what made the reading suspect. The
four `Undecided` readings are a separate decoding failure (`type_slot_value=true`,
`struct_fields=None`), not evidence about this one.

### The measurement that refutes it

The old program, run alone with an env-gated probe (re-applied and reverted, nothing committed), on
`dev@f5991ce6`:

```text
ROLESPROBE names=[Some("n"), Some("in"), Some("out")] in=Some(1) out=Some(2)
           roles=KernelRoles { scalars: [[0], [1, 0]], inputs: [], outputs: [[2, 0]], … }
DECODEPROBE named_abi=Some(true) roles_in=0 roles_out=1 roles_scalars=2
ROUNDPROBE round=1 recorded=false nodes=0
DISPATCHPROBE at=0 outputs=1 inputs=0 live=false
INPUTPROBE position=0 path=[1, 0] placeholder=true value=a placeholder for a value the graph has produced
DISPATCHPROBE at=1 outputs=1 inputs=1 live=false
ROUNDPROBE round=2 recorded=true nodes=2
CAPTUREPROBE named_abi=Some(true) live_dispatch=None
```

Three facts, each measured rather than inferred:

- **`named_abi` is never `Some(false)` in the nine `graph_jit` programs**, and `live_dispatch` is
  never `Some`: the conjunction's two halves are both false.
- **Node 0 is `held`'s own `plrun`** (`at=0`, no input operands), so the second dispatch's buffer
  operand arrives as a `placeholder=true` `GraphValue`: `place` is reached and correctly sees an
  edge, which is why the old sentence never fires.
- **The program is accepted and the graph answers `2,2,2`**: the `fail()` panic reads
  `expected this program to fail: "raw[raw 2, raw 2, raw 2]: ?a"` — the false acceptance the test was
  written to catch, with no capture behind it.

### What is true instead: the run is lazy, so the walk contains the closure

`graph::begin` opens before the apply and the language's blocks are lazy, so a free variable the body
reads is forced **inside the recording window**; its dispatch is recorded as part of the walk and its
result reaches the body as an edge. The graph then recomputes the closed-over launch on every run —
the same numbers for a pure kernel — and it holds no block, arena or device lifetime, so nothing is
captured and there is no use-after-free. The refusal's precondition is a **live** buffer — host memory,
or a result that already ran — and `place` refuses exactly that. So
[compute-graph-jit](compute-graph-jit.md)'s "a buffer the body reads as a free variable is refused" is
a statement about that case; a launch the body reaches as a free variable is not a buffer.

The rule was intact. The **test's premise** was wrong: its program was never a capture, and it
asserted a refusal for a program the language legitimately records.

### The resolution, and what landed

- The capture test now closes over a real host buffer —
  `captured = (compute.buffer ([1, 1, 1] : array{Int, 3}) : (compute.Buf Int))` — so `place` refuses
  it through the recorded dispatch, the diagnostic is filed under `compute.graph`, and the test passes
  **with its original sentence**, which still contains `hold` and `parameter`.
- A new test, `a_launch_the_body_closed_over_is_recorded_into_the_graph`, pins the other half: the
  program answers `2,2,2`, and the backend sees exactly the two dispatches the walk performs —
  `held`'s and the one that reads it.
- **No production code changed.** The per-dispatch live-buffer boolean, `named_abi` and the
  conjunction were implemented, measured to be unreachable, and reverted before commit; a flag with
  no consumer is not worth its field.
- **The test change was approved by the owner before it was written** — the reworked program and the
  new test alike. The repository's rule is *no test without permission* (`AGENTS.md`), so this is the
  record of that permission, not a review after the fact. The old program's stated premise ("`held` is
  a live buffer") is not what the language does.

### The readings

`cargo test -p lichen-compute -j 3 --no-fail-fast`, before and after, both on `dev@f5991ce6` with
only the test file differing (a private `CARGO_TARGET_DIR` inside this checkout):

| | passed | failed | ignored |
| --- | --- | --- | --- |
| before | 200 | 17 | 2 |
| after | **202** | **16** | 2 |

The failure-name set differs by exactly one name:
`graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture` leaves it, and nothing else
moves. The unit suite is 19/0 in both. The note's older `201/17/2` baseline was taken on `b3554f76`;
the two `ln` commits landed in between, which is why the before count here is 200. Rebased onto
`01f5a6ac` — which carries `consumer_guard`'s own fix — the suite reads **203/15/2**: this table plus
that commit's one pass. The isolated comparison stays the f5991ce6 one, because only it can attribute
a difference to this change.

**Reproducing.** The probe is not an artefact: `graph::probe_node_count`, an `eprintln!` at each
round's end in `evaluate_until_the_return_is_nameable`, a per-input print in `record_launch`'s
argument loop, a per-dispatch print in `graph::record_dispatch`, and the `named_abi`/live-dispatch
print at the conjunction point in `build_graph`, all gated on one environment variable and reverted
before the commit. Each program was run alone with the full path and `--exact`, so no line belongs to
another program.
**The slot encoding's seam (measured on the twenty-six-test graph block).** With a
compound's elements carrying complete `[value, type, attributes]` slots, the ten
`graph_jit` tests refuse at `compute.parallel`'s output walk: "no output of this
parallel index function carries a decided scalar class". Three readings, each taken
with a temporary print and reverted:

- the applied pair the checker records is `[value, type, wildcards…]`, and its **value
  position is a two-item frozen array** — the annotation's value, not the `$write`
  operation — so `write_value_node`'s walk finds no write;
- `Shape::clone_static_signature_node` now copies a frozen node's **operation**, so a
  reader that resolves operators can see one; this did not move the ten, which is what
  points at the frozen return pair's own value;
- scanning the index function's **block subtree** for the write call does not find it:
  the applies found there belong to the graph body (`.I/.O` structs), not to `f1`'s
  body, so the write call's node is homed elsewhere than `Function::block`'s subtree.

`compute::jit_*` is 30/40 overall (all non-graph tests pass); the remaining ten are
this walk.

**Two more negatives (round five, same ten).** Disabling `materialize_static_return_apply`
— the applied-pair clone the certificate work added — leaves the ten red, so the clone is
not their cause. A whole-module scan for the write call (every block, every apply, the
argument's type through `struct_fields`) also finds nothing: the argument's type cell does
not resolve to field names at check time (`names=None` for every apply that is not a
`.I/.O` struct), so the write's struct is not identifiable that way. The frozen write
body's reachable graph has **no operators at all** (`op=None` at every node), so the write
is not recoverable from the artifact's nodes either.
