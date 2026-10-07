# Unify without forcing: can the pending/force/commit machinery go?

> Status: **proposed** — the design question only.  Nothing here is built: the
> `force`/`defer`/`pin` mechanisms it asks about are live on `dev` and this note
> records what removing them would cost and which parts of the question are
> already answered by measurement.  §2.5 is the strongest of those: the
> "unify unconditionally, reconcile when the operator finishes" form was built
> twice, the second time with reads that always answer and trigger the
> computation, and it lands on the compute boundary.  What *did* land out of the
> same review is the run-state half — see §4.
>
> Companions: [eval-before-unify](eval-before-unify.md) (the same unifier seen
> from the staleness side, and where the *other* wakeup design — a class-
> attached blocked list — is sketched), [defer-pending-type-forms](defer-pending-type-forms.md)
> (the deferral as a *fix*, i.e. the case it exists for),
> [type-system-cleanup-plan](type-system-cleanup-plan.md) §4 (D1: the deferral
> policy is the program's hook, the lowlevel is untyped).

## 1. The question

Today the unifier **computes before it compares**: `unify_inner` forces a
pending side (`force_pending`), and when forcing is impossible it either
resolves the read into a plain reference (`alias_index`), or asks the
program's policy to merge and then **commits** a value onto the pending
computation (`defer_pending` → `pin_committed_value`).  The proposal:

> Unify a node **whether or not it carries an operation**, and decide whether an
> operation runs from **"has it run"** rather than **"does it hold a value"** —
> then delete the pending, force and commit mechanisms.

The two halves are independent, and only the second is cheap.  §2 is the part
that is not cheap; §3 is why the first half is not a *substitute* for the
second.

## 2. What "unify regardless of operation" costs

### 2.1 The binding fast path refuses by construction

[`unify_inner`](../crates/lichen-lowlevel/src/equality.rs) binds at exactly one
site, gated on `class_is_pure_cell`: *"A class with an unevaluated operation is
not bindable — it is a pending computation, and a concrete value bound over it
would erase the computation."*  A node is either value-carrying or
operation-carrying, never both (the invariant `alias_index` must honour when it
degrades a read into a plain reference by clearing the operation), so writing
the value **is** deleting the operation.  The operation is also the only edge
back to the applied parameter, and the clone walk deliberately keeps an
operation node's operand chain and drops its cached value so the computation
re-runs against each call's argument.  Erasing it loses the per-apply check.

So "unify regardless of operation" cannot mean "bind the value over the
operation".  It can only mean "merge the classes and leave both computations
alone" — which is where the semantics get hard.

### 2.2 A class would hold more than one operation

Every mechanism above exists to keep a class's answer **eager and single**.  Not
forcing means a merged class can carry several operations, and then these have
to be answered:

| question | who answers it today |
|---|---|
| two pending operations in one class — whose value counts? what if they disagree? | nobody: `force_pending` resolves before the merge, `alias_index` removes the read, `defer`+pin commits one value |
| one operation already computed, another has not — which is authoritative? (invalidate upstream caches and low types?) | `force_pending`'s reconcile against the class's committed value |
| what does a lazy reader (the `Parameterized` resolution path, `TableGet`'s undecided branch) see? | the class's committed value, via `class_committed_value` |
| how do snapshot consumers (the deep pass's `undecided` verdict, freeze, the codec) tell a promised value from a computed one? | they cannot: they read `node.value` |

`reconcile_value` **is** the answer to the first two — a class's committed
value is a *constraint* the computation must satisfy, compared when the
computation runs.  That is the shape this design wants; the difference is only
**where** it lives (today: inside `force_pending`, i.e. at unify time).

### 2.3 Not forcing means the class's value cannot be eager

If unify stops forcing, a reader of the class's value must either observe "not
computed yet" or compute it on the spot — and the latter *is* `force`, moved
from unify time to read time.  So the proposal is not "delete force" but "make
force lazy", and it is not local: the deep pass, freeze/solve, the codec, the
table's key hash and `lichen-compute` all read `node.value` as an eager
authority.

**Measured sensitivity.**  Two probes on `dev`:
- Reordering the apply parameter check to unify *before* evaluating the argument
  (not even removing anything) makes a genuinely wrong call report
  `expected Int, found Int` instead of `expected Int, found string`: with the
  argument's type still undecided, the unify binds rather than compares, so the
  parameter check loses its subject.  All three suites stay green — the
  behaviour this pins has no test.
- Disabling the deferral policy entirely breaks `P = ins => struct<.I ins.x>`
  applied to a placeholder (`expected raw[raw[?a, ?b], raw[?c, ?d]], found Type`)
  and 58 of 62 `lichen-language` compute tests: the deferral is what commits a
  type value onto an undecided type read that the compute boundary forces
  *before* any apply.

### 2.4 The alternative that already has a name

A class-attached **blocked list** — operations that evaluated to
`Parameterized`, drained and re-attempted when the class commits — is
[eval-before-unify](eval-before-unify.md) §5.1's sketch.  It is the honest form
of "do not force, retry later", and it carries the three problems that note
lists (re-entrancy of a drain inside a bind; GC versus a waiter nothing else
references; a template's parameters never bind, so its list must not fire).
Choosing between the two is choosing between *commit early and reconcile* and
*retry later*; the repository implements the first.

### 2.5 The obvious middle — a claim, reconciled on completion — was built, and the wall moved

There is a shape between the two that looks like it should work, and it was
**built and measured twice**, in throwaway worktrees (both discarded).  The
design: let the unifier always settle an operation node by writing the other
side's value as a **claim** (the value is kept for readers, the operation is
kept so its operand edge still reaches an applied parameter), let a read of a
claimed node **trigger** its computation, and give the operator one
reconciliation point where its outcome is checked against the claim.  That is
the proposal's literal form — unify unconditionally, reconcile when the operator
finishes.

**Attempt 1 — re-running a claimed node mid-flight.**  The evaluator was changed
to fall through a claimed node's cached value and re-run its operation.  A
claimed node whose computation is already running is then re-entered from inside
that computation, which the visit mark cannot tell from a cyclic read:
`run examples/` dies with `unreachable!("cycle detected: node … is being
evaluated")`.  An operation node's slot was, until then, never readable while
its own computation ran, so nothing re-entered it.

**Attempt 2 — a value is always readable; the read triggers the computation.**
The rule was made explicit: a node with a value answers with it, whether or not
its computation is in flight, and a claimed node additionally triggers that
computation (bounded by the visit mark, so recursion terminates).  This is a
semantic choice, and it **fixed attempt 1's deadlock**: the mid-flight reader
gets the claim, exactly as asked.

It then broke exactly one thing, and it is the boundary this note predicted
rather than a new one:

| suite / program set | with the claim semantics |
|---|---|
| `lichen-lowlevel` | 155 of 156 (the one failure is a *test of the deferral* whose conflict the claim path no longer resurfaced; fixed by also claiming the class's committed value at bind time) |
| `run examples/` | every program runs except `compute_jit.lichen` |
| `lichen-language` — everything but compute | green |
| `lichen-language` **`tests/compute.rs`** | **9 passed, 53 failed** |

So the wall is the compute extension's template walk, as in §2.3 and as the
parked `feature/read-kind-unify` measured for the static pin: once a claim can
reach it, a template's parameter type reads as settled instead of deferrable, and
the extension has nothing to force through.  Unconditional unify therefore does
not have a sound target to write into *until the compute boundary is answered* —
and that is a redesign of how the extension reads a template's types, not a
change to the unifier.

**What is *not* a win, and was checked rather than assumed.**  Lifting the
"never readable mid-flight" restriction on its own (without claims) changes
nothing: an operation node's slot is not written until its postlude caches a
decided answer, so `is_unbound` and "has a value" already agree for it.  The
restriction was a consequence of the value slot's discipline, never a separate
rule — which is also why the pin can keep it.

**One method note, because it cost a wrong measurement here.**  Two throwaway
changes in the same afternoon both shared `CARGO_TARGET_DIR` with the main tree
at different times; cargo then served the *other* tree's artifact, and a run
that looked green was the unmodified baseline.  Isolate the target directory per
tree, and treat a suspiciously fast `Finished` as a red flag.


## 3. Why the run-state half is not a substitute

The proposal's second half is about **representation**, and it does not touch
§2.  Naming the run state (what landed: §4) removes a conflation, not a
mechanism: the unifier still has to decide whether a pending computation can be
compared, and that decision is `force`/`defer`/`pin`, whatever the state is
called.

## 4. What landed from this review

The **run axis** is now named rather than inferred from the value slot: it is the
`runned` field beside the value, read through `Module::has_no_result_yet`
(`crates/lichen-lowlevel/src/static_module.rs`) — which is also the evaluator's
own run gate, so the named query and the decision it feeds are one definition.
The class scans that asked "has this computation run?" through `is_unbound` are
gone; of them only `class_committed_value` survives.

This section first recorded the representation as deliberately **derived** from
the value slot, arguing that a stored `state` beside `value` would have to be
updated at every write site (including the ones that wrote `Parameterized` on
purpose) and would be a dual bookkeeping of one fact.  The measurement below is
what broke that argument: what the class channel needs is *an answer this
operator produced* versus *a value a unification asserted into its slot*, and the
slot cannot tell the two apart (`class-channel.md` §1.1).  The field is that
distinction; the derivation and its predicates were deleted
([lowlevel-low-types](lowlevel-low-types.md) §7).  What the slot still decides is
whether a *read* runs the operator — an empty slot is re-read, which is how a
later binding is observed.

One measured finding from building it: an operation node **can compute** the
undecided marker — a probe watched `Eq` over an undecided operand do exactly that
— and what the evaluator declines is only to **cache** it.  So the invariant is
about caching, not about computing, and it is recorded where it is enforced
(`evaluate_node_operation`'s postlude).

## 5. If it is ever taken on

Order, so the risk stays auditable:

1. Make the run state explicit and total over the evaluator's decision points
   (done; §4), and grow a regression test for the parameter check's subject
   (§2.3's first probe) — that behaviour currently has none.
2. Define "a class holds several operations": propose *the operation's own
   computed value is authoritative; a committed value is a constraint
   reconciled when it runs*, and write down what a disagreeing pair does.
3. Only then remove `force_pending`/`defer`/`pin`, one mechanism per commit,
   re-measuring the compute extension (`crates/lichen-language/tests/compute.rs`,
   62/0 on `dev`) at each step.
