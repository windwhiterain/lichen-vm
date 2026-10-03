# Evaluation runs before unification, and nothing wakes what it read

> Status: **proposed** — the defect is **measured on `dev@5e89bb1`** (§2's matrix
> is compiler output, reproducible from the inline programs); the node-level
> chain behind each row is named but not instrumented (§3); §4 is the
> comparison to other languages; §5 is the design sketch, not landed work.
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

Every program below was run through `lichen-compiler` at `dev@5e89bb1`.  The
pairs differ **only in statement order**.

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
| read, then annotate | `20: ?a` — the type **stays undecided** |
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
| read, then annotate | `20: ?a` — **accepted** |
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
among the accepted kinds while the predicate refuses it — the message and the
predicate disagree.)

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
through a class merge**.  There is no registry of "who is waiting on this
class".

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
passes), while nothing ever **commits** the observed member to the class, so
the result type cell reads `?a` forever (§2.1) and a skipped guard stays
skipped (§2.2).

The exact node-level chain of the §2.1 `?a` (which member of the shared class
the annotation's bind failed to reach, and why the apply's clone recompute
commits in the no-annotation row but not the annotate-after row) is **named
but not instrumented** — the first step of any fix is a probe build that
traces the commit/replication path for that class, in the style
[defer-pending-type-forms](defer-pending-type-forms.md) §2 describes.

### 3.3 Errors are permanent

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

Two halves, matching the two halves of §3.  Neither is landed; this section
is the shape of the work, not a plan of record.

### 5.1 Runtime: class-attached blocked operations

Give each union-find class a **blocked list**: the operations that evaluated
to `Parameterized` because they read this class while it was unbound.  The
list lives *on the class* (merged when classes merge — never keyed by
`NodeId`, which `find` changes under you), and `write_node_value`/`bind`
drains it on commit: forcing each blocked operation, which is exactly
`force_pending` turned event-driven instead of unify-driven.  Replication
already walks the class's members on every commit, so the drain hangs off the
same hook.

Watch-points: re-entrancy (a drain runs evaluation inside a bind that may
itself be inside a unify inside an evaluation — the visit-mark invariant and
the depth budget both apply); GC (a blocked edge keeps the waiter alive; a
waiter nobody else references is what "undecided forever" currently *means*,
so draining must not resurrect semantics by keeping dead nodes live); and the
template/clone split (a template's parameters never bind — its blocked list
must not fire).

### 5.2 Checker: guards as re-checkable registrations

The assert channel already is the model: a condition registered once,
evaluated when it can be, re-checked per apply clone
([operator-polymorphism](operator-polymorphism.md) §3).  The §3.2 guards that
currently skip-when-undecided (`check_field`'s kind guard is the §2.2 hole)
can register the same way instead: skip now, **re-ask at the first moment the
container's class commits** — which §5.1's drain provides — and at the
apply's argument unify, where the runtime net currently cannot help (the
tuple/array confusion is invisible to `LowValue::Array`).

The one-shot *class questions* (`check_binop`'s `stated`/`float`) are the
harder half: the polymorphic path they fall back to is correct for the value
but leaves the result type open (§2.1).  The minimal change is on the commit
side, not the question side: when the shared class's first member commits,
the commit should replicate to the class — the class-channel principle
([class-channel](class-channel.md)) applied to the case where the decider is
a read that resolved late.

### 5.3 What each measured row needs

| row | fixed by |
|---|---|
| §2.1 `20: ?a` | the commit reaching the shared class (§5.1's drain, or §5.2's commit-side replication) |
| §2.2 accept/refuse flip | the guard re-firing when the container's class commits (§5.2), or a runtime `Index` that distinguishes tuple from array — which the encoding currently cannot |
| §2.3 | already caught; only the diagnostic quality differs by order |

## 6. Open questions

- Should a woken re-check be able to **retract** a diagnostic (§3.3), or only
  add one?  Retraction makes the error stream order-dependent in a new way.
- The drain makes evaluation order observable through the budget guards: a
  program that today stays lazy could start computing (and hitting limits)
  because a bind woke it.  Is that the intended semantics — "binding is
  strictness"?  (Agda's answer is yes: instantiation wakes.)
- Does §5.1 subsume `defer_pending`'s pin, or coexist with it?  The pin
  commits early and reconciles; the drain commits late and re-evaluates.  One
  of them is the policy and the other an optimization of it, and which is
  which is a decision.
