# Handoff: one `unify`, and the write rules are member-local

> Status: **in progress.**  The tree is green at `925bbae` (docs) on top of
> `fd92bef` (the unified `unify_inner`).  Two attempts at the write path failed
> and were reverted twice; the author's instruction is to **stop reverting** and
> carry the change forward, so the next session should re-apply it — corrected —
> rather than start from scratch.
> Companions: [class-channel](class-channel.md) §1.1 (the decided rules and the
> refutation), §2 (the measured mixture of values one class holds),
> [eval-before-unify](eval-before-unify.md) §5.1 (`replicate_class_value`'s
> origin), [compute-kernel-struct](compute-kernel-struct.md) (stale: it still
> spells `.sig` and `$launch(native, sig, a)`).

## 1. What is being built

One unification, no variants.  `unify` is recursive; writing a value, reporting a
conflict and merging classes are one recursion, not a comparison path beside a
write path.  **Landed**: `union_with_value` — the separate "unbound side" merge
with its agree-then-copy — is deleted (`fd92bef`), and `unify_inner` answers in a
single match over the two classes' values.  **Not landed**: the write path still
skips a member that already holds a value (`replicate_class_value`'s `is_unbound`
filter) and still overwrites its own target without comparing
(`write_node_value`).

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
  That scan is *load-bearing* in the merge path — it stands in for the deleted
  guard's member-aware read — so it cannot simply be swapped back for the O(1)
  representative read without losing the conflict check for member-held values.

## 4. Next step, precisely

Keep `fd92bef`'s unified match.  Make the write path member-local:

- `write_node_value(node, value)`: write the node's own slot (its own answer);
  consult **the node's own** previous value, not the class's (rule 3) — unify and
  report on difference; then hand the value to the class for distribution.
- `replicate_class_value` → a per-member loop with no `is_unbound` skip: no value
  → take it; has a value → `unify_values(member, held, value)` (rule 2); an
  operation-bearing member → `continue` (the veto).
- `unify_values(member, a, b) -> bool`: the one value-to-value unification.
  Arrays descend through their elements' **nodes** (normal `unify` resumes there);
  functions compare by identity; leaves agree or are reported on `member`.  A free
  cell is a wildcard.  It writes nothing, which is why it cannot cascade.
- Only after that is the O(1) question well-posed.  The two candidate answers,
  neither measured: **(a)** let the representative's slot carry the class's own
  value as a maintained cache, written at the write sites, so the recursion reads
  O(1) while members keep their own values (compatible with member-local rules);
  **(b)** keep the scan but hoist it out of the element recursion.  (a) is the one
  that matches "one value, one class" without contradicting member-local
  propagation; `disjoint::rebuild` (used by `Module::flatten_class`) is the
  primitive if a class must be re-rooted at a value carrier, and the class's
  `low_shape` lives on the representative, so re-rooting has to carry it.

## 5. Pointers

`crates/lichen-lowlevel/src/equality.rs` — `unify` 372, `unify_inner` 577,
`add_equality` 80, `write_node_value` 301, `replicate_class_value` 328,
`class_value` 107, `class_committed_value` 1170, `class_committed_node` 1161,
`values_agree` 913, `value_matches` 852, `reconcile_node_claim` ~931,
`UnifyError` construction for a standalone report ~931.
`crates/lichen-utils/src/disjoint.rs` — `union` 143 (representative by class
size), `rebuild` 184 (re-root at a chosen member).
Docs to sync when the write lands: `code-audit.md` P4-2,
`incremental-evaluation.md` row 6, `compute-kernel-struct.md` (already stale).

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
