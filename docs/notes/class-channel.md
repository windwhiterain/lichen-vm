# One channel for a class's value and its low type

> Status: **§1.1 is the author's decision on the write rule, and it is landed** —
> unconditional write, the operation-bearing member as the one veto, an
> already-valued member compared by the unify recursion and left at its own value
> when the two cannot be one.  It also records the half that landed before it:
> `unify_inner` is one recursion and `union_with_value` is gone.
> **§2 refuted by measurement**, and **§3 has no receiver on the run side**
> (§3 below, two independent blockers).  What is left is §5, which therefore comes
> first: it is the only place a result's type can be stated, and the two red
> targets the operator-polymorphism branch recorded
> (`operator-polymorphism.md` §8.8) are its acceptance.  §4 then lands on top of
> it.  §1 is the incoherence as first read; §2 and §3 record what measurement says
> that first reading got wrong.
> The remaining halves are carried by the branches the body names:
> `feature/unconditional-class-writes` (the §1.1 write rule, merged as `09b4640`) and
> `feature/class-value-on-representative`.
> §5.1 (the class a lowering runs in) and §5.3 (the wrapper's `.I`/`.O` cells
> carrying its signature) are landed.  The remaining half — §5.2's statement and
> §5.4's carrier —
> is the **open class** itself, which is the operator-polymorphism workstream's to
> decide ([operator-polymorphism](operator-polymorphism.md) §8.4 and §5: the class
> domain is that workstream's value, and its reader is what commits to a member).
> §4, the struct-argument migration, is the carrier for the element cell the two red
> targets want.
> **Then the operator routing landed** (§5.1.1): the operator *is* the prelude's
> binding, so a kernel body's `+` is an apply of a frozen function.  The kernel
> side of that — the static operands and the one class reader — is landed and
> measured (12 → 58 of 60 in `--test compute`), and **§5.1 is superseded by the
> requirement that a kernel function be explicitly specialized**: the class a
> lowering runs in is the author's statement (the parameter's type), never a
> domain the compiler reads or a default it invents.  What remains is the
> cross-kernel/`launch` argument shape §6 names.
> Companions: [compute-runtime-scalars](compute-runtime-scalars.md) (the measured
> case that exposed this — this note is the fix),
> [lowlevel-low-types](lowlevel-low-types.md) (the seed → pass → read chain),
> [eval-before-unify](eval-before-unify.md) §3 (the deferral whose side
> effect is being relied on), [checker-encoding-instability](checker-encoding-instability.md)
> (the same "decide it where it is decided" principle one layer up),
> [kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6 (the
> compiler-side "specialize before JIT" direction §5 here **supersedes**: the
> author specializes, and §6's placeholder apply is the rejected alternative).

## 1. The incoherence

A class's value and low type have **one authority in the lowlevel and a second
reading in the highlevel**, and the second one is what the checker and the
compute plugin ask.

The lowlevel is class-routed, and says so:

| Site | What it states |
|---|---|
| `Module::class_value` (`equality.rs:77-87`) | "the class's value, read **through its representative** — the value the unification machinery sees" |
| `Module::class_low_type` / `low_type_of_node` (`equality.rs:89-113`) | "read through its representative … so a read **never depends on which member of the class resolved first**" |
| `Module::write_node_value` (`equality.rs:310-322`) | "the **single choke-point** for value writes"; it propagates the value over the class's operation-free members and calls `observe_class_low_type` — one of the low-type layer's **two observation sites** (`Module::add_node` is the other) |
| `Module::seed_class_low_type` (`equality.rs:115-125`) | "a layer above the lowlevel **that has the type** calls this" — the write half of the same channel |

The highlevel reads the **node's own slot** for the same questions:

| Site | Reads |
|---|---|
| `shape::low_type_of_slot` (`shape.rs:1092-1111`) | the node's own value and structure; never `class_low_type`/`low_type_of_node` |
| `compute::node_class` (`compute.rs:4302`) | the node's own value, then `low_type_of_node` |
| `checker::names_float_class` (`operators.rs:137`) | rides on `low_type_of_slot` |

That split is what makes today's behaviour accidental rather than stated:

- **A run-decided class reaches the type graph only as a side effect of a
  check-time deferral.** `defer_pending` (`shape.rs:471-482`) fires only when one
  side is a *pending* `Index`/`Apply` and the other holds a type; on `Merge` the
  lowlevel commits the class's value (`equality.rs:554-558`, `pin_committed_value`
  at `:884-903`). Measured: a single-kernel `collect` of a `plrun` result prints
  `array<?a, ?b>` (nothing was ever committed), while the GPU chain test prints
  `array<Int, ?d>` — because the consumer's **array** argument supplied the pending
  read that triggered the commit. The same test with a **tuple or struct** argument
  prints `array<?d, ?e>`: no pending read, no commit.
- **Which node a reader asks decides what it sees.** `pin_committed_value` writes
  to the representative (and to the class's pending ops, for the apply cloner's
  sake), so a member that is neither reads nothing — an allocation-order accident,
  exactly what `class_low_type`'s doc says must not matter.

The point of the plan: a fact is decided once, the class is where it lives, and
every reader asks the class.  §2 is where that second clause is measured and
narrowed: a fact *about a value's class* does live on the class, and a fact about
a *type* does not.

## 1.1 The write rule — decided

The write side is **one rule**, and the replication below is a deviation from it
rather than a second case:

- **A unification is unconditional: it must write.**  A write is not gated on the
  slot being undecided — "this member already knows something" is not a reason to
  skip it.  It reaches **every** member, operation-bearing ones included.
- **The run axis, not a veto, keeps a computation's own answer readable.**  The
  operation-bearing member does receive the class's value in its slot, and
  `runned` — which a propagated value never sets — is what keeps "this operator
  produced this" distinct from "this is asserted of it"
  ([`crates/lichen-lowlevel/src/evaluation.rs`]).  An earlier version of this
  rule made the operation-bearing member a *veto* instead; §1.1.3 measures why
  that veto is not the protection, and the guard on the evaluator's cached-value
  arm is.
- **A member that already holds a value is compared**, and the comparison is the
  unify recursion — a structure's elements are nodes, so comparing binds the
  cells inside it.  Two values that cannot be one value are **reported**.
- **A merge is not a write** (§1.1.3): it carries the class's value to the
  members that hold nothing and leaves every member that holds something with
  what it holds.

So "unify, report, merge" are one recursion and there is no variant of it, on the
class's own members or between classes.

**Landed** (`fd92bef`): `unify_inner` is that one recursion — it reads what each
class knows once, and answers in a single match (nothing known is a merge, one
side known is a merge that carries the value, both known is a reconciliation:
arrays by recursing their elements, functions by identity, otherwise by value
equality).  `union_with_value` and its agree-then-copy are deleted, and the
comparison it asked has no client left: the compiler reports `value_eq` unused.
`values_agree`/`reconcile_value`/`reconcile_node` are *not* orphaned by it — they
are still reached from the value-versus-class half (`value_matches`,
`reconcile_computed`), which is the half rules two and three below turn into the
same unification.

**Measured when landed**: lowlevel 155, highlevel 87, `compute` 60 of 62,
`pipeline` 137 of 140 — every number identical to before, so the merge carried no
behaviour of its own.

**The write rule landed.**  Its statements in this note are now true:
`propagate_class_value` asks every member of the class, and `write_node_value`
writes its node's slot unconditionally.  Read the corrections below before
touching it — three of the four were mistakes a later session would repeat.
`write_node_value`'s distribution half is `propagate_class_value`, and its
comparison is the write rule's own question; the class's *value* is read through
the representative (`class_value`) — what is member-local is the write rule's
test, not the read API.  A **merge** is a separate rule, and §1.1.3 is where it
is stated and measured.

### 1.1.1 The invariant this rule exists to keep

**A class holds one value; a node's own answer is a separate, per-node fact.**
Both are needed, and the rule above is what keeps them apart:

- an operator's answer is written into **its own slot** by
  `write_node_answer` (`crates/lichen-lowlevel/src/equality.rs`), and is
  reconciled against the class's value through the one unification;
- the class's value reaches the members only through
  `propagate_class_value`, and that walk reaches **every** member — an
  operation-bearing one included, which is what makes the next bullet load
  bearing rather than decorative;
- `runned` therefore means exactly *this operator produced the answer in this
  slot* — a propagated value never sets it — which is what
  [`Module::has_no_result_yet`] reads, what stops a second run, and what lets
  the evaluator tell an **asserted** value from a **produced** one.

**Both halves must stay separate, and each has a reader.**  The class value is
read by the unifier (`class_committed_value`) and by the class channel's
consumers; the node's own answer is read by the evaluator's
`if let Some(value) = node.value` arm.  That arm **still runs the operator** when
the slot holds a value this operator did not produce
(`evaluate_node`'s `has_no_result_yet(node) && !visiting` guard, whose `runned`
term is exactly that distinction), so an asserted value cannot silence the
computation that owes an answer: the operator runs, its answer is reconciled, and
`runned` becomes true.

An earlier version of this section made the operation-bearing member a **veto**
in `propagate_class_value` instead, and argued from a `runned`-less evaluator.
§1.1.3 measures what actually broke when the veto was removed — it was not this.

The two red cases this section listed are **fixed**; §1.1.3 is the measurement
and the rule that fixes them.  Kept here as the record of what the third attempt
measured at the time:

| suite at the time | result |
|---|---|
| `--test basic` + lib (`lichen-lowlevel`) | **155 of 155**, unchanged |
| `--test checker` and the small targets (`lichen-highlevel`) | **87 of 87**, unchanged |
| `lichen-language --test examples` | **1 failed**: `examples/import/_.lichen` declares `(42, 10, 7)`, prints `(42, none, none)` |

Instrumented, one hypothesis at a time: a write that replaces an operation's
decided slot with an undecided value (**never fired**), a member write that leaves
the representative undecided (**never fired**), a merge that drops the class value
(a `debug_assert_eq!` on `class_committed_value(representative)`, **held all
suite**).  What the trace did show is a count of the writes the veto was blocking:
**4069 propagation writes change an operation member's slot from `None` to the
class's value**, e.g. `class 17v1 value USize(5)` into operation member `63v1`.

Those three refutations are correct, and none of them is the defect.  The
comparison was made against the **wrong partner**: every one of them asks about
the *write* (`write_node_value`) and the *read* (`class_committed_value`), while
the loss happened at the **merge**, which was the experiment's third write site
and the only one that could overwrite a member's own value.  §1.1.3 is that
measurement.  A second step kept the producing operator's own answer while still
distributing to everyone (`write_node_answer` restores the node's slot after the
walk); that change is still in the code and is still right — the producer's slot
is what stops a second run.

### 1.1.2 Done: `LowValue::Parameterized` is deleted

**Completed on this branch, left uncommitted for review.**  *Undecided* now has
exactly one representation on each side of the boundary: the **empty node
slot** (`Node::value: Option<P::Value>`) inside the VM, and **`None`** from an
operator that cannot decide.  The variant is gone, and with it its `PartialEq`
arm, the table's `KeyState` arm, the printers' spelling, and `is_unbound` —
every former caller reads the slot directly.

Codec tag `4` is **reserved**, not reused: a reader that meets it refuses the
artifact by name, saying it was written by a version that still had the marker.

Two further cases are parked by a **different** feature, and are listed here
only so the workspace's parked set has one place to be read from:

| parked case | kind |
|---|---|
| `compute::wrapper_functions_render_with_named_type_variables` | **not this feature's own** — a written arrow in a *frozen* module collapses the signature onto the `Function` marker, so an unapplied wrapper no longer renders as the open `?a` the test states it must be.  **Do not re-pin it**: the test's own comment says the wrapper stays generic, so re-pinning would record the defect as intended |
| `compute::a_float_domain_is_permitted_at_every_position_the_walk_reaches` | same cause, reached through a domain that contains a function — the walk refuses at the `jit` rather than at the launch argument, and the message asks the author to annotate a parameter the test already annotated.  That misleading message is a second defect in its own right |

Both come from [function-type-merge](function-type-merge.md), have nothing to do
with the marker, and each un-parks by deleting one `#[ignore]` once that note's
open mechanism is found.

The in-flight paths followed the operator seam: `evaluate_node` /
`evaluate_node_body` / `evaluate_node_operation`, **`evaluate_node_deep`** (and
the `evaluate_node_forced` entry point that stood beside it until it was deleted —
`code-audit.md`, the operand-arm follow-up), `with_apply_frame`, `wire_apply_result`, `function_apply`,
`apply_loop`, `StaticModule::read` / `static_read`, `static_function_apply`,
and `AttrExt::missing_value` (now `Option<LowValue>`, with the curated
`Ctx::fresh` supplying the empty cell when the absent form is an undecided one).

**Three judgement calls, each recorded where it is made.**

1. **A value-less, operation-less node reads as undecided (`None`), not as an
   error.**  It used to be unreadable (the postlude's `operation.unwrap()`),
   which was safe only because the marker made such a node unnecessary.  A
   fresh cell is exactly that node now, so `evaluate_node_body` answers `None`
   for it.
2. **Two sites stored the marker *in a slot* to mean "this side is
   undecided"** — `compute.rs`'s `build_graph` (the placeholder's type half)
   and `loop_run.rs`'s `loop_argument` (a fresh type cell).  Both now leave the
   slot **empty**, which is that meaning's only spelling.
3. **An undecided *root* has no value to print.**  `run::render_build` and the
   test harnesses that returned a value now substitute the printer's existing
   no-value reading (an empty element already rendered that way), so a program
   that stays undecided renders as `none` where it used to render as
   `parameterized`.  The only assertions that pinned that spelling are in the
   parked `statement_values` file.

**Measured**: lowlevel 155 of 155, highlevel 87 of 87, `--test pipeline` 137 of
140 (the same three parked), and `cargo test --workspace --no-fail-fast` green.

**The plan below is the record of how it was reasoned, kept as written.**

**The goal.** Delete [`LowValue::Parameterized`], so that *undecided* has exactly
one representation on each side of the boundary: the **empty node slot**
(`Node::value: Option<P::Value>`) inside the VM, and **`None`** from an operator
that cannot decide.  Today `None` (an empty slot) and `Some(Parameterized)` (a
marker in a slot) both mean undecided and `is_unbound`
(`crates/lichen-lowlevel/src/lib.rs:825`) exists only to span them; that is the
ambiguity every attempt in §1.1.1 kept tripping over.

**Why the variant cannot simply be deleted.** It is load-bearing as a *value*,
not merely as a slot state:

- `OperatorExt::run` (`crates/lichen-lowlevel/src/lib.rs:696`) returns `P::Value`,
  so an operator that finds its operand undecided has exactly one way to say so —
  it returns the marker.  `compute.rs` alone does this in about eighty places
  (every native operator that must propagate "not decided yet"), and
  `run_deferred` (`:723`) does it for the structural operators.
- The marker is also a **value in the graph**: it is written into slots, compared
  (`PartialEq`), hashed as key content (`table.rs`, `KeyState::Undecided`), and
  carries codec tag `4`.  A solution that only changes slots leaves all of those
  in place.

So the refactor is: **make the operator's return optional**, and let the VM own
the marker's meaning — `None` from an operator *is* "ran, undecided", and the
node's own slot stays empty.

**Three uses are not undecidedness**, and each needs a different answer.  Do not
replace these with `None`:

| use | site | what `None` would mean there |
|---|---|---|
| a nullary operator's no-operand stand-in | `run_deferred`'s `None` arm (`crates/lichen-lowlevel/src/lib.rs:756`) | wrong: the operand is *absent*, not undecided |
| a **lazy callee** — a callee that may become callable, so the apply must stay lazy | `LowOperator::Apply`'s marker-target arm (`crates/lichen-lowlevel/src/evaluation.rs:330`) | wrong: the read must stay lazy *and* the node must remain re-runnable |
| a **non-function target** refused by the language's own gate | the same guard's other arm | wrong: the refusal is the program's answer, not the VM's |

**Migration order**, each phase compiling and measured on its own:

1. **The trait and the union leaves.** `run` and `run_deferred` return
   `Option<P::Value>`; every implementation returns `Some(..)` unchanged and the
   marker arms return `None`.  `lowlevel`'s VM postlude turns `None` into "node
   marked run, needs no change to its own slot unless the written value is a
   `Parameterized`".  No behaviour change; the suites must be untouched.
2. **The compute extension.** Its ~80 marker constructions become `None`;
   its ~20 `matches!(_, Some(LowValue::Parameterized))` guards become `is_none()`
   — but only for the operand checks whose `None` arm then returns `None`.  This
   is the phase with judgement in it, and the one to measure after each crate.
3. **The codec and the value-level uses.** Drop tag `4` (keeping it reserved), the
   `PartialEq` arm, the table's key-state arm, and the printers' spellings.  An
   artifact that carries tag `4` must then be refused by version, not silently
   read as something else.
4. **Delete the variant**, and with it `is_unbound`'s two-case body — it becomes
   `value.is_none()`.

**What this does not settle.** The run axis stays: a node that returned `None`
has run and may not re-run until its operands change.  That is the fact §1.1.1
says the slot and `runned` must keep separately readable, and this refactor
preserves it — the marker stops being the carrier of that fact, the slot does.

**Measured while planning it, and it changes the plan.** The marker is not only
an operator's verdict: it is **copied into a clone's slot** by the two carry
paths, and those copies are load-bearing.

| finding | evidence |
|---|---|
| the marker enters a slot by exactly one path | instrumenting all three write sites (`write_node_value`, `write_node_answer`'s restore, `propagate_class_value`) over the whole example corpus: `write_node_value` fires 7542 times, the other two **never** |
| it is a *copy*, not a computation | the two callers that pass it are `function.rs`'s clone walk (`write_node_value(clone, mapped)`) and the static materializer (`static_module/apply.rs`, `write_node_value(clone, Some(value))`) — both carry a source's value to a clone |
| a marker never lands in a class that holds a value | a probe at the write path over the whole corpus: **0** cases where the class already held a decided value |
| refusing the copy is not available | making `write_node_value` drop an undecided value (leaving the slot empty, the honest copy) breaks `let_bound_functions_are_polymorphic` and `a_wrappers_parameter_type_is_inferred_from_a_body_call` in `--test checker`, and panics the example sweep at `evaluation.rs`'s operation unwrap — the clone then has no operation and no value where the walk expected the carried one |

**Superseded while planning.** The paragraph here claimed the clone's copied
marker blocks the change — that phase 2 could not replace it with `None` until
the clone walk was taught what an undecided source means.  **That was wrong, and
phase 1 measured it wrong.**  With the trait returning `Option`, a copied
undecided source is simply `None`: the carry still happens, only the thing
carried is nothing instead of a marker, and the clone runs its own operator
exactly as it did.  The migration is mechanical — the old logic is preserved by
treating the empty slot and the marker as one, which `is_unbound` already did
(`value.is_none_or(.. == Some(Parameterized))`).  The clone walk needed no
redesign; it needed the boundary moved, which is what phase 1 did.

**Four corrections were needed, and each says something about the rules.**

1. **A refusal is not a report.**  The first version refused the write when the
   member's held value disagreed.  That is not a refusal: the write is asserted,
   the member keeps its own value, and the mismatch is the **tolerated** case
   §2 measured.  Refusing destroys the value the class needs.
2. **A false report is worse than none.**  The first version also recorded every
   propagation disagreement in `unify_errors`.  `try_unify` answers with the
   range `before..now`, so a report raised inside a unify is *counted as that
   unify's failure*: four tolerant member-local disagreements became four phantom
   `Check` diagnostics on a program whose real error was one annotation failure
   (`pipeline`, `an_applied_struct_constructor_keeps_the_occurrence_identity`:
   1 expected → 5).  Propagation is silent; the checker's own comparison reports.
3. **A structural comparison cannot be bounded by depth alone.**  `[cell, self]`
   — the term pair a type is — repeats the *same node pair* at every level, so a
   depth bound (64) reports a conflict on a value that agrees with itself.
   `unify_values` carries the element-node path the old `reconcile_node` used,
   with the depth bound as the static case's guard only.
4. **The representative is not special.**  Skipping it in the walk looks like a
   free optimization — the write site just put the value there — and it is
   wrong: the write site wrote *its own* node, which need not be the
   representative (a class whose representative is a value-less operation node is
   what `add_equality` exists for), so the representative's slot is an ordinary
   member slot and must be asked the same question.  It cost the compute suite a
   case (`a_struct_parameter_kernel_runs_through_the_signature_carrying_wrapper`:
   the parallel parameter ended up declaring no inputs).

**Measured**: lowlevel 155, highlevel 87, `--test compute` 60 of 62 (one of the
two reds is `jit_cross_kernel_subexpr`, parked with an `#[ignore]` and its
reason), and `pipeline` 137 of 140 — every number identical to `dev`'s.

**What this does not settle.**  The write walk (`propagate_class_value`) is
O(members) per write — no index makes writing to *m* members O(1) — and it is the
remaining item.  The read is done: `class_committed_value` scanned the member
list (twice per `unify_inner` pair, every array element included), which is the
measured 2.8s → 5.5s step `fd92bef` introduced; the class's representative now
carries the member that holds the class's value, so the read is one `find` plus
one field read.

**Why the representative's own slot is not the carrier.**  The obvious shape —
"the class value *is* the representative's value" — cannot work: a
representative that bears an operation is never written (the veto), so the value
frequently has nowhere to land.  A debug assertion written during this work
named the case directly (`members [6, 7]`, representative `6` an operation
node), which is why the carrier is a *pointer* to the member that holds the
value rather than the value itself.

**Measured, and it refutes reading the rules at the class level** (two attempts,
identical signature: `--test checker` 34 of 87, `--test compute` 4 of 61).  Rules
two and three are **member-local**, not class-level: a member's existing value
against the propagated value, and an operation's result against *its own* node's
value.  Implementing them as "read the class's value and unify the incoming one
against it" reports a conflict on every ordinary write, because a class here
routinely holds *different* values on different members — a term pair on one, the
resolved value on another, a type cell on a third (§2 below measured exactly that
mixture).  The tolerated comparison this note's §2 relies on is what keeps those
ordinary writes from being conflicts.

That also settles the O(1) question's real shape: `class_committed_value`'s scan
was **load-bearing** in the merge path — it stood in for the deleted guard's
member-aware read — and it cannot be made O(1) by *moving* the value: a class's
value may sit on a member no write may touch (an operation-bearing
representative), so the class's representative records **which** member carries
it and the scan became one field read (§1.1).  The write walk stays O(class size)
per write, which is inherent to distributing a value over members.

**Re-measured, and the conclusion above is superseded** (branch
`feature/unconditional-class-writes`, worktree `.worktrees/unconditional-nodes`,
merged as `09b4640`).  A third attempt was made on the §1.1 decision itself:
`propagate_class_value` writes **every** member with no operation check, the
class value is kept in the representative's own slot, and `class_carrier` — with
`class_committed_node`, `commit_class_value`, `reselect_class_carrier`, and the
two `class_committed_node` readers in `highlevel::shape` and
`is_function_type_node` — is deleted.  The marks at the time:

| suite | result |
|---|---|
| `--test basic` + lib (`lichen-lowlevel`) | **155 of 155**, unchanged |
| `--test checker` and the rest (`lichen-highlevel`) | **87 of 87** plus the small targets, unchanged |
| `lichen-language --test examples` | **1 failed**: `examples/import/_.lichen` declares `(42, 10, 7)` and prints `(42, none, none)` |

The minimal reproduction is `(geo.double 5)` with `geo`/`math` imported (both
files from `examples/import/`): the value survives as `10`, but its **type cell
does not resolve** — it renders `raw 10: ?a` where the example declares
`10: Int`.  The two `none`s in the full example are the tuple's second and third
apply results, whose slots are empty at render time.

Instrumented rather than inferred, and each hypothesis refuted in turn: no
operation's decided slot is ever overwritten with an undecided value
(`write_node_value`), the class's value is never shadowed on the representative,
and the merge preserves the class value through the new representative
(a `debug_assert_eq!` on `class_committed_value(representative)` held for the
whole suite).

**All three of those hypotheses are true, and this passage's conclusion — "the
carrier is the third option, and it is the one that works" — was wrong.**  They
all ask about the class's *value*, and what breaks is the class's **structure**:
the merge, the experiment's third write site, overwrote a member's own value, and
the walk that later descends through the value graph took the other branch.
§1.1.3 states the rule and carries the measurement; with it, both reds this
passage lists are green and no carrier is needed.

#### Superseded: the scan stood in for the carrier

The paragraph above (before this re-measurement) read the O(1) question as
"`class_committed_value`'s scan was load-bearing in the merge path, so the
representative must record which member carries the value".  That is no longer
true: the merge does not need a class-level read of the merged class at all — it
reads both sides **before** the union, which is where their values still live on
the members they were written to.  What the representative's slot must be is the
class's value slot for *later* readers, and §1.1.3's rule keeps it that way
without a carrier.

### 1.1.3 A merge is not a write: it fills the members that hold nothing

**Landed**, and it un-parks both red cases the sections above had parked:
`pipeline::a_dependent_array_length_rejects_other_lengths` and
`examples/import/_.lichen`.

The experiment made `add_equality` distribute `left_value.or(right_value)` over
**every** member of the merged class.  The pre-experiment merge wrote **nothing**
there — it nominated the side that held a decided value as the class's carrier
and left every member's own slot as it was.  Deleting the carrier turned the
merge into a second, unconditional write site, and that is the whole of both
regressions.

**What was measured.**  `((n => ([1, 2, 3] : array<Int, n>)) 5)` must fail to
compile: the annotation pins the parameter `n` to 3, and applying 5 clashes at
the apply.  Green at `4be9180`, red from `03ed3e2` + `dde4011`.  One node dump at
the apply and one `evaluate_node` trace per tree (worktree
`.worktrees/green-4be9180`, instrumented identically):

| what | green | red |
|---|---|---|
| the parameter's own value cell (`17v1`) | a member of the class that holds `USize(3)` | alone, empty |
| that class's size | 3 | 2 |
| the annotation's length cell (`37v1`, an `Index` view) | **evaluated** | **never evaluated** |
| the cell the deep walk descends through (`40v1`) | its **own** array `[38v1, 39v1]` | the merged class's array `[32v1, 33v1]` |

The chain, every step read off a trace rather than argued:

1. the annotation's length position is a **view** (`37v1 = Index(36v1, 0)`); the
   parameter's cell joins the class that holds 3 through `alias_read`, which is
   what *evaluating the view* runs — so the pin exists only if the view runs;
2. the view runs when the deep pass descends into the array **item** that names
   it (`evaluate_node_deep_inner`'s item loop).  Nothing else evaluates it: an
   operation whose slot holds a value still runs (the `runned` guard), but a walk
   that never reaches the node asks nothing of it;
3. the array that walk descended through was `40v1`'s **own**, and merging
   `40v1`'s class with `34v1`'s overwrote it with the other side's array — so the
   item list it read changed from `[38v1, 39v1]` to `[32v1, 33v1]`;
4. the walk therefore never reached `37v1`, `alias_read` never ran, `17v1` never
   joined the class, the parameter stayed undecided, and the apply's
   `(None, Some(5))` arm **learned 5** instead of reporting the clash.

Two candidate causes were **refuted** on the way, and neither is this defect:
restoring the operation-bearing **veto** in `propagate_class_value` leaves the
test red — the overwritten member `40v1` has **no** operation, so no veto can
protect it; and reading the class's value by scanning the members instead of
through the representative also leaves it red — the value was not misplaced, the
structure was rewritten.

**The rule, and why it is a rule.**  A merge is the one place two decided sides
meet *without* their values being compared: the positional descent agrees they
**unify**, which is not their being one value — two type terms that unify still
name different nodes.  A class can therefore hold two values, on two members, and
the merge may not choose between them by overwriting, because the structure still
names the node it would erase.  So:

- a **write** states the class's value and reaches every member
  (`propagate_class_value`, unconditional);
- a **merge** carries the value to the members that hold nothing, and leaves
  every member that holds something with what it holds (`add_equality`).

**Measured after the change**: `pipeline` 138 of 140 (both remaining ignores
pre-existing and unrelated), `examples` green with `import/_.lichen` un-parked,
`lichen-lowlevel` 155 of 155, every `lichen-highlevel` target green, and
`lichen-language`'s whole test set green — **670 passed, 0 failed, 13 ignored**
across the three crates.

### 1.1.4 The read's join does not report: the reconcile owns the conflict

**Landed**, and it un-parks `highlevel::dependent::
a_concrete_type_is_never_bound_over_a_dependent_codomain`.

The case is the deferral working as designed: a dependent codomain
(`[0, 1][x]`) meets a concrete `1` while `x` is still undecided — unify does not
evaluate, so the class holds `1` and the computation is left owing an answer
(the §1.1.3 rule) — and when the read runs it selects the `0` branch.  That is
**one** disagreement.  It arrived as two, mirrored:

| record | roots | value_a | value_b | mechanism |
|---|---|---|---|---|
| the read's join | `(10v1, 1v1)` — reader, element | `USize(1)` | `USize(0)` | `alias_read`'s `unify(reader, target)` |
| the answer's reconcile | `(10v1, 10v1)` — the reader twice | `USize(0)` | `USize(1)` | `write_node_answer` against what the class held |

So a reader is shown `expected 1, found 0` **and** `expected 0, found 1` for one
conflict.  The mirrored shape is `pipeline`'s parked
`an_applied_struct_constructor_keeps_the_occurrence_identity`'s;
`function-type-merge.md` measured a *non*-mirrored double in the function arm
(one message, twice).  This is a **third** mechanism, and it is none of theirs:
both of those are still parked and neither moved when this one was closed.

**Why the reconcile is the one that stays.**  The join is the read's own
bookkeeping — "a read of a cell is a reference, not a snapshot" — not a
unification the program states, and it cannot be the report: a failed join
merges nothing, so the read falls through to the target's own value, and the
postlude's reconcile compares exactly that answer against the class it could not
join (`write_node_answer`'s *"a disagreement is exactly the conflict the unify
deferred to here"*).  One disagreement, one report — attributed to the
expression that was read, and with the descent path the reconcile's own
`steps` rebuilds.

So `alias_read` keeps the merge and drops its own error range: `try_unify` plus
`Vec::truncate`, the suppression form `try_unify`'s own doc names for a caller's
own failures.  The target's evaluation runs **after** the truncate, so a failure
of the computation itself is still recorded.

**Measured after the change**: `lichen-lowlevel` + `lichen-highlevel` +
`lichen-perspective` + `lichen-compute` + `lichen-language` = **695 passed,
0 failed, 7 ignored** (was 694/0/8), and the parked set went 8 → 7 with the
other six reds byte-identical.

## 2. Half one — refuted: a class's low type is not a second reading of a type slot

**What was proposed**: route `shape::low_type_of_slot`'s dynamic slots through
`Module::low_type_of_node`, keeping the node walk as the fallback, so that
"which member of the class a reader asks" stops mattering.

**What the measurement says** (branch at `3d55923`, every suite green before the
change):

- Routing the dynamic slot through the class channel broke **15 of the 58**
  `--test compute` cases, several as `(none, …)` values rather than as type
  differences.  The cause was on the *write* side of the chain, not the read
  side: `compile_fragment` (`compute.rs:2159`) and `seed_template_term_low_types`
  (`:2544`) *seed* a parameter/term slot from what `low_type_of_slot` answers, so
  a class answer there pins a slot to a shape nobody wrote and the pass then
  conflicts with it — `the kernel parameter's type is not decided when the kernel
  is compiled`.
- Restricting the class answer to the two scalar classes (`USize`, `Float`) left
  **6** failures, all of the same seed-side kind, so the breakage is not the
  compound answers alone.
- A trace of every call that reached the channel shows what it answers with:
  `Array(Unknown, 2)` **for a `[value, type]` term pair** (a pair is a two-element
  array), `Tuple([…])` for tuple values, `USize` for template cells whose class
  was unified with a runtime value.
- Narrowed further — the class read added at one *reader* only
  (`checker::names_float_class`, which asks a class question) — all suites are
  green **and the new branch never fired once** across the crate's test binaries.

**Why**: the low type vocabulary serves two subjects.  `low_type_of` decodes a
**type expression**; `Module::class_low_type` states the machine shape of the
**values** in a class.  The two coincide exactly for the scalar classes — there a
value's shape *is* its class — and nowhere else.  A type cell and a value node can
also share one class (an annotated parameter's type cell holds the annotation's
pair term; a frozen template's type cell resolves to a runtime *value* at the
apply), which is why a scalar answer from the channel is not evidence about a
slot's type either: in the trace the class was describing the *template's* value.

So the "second reading" in §1 is not a reading of the same fact, and deleting it
would delete a correct answer rather than a duplicate one.  `low_type_of_slot`
stays the type decode; `compute::node_class` was already class-routed
(`low_type_of_node`, `compute.rs:4320`) and is the one site that reads a *value's*
class — the right subject for it.

**What survives from the half**: the class channel is the only reading that sees
a class a *run* decided (a value's class), so a reader that asks a class question
can be routed to it — but only once §3 has made a run state one, and only as a
*class* (a scalar answer), never as a type.  That reader-side routing is therefore
part of §3's landing, not a step of its own.

## 3. Half two — the decider states the fact, and where that is not possible today

**What**: the layer that decided a class states it on the class, instead of
leaving it to a deferral's side effect.  Two measurements say the statement has no
receiver on the *run* side, and that a low type would not be enough if it had one.

**(i) The run cannot name the type cell.**  `NativeOp` has exactly one method,
`build` (`native.rs:55-65`), and the run half is
`OperatorExt::run(operand, _block, module)` (`program.rs:830-835`) — an arm
receives the *evaluated* operand array and no node id for its own expression.
`ParLaunchOp::build` creates `out_ty` (`compute.rs:8676`) and references it only
from the pair `[op, out_ty]`; nothing at run time can name that cell, so
"`seed_class_low_type(node, …)` at the `ParLaunch` arm" has no `node` to receive
it.  The one node the arm *does* create for a several-output result
(`compute.rs:1467`) is the buffer **value**, and the observation sites derive
nothing from it (`observed_low_shape`, `equality.rs:1190`: a `Buffer` is not a
`LowValue`).

**(ii) A low-type seed cannot change the printed type.**  The failing reader here
is the printer, and `type_printer::node` (`type_printer.rs:62-79`) renders a cell
that holds no value as its **class name** (`class_name` keys on
`representative`) and never consults low types.  `array<?a, ?b>` →
`array<Int, ?b>` therefore requires the element cell's **class to hold the `Int`
type value** — a `write_node_value`, not a seed.  This is §2's lesson applied to
§3: the reader asks for a value.

**What today's `array<Int, ?d>` actually is** (measured on a "cpu" probe pair,
scratch files, deleted):

| spelling | printed |
|---|---|
| a two-kernel chain with the reads **and** the collect in one tuple | `(20, 22, 24, [20, 22, 24]): <?a, ?b, ?c, array<Int, ?d>>` |
| the same chain, `collect` alone | `[10, 11, 12]: array<?a, ?b>` |

The `Int` exists only where a *consumer's array literal* is present, and the
printer renders it at a node created during that consumer's check — the array
literal's homogeneity puts the buffer's type cell and the ordinal's `Int` type
cell in one class, and the deferral's `pin_committed_value` then replicates that
committed type value into the element cell.  Read strictly, the `Int` printed
there may be the **ordinal's** type rather than the buffer element's, which is
exactly "accidental rather than stated".  So the struct-argument migration does
not lose an answer when it prints `array<?d, ?e>`; it removes a coincidence, and
the decided answer it needs is the one §5 states.

**Where a statement is possible today**: only where types are stated — a
`build` with concrete argument types and a `ctx`, i.e. **after** the signature is
concrete.  That is §5, and it is why §5 now comes first.

**The remaining specification, for whoever lands it** (either as §5 or, if the
extension-private operand route is taken, in `ParLaunchOp::build`):

- the fact for a class reader is `seed_class_low_type(node, low_shape_of(class))`;
  the fact for the printer is the **type value** for that class written through
  `Module::write_node_value` (`equality.rs:273`), into the element cell of the
  result's type rather than into the result's type cell (its class holds
  `[element, BufferKind]`, so writing the element type there would conflict);
- a canonical type value is needed at that point (`Int`/`Float`), and one is
  reachable from `ctx` (`ctx.int_type()`) but **not** from a `Module` at run time:
  the compute extension never touches `HighGlobal`, and the universe
  `K = [Type, ↺]` is self-referential so `alloc_array` cannot build one;
- `kernel_results_value` (`compute.rs:6200`) needs none of this for the scalar
  form: `add_node` already observes `USize`/`Float` from the scalar value
  (`observed_low_shape`), which is why a scalar result's class is stated today.

## 4. The carrier: the struct-argument migration

The migration that exposed all of this, and the reason to do §5 now: replacing
the raw array arguments of `compute.read`/`compute.write` with struct instances
(approved direction, explicit constructors — and the construction site is worth
trying with `_` in place of the explicit `(Read _)`, since each site would then
get its own inferred type rather than sharing one lambda application).

**The recipe, verified** ([compute-runtime-scalars](compute-runtime-scalars.md)
§4.3): the types must be **lambdas**, because a type *value* has one occurrence
whose `_` cells the first instantiation specializes —

```lichen
Read  = _x => struct<.from _, .at _>
Write = _x => struct<.to _, .at _, .value _>
read  = (x : Read _)  => $read(x.from, x.at)
write = (x : Write _) => $write(x.to, x.at, x.value)
```

with call sites `compute.read ((compute.Read _)(.from buf, .at i))` and
`compute.write ((compute.Write _)(.to n, .at i, .value v))`.  Written that way the
213 scripted call sites give **57 of 58** `--test compute` green, the frozen-module
panic gone and the `raw[…]` field-type leak gone (fields render concretely).

**Order**: §5 → this.  **Landed**: the wrappers are the recipe above and all 247
call sites in the tree (tests, examples, the documentation's code blocks, and one
parenthesized site a bracket-shaped script could not see) use the struct spelling
([compute-runtime-scalars](compute-runtime-scalars.md) §4.3 records the measured
result and the refuted `_` spelling).  The one remaining failure after the
migration is the element cell that §3 could not state and that §5.2's statement
owns — so the migration's acceptance is 57 of 58, with the honest answer for the
struct spelling being `array<?d, ?e>`, which is *more* correct than the array
spelling's accidental `Int`.

**Not to forget**: the migration touches `crates/lichen-language/tests/*`,
`crates/lichen-language/examples/*`, and the docs' code blocks; the script must
handle nested occurrences innermost-first (`compute.write ((compute.Write _)(.to a, .at b, .value compute.read ((compute.Read _)(.from c, .at d))))`).

## 5. The structural removal, and why it is now the first step

`ParLaunchOp::build` (`compute.rs:8623`, `out_ty` at `:8676`) leaves the result
type a **fresh cell**, and its own doc says why:

> "The signature's *arity* is what decides the result's shape — a bare `Buffer`
> for a one-write index function, a tuple of buffers for a several-write one — and
> the arity cannot be read here: `build` runs once, on the frozen `plrun` template,
> where the kernel's `.I`/`.O` are undecided cells that only resolve at run time."

§3 established that the run cannot repair that cell afterwards, so the cell has to
stop being late: **the signature must be concrete before `build` runs**.  Then the
result type is stated where types are stated, with a `ctx` in hand (so the
canonical `Int`/`Float` type value is reachable, which it is not from a `Module`),
and the fresh cell, the deferral side-effect dependency, and any run-time
statement all become unnecessary rather than merely fixed.

**Superseded naming: "specialize before JIT" means the *author* specializes.**
The direction recorded at the time
([kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6) described a
*compiler-side* pass — "at `jit`/`parallel` time the function is applied to a
placeholder typed by the annotated domain".  That is **not** the requirement: the
requirement is that the **user states the class before `jit`** (the parameter's
type), and the lowering reads that statement and nothing else (§5.1).  No
placeholder apply, no call-time evaluation, and no reading of the class domain is
part of it; the compiler-side framing is kept only as the rejected alternative
§6 names, because it is where the residual question below was once expected to be
answered.

**Measured: the gap is exactly the open class.**  With the parameter annotated
there is nothing left for the lowering to decide —

```lichen
annotated = compute.jit ((y : Int) => y + y)
annotated
```

prints `(raw Kernel, raw parameterized): struct<.native raw[?a, ?b], .sig Int -> Int>` —
the target string, already, on today's tree.  The red target is the *unannotated*
body, where `+`'s class is open and the recorded choice is the class domain's
`default`.  So the slice is: **choose the class where the domain says to choose
it, and state it in the kernel's `.I`/`.O`** — not a new lowering path.

### 5.1 Superseded: the class a lowering runs in is the **author's** statement

`open_class_of` (the first landing here) walked the function's own `asserts` for
the `InDomain(cell, set)` condition `check_binop` registered and took the set's
**first** member as the class to lower in.  **The routing removed the premise**,
and the decision that replaces it is the requirement itself: **a kernel function
must be explicitly specialized** — a kernel is lowered for one class and compiled
before any apply, so the parameter's type is where the language says which class
this artifact is for, and a body that left its class open is refused by name
([`UNDECIDED_DOMAIN`]: "the kernel parameter's class is not decided when the
kernel is compiled … annotate the parameter").  Measured: `compute.jit (y => y +
y)` is refused, `compute.jit (y : Int => y + y)` is `(raw Kernel, raw
parameterized): struct<.native raw[?a, ?b], .sig Int -> Int>` and launches `3 + 3 = 6`, and the
function itself stays polymorphic for its other uses (`f = y => y + y; k =
compute.jit f; f 1.5` is `3.0: Float` — the refusal does not touch the class).
`open_class_of` and `compile_fragment`'s `open_class` parameter are **deleted**;
the seed is the parameter's own type and nothing else.

### 5.1.1 The routing took the domain out of the caller — measured

**Landed in `feature/operator-std` (§6.1 of
[operator-polymorphism](operator-polymorphism.md)) and re-measured here**: the
surface operators now lower to the prelude's bindings, so a kernel body's `y + y`
*is* `Apply(Static(<core's add>), [y, y])`.  Two consequences for this pass, both
measured on this tree:

- **The domain is no longer in the caller's function.**  `open_class_of` reads
  `InDomain` out of `module.functions[fid].asserts`, and after the routing the
  jitted function's assert list is **empty** while the only `InDomain` in the
  program is inside frozen module `158` (the `core` prelude): the *callee's*
  refinement (`add = operands => { operands : array<(_ ! in_num), 2>; … }`) is
  what confines the class now, and it is enforced by the apply, in the callee's
  own module.  So `compute.jit (y => y + y)` is refused with `UNDECIDED_DOMAIN`
  again — the parallel path is unaffected only because the ABI seeds every scalar
  leaf `USize`.  **This is the one red target of the two §5 named that is a
  *typing* question rather than a printer's**, and the answer taken is §5.1's:
  the author states the class (a kernel must be explicitly specialized).  The
  three candidates that were weighed — the routing restates the domain at the
  call site, the pass reads a frozen function's own statements through a new
  lowlevel accessor, or an open body must be annotated — are settled on the
  third: **no default is invented and no frozen structure is read**.
- **The frozen callee's constants reach the caller as static refs.**  The static
  apply's residual clone keeps the callee's unchanged subterms as references into
  the frozen module, so a routed body's `operands[0]` selector arrives as
  `Static(…)` beside nodes of the caller's own.  A frozen node is a **value** and
  never a computation (a static module is fully solved), and `low_type`'s own
  reading says so (`low_type.rs`: "a static ref is a decided leaf with no class
  to refine").  The emitter therefore reads an operand's *value* from either kind
  (`emit_operand`, `AnyNodeId::dynamic`, and the readers that take
  `impl Into<AnyNodeId>`) instead of refusing it, and a static apply emits the
  residual the lowlevel already wrote for it.

**Landed with that reading: the class of a value had two readers and they
disagreed.**  A body lowered from a template is made of parameter reads the
low-type pass cannot transfer through, so `node_class` declined and fell back to
the integer default while the *emission* used the operands' class.  Measured on
the routed shapes: `compute.jit (x : Float => x * x)` declared
`result_classes = [Int]` over a body of `[LocalGet(0), LocalGet(0), Bin(Float,
Mul)]`, which the wasm validator refused ("expected i64, found f32"), and the
float index function of `a_varying_float_element_is_seeded_from_the_index` wrote
through `BufferWriteCall(Int)` a value it had computed as `f32` ("instruction 6
mixed an integer and a float").  `node_class_in` is now the one reader: the
emission's own order — a parameter read resolved through its slot's shape, the
same peel `node_class` makes, a whole-parameter read matched by equality class,
and an arithmetic operator's class taken from its operands with a comparison's
`0`/`1` the one exception.

**The same reader had a third site, and it was the one that mattered for a
*chain*.**  `compile_parallel_fragment`'s pre-scan — the class a buffer **read**
is declared in (`Positions::element_class`) — still used `node_class`, so a
routed `0.0 + a + a` was declared `Int` there while the write itself was emitted
in `Float`: the consumer's read of that buffer then declared the wrong class and
the chain's value stayed `Parameterized` (measured through the new structural
comparison: a whole two-kernel float chain, both sides, every element `false`).
Read through the slots, the chain computes.  `--test compute` went 12 of 60
(after the routing) to **58 of 60** with the other suites unchanged.

### 5.2 Open: the signature the wrapper publishes in `.I`/`.O`

The signature a `jit` result publishes lives in its `.I`/`.O` fields, and they
must be **decided** — a scalar kernel's fields are the function's own argument
and result slots, and a parallel wrapper binds `.O` to the parameter type's
`.out` group rather than to the body's codomain.  Four measurements constrain
how the statement reaches them.  Each is a trap that cost an attempt:

1. **A node that appears only in the type graph is never evaluated.**  A
   statement that exists only as a type never runs: the printer reads a value
   that nothing computed.  The statement must therefore be one an operator
   already runs over, and that operator is `$jit`/`$parallel`, whose operand is
   the function itself.  In the model that preceded this one the signature had
   to ride in *as a value* for exactly that reason — `.sig s` beside
   `$jit(f, s)`; the statement now is the wrapper's own annotation
   (`f: I -> O`), checked against the function's type, and `.I`/`.O` are the
   cells every later read of the kernel resolves through.
2. **`run_deferred` reports an unstamped node as `undecided`.**  Putting the
   function's *type* node (`f.ty`) in an operand array makes the whole array read
   `undecided` and the arm never runs at all (`OperatorExt::run_deferred`,
   `lowlevel/src/lib.rs`); the deep pass never stamps a type node.  So the
   signature expression must reach the operator without a type node among its
   operands.
3. **A `LaunchOp` domain read has to land on a pure cell.**  When the arrow's two
   sides are `Index` op nodes, `LaunchOp`'s `check_unify(a.ty, d)` against one of
   them defers and never resolves: measured, the four `jit_cross_kernel_*` tests
   print `parameterized` and the launch computes nothing, while the identical
   wrapper with a cell-sided signature passes 7 of 7.
4. **The generic wrapper's type must still render as an arrow with the function's
   own variables.**  `wrapper_functions_render_with_named_type_variables` pins
   the arrow — `.sig ?a -> ?b` then, `.I ?a` / `.O ?b` now — at the *same*
   classes as the wrapper parameter's own type.  A fresh arrow renders its own
   names, and a shape slot that is an unset operator renders
   `raw[?c, TypeFunction]` (the printer's kind branch needs the shape slot's
   *value* to be a two-element array).

Traps 3 and 4 pull apart: the sides must be *cells in the function's own type
classes* (4) yet must never be written (the pin the handoff forbids), while a
launch's read of them must resolve (3).

**Measured, this session — the pin is real, and per-application cloning is what
makes any statement of it possible:**

- an annotation written *inside* a wrapper body reaches the argument
  (`pin = g => {t = type_of g; h = g : Int -> Int; t}`, then `pin f; f 1.5`) is
  refused — "expected Int, found Float": the applied parameter's cells are one
  class with the argument's, so a class written there *is* the parameter cell the
  handoff forbids writing;
- the same function called twice with different classes is accepted
  (`f (1 : Int)` then `f (1.5 : Float)`), so a *callee's* cells are cloned per
  application — which is why a wrapper *definition* can stay generic while an
  *applied* result states its own class;
- the printer's arrow branch reads the **shape slot's value** at print time
  (`type_printer::elements`: `elements[0]`'s value must be a two-element array),
  so the statement is exactly "rewrite the shape node's value" — and it has to be
  a shape the *wrapper* owns, because the function's own shape node is what every
  other use of the function reads.

### 5.3 Landed: the wrapper's `.I`/`.O` cells carry its signature

`jit` and `parallel` no longer reach for a signature operator: their wrappers
bind their own cells (`I = _; O = _`) and state `f: I -> O` on the function, so
the unify against `f`'s type makes those cells the function's own argument and
result slots (trap 4) without pinning a polymorphic body.  The retired carrier
that first published such an arrow was `$sig(f)` (`compute.rs`), which built the
arrow `JitOp`'s own gate built — two fresh cells through `ctx.arrow`'s
shape/kind/pair triple — and unified it against `f`'s type, with the shape node
belonging to the wrapper (no pin); `jit`'s `.sig` field was `($sig(f))` instead
of `(type_of f)`, and the operator was deleted once the struct carried
`.I`/`.O`.

Measured this session: the wrapper definition then rendered
`Function: ?a -> ?b -> struct<.native raw[?c, ?d], .sig ?a -> ?b>` — a native call
*is* accepted in a type position and its term is what the field type becomes — and
an applied result then rendered `.sig ?c -> ?c`, with the tree at `57 of 58` and
every other suite unchanged.  `$sig`'s *value* node was the shape: the node §5.2's
statement used to have to rewrite.

### 5.4 Open: who rewrites the shape, and how it reaches them

The member is knowable **only at run time** (`open_class_of` reads the function's
asserts, and the frozen template's parameter has no body), and every node an
operator's operand array holds is deep-pass gated by `run_deferred`'s default —
the gate the LaunchOp idiom's inert read rides on and that an open signature
therefore fails.  So the writer needs one of:

- **an operand that is already concrete** — **refuted by measurement**: giving
  `ComputeOperator::Jit`'s operand array the shape as an inert second element (the
  `LaunchOp` idiom) turns the deep pass's answer for the whole array into
  `undecided`, so the default gate returns before the arm runs and the artifact
  is never compiled: `35 of 58` `--test compute` cases go red, every one of them
  reading `parameterized`.  A bound array whose items are still-open cells is
  *not* concrete, and the shape's items are those cells by construction;
- **a `run_deferred` override** on the compute vocabulary: the one op that must
  read its operand *structurally* is exactly the case the default gate cannot
  serve.  The default's body (deep-evaluate the operand node, refuse when its
  stamp says undecided, hand the arm the value) is **one policy**, so the
  clean landing is to factor it into a shared step both the default and the
  override call — otherwise the override restates it for all ten other arms; or
- **a node-carrying opaque value**: a `ComputeValue` variant holding the shape is
  concrete for the pass and carries the node to the arm — at the cost of a value
  variant whose GC tracing and codec must keep that node alive.

The write itself is the same in every case: for each shape item whose class is
still open, use the class domain's first member as that item, rebuild the shape
array, `Module::write_node_value` it — never a write into the function's cells.

## 6. How to verify, at each step

```bash
cargo check --workspace
cargo test -q -p lichen-highlevel -p lichen-compute
cargo test -q -p lichen-language --test compute --test pipeline --test graph_jit \
  --test graph_structure --test examples --test defer_pending
```

§5's own acceptance is the two targets
[operator-polymorphism](operator-polymorphism.md) §8.8 recorded as red:

- `a_kernel_value_and_type_render_by_name` (`crates/lichen-language/tests/compute.rs:466`)
  — the open `.sig ?c -> ?c` this test recorded then had to become the decided
  `.I`/`.O` the struct carries now;
- `an_imported_package_that_jits_at_its_top_level_still_runs`
  (`crates/lichen-language/tests/runtime_only_package.rs:41`) — the `launch` gate
  must resolve the domain of an open kernel signature.

**Where the tree stands after §5.1 and §5.1.1** (`--test compute`, 60 cases): 58
pass.  The two reds are one shape each, and neither is a printer:

| case | what it needs |
|---|---|
| `jit_cross_kernel_call`, `jit_cross_kernel_wrapper` | a cross-kernel call or `compute.launch` whose **argument** is a routed operator (`k0 (x + 1)`): the emitter reaches the argument's cell and its equality class holds a *bare cell*, not a computation, so there is nothing to emit.  **This is not a class question and no author-side specialization answers it** — the parameter is annotated and the body still has the shape.  **Measured root**: the *routing* lowered `x + 1` to a **call of the prelude's binding** (`Apply(Static(add), [x, 1])`), and inside a kernel **template that call is never evaluated** — `wire_apply_result` (the lowlevel's own wiring, whose doc says "the apply node *is* the return pair") is never called for it — so the body the call stands for has no edge from the call.  Three routes were tried and measured: (a) reading the residual from the call's own value — nothing is written there for a template call; (b) merging call with body in `wire_apply_result`'s scalar/undecided arm — **breaks** `lichen-highlevel --test dependent` (`dependent_type_resolves_per_argument_via_laziness`), so it cannot be unconditional; (c) forcing the body once at `jit` time (`evaluate_node_deep(ret_value)`) — the class still does not hold the residual.  What is left is the operator workstream's own recorded alternative: **expand the binding's body at the call site** instead of calling it (`operator-polymorphism` §7.1), which leaves `x + 1` an `Add` node in the body and removes the question entirely; until then the refusal stands |
| `examples/compute_jit.lichen` (`--test examples`) | the same shape through a `compute.launch` argument |

The `jit_cross_kernel_*` cases and the example are annotated as the rule requires;
their remaining failure is the residual edge above, not the class.  Both
`a_kernel_value_and_type_render_by_name` and
`an_imported_package_that_jits_at_its_top_level_still_runs` — the two targets
§5.2/§8.8 recorded as red — **pass** with the class stated, which is what the
rule says they need.

The probes that pin the rest (scratch files, not committed):

- **§5's own probe** (the decided element cell, stated where types are stated): a
  single-kernel `plrun` + `collect` must print `array<Int, ?b>`; today it prints
  `array<?a, ?b>` on the array API and on every other spelling.
- **§4's own assertion, measured and corrected**:
  `a_gpu_program_chains_two_kernels_on_a_device`
  (`crates/lichen-language/tests/compute.rs:1308-1345`) does **not** keep its
  `array<Int, ?d>` under the struct spelling — it renders `array<?d, ?e>` with the
  same values, which is the element cell §5.2 states and the same commit path
  measured above.  The earlier
  reading of this bullet as "must keep" was written before §1's measurement showed
  the `Int` arriving only where a consumer's array literal is present.
