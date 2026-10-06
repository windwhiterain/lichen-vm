# An applied struct type expression is not a function

> Status: **fixed** on `feature/applied-struct-nominal-id`. A struct type's
> identity payload `[id, names, names_in_order]` (the value half of the marker
> pair `[payload, TypeStruct]`) is now decided when the expression is **checked**, so
> one written occurrence is one nominal type however many times it is
> evaluated; §1's repro runs, §2's controls still hold, §6 records the fix and
> the two mechanisms instrumentation isolated.
>
> Companions: [defer-pending-type-forms](defer-pending-type-forms.md) and
> [type-rendering-inconsistent](type-rendering-inconsistent.md) — the other two
> spelling/identity defects found the same way.
> Unblocks: [compute-kernel-struct](compute-kernel-struct.md)'s follow-up (a
> kernel's JIT'd input type is built by the JIT and named by the host, which is
> exactly the two-evaluations case below) — §5's workaround is no longer needed.
> Points at: `crates/lichen-highlevel/src/checker.rs` (`struct_type_type` and
> `fresh_nominal_id`, the construction that pins the identity),
> `crates/lichen-highlevel/src/checker/structs.rs` (the struct type's two
> emitting sites), `crates/lichen-lowlevel/src/function.rs` (`node_apply`'s
> clone rule), `crates/lichen-highlevel/src/shape.rs`
> (`STRUCT_MARKER_ID_SLOT`).

## 1. Minimal reproduction

```lichen
A  = I => struct<.n Int, .I I>
In = struct<.x _, .y _>
S1 = A In
S2 = A In
x  = S1(.n 3, .I In(.x 10, .y 20))
y  = (x : S2)
y
```

Before the fix:

```
error: expected struct<.n Int, .I struct<.x Int, .y Int>#0>#2,
          found struct<.n Int, .I struct<.x Int, .y Int>#0>#1
```

`S1` and `S2` are two evaluations of **one written struct expression** (`A`'s
body) applied to **one argument type**, and they were two nominal types.

The **inner** struct was shared — both sides printed
`struct<.x Int, .y Int>#0` — because `In` is a *binding*: the frontend resolves
the name to one `ExprId`, so its struct expression is compiled once and keeps
one id. The **outer** struct was not, and that asymmetry was the defect.

After the fix the program runs:

```
(3, (10, 20)): struct<.n Int, .I struct<.x Int, .y Int>>
```

## 2. What must not change

Two measured controls, both **correct before and after the fix**, and both
still measured against the fixed checker:

**Different arguments stay different types.** A type constructor is a function,
not a constant:

```lichen
A  = I => struct<.n Int, .I I>
S1 = A Int
S2 = A Float
x  = S1(.n 3, .I 5)
y  = (x : S2)
```

```
error: expected struct<.n Int, .I Float>#0, found struct<.n Int, .I Int>#0
```

Note the two sides now carry the **same** nominal id (`#0`): one occurrence,
two instantiations. They conflict because the field types ride in the shape, so
the two *type expressions* differ. §3 of the pre-fix note inferred that a
one-id-per-occurrence fix would merge `A Int` and `A Float`; that inference was
wrong, and this is the measurement that refutes it — it is the reason the fix
needs no field types folded into the id.

**Two written declarations stay two types.** Nominal typing is the point of the
id, and two `struct<.n Int>` written apart are two declarations:

```lichen
S1 = struct<.n Int>
S2 = struct<.n Int>
x  = S1(.n 3)
y  = (x : S2)
```

```
error: expected struct<.n Int>#1, found struct<.n Int>#0
```

**Distinct argument types stay distinct.** Sharing is per occurrence, not per
field-type *value*: the same occurrence applied to two written-apart (but
structurally equal) struct types is still two types.

```lichen
A   = I => struct<.n Int, .I I>
In  = struct<.x Int, .y Int>
In2 = struct<.x Int, .y Int>
S1  = A In
S2  = A In2
x   = S1(.n 3, .I In(.x 10, .y 20))
y   = (x : S2)
```

```
error: expected struct<.n Int, .I struct<.x Int, .y Int>#2>#0,
          found struct<.n Int, .I struct<.x Int, .y Int>#1>#0
```

The outer occurrence is shared (`#0` on both sides, so the fix is in force) and
the conflict is entirely the two argument structs' own ids (`#2` vs `#1`).

## 3. Mechanism

Measured, by tracing the two candidate sites (an `eprintln` in the checker's
struct-type construction and in the apply clone walk):

