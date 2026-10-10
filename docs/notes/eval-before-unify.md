# Evaluation and unification interleave, and a read can be taken too early

> Status: **current.** §2.1 is fixed and the container reads with it: unification
> merges a class and carries the class's decided value to the members it adds, the
> paren read `a(k)` is the tuple read, and a named read states its container
> requirement in both tiers (the decided one as a kind unify, the undecided one as
> a registered `IsStructType` condition re-checked per apply clone, which also
> closed `X::a`'s panic). The class-attached blocked list remains a sketch, and
> the rest of §5 stays a sketch.
>
> Points at: `crates/lichen-lowlevel/src/equality.rs` (`unify`/`unify_inner`,
> `add_equality`, `write_node_value`), `crates/lichen-lowlevel/src/evaluation.rs`
> (the evaluator's read and run gates), `crates/lichen-highlevel/src/checker/operators.rs`
> (`check_binop`), `crates/lichen-highlevel/src/checker/structs.rs` (`check_field`,
> `check_named_field`, `slot_read`), `crates/lichen-highlevel/src/program.rs`
> (`TypeOperator::IsStructType`), and `crates/lichen-language/tests/`
> (`merge_carry.rs`, `field_read_kinds.rs`).
>
> Companions: [class-channel](class-channel.md) (the write-side half — a fact is
> decided once and stated on the class; this note is about the *read* side being
> taken too early), [lowlevel-vm](lowlevel-vm.md) (the unification and write
> rules), [unify-without-forcing](unify-without-forcing.md) (the wider question
> the open residue belongs to).

## 1. The problem

Lichen interleaves evaluation and unification in one graph: types are runtime
values, a check-time read can meet a cell nothing has bound yet, and a unify can
meet a computation. The ordering hazard this creates:

> An expression (or a check-time decision about one) reads a cell **before** the
> unification that would bind it has happened. The read finds the cell unresolved
> (`Parameterized`, undecided) and acts on that answer. The binding arrives later;
> nothing re-asks the question.

The runtime half is built around this (§3.1): a lazy answer is never cached, a
read of a pure cell joins the cell's class so a later bind reaches it, and a merge
carries the class's decided value to the members it adds. The **decision** half is
where the residue lives: the checker's guards and class questions read the graph
once, at the moment the expression is checked, and the answer they compute then is
final — including when the answer was "I cannot tell yet".

## 2. The measured matrix

Each pair below differs **only in statement order**.

### 2.1 A binop's result type: decided in both orders now

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
| read, then annotate | `20: ?a` before; **`20: Int`** since the merge carries the class's decided value |
| annotate, then read | `20: Int` |

The annotation makes the result type fully resolvable in both orders — the
annotate-first row proves it. In the read-first row `check_binop` asks its class
question while the field type is still a lazy read it cannot see through, takes
the polymorphic path (a shared open class and a pending `InDomain` assert), and
the value the annotation commits onto another member of that class now reaches
the result cell through the merge.

Controls that bound where the staleness lives: a bare read before the annotation
resolves (`10: Int`); two reads, one on each side of the annotation, both resolve;
and the same binop with no annotation at all is `20: Int`, because the apply-time
bind resolves what the check-time bind did not. So the stale case is precisely a
**check-time** unification (the annotation) binding the container after a
check-time class question already read the pending read and committed to an answer
shape. The apply-time bind wakes the graph; the check-time bind did not wake the
decision that read it — which is what §5.1 closes.

### 2.2 A container-kind guard: refused in both orders now

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
| read, then annotate | **refused**: `error: expected <?a, …>, found array<Int, 3>` — the read's pin meets the array at the annotation that binds it (before: `20: ?a`, **accepted**) |
| annotate, then read | refused: the decided container is judged where it stands |

The paren read `a(k)` **is** the tuple read, and arrays read with `a[i]`. The old
guard fired only when the container type was concrete **at check time**; an
undecided container "stayed lazy and resolved at the apply", and nothing re-ran
the guard there. The runtime `Index` cannot: arrays and tuples are both
`LowValue::Array`, so the runtime read of `[10, 20, 30](0)` legitimately computes
`10`. The deferral path silently dropped a check the other order enforced.

**Landed:** the guard is gone. `check_field` states the requirement as a unify —
an undecided container is pinned to a fresh tuple type `[?shape, [TypeTuple, K]]`
(the `check_index` mirror), and a decided container is refused where it stands
with the same stated requirement. Both orders refuse now, and what differs is
*whether* the program is accepted rather than which diagnostic fires.

### 2.3 The contrast case: apply **is** caught in both orders

```lichen
f = x => { a = x 1; p = x: Int; a }   -- read first
f = x => { p = x: Int; a = x 1; a }   -- annotate first
```

| order | output |
|---|---|
| apply, then annotate | `error: this value is not a function — it cannot be applied` (runtime `EvalError::ApplyTarget`) |
| annotate, then apply | `error: expected ?a -> ?b, found Int` (static, with a span) |

Both orders refuse — applying an `Int` has a runtime net (`evaluation.rs`'s
`Apply` arm records the failure) because a function is distinguishable from a
scalar at runtime. §2.2's pair differs in **whether the program is accepted at
all**, which is why a deferred check needs a net at the later tier.

### 2.4 The pin route is sound; the guard route was not

The container reads defer an undecided container by different mechanisms: `e[i]`
and `t{k}` **pin** the container's type to a fresh array/table type
(`check_index`/`check_table_find`), and the pin is enforced by the apply's
argument unify per call; `a(k)` **guarded** on concreteness and skipped when
undecided. Measured with no annotation anywhere, so no statement order is
involved:

```lichen
f = x => x(0)      f = x => x[0]
f [10, 20]         f (10, 20)
```

| read | before | now |
|---|---|---|
| paren `x(0)`, array argument | `10: Int` — **accepted**; nothing ever asked whether an array may be paren-read | `error: expected <?a, …>, found array<Int, 2>` — refused at the apply, the pin route |
| bracket `x[0]`, tuple argument | refused at the apply, statically, with a span | unchanged |

So §2.2's order flip was one face of a wider gap: the paren read's deferral had
no enforcing tier at all — not at the definition pass, not at the apply, and not
at runtime, because a tuple and an array are both `LowValue::Array` and the
lowlevel is untyped by design. The fix takes the pin route the bracket read
already used, which is only available because `a(k)`'s accepted set is now **one**
kind: a disjunction (tuple *or* struct) cannot be pinned, which is what §6.2's
ranking turns on. A tuple read of a tuple is unaffected, and `x[0]` over a tuple
still refuses as before.

The **named** reads (`check_named_field`, `check_raw_named_field`) are the guard
route — a struct type is refused by `a(k)` now, so `.name` and `::a` are a struct
instance's only reads — and their deferred half is measured the same way, again
with no annotation and no statement order:

| read | concrete container | deferred container (a parameter) |
|---|---|---|
| `l.a` on an array | `error: expected TypeStruct, found TypeArray` | before: two runtime diagnostics about tables; **now** those two *plus* `error: expected a struct type, found array<Int, 2>`, the read's registered condition firing at the apply (§5.2) |
| `l::a` on an array | `error: expected TypeStruct, found array<Int, 2>` | before: **a panic** — `unreachable!("TableGet target must be a table")`; **now**: refused at the apply, `expected TypeStruct, found array<Int, 2>` (the struct-kind unify) |
| `l.a` / `l::a` on a tuple | `expected TypeStruct, found TypeTuple` / `found <Int, Int>` | — |

Only `.a`'s **decided** tier is judged where it stands; its undecided tier
registers the requirement as a condition and re-checks it per apply (§5.2), which
is why the deferred array now carries a message about the read as well as the two
about tables — the table messages stay, because the apply's parameter check still
passes and the body the condition refuses still runs. `X::a` states its
requirement as a unify in both tiers, and its deferred half is refused by the
apply that binds the container, which is what closed the panic. The paren read's
hole was *silent* (a wrong program accepted and evaluated); the named reads' were
loud and mislabelled, and `X::a`'s was fatal.

## 3. The mechanism, as it stands

### 3.1 What the runtime does right

- A `Parameterized` answer is **never cached**, so any later read re-evaluates
  against whatever has since bound.
- A read of a pure cell **joins the cell's class** (`alias_read`), so a later bind
  replicates the value to the reader through `write_node_value`.
- **Unify is one recursion that does not compute first.** It merges the two
  classes, settles the value the merged class holds, and records the conflict; an
  operation on either side is not a reason to force it — what a class's
  computation produces is reconciled when it runs, and a reader that needs a value
  finds it through the class.
- A **merge carries the class's decided value**: `add_equality` reads what either
  side already knew before the union and fills the members that hold nothing. A
  member that already holds something keeps it, because a class routinely holds a
  different value on each member. A **write** is the other half and is
  unconditional, so one class has one value; the two rules are stated together in
  [lowlevel-vm](lowlevel-vm.md).
- An `Index` whose element is its own class — a lazy type slot that reads itself —
  answers from the class's committed value, or stays lazy, instead of re-entering
  the visiting reader and tripping the cycle guard.

The common shape of the successful answers: resolution happens **when somebody
asks again** (a re-read, a re-unify, the apply's clone recompute) or is **pushed
through a class merge**. There is still no registry of "who is waiting on this
class", and none is needed: the class is the registry.

### 3.2 Where the one-shot reads are

The checker's single pass asks class/concreteness questions and acts on the
answers permanently:

| site | question | acts on "cannot tell yet" by |
|---|---|---|
| `check_binop` | "has either operand stated a class" / "is one a float" | taking the polymorphic path — correct for the value, and the result type is fixed by the merge carry |
| `check_field` / `check_named_field` / `check_raw_named_field` | "is the container concrete" | skipping the guard; the named halves re-ask through a registered condition (§5.2) |
| `slot_read` | "is the position constant", "what is the field type" | keeping the lazy `Index` — sound, but any *reader* of that type cell before it resolves snapshots "unknown" |
| `check_convert` | "is the operand's low type known" | skipping the source-class unify entirely |
| `check_instantiate` | a one-time force of the callee | deferring the nominality check to the apply's argument unify |
| the kernel boundary (`lichen-compute`) | the parameter domain | refusing — the documented hard boundary, the honest version of the same snapshot |

A type read that must be recognised may be a **bare field read** (a lazy `Index`)
or a `type_of` call (an ordinary function, so a lazy `Apply`); both stall the same
way and both meet the type value through the class. A gate that matched only one
form silently missed the other.

### 3.3 The traced chain of §2.1

For the **read-first** order, with `T₁` the first read's lazy type and `T₂` the
second's:

1. `check_field` leaves each read's type as `Index(Index(container_ty, 0), k)`:
   the container type is still an undecided cell, so `slot_read` keeps the lazy
   form and neither read is decided.
2. `check_binop` finds neither operand stated, takes the polymorphic path, and
   unifies the two reads **with each other**. Both are pending `Index` reads, so
   the unify merges their classes with a bare `add_equality` — no value and no
   computation to carry.
3. The statement's skeleton epilogue unifies the skeleton's type cell into that
   same class, again with both representatives undecided, so again nothing is
   carried. The class now has three members and holds no value.
4. The annotation unifies the **template's** parameter type cell — still an
   undecided cell — with the tuple type. That is a concrete value, so the write
   reaches that class: the annotation decides the container, and therefore what
   the lazy reads of steps 1–3 resolve to, but it touches no member of *their*
   class.
5. The definition pass deep-evaluates the body. `T₁` now reads through the bound
   container to the field's type, and the evaluation postlude caches that value on
   the operation node and replicates it over the class. Replication reaches the
   members — the write rule is unconditional — so the class is decided.
6. The apply wires its result cell: `wire_apply_result` unifies the fresh cell
   with the cloned return's type, which is the class of step 5. That class is
   already decided, and this is the moment the **merge** must carry its value to
   the cell it adds; before §5.1 the merge read only the two representatives'
   slots, so the cell joined a decided class and never received the value — the
   printed type read `?a` while the value `20` arrived normally.

So the read was not "taken too early and never re-taken" at the *value* level —
the class committed on time. What was one-sided was the invariant: a class's
decided value reached its members on every **write** and on no **merge**, and a
merge is the only other moment a class gains members. A cell that joined the class
after the commit was never told.

### 3.4 Errors are permanent

A recorded diagnostic is never retracted. Every guard that *skips* when undecided
is therefore safe-by-construction (it can only under-report), but the flip side
exists: any evaluation a check-time force performs that records an error against a
not-yet-bound graph has recorded an error a later binding would have prevented. A
wakeup design must decide whether woken re-checks may retract; §6.1 records the
answer.

## 4. How other languages handle it

Systems that **separate** types from evaluation never have the problem: Haskell
and OCaml run HM inference as its own phase over pure type terms, and nothing is
evaluated during inference. Lichen cannot take this route: its types are runtime
values by design.

Systems that **do** interleave all land on suspended work keyed on what would
unblock it, woken when that happens: Agda's constraint `Blocker` set; GHC's
constraint solver kicking out inert constraints on unification
(`kickOutRewritable`); Lean 4's `SyntheticMVars`; and Prolog coroutining
(`freeze(X, Goal)` / attributed variables), the oldest form and the one lichen's
runtime is closest to — a constraint store rather than a Haskell-style inference
phase. Lichen's class is that store's key: nothing has to be registered, because
a merge already visits every member, and a later read of a pure cell joins the
class.

## 5. Design

### 5.1 Runtime: the class as a channel (landed) and blocked operations (sketch)

**Landed.** §3.3's chain needs only the smaller half: `add_equality` now carries
the merged class's decided value to the members the merge adds
(`fill_class_holes`). The value is read through `class_committed_value`, which
finds it on whichever member carries it rather than only on the representative's
own slot — the merge previously read just those two slots, which is exactly why a
value committed onto an operation-bearing member was invisible to it.
`write_node_value`'s distribution half is factored out as `propagate_class_value`
so the write site and the merge site state the same invariant once.

Two properties of the landed form are deliberate. It never writes over an
operation-bearing member's **run state**: the member's slot may hold the class's
value while `runned` stays false, so the operator still owes its own answer. And
the *merge* joins the two representatives' low types (the lattice join) but does
not run the variant-tag **observation**, which belongs to a class *gaining* a
decided value — this merge adds no value, only members. Nothing is forced and no
pending computation runs, so no program starts computing because something bound;
what changes is that a cell added after the commit reads the value its class
already had.

**Sketch.** A **blocked list** on each class — the operations that evaluated to
`Parameterized` because they read the class while it was undecided — drained on
commit by forcing each one remains a proposal. **Neither measured row needs it**:
§2.1's missing fact was a value the class already had, and §2.2 is not a runtime
question at all. It would be the honest fix for a computation — not a read — that
must start when what it waited on arrives, and its watch-points are unchanged:
re-entrancy (a drain runs evaluation inside a bind that may itself be inside a
unify inside an evaluation, so the visit-mark invariant and the depth budget both
apply); GC (a blocked edge keeps the waiter alive, and a waiter nobody else
references is what "undecided forever" currently *means*, so draining must not
resurrect semantics by keeping dead nodes live); and the template/clone split (a
template's parameters never bind — its blocked list must not fire).

This sketch is also one horn of a wider question — whether the pending/force/
commit machinery can be replaced by "unify regardless of whether a node carries
an operation" — analysed, with the measurements that bound it, in
[unify-without-forcing](unify-without-forcing.md).

### 5.2 Checker: guards as re-checkable registrations

The assert channel already is the model: a condition registered once, evaluated
when it can be, re-checked per apply clone
([operator-polymorphism](operator-polymorphism.md) §3). A guard that
skip-when-undecided can register there instead: skip now, **re-ask at the apply's
argument unify**, where the runtime net cannot help (the tuple/array confusion is
invisible to `LowValue::Array`). What that needs is a *condition the runtime can
evaluate* — a type-level "is this a positional type" operator beside
`TypeOperator::InDomain`, whose answer is `USize(0/1)` over a type value — plus a
spelling for the diagnostic it records.

**Landed for the named read `a.name`.** The operator is
`TypeOperator::IsStructType` (tag 18): operand `[container type, universe]`,
answering `USize(0/1)` through the same tag-based reader the decided tier judges
with (`shape::is_struct_type_any`), so the two tiers cannot drift; the universe is
an operand because the runtime has no canonical universe node to read.
`check_named_field` registers it, in the **undecided** tier only, with a new
`AssertSpelling::StructKind`, which renders `expected a struct type, found
<container type>`. Measured: the condition fires for an array argument and stays
silent for a struct — named or a block's record — through the same apply clone;
the decided tier remains the kind unify it was; and the refusal is **added to**,
not instead of, the two runtime table messages, because the apply never fails its
parameter check and so still runs the body it refuses. `X::a` was already
enforced in both tiers (its kind unify binds the container's own type cell, which
the parameter check does meet), and the paren read keeps its unify.

**The paren read did not need it** (landed): narrowing `a(k)` to tuples made its
accepted set one kind, so its check is a plain **unify** — `check_field` pins an
undecided container to a fresh tuple type and refuses a decided non-tuple
outright, stating the same requirement. The predicate pair
`is_positional_type_any`/`is_positional_type` was the disjunction and is gone with
it. The §6.2 option-1 machinery is what the **named** read's undecided tier takes
instead, because its accepted set *cannot* be narrowed to one kind.

One structural consequence, measured in `tests/graph_structure.rs`: a positional
read of a *parameter* used to put one cell per read into the parameter's type
tuple, so an unapplied function's arity was readable from it; the pin replaces
that with one open tuple type, so the arity is no longer readable before the apply
and the consumer sizes its placeholder at its ceiling and trims.

The one-shot *class questions* needed no change on the question side: the
polymorphic path is correct for the value, and the result type it leaves open was
fixed by the commit reaching the class (§5.1) rather than by asking later.

### 5.3 What each measured row needs

| row | fixed by |
|---|---|
| §2.1 `20: ?a` | **landed** (§5.1): the merge carries the class's decided value to the members it adds |
| §2.2 accept/refuse flip | **landed**: the paren read is a unify against a tuple type (§5.2), so both orders refuse |
| §2.4 paren `x(0)` over an array | **landed**: the same unify, refused at the apply |
| §2.4 named `x.a` over a non-struct | **landed**: the undecided tier registers `IsStructType` as a condition, re-checked per apply (§5.2), so the refusal is a check diagnostic as well as the two runtime table messages; `X::a` was already refused in both tiers by its kind unify, which closed its panic |
| §2.3 | already caught; only the diagnostic quality differs by order |

## 6. Open questions

### 6.1 Recorded decisions

- **No retraction.** A woken re-check may only *add* a diagnostic, never retract
  one (§3.4). The landed change records none at all, so it leaves the error stream
  as order-independent as it found it; retraction stays rejected, because it would
  make the error stream order-dependent in a new way.
- **Binding is not strictness.** The landed carry propagates a value the class
  already holds and forces nothing, so a program that stays lazy today stays lazy.
  A future drain would have to answer this on its own — its whole effect is to
  make a bind wake a computation, and the budget guards would make that
  observable.
- **The paren read is the tuple read, and every struct field is named.** Chosen so
  that `a(k)`'s accepted set is **one** kind, which is what lets its check be a
  plain unify instead of a re-checkable condition. The cost is a language change,
  taken deliberately: `s(0)` and `struct<Int, Type>` are refused, and a struct
  instance reads by name only — which is also what makes the named reads' gap the
  *only* way to read one. A block's record is unaffected beyond that: its fields
  are its **bindings**, and an expression statement is an ordinary statement. The
  alternative was purely additive (keep the disjunction, register the condition),
  and it is what the named reads still need, §6.2.

### 6.2 The fix direction (the paren read landed; the named reads open)

The gap is that the paren read could not simply copy the bracket read's pin: a pin
is a unify against **one** concrete kind, and `a(k)` accepted a *disjunction* — a
tuple type **or** a struct type. An open marker cell cannot express "tuple or
struct, not array": whatever flows in binds the cell, `ArrayType` included. The
options, in the order the analysis ranked them (the ranking is **reasoned from the
encoding**, not measured — only §2.4's two rows are measurements):

1. **Re-checkable assert (landed for `a.name`; see §5.2).** Keep skipping the
   static guard when the container is undecided, and register the condition on the
   assert worklist exactly like `InDomain` and the bounds assert: a
   `TypeOperator` over the container type answering `USize(0/1)` for "a struct
   type" — the predicate to wrap is `shape::is_struct_type_any` — plus an
   `AssertSpelling` for the diagnostic. The machinery is the one §2.1's
   polymorphic binop already rides: pending at the definition pass, re-checked per
   apply clone. That closes **both** faces: §2.2's same-scope flip (the definition
   pass evaluates the assert once the annotation binds) and §2.4's template hole
   (the clone's re-check meets the actual argument). Measured cost: the named read
   refuses `x.a` over an array with a check diagnostic at the apply, but **not**
   instead of the runtime table errors — the apply's parameter check still passes,
   so the body the condition refuses is still evaluated and records its two table
   messages first. Only option 2's pin refuses *before* the body.
2. **Shape-pin plus the same assert.** Additionally pin the undecided container to
   the skeleton `[shape, [marker, K]]` with an open marker — the shape
   `check_index` pins, minus the kind — so `slot_read`'s lazy `Index` reads a
   decided structure and a lowering sees a field list before the kind is known.
   Strictly more than option 1, and the only ranked route that makes the apply's
   parameter check fail (so the refused body is never evaluated, and no runtime
   table message is recorded); it was measured to cost the compute extension,
   which is why option 1 landed without it.
3. **Runtime net — impossible by design.** Evaluation cannot distinguish a tuple
   from an array (both are `LowValue::Array`) and must not: the lowlevel is
   untyped. This is why §2.3's apply has a net and §2.2's read cannot have one.
4. **A settle pass (re-ask skipped guards after the statement pass) —
   insufficient alone.** It fixes §2.2's same-scope flip but not §2.4's template
   hole: a template's container legitimately stays undecided, and only the
   per-apply re-check can refuse a wrong-kind argument.

**The parked experiment that tried the direct pin.** Pinning the undecided tier's
container to the skeleton, and stating each read form's accepted kind as a unify
on the container's own cell, was tried in three parts: `X<e>` requiring the tuple
kind, `X::a` requiring a struct kind, and `.a` stating its requirement on the
container's kind slot; then keeping `X::a`'s requirement for both tiers while
`.a` keeps the lazy name-table read when undecided; then stating the accepted kind
in tests, the example and the docs. It fixed a false diagnostic it exposed — a
*second* named read on the same undecided container was refused as "no field with
this name", because the first read's pin made the container look decided while its
name table was still a cell — and it is **parked, as a measurement of the rejected
route**, because the undecided tier's term-shaped pin binds the container's own
**type** cell, so a consumer that reads a type structurally through the class —
`lichen-compute`, which forces a template's parameter type before any apply —
reads the pin's open cells instead of deferring through the class, and the compute
suite collapses. What it needs is option 1's **re-checkable assert**, which is
what has since landed for `.a`. Whoever needs the pin's earlier refusal can lift
its refusals, its diagnostic fix and its docs, and must **re-measure** the compute
suite rather than trusting any number here.

**The narrower kind-slot pin is inert — measured, not landed.** Stating `.a`'s
requirement for the undecided tier as a unify on the container's **kind slot**
alone (against a fresh struct kind — the decided tier's own statement) refuses
nothing: `f = x => x.a; f [10, 20]` and its call-result form each keep the two
runtime table messages. The unify already **merges**, but the pin is invisible to
the apply: `apply_parameter_check` unifies the parameter's `[value, type]` pair,
whose type element is the container's own type cell, while the requirement node is
a derived read the checker allocates outside it. A pinned read holds a decided
array and is never re-evaluated, and suppressing the pin changes nothing either.
The deferral itself is not optional: without it the same unify is a check-time
conflict (`expected TypeStruct, found ?a`) that refuses the struct case too. Only
the whole-term pin refuses at the apply, and it still costs the extension. Option
1's assert is the route that landed for `.a`; the whole-term pin stays the
rejected one.

### 6.3 The message/predicate disagreement

> **Superseded in part (measured).** The struct marker became a **tag**
> (`[payload, TypeStruct]`), so the three read sites no longer share one wording
> and `DiagKind::IndexTarget` has no producer left: `.a`'s decided tier refuses
> `expected TypeStruct, found TypeArray` (or `TypeTuple`), `::a` refuses
> `expected TypeStruct, found array<Int, 2>`, and `.a`'s undecided tier renders
> `expected a struct type, found …` (§5.2). The analysis below is the record of
> the pre-tag state; the wording it asks for is what landed.

`DiagKind::IndexTarget` was shared by three guards with three different accepted
sets, and its single message named a kind **none** of them accepted: `check_field`
accepted a tuple or struct but the message claimed array too; the two named sites
accepted only a struct but claimed a tuple and an array as well. Two spellings
suffice — positional for `a(k)`, named for `.a`/`::a` — and the named read's
registered condition reuses the named one rather than inventing a fourth wording.
The `TableGet` arm needed no recorded-failure form after all: the checker side
closed it by leaving a refused read **unbuilt** (`Checker::refused_pair`), so no
`TableGet` against a non-table is ever built and the lowlevel arm stays the
invariant its message claims.

One drift found while reading: the [spec](../language-spec.md) §Indexing says a
concretely non-indexable `e[i]` "is an `IndexTarget` diagnostic at check time" —
but `check_index` pins and fails through `DiagKind::Guard`, at check time for a
concrete container and at the apply otherwise. The spec sentence predates the pin
and is corrected to describe the Guard-style refusal.

### 6.4 Still open, unanswered

- Should a woken re-check be able to **retract** a diagnostic, if some future
  wakeup needs it? Answered *no* for now (§6.1), on the order-independence
  argument alone.

## Recovered measurements

- Every lambda's value node is deep-evaluated before any application, in the build
  itself. Without that pass the first application deep-evaluates the *parameter
  clone* of the template; a second application then finds that clone already bound
  and reuses it instead of cloning the parameter fresh, so the argument unify
  conflicts and the recursion cannot descend. With the pass the function value
  itself is concrete before any application, every recursion level re-applies the
  template, and each level clones its parameter fresh.
- The single-node run is the right strength for both one-shot reads
  (`Checker::compute_operands`, and `element_read`'s inline force): the operand is
  *one* node whose own operator reads what it needs, and the chain it needs is
  followed through the operand edges `evaluate_node` walks anyway — for a raw
  element read, the element, the name table's `TableGet` and the container's own
  value. A deep pass would also descend the operand's whole reachable subtree and
  publish a concreteness verdict over it — for a type operand, the *entire type
  value*, every field pair and every field's type expression, for every raw read —
  which is both stronger than the claim needs and wrong in kind, since it decides
  concreteness for nodes the check never asked about.
- `check_field`'s decided tier is refused outright: its kind is readable, so the
  term is judged where it stands (`kind_marker_is_any`) and the refusal is recorded
  as a fact through `record_guard`, not as a failed unify — which keeps the *found*
  side the container's own type instead of a shape a failed unify would have bound.
  An out-of-range slot is not a check at all: the runtime `Index` read records
  `IndexOutOfBounds`, reporting the container's actual arity.
- A known field position is what makes a field read's type decided rather than
  lazy: a concrete container type states it (the named form resolves the name
  through the type's own name table for its guard; the positional form's key is a
  literal), so the type is read straight out of the field list
  (`shape::field_type`) — the same node an `Index(Index(container_ty, 0), key)`
  would evaluate to, *one unify earlier*. That earliness is observable because a
  class question is asked of a **cell** (`shape::low_type_of_slot`), which cannot
  see through an unevaluated `Index`: `(x : struct<.a Float>) => x.a + x.a` used to
  find neither operand concretely `Float`, pin the operation to the `Int` default
  in `check_binop`, and then refuse both operands against it.
- A constant subscript is not an optimisation but the whole difference for the
  lowering: a kernel body is walked by its operands, and a name table's `TableGet`
  is not a value anything can resolve without re-deriving the struct's field order,
  so a concrete read's field position is written where it is decided (`slot_read`'s
  type read is the other half of the same decision). The lazy form stays for the
  one case that needs it: a container whose type is not known yet.
- The deferred named instantiation's instance definition position `i` is the
  argument that supplies the field whose name sits at `names_in_order[i]` — the
  marker's definition-order names (`shape::STRUCT_KIND_NAMES_ORDER_PATH`), the
  inverse of the name→index table a lazy read cannot derive:
  `value[i] = Index(call_values, TableGet(supply, key(i)))`. `supply` is a constant
  table the checker builds from the source. An argument's key is its **name**, or —
  for a positional argument — its **rank** among the positional ones; a definition
  position whose name no argument supplies is the positional `rank`-th unclaimed
  position (`rank` counts the earlier definition positions whose names are
  unsupplied, the running sum of `InDomain(names_in_order[i], supplied_names)`),
  and with no positional argument there is no fallback to select and the key is the
  name itself. The key also carries the **type** whenever every argument's type is
  decided (`[tag, argument type]` against `[tag, field type]`), so the lookup's own
  content comparison *is* the per-field type check: a field whose declared type no
  supplying argument matches is a miss. That is the only form the check can take
  here — a `unify` between the gathered type and the field list is deferred and
  pinned before the marker binds (the callee's shape half unifies before its kind
  half, so no name-dependent read can resolve yet), and a pinned read is masked
  from then on; the lookup, by contrast, is *evaluated* when the instance is, and
  the lowlevel's table read deep-evaluates its key. Two facts stay check-time,
  because neither depends on the struct type: a **duplicate** name is refused
  whatever the field list is (one name supplying two positions is a structural
  mismatch, and a name-keyed table would silently keep only one), while a
  **missing** argument is not — the returned field-type list is a probe with one
  cell per argument, so the caller's field-list unify is an arity check that fires
  when the type resolves.
- Two measured regressions sit behind the message and name-table rules. The kind
  read has to run before the unify states the requirement: `container_kind` is an
  `Index` operation, and an operation that has not run holds no value, so unifying
  it against the requirement takes the "one side knows a value" arm, which writes
  the requirement into the read's class and asks nothing — the guard passes over an
  array or a tuple instead of refusing it. Measured: `l.a` on `[10, 20]` reported
  the generic "not a container" plus a name-table miss, where the raw sibling
  `l::a` on the same container states `expected TypeStruct, found array<Int, 2>`.
  The shared "tuple, array, or struct" wording is likewise what this read does not
  accept, and is why the *kind* is the requirement rather than the shape. A
  container whose type is stated through a class is readable: `inbuf`
  (`inbuf = compute.plrun …`), whose type is the callee's returned `k.O`, has names
  the structural reader can resolve — but a lazy `TableGet` built for it resolves
  the name against a name table the statement pass has not materialized, which
  leaves the read and everything downstream undecided; `type_is_concrete` answers a
  narrower question ("the cell holds an array right now"), which is why the lookup
  is asked whether or not the cell is decided, and a container whose names are
  genuinely unreadable still gets `None` and keeps the lazy form. "The name table
  is readable" is itself a separate condition: a readable table that lacks the name
  is a genuine miss, while one that is still undecided is only undecided and the
  read stays lazy — without the distinction a *second* `k.name` on the same
  undecided container is refused falsely, because the first read's pin is an array
  value, so the container now looks decided while the pin's name table is a fresh
  cell.
- An unevaluated callee (a call result, `(mk (Int))(1, 2)`) has no statically
  readable pair — it is an apply node, not an array — so its evaluation is forced
  for the nominality check and the field-list read. A callee that depends on an
  undecided parameter stays lazy (the checks defer to the apply), and a
  non-terminating one trips the VM's guard: it is left lazy, because the build's
  statement pass evaluates the statement again and reports the `NonTerminating`
  diagnostic. A refused callee leaves a partly walked graph, so the checker must
  never force twice — `force_failed` records the first refusal.
- The apply's result type cell is undecided unless the apply's evaluation syncs it:
  the cell rides in the apply's operand, and the runtime apply unifies the return
  pair with the apply node — the apply node *is* the return pair — and binds the
  cell to the return type, so a concrete result syncs its type while a polymorphic
  template's lazy result leaves it undecided.
- `ExprKind::Function::parameter_type` is compiled *in body scope*, so in-body
  readers of the parameter see the annotated kind — an array annotation's length, a
  function annotation's arrow — while the parameter's type slot still performs the
  argument check at each apply: compiling in body scope changes what the body sees,
  not what is checked. `parameter_attribute` (`x # n => e`) is compiled in body
  scope for the same reason and is the optimization for the `x # n => e` →
  `x => { x # n; e }` desugar (an unannotated body statement would otherwise
  materialize a block); the field rides the `Function` node.
- The `Loc` descent exists so the language layer need not re-derive the type
  grammar: the highlevel is deliberately source-blind — it never sees a source span
  — so a location must be expressible purely in terms of the expression's
  structure. The highlevel *does* parse each level of that structure, tagging it as
  either an expression's `[value, type]` pair (a `LocStep::Value` /
  `LocStep::Type` / `LocStep::Attr` slot) or a tuple/array/struct shape (a
  `LocStep::Elem`). There is no distinct "kind" in lichen: `kind` is just the
  type's type, one more `[value, type]` pairing, and that chain is unbounded
  (`Type : Type`), so a `LocStep::Type` may repeat arbitrarily.
- A call's result type cell is a lazy record, so an annotation on a call result
  *binds* the cell at check time and nothing more. The check happens later and
  elsewhere: the apply's evaluation syncs that cell with the callee's return pair,
  and a disagreement between the annotation and the real return type is what gets
  reported.
- `if x > 0 then int else float` is a dependent type: an unevaluated `Index` over
  the branch types with the parameter as its condition. It stays lazy until forced,
  so the branch selection only resolves once the argument binds, and the *same*
  template yields a different type per argument. The checker's expression IR cannot
  express conditionals yet, which is why those tests build lowlevel graphs
  directly.
- A concrete expectation meeting an `Index` read over an undecided element with a
  concrete index resolves the read to a pure reference — the operator node is
  aliased to the element — and the concrete value is written onto it: the
  "monomorphized" trade. The read keeps its operation: the operand edge stays live
  so an apply's clone can reach the element and *enforce* the pin, and a
  conflicting expectation later fails against the pinned value. The read's
  subscript is only known by evaluating the read, so the alias is established by
  the evaluation (`alias_read`), not at unify time.
- The raw reads state their container kind: `X<e>` reads a component of a **tuple
  type value** (the container's kind is unified against `TypeTuple`), `X::a` a
  struct type value's named field, and `.a` / `::a` over an array are the two named
  sides of one refusal — `.a` states the container's *kind*, `::a` its whole type.
  A tuple *value* is not a tuple *type* value: `(1, 2)`'s type is the shape
  `<Int, Int>`, not `TypeTuple`. A deferred container is refused at the argument,
  where this used to be an internal panic at the apply.
