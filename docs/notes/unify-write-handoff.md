# Handoff: one `unify`, and the write rules are member-local

> Status: **landed.**  §1's "not landed" half is in: the write path is
> member-local and unconditional.  What the four corrections cost, and the one
> thing this does *not* settle (`unify_inner`'s `class_committed_value` scan),
> are recorded in [class-channel](class-channel.md) §1.1 — read that for the
> state of the design; this note keeps the task as it was handed over.
> Corrections made against §4's sketch are marked **[corrected]** below.
> Companions: [class-channel](class-channel.md) §1.1 (the decided rules and the
> refutation), §2 (the measured mixture of values one class holds),
> [eval-before-unify](eval-before-unify.md) §5.1 (`replicate_class_value`'s
> origin).

## 1. What is being built

One unification, no variants.  `unify` is recursive; writing a value, reporting a
conflict and merging classes are one recursion, not a comparison path beside a
write path.  **Landed**: `union_with_value` — the separate "undecided side" merge
with its agree-then-copy — is deleted (`fd92bef`), and `unify_inner` answers in a
single match over the two classes' values.  **Landed after this note**: the write
path no longer skips a member that already holds a value
(`replicate_class_value`'s `is_unbound` filter is gone; the walk is
`propagate_class_value`), and `write_node_value` writes its own slot
unconditionally.  What a disagreement means is the correction this note got
wrong: it is the **tolerated** case, not a report (`class-channel.md` §1.1,
correction 1–2).

## 2. The rules, as decided by the author

1. **Two nodes**: unify normally; two nodes already in one class exit fast.
2. **Propagation inside a class is member-local**: a member that knows nothing
   takes the value; a member that already knows one has `existing ⊔ propagated` —
   a *value-to-value* unification, which has no class to propagate into, so
   propagation cannot trigger propagation again.
3. **An operation's result meets the node's own value**: unify them; a class's
   value is never overwritten.
4. **The one veto**: a *member* that bears an operation is skipped — its own
   computation settles it, and a value arriving from elsewhere is not a proof of
   what that computation will produce.

There is no separate write policy: the arms decide whether a write happens.

## 3. What is measured

| change | lowlevel | highlevel | compute | pipeline | compute time |
|---|---|---|---|---|---|
| before `fd92bef` | 155 | 87 | 60/62 | 137/140 | 2.8s |
| `fd92bef` (landed) | 155 | 87 | 60/62 | 137/140 | **5.5s** |
| write path, class-level read (×2) | — | **34/87** | **4/61** | — | — |
| write path, member-local (landed) | 155 | 87 | 60/62 | 137/140 | 2.8s |
| class value carrier, O(1) read (landed) | 155 | 87 | 60/62 | 137/140 | 2.7s |

- The three `pipeline` failures are **not** this work: they pass on `dev`
  (`26c3cb5`) and already fail at `d5259ee`, i.e. inside this branch's earliest
  commits.  They are a separate, older regression.
- The two failed attempts have an **identical** signature (the same 53 names), so
  the cause is one thing: they read the *class's* value and unified the incoming
  value against it.  The rules are member-local, and a class here routinely holds
  **different** values on different members (a term pair on one, the resolved
  value on another, a type cell on a third — `class-channel.md` §2 measured that
  mixture), so a class-level read turns every ordinary write into a conflict.
- `fd92bef`'s cost is the read it introduced: `class_committed_value` **scans the
  class's members** on every `unify_inner` call, including every array element.
  That scan was *load-bearing* in the merge path — it stands in for the deleted
  guard's member-aware read — so it cannot simply be swapped back for the O(1)
  representative read without losing the conflict check for member-held values.
  **[landed, O(1)]** It is now a carrier on the representative: the class records
  *which* member holds its value, so the read is one `find` plus one field read.
  The value is not moved onto the representative because a representative bearing
  an operation is never written (the veto), so it often could not land there; see
  `class-channel.md` §1.1.  What remains open is the **write walk**, which is
  O(members) per write by nature.

## 4. Next step, precisely

**This section is the sketch as handed over; the landing corrected four of its
answers (marked below).**  It is kept so the next reader can see what the rules
were expected to mean before the measurements said otherwise.

Keep `fd92bef`'s unified match.  Make the write path member-local:

- `write_node_value(node, value)`: write the node's own slot (its own answer);
  consult **the node's own** previous value, not the class's (rule 3) — unify and
  report on difference; then hand the value to the class for distribution.
  **[corrected]** write the slot unconditionally and hand the value to the class:
  the comparison is the class's per-member question, and a disagreement is not a
  report (corrections 1–2).
- `replicate_class_value` → a per-member loop with no `is_unbound` skip: no value
  → take it; has a value → `unify_values(member, held, value)` (rule 2); an
  operation-bearing member → `continue` (the veto).
  **[corrected]** landed as `propagate_class_value`, and the value-to-value
  question is `reconcile_held_value(held, incoming)` — a member that disagrees is
  **left at its own value**, not reported.  The loop must visit the
  representative too: the write site wrote its own node, which need not be the
  representative (correction 4).
- `unify_values(member, a, b) -> bool`: the one value-to-value unification.
  Arrays descend through their elements' **nodes** (normal `unify` resumes there);
  functions compare by identity; leaves agree or are reported on `member`.  A free
  cell is a wildcard.  It writes nothing, which is why it cannot cascade.
  **[corrected]** landed as `unify_values`/`reconcile_held_value`: arrays descend
  through element nodes but are **compared, not unified** (nothing here writes),
  and the descent carries the element-node path plus a depth bound — a `[cell,
  self]` term pair repeats the *same* node pair at every level, so a depth bound
  alone reports a conflict on a value that agrees with itself (correction 3).
- Only after that is the O(1) question well-posed.  The two candidate answers,
  neither measured: **(a)** let the representative's slot carry the class's own
  value as a maintained cache, written at the write sites, so the recursion reads
  O(1) while members keep their own values (compatible with member-local rules);
  **(b)** keep the scan but hoist it out of the element recursion.  (a) is the one
  that matches "one value, one class" without contradicting member-local
  propagation; `disjoint::rebuild` (used by `Module::flatten_class`) is the
  primitive if a class must be re-rooted at a value carrier, and the class's
  `low_shape` lives on the representative, so re-rooting has to carry it.
  **[open]** the write rule cannot reach this: the scan is `unify_inner`'s
  (`class-channel.md` §1.1).

## 5. Pointers

`crates/lichen-lowlevel/src/equality.rs`, as landed — `add_equality` 80,
`class_value` 107, `write_node_value` 310, `propagate_class_value` 342,
`unify_values` 387, `reconcile_held_value` 455, `unify` 495, `unify_inner` 700,
`value_matches` 875, `class_committed_node` 1184.  `values_agree` 936 and
`Module::value_eq` 861 are now **dead** (the compiler reports them; nothing calls
them since the write path stopped reading a class) — delete them with the item
that removes the rest of the pair, rather than leaving a second path beside
`unify_values`.
`crates/lichen-utils/src/disjoint.rs` — `union` 143 (representative by class
size), `rebuild` 184 (re-root at a chosen member).
Docs synced with the landing: `class-channel.md` §1.1 (the four corrections,
which are the part to read first), `code-audit.md` P4-2,
`incremental-evaluation.md` rows 6 and the predicate list,
`eval-before-unify.md` §5.1 (the walk's new name and rule).

## 6. Discipline that cost real time here

- **Never run cargo in the foreground while a background job holds the target
  directory** — `link.exe` LNK1104 killed a whole measurement run.
- **Background means start async and keep working.**  Following it with
  `job_output(wait: true)` makes it foreground and buys nothing.
- **Docs are part of the change, not a follow-up.**  The write rules sat
  undocumented across two commits, and the design was then mis-attributed to
  documents that do not state it — the rules exist only in the author's messages.
- **Do not revert on first failure.**  Reverting twice destroyed the same work
  twice and hid the fact that both failures had one cause; the second attempt
  should have been the correction of the first.
- **A failing test that the checker's diagnostics own is a reporting bug, not a
  semantic one.**  The landing's first regression was four `unify_errors` entries
  raised *inside* a `try_unify` and therefore counted as that unify's failures —
  `try_unify` answers with the range `before..now`, so anything that appends
  during a unify is attributed to it.  Reading the range's consumer
  (`checker/diagnostics.rs`) before touching the semantics is what located it.
- **Measure the baseline with the same command before believing "pre-existing".**
  `--test compute` is a `lichen-language` test binary (60 of 62), not
  `cargo test -p lichen-compute` (19) — and the second regression was found only
  by running the real binary against `aeb0c8d` and the branch side by side.  A
  test name seen failing is not evidence that it failed before.
- **An optimization nobody asked for is a regression you will have to bisect.**
  The fourth correction (skipping the representative) was invented during the
  landing; it took a compute case out and cost three bisection rounds, one of
  which measured *stale source* because a `git checkout` had left the worktree
  detached.  Change one thing at a time, and confirm the tree is on the branch
  you think it is.