- A written struct expression is compiled **once**. The pre-fix repro's two
  struct expressions were checked once each — `ExprId(9)` (the inner) and
  `ExprId(1)` (the outer) — which is `check_term`'s per-`ExprId` cache doing
  what its comment says.
- The `Fresh` operator nevertheless ran **three** times: once for the inner
  struct and **twice for the outer**, once per application of `A`.
- The clone walk is where the second run comes from. `node_apply` copies a
  member node unless the deep pass proved it concrete
  (`evaluated_deep == Some(EvaluatedDeep { parameterized: false })`). The outer
  struct's `Fresh` node was reported as `deep=None` — nothing had proven it —
  so it was copied per apply, and the copy re-ran the operator (the walk drops
  a copied operation node's cached value, so the clone recomputes). *Why* the
  deep pass had not reached it was not traced; the fact that it had no verdict
  is what the clone rule reads.

So the pre-fix note's inference was right, and its "distinguishing question"
(where the `Fresh` runs) is answered: **it ran at evaluation**, and evaluation
happened once per application.

**The id is not the whole identity.** With the id alone pinned, the repro still
failed — this time on the *name table*. Tracing the remaining failure
(`apply_parameter_check`'s recorded `UnifyError`) showed the conflict at step
`[1, 0, 0, 1]` of the type expression — the kind's marker pair, its payload's
names slot —
between two `LowValue::Table` payloads at different addresses. The name table is
a value node the clone walk also copies, and a copied table is a **different**
table (a fresh handle with the same entries) that does not unify with the
original. The identity that must be per-occurrence is therefore the whole
marker payload `[id, names, names_in_order]`, not the id alone. (An *anonymous*
struct has
`LowValue::Error` in that slot, which copies to an equal value — which is why
`struct<t>` in a function body appeared to work, and why the defect looked like
it was about named fields.)

## 4. Why it mattered

A type constructor that is not a function breaks the reasoning every caller
needs:

- `A In` and `A In` were not interchangeable, so a type could not be *named* by
  writing its expression — only by binding it.
- Generic equality over such a type failed for the same reason: two occurrences
  of one type were not equal.
- It made the *natural* spelling of a derived type wrong, and the workaround
  (bind it once) invisible: nothing in the language said which spellings were
  load-bearing.

The blocked case was concrete. A parallel kernel's JIT'd signature is
`struct<.n Int, .I I> -> O`, built from the author's parameter type
`struct<.n Int, .in I, .out O>`: the JIT constructs the input type, and the host
names it to build the argument it passes. Those are two evaluations of one
expression applied to one argument, so they did not unify — and the only way to
make them was to hand the host a binding for a type the JIT was supposed to
derive.

## 5. The workaround, and its cost

The pre-fix workaround was to bind the derived type once and use the binding on
both sides:

```lichen
Sig = A In      # one occurrence, shared by the JIT and the host
```

It worked and it was what `ir.rs` documented. Its cost was API shape: the JIT
had to *receive* the type it should have *derived*, an extra argument on every
call that needed the derived type to be nameable.

**It is no longer needed** — §1's spelling is now correct — and the `ir.rs` note
that stated it as *the* mechanism has been replaced by the fixed statement.

## 6. The fix

The struct type's identity is pinned at the single point that builds the
encoding, `Checker::struct_type_type` in `crates/lichen-highlevel/src/checker.rs`:

```rust
let marker = self.struct_marker_node(id, names, names_in_order);
self.module.evaluate_node_deep(marker, None);
```

Deep-evaluating the marker decides its value **while the occurrence is being
checked**, and the clone walk then reads a proven-concrete node and references
it in place instead of copying it. Both halves of the identity are covered by
the one evaluation: the nullary `Fresh` node and the constant name table are the
marker's items, and each gets its own verdict from that descent. The same
verdict is what `static_module/freeze.rs` reads to mark a solved module's node
non-parameterized, so a persisted artifact bakes the identity too rather than
re-minting it per materialization.

`Checker::fresh_nominal_id` remains the id's allocation point (one `Fresh` node
per emitting site, so one id per written occurrence), and the emitting sites —
`check_type_struct` and `check_record` — are unchanged apart from calling it.

What was deliberately **not** changed:

- The `Fresh` operator, the marker layout, and the id space. The id still comes
  from the program's `HighGlobal::next_type_id`, so ids stay unique across a
  program and the counter tests keep their meaning.
- The clone walk's rule. Baking a proven-concrete node is its documented
  contract; the defect was that nothing had proven this node, not that the rule
  was wrong.
- Field types stay out of the id (they ride in the shape) — §2's first control
  is what this buys.
