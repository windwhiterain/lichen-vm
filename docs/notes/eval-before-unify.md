# Evaluation runs before unification, and nothing wakes what it read

> Status: **§2.1 fixed, §2.2/§2.4 open** — the order-sensitivity is **measured on
> `dev@5e89bb1`** (§2's matrix is compiler output, reproducible from the inline
> programs); the node-level chain behind §2.1 is now **traced**, not guessed
> (§3.3); §4 is the comparison to other languages; §5.1's minimal half — a merge
> carries the class's decided value to the members it adds — is **landed** on
> `feature/lazy-wakeup` and fixes §2.1 (pinned by
> `crates/lichen-language/tests/merge_carry.rs`), while §2.2's accept/refuse
> flip, §2.4's template-level face of the same gap, and the re-checkable guard
> of §5.2 stay open — §6.2 analyzes the fix directions and §6.3 the shared
> diagnostic's wording, and the rest of §5 stays a sketch.
>
> Companions: [class-channel](class-channel.md) (the write-side half of the
> same principle — a fact is decided once and stated on the class; this note is
> about the *read* side being taken too early and never re-taken),
> [defer-pending-type-forms](defer-pending-type-forms.md) (the deferral
> machinery this note's repros exercise),
> [incremental-evaluation](incremental-evaluation.md) (re-evaluation across
> edits — the same wakeup question one level up),
> [operator-polymorphism](operator-polymorphism.md) (the open class and its
> domain, which §2's binop rows route through).

## 1. The problem

Lichen interleaves evaluation and unification in one graph: types are runtime
values, a check-time read can meet a cell nothing has bound yet, and a unify
can force a computation.  The ordering hazard this creates:

> An expression (or a check-time decision about one) reads a cell **before**
> the unification that would bind it has happened.  The read finds the cell
> unresolved — `Parameterized`, unbound, "not concrete" — and acts on that
> answer.  The binding arrives later; nothing re-asks the question.

The runtime half of the system is built around this (§3.1): a lazy answer is
never cached, a read of a pure cell joins the cell's class so replication
reaches it, and a stalled unify can defer-and-pin.  The **decision** half is
not (§3.2): the checker's guards and class questions read the graph once, at
the moment the expression is checked, and the answer they compute then is
final — including when the answer was "I cannot tell yet".

## 2. The measured matrix

Every program below was run through `lichen-compiler` at `dev@5e89bb1`; **this
is the pre-fix measurement**, kept as the defect's record.  Where a landed fix
has changed one of these outputs, the new output is stated with the row (§3.3
for §2.1).  The pairs differ **only in statement order**.

### 2.1 A binop's result type: decided or `?a` by statement order

```lichen
-- read first, annotate after                    -- annotate first, then read
f = x => {                                       f = x => {
  a = x(0) + x(0)                                  p = x: <Int, Int>
  p = x: <Int, Int>                                a = x(0) + x(0)
  a                                                a
}                                                }
f (10, 20)                                       f (10, 20)
```

| order | output |
|---|---|
| read, then annotate | `20: ?a` — the type **stays undecided** (pre-fix; `20: Int` since §5.1's landed half, §3.3) |
| annotate, then read | `20: Int` |

The annotation makes the result type fully resolvable in both orders — the
annotate-first row proves it.  In the read-first row `check_binop` asks its
class question (`class_is_stated`/`names_float_class`,
`checker/operators.rs`) while the field type is still a lazy `Index` it cannot
see through, takes the polymorphic path (shared open class + a pending
`InDomain` assert), and the shared class never receives `Int` afterwards.

Controls that bound where the staleness lives:

| variant | output |
|---|---|
| bare read, then annotate (`l = x(0); p = x: <Int,Int>; l`) | `10: Int` — the read itself resolves |
| two reads, one on each side of the annotation | `(10, 10): <Int, Int>` |
| the same binop with **no annotation** (x bound only by the apply's argument) | `20: Int` — the apply-time bind resolves what the check-time bind did not |

So the stale case is precisely: a **check-time** unification (the annotation)
binds the container after a **check-time** class question already read the
pending read and committed to an answer shape.  The apply-time bind wakes the
graph; the check-time bind does not wake the decision that read it.

### 2.2 A container-kind guard: accept or refuse by statement order

```lichen
-- read first, annotate after                    -- annotate first, then read
f = x => {                                       f = x => {
  a = x(0) + x(0)                                  p = x: array<Int, 3>
  p = x: array<Int, 3>                           a = x(0) + x(0)
  a                                                a
}                                                }
f [10, 20, 30]                                   f [10, 20, 30]
```

| order | output |
|---|---|
| read, then annotate | `20: ?a` — **accepted** (pre-fix; its type decides to `20: Int` since §5.1's landed half, but the read is still accepted) |
| annotate, then read | `error: expected a tuple, array, or struct type, found array<Int, 3>` — refused |

The paren read `a(k)` is for tuples and structs; arrays read with `a[i]`
([language-spec](../language-spec.md) §Indexing).  `check_field`'s guard
(`checker/structs.rs` — `concrete && !is_positional_type`) fires only when the
container type is concrete **at check time**; an undecided container "stays
lazy and resolves at the apply".  But nothing re-runs the guard at the apply,
and the runtime `Index` *cannot*: arrays and tuples are both
`LowValue::Array`, so the runtime read of `[10, 20, 30](0)` legitimately
computes `10`.  The deferral path silently drops a check the other order
enforces.  (Aside, found while probing: the guard's message lists `array`
among the accepted kinds while the predicate refuses it — analyzed in §6.3.)

### 2.3 The contrast case: apply **is** caught in both orders

```lichen
f = x => { a = x 1; p = x: Int; a }   -- read first
f = x => { p = x: Int; a = x 1; a }   -- annotate first
```

| order | output |
|---|---|
| apply, then annotate | `error: this value is not a function — it cannot be applied` (runtime `EvalError::ApplyTarget`) |
| annotate, then apply | `error: expected ?a -> ?b, found Int` (static, with a span) |

Both orders refuse — applying an `Int` has a runtime net
(`evaluation.rs`'s `Apply` arm records the failure) because a function is
distinguishable from a scalar at runtime.  The two rows differ in *which*
refusal fires and how good the diagnostic is; §2.2's pair differs in
**whether the program is accepted at all**.  A deferred check needs a net at
the later tier; the paren read is the case that has none.

### 2.4 The pin route is sound; the guard route is not

The two container reads defer an undecided container by different mechanisms:
`e[i]` and `t{k}` **pin** the container's type to a fresh array/table type
(`check_index`/`check_table_find`, `checker/indexing.rs`), and the pin is
enforced by the apply's argument unify per call; `a(k)` **guards** on
concreteness and skips when undecided (§2.2).  Measured, with no annotation
anywhere in either program — so no statement order is involved:

```lichen
f = x => x(0)      f = x => x[0]
f [10, 20]         f (10, 20)
```

| read | output |
|---|---|
| paren `x(0)`, array argument | `10: Int` — **accepted**; nothing ever asks whether an array may be paren-read |
| bracket `x[0]`, tuple argument | `error: expected array<?a, ?b>, found <Int, Int>` — refused at the apply, statically, with a span |

So §2.2's order flip is one face of a wider gap: the paren read's deferral has
no enforcing tier at all — not at the definition pass, not at the apply, and
not at runtime (a tuple and an array are both `LowValue::Array`, and the
lowlevel is untyped by design — `type-system-cleanup-plan` D1).  §6.2 analyzes
the fix directions this measurement leaves.

## 3. The mechanism, as it stands

### 3.1 What the runtime already does right

The lowlevel's answer to the ordering hazard is pull-plus-replicate, and for
plain reads it works — §2.1's controls resolve:

- a `Parameterized` answer is **never cached** (`evaluation.rs`'s postlude),
  so any later read re-evaluates against whatever has since bound;
- a read of a pure cell **joins the cell's class** (`alias_read`,
  `equality.rs`), so a later bind replicates the value to the reader through
  `write_node_value`;
- a unify that meets a pending computation **forces** it
  (`force_pending`) or **defers and pins** (`defer_pending` +
  `pin_committed_value`), reconciling when the computation finally runs.

The common shape of all three: resolution happens **when somebody asks
again** (re-read, re-unify, the apply's clone recompute) or is **pushed
through a class merge**.  The push was the half that was missing — a merge
carried the class's *shape* but not its decided *value* — and §5.1's landed
change closes it.  There is still no registry of "who is waiting on this
class", and none is needed: the class is the registry.

### 3.2 Where the one-shot reads are

The checker's single pass asks class/concreteness questions and acts on the
answers permanently:

| site | question | acts on "cannot tell yet" by |
|---|---|---|
| `check_binop` (`checker/operators.rs`) | `class_is_stated` / `names_float_class` | taking the polymorphic path — correct for the value, but the result-type cell is the shared open class (§2.1) |
| `check_field` / `check_named_field` / `check_raw_named_field` guards (`checker/structs.rs`) | `type_is_concrete` | skipping the guard; no re-fire (§2.2) |
| `slot_read` (`checker/structs.rs`) | `constant_position`, `field_type` | keeping the lazy `Index` — sound, but any *reader* of that type cell before it resolves snapshots "unknown" |
| `check_convert` (`checker/operators.rs`) | `low_type_of_slot(..).is_known()` | skipping the source-class unify entirely |
| `check_instantiate` (`checker/structs.rs`) | a one-time `evaluate_node_deep` force of the callee | deferring the nominality check to the apply's argument unify |
| the kernel boundary (`lichen-compute`) | `low_type_of_slot` for a parameter domain | refusing — the documented hard boundary, the honest version of the same snapshot |

Some of these are fine alone: the pending `InDomain` assert **is** a
re-checkable registration, and it is what keeps a polymorphic `+` correct per
apply.  The defect is the combination — the assert observes the class (and
passes), while nothing ever **commits** the observed member to the class.  That
last clause was the guess this note opened with; §3.3's trace disproves it.  The
class *is* committed; what was missing is that a merge which grows a class does
not carry the class's already-decided value to the members it adds.

### 3.3 The traced chain of §2.1

A probe build traced `unify`/`bind`/`add_equality`/`write_node_value` in
`equality.rs` and the `check_field`/`check_binop`/`check_ann` sites in the
checker, in the style [defer-pending-type-forms](defer-pending-type-forms.md) §2
describes (the instrumentation was reverted).  For the **read-first** order of
§2.1, with `T₁` the first `x(0)` read's lazy type and `T₂` the second's:

1. `check_field` leaves each read's type as `Index(Index(container_ty, 0), k)`:
   the container type is still an unbound cell, so `slot_read` keeps the lazy
   form and neither read is decided.
2. `check_binop` finds neither operand stated, takes the polymorphic path, and
   unifies the two reads **with each other**.  Both are pending `Index` reads, so
   `unify_inner`'s pending-read branch merges their classes with a bare
   `add_equality` — no value and no computation to carry.
3. The statement's `check_term` skeleton epilogue then unifies the skeleton's
   type cell into that same class, again down `bind` with both representatives
   unbound, so again nothing is carried.  The class now has three members and
   still holds no value.
4. The annotation `p = x: <Int, Int>` unifies the **template's** parameter type
   cell — still an unbound cell — with the tuple type.  That is a concrete value,
   so `bind` writes it and replication covers *that* class: the annotation
   decides the container, and therefore what the lazy reads of steps 1–3 resolve
   to, but it touches no member of their class.
5. The definition pass deep-evaluates the body.  `T₁` now reads through the
   bound container to the field's type — the `Int` type pair — and the
   evaluation postlude caches that value on the operation node and replicates it
   over the class.  **Replication skips operation-bearing members**, so the
   class's representative (itself a pending `Index` read) keeps an empty slot
   while two other members carry the value.
6. The apply wires its result cell: `wire_apply_result` unifies the cell (a
   fresh unbound pure cell) with the cloned return's type, which is the class of
   step 5.  `bind` reads only the two **representatives'** slots, finds both
   unbound, and merges with nothing to carry.  The cell joins a class decided
   since step 5 and never receives the value, so the printed type reads `?a`
   while the value `20` arrives normally.

So the read is not "taken too early and never re-taken" at the *value* level —
the class commits on time.  What was one-sided is the invariant: a class's
decided value reached its members on every **write** (`write_node_value`) and on
no **merge**, and a merge is the only other moment a class gains members.  A
cell that joined the class after the commit was therefore never told.

§2.1's **bare-read** control (`l = x(0)` read before the annotation) resolves on
the unfixed build too, so the chain above says nothing about it: its own read's
class is reached by a different path.  That path is not traced here, and the fix
does not depend on it.

### 3.4 Errors are permanent

A recorded diagnostic is never retracted.  Every guard that *skips* when
undecided is therefore safe-by-construction (it can only under-report), but
the flip side exists: any evaluation a check-time force performs (e.g.
`check_instantiate`'s callee force) that records an `EvalError` against a
not-yet-bound graph has recorded a error a later binding would have
prevented.  No row of §2 exhibits this; it is listed because a wakeup design
(§5) must decide whether woken re-checks may retract.

## 4. How other languages handle it

The systems that **separate** types from evaluation never have the problem:
Haskell and OCaml run HM inference as its own phase over pure type terms —
constraint generation walks the syntax once, unification is confluent
union-find over metavariables, and nothing is ever *evaluated* during
inference.  (Haskell's laziness is about runtime values and is unrelated to
its inference.)  Lichen cannot take this route: its types are runtime values
by design.

The systems that **do** interleave evaluation and unification all land on
the same mechanism the question proposes — suspended work keyed on what
would unblock it, woken when that happens:

- **Agda** — the closest analog (dependent types; elaboration and evaluation
  are mutually recursive).  A constraint that cannot proceed sleeps with a
  [`Blocker`](https://agda.github.io/agda/Agda-Syntax-Internal-Blockers.html)
  set — `UnblockOnMeta MetaId`, `UnblockOnAll`/`UnblockOnAny` of blockers,
  `UnblockOnProblem`, `UnblockOnDef` — and instantiating a metavariable wakes
  every constraint blocked on it for re-attempt.
- **GHC's constraint solver** — the part of Haskell that *does* interleave
  (type classes, GADTs, implications).  An inert set plus a worklist;
  unifying a touchable metavariable **kicks out** every inert constraint
  mentioning it back into the worklist
  ([`kickOutRewritable`](https://hackage-content-origin.haskell.org/package/ghc-8.10.1/docs/src/TcSMonad.html#line-2667)).
- **Lean 4** — the elaborator postpones terms whose synthetic metavariables
  are unassigned and loops re-attempting them as assignments accumulate
  ([`SyntheticMVars`](https://github.com/leanprover/lean4/blob/master/src/Lean/Elab/SyntheticMVars.lean)).
- **Prolog coroutining** — the oldest form, and lichen's runtime is closer to
  a constraint store than to Haskell: `freeze(X, Goal)` / attributed
  variables suspend a goal on a logic variable and run it on instantiation.

## 5. Design sketch for lichen

Two halves, matching the two halves of §3.

### 5.1 Runtime: the class as a channel (landed) and blocked operations (sketch)

**Landed.**  §3.3's chain needs only the smaller half: `add_equality` now carries
the merged class's decided value to the unbound pure cells the merge adds to it
(`equality.rs`).  The value is read through `class_committed_value`, which scans
the members, so it is found on whichever member carries it rather than only on
the representative's own slot — the merge previously read just those two slots,
which is exactly why a value committed onto an operation-bearing member was
invisible to it.  `write_node_value`'s replication half is factored out as
`replicate_class_value` so the write site and the merge site state the same
invariant once.

Two properties of the landed form are deliberate.  It writes the class's pure
cells and never the representative's own slot, because an operation-bearing
representative is a pending computation whose resolved value is the authority —
caching the commit onto it would answer every later read with the bet instead of
running the computation.  And it performs no low-type observation: observation
is a class *gaining* a decided value, and this merge adds no fact to the class —
only members.  Nothing is forced and no pending computation runs, so no program
starts computing because something bound; what changes is that a cell added
after the commit reads the value its class already had.

**Sketch.**  A **blocked list** on each class — the operations that evaluated to
`Parameterized` because they read the class while it was unbound — drained on
commit by forcing each one (`force_pending` turned event-driven instead of
unify-driven) remains a proposal.  **Neither measured row needs it**: §2.1's
missing fact was a value the class already had, and §2.2 is not a runtime
question at all.  It would be the honest fix for a computation — not a read —
that must start when what it waited on arrives, and its watch-points are
unchanged: re-entrancy (a drain runs evaluation inside a bind that may itself be
inside a unify inside an evaluation — the visit-mark invariant and the depth
budget both apply); GC (a blocked edge keeps the waiter alive; a waiter nobody
else references is what "undecided forever" currently *means*, so draining must
not resurrect semantics by keeping dead nodes live); and the template/clone split
(a template's parameters never bind — its blocked list must not fire).

### 5.2 Checker: guards as re-checkable registrations

The assert channel already is the model: a condition registered once,
evaluated when it can be, re-checked per apply clone
([operator-polymorphism](operator-polymorphism.md) §3).  The §3.2 guards that
currently skip-when-undecided (`check_field`'s kind guard is the §2.2 hole)
can register the same way instead: skip now, **re-ask at the first moment the
container's class commits** and at the apply's argument unify, where the runtime
net currently cannot help (the tuple/array confusion is invisible to
`LowValue::Array`).  What that needs is a *condition the runtime can evaluate* —
a type-level "is this a positional type" operator beside
[`TypeOperator::InDomain`](../crates/lichen-highlevel/src/program.rs), whose
answer is `USize(0/1)` over a type value — plus a spelling for the diagnostic it
records when the condition fails at the apply.  **Not landed**; §6.2 ranks this
against the alternatives (a shape-pin, a runtime net, a settle pass) and §2.4
measures the template-level face it must also close.

The one-shot *class questions* (`check_binop`'s `stated`/`float`) needed no
change on the question side after all: the polymorphic path they fall back to is
correct for the value, and the result type it leaves open was fixed by the
commit reaching the class (§5.1) rather than by asking the question later.

### 5.3 What each measured row needs

| row | fixed by |
|---|---|
| §2.1 `20: ?a` | **landed** (§5.1): the merge carries the class's decided value to the members it adds |
| §2.2 accept/refuse flip | **open**: the guard must re-fire when the container's class commits (§5.2), or the runtime `Index` must distinguish tuple from array — which the encoding currently cannot |
| §2.3 | already caught; only the diagnostic quality differs by order |

## 6. Open questions

### 6.1 Recorded decisions

- **No retraction.**  A woken re-check may only *add* a diagnostic, never retract
  one (§3.4).  The landed change records none at all, so it leaves the error
  stream as order-independent as it found it; retraction stays rejected, because
  it would make the error stream order-dependent in a new way.
- **Binding is not strictness.**  The landed carry propagates a value the class
  already holds and forces nothing, so a program that stays lazy today stays
  lazy.  A future drain (§5.1's sketch) would have to answer this on its own —
  its whole effect is to make a bind wake a computation, and the budget guards
  would make that observable.
- **The pin stays the policy.**  `defer_pending`'s pin commits early and
  reconciles; the merge carry is not a competing policy but the same
  class-channel fact applied where a member arrived after the commit.  Building
  the drain would reopen the question — the pin commits early, the drain commits
  late, and one of them would then be the policy and the other an optimization of
  it.

### 6.2 The §2.2/§2.4 fix direction (analyzed, not landed)

The gap is that the paren read cannot simply copy the bracket read's pin: a
pin is a unify against **one** concrete kind, and `a(k)` accepts a
*disjunction* — a tuple type `[shape, [TypeTuple, K]]` **or** a struct type
`[shape, [TypeStruct{id, names}, K]]`.  An open marker cell cannot express
"tuple or struct, not array": whatever flows in binds the cell, `ArrayType`
included — which is exactly what §2.4's accepted row is.  The options, in the
order the analysis ranked them (the ranking is **reasoned from the encoding**,
not measured — only §2.4's two rows are measurements):

1. **Re-checkable assert (recommended).**  Keep skipping the static guard when
   the container is undecided, and register the condition on the assert
   worklist exactly like `InDomain` and the bounds assert: a `TypeOperator`
   over the container type answering `USize(0/1)` for "positional" — the
   predicate to wrap is `shape::is_positional_type_any` — plus an
   `AssertSpelling` for the diagnostic.  The machinery is the one §2.1's
   polymorphic binop already rides: pending at the definition pass, re-checked
   per apply clone.  That closes **both** faces: §2.2's same-scope flip (the
   definition pass evaluates the assert once the annotation binds) and §2.4's
   template hole (the clone's re-check meets the actual argument).  Cost: one
   operator, one spelling, one diagnostic string.
2. **Shape-pin plus the same assert.**  Additionally pin the undecided
   container to the skeleton `[shape, [marker, K]]` with an open marker — the
   shape `check_index` pins, minus the kind — so `slot_read`'s lazy `Index`
   reads a decided structure and a lowering sees a field list before the kind
   is known.  Strictly more than option 1; worth it only if a consumer needs
   that, and none measured does.
3. **Runtime net — impossible by design.**  Evaluation cannot distinguish a
   tuple from an array (both are `LowValue::Array`) and must not: the lowlevel
   is untyped (`type-system-cleanup-plan` D1).  This is why §2.3's apply has a
   net and §2.2's read cannot have one.
4. **A settle pass (re-ask skipped guards after the statement pass) —
   insufficient alone.**  It fixes §2.2's same-scope flip but not §2.4's
   template hole: a template's container legitimately stays undecided, and only
   the per-apply re-check (option 1's machinery) can refuse a wrong-kind
   argument.  A settle pass would be a partial duplicate of it.

### 6.3 The message/predicate disagreement (analyzed, not landed)

`DiagKind::IndexTarget` is shared by three guards with three different
accepted sets, and its single message names a kind **none** of them accepts:

| site | read | accepted | message claims |
|---|---|---|---|
| `check_field` | `a(k)` | tuple, struct | tuple, **array**, struct |
| `check_named_field` | `a.name` | struct | tuple, array, struct |
| `check_raw_named_field` | `X::a` | struct type value | tuple, array, struct |

So "expected a tuple, array, or struct type" is wrong at every site — the
array at all three, the tuple at the two named ones (the existing pipeline
tests pin both named sites: `a_named_field_read_on_a_non_struct_is_rejected`,
`a_raw_named_read_requires_a_type_struct_container`).  The predicate is the
correct half and the wording the stale half.  The fix is per-site
expected-sides — two spellings suffice, positional for `a(k)` and named for
`.name`/`::a` — and §6.2 option 1's assert diagnostic should reuse them rather
than invent a fourth wording.

One drift found while reading: the [spec](../language-spec.md) §Indexing says a
concretely non-indexable `e[i]` "is an `IndexTarget` diagnostic at check
time" — but `check_index` pins and fails through `DiagKind::Guard` ("expected
`array<…>`, found …", §2.4's refused row).  The spec sentence predates the
pin; it should describe the Guard-style refusal.

### 6.4 Still open, unanswered

- Should a woken re-check be able to **retract** a diagnostic, if some future
  wakeup needs it?  Answered *no* for now (§6.1), on the order-independence
  argument alone.
