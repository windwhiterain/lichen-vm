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

1. `reenter_recording` (`crates/lichen-compute/src/compute.rs:5369-5382`) deep-evaluates the apply, and when
   the returned value is not nameable it calls `Module::reopen_stalled(block)` and evaluates again, up to
   `RECORDING_ROUNDS`.
2. `reopen_stalled` (`crates/lichen-lowlevel/src/module.rs:368-397`) clears the run stamp of every operation
   under the block that ran and could not decide. Its invariant: only an operation whose value slot is empty is
   touched, so an answer is never discarded.
3. The second round therefore resolves the reads that stalled in the first, and the deep walk **continues past
   the point where it previously stopped**. Operators beyond that point run for the first time, and each
   dispatch appends its algebra role to the producer's claim.
4. The recording, however, is opened once around **all** the rounds (`graph::begin` before the loop,
   `graph::finish` after it), so the appended dispatches of every round land in one graph. That is the defect:
   `graph::is_recording`'s invariant says *"one graph is built by one evaluation"*
   (`crates/lichen-compute/src/compute/graph.rs:194-197`), and a body walked twice is two evaluations' worth of
   dispatches.

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
  (`recordings.borrow_mut().push(...)`), so `begin`/`abandon` inside `reenter_recording` is on the right thread
  by construction.
- The re-drive loop is `reenter_recording` (`compute.rs:5369-5382`); `RECORDING_ROUNDS` bounds it and
  `returned_value_ids` decides whether a round succeeded.
- `reopen_stalled` is lowlevel and is **not** the thing to change: it is what makes a re-drive possible at all,
  and its stamp discipline is pinned by the lowlevel's own tests.
- The other two compute reds of this class are `graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture`
  (a capture rule that stopped firing) and `graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches`
  (a test premise: under a plain run gate the host run performs no dispatch, so plain and recorded cannot
  agree). Only the first is fixed by this note's proposal.
