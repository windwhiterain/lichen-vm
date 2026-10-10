# Function types, and a struct type's nominal identity

> Status: **current.** A written arrow lowers to a *function*, so a function's
> type is the function itself (`f : f`) and there is only one representation of a
> function type. Two functions unify the way two arrays do. A struct type's
> nominal identity is pinned when the type expression is checked, so one written
> occurrence is one type however many times it is evaluated.
>
> This note absorbs the still-true halves of *"A function's type is the function
> itself"* (the `f : f` half; its arrow half is what this note replaces) and of
> *"An applied struct type expression is not a function"*.
>
> Points at: `crates/lichen-lowlevel/src/equality.rs` (the function/array unify
> arm, the function-type recognisers, the signature reads),
> `crates/lichen-highlevel/src/checker/lambda.rs` (`check_signature`, and the
> `check_lam` wiring that makes a lambda's type its own pair),
> `crates/lichen-highlevel/src/checker.rs` (`struct_type_type`,
> `fresh_nominal_id`, `struct_marker_node`),
> `crates/lichen-highlevel/src/checker/structs.rs` (the struct type's emitting
> sites), `crates/lichen-highlevel/src/shape.rs`
> (`STRUCT_MARKER_ID_SLOT`, `low_type_of`'s function arm),
> `crates/lichen-render` (the `dom -> cod` spelling), and `crates/lichen-compute`
> (the kernel signature readers).
> Open questions are in [What is open](#what-is-open).

## One representation: a written arrow is a function

`A -> B` in a type position compiles to a **real function** — `_: A => _: B`,
a lambda whose parameter is annotated `A` and whose return is annotated `B`
(`Checker::check_signature`). The surface syntax is unchanged; only the lowering
is.

A function value's *own* type is then the same thing, and it needs no second
node: `check_lam` binds the function pair's type cell to the pair itself, so a
function's type is the self-referential node

```text
[Function(fid), ↺]
```

— value slot holds the function, type slot points back at itself, exactly the
shape the universe `K = [Type, ↺]` has. It is recognised as a type by that
self-cycle (`Module::is_function_type`), and distinguished from the universe by
slot 0: the universe holds the `Type` marker, a function type holds a
`LowValue::Function`.

Two consequences, and both are deliberate:

- **`f : f` is a consequence, not a rule anybody maintains.** The type chain
  closes at the function the way it closes at `Type`, so a function's type is the
  function (`overview.md`).
- **A function's signature is not in the type node.** It lives in the function
  template: the parameter is the `[value, type, attrs…]` pair the body binds and
  the apply clone walk clones (`Function::parameter`), and the return side is
  `Function::return_type` — the return's *type* cell, because `r#return` may be
  an unevaluated operation node whose own slots do not name the type. An apply
  clones those two cells fresh per call and never binds the template, so a
  function's signature is let-polymorphic: `f = x => x` accepts `1` and `1.5` in
  turn, and an annotation checks a clone rather than specialising the template.

`Type` is the **terminal** of the chain, not a supertype: there is no subtyping
relation anywhere (`Int` is not `<: Type`; a compound type is typed by its kind),
and **there is no occurs check** — cyclic types unify (equi-recursive
behaviour), which is exactly what lets the universe and a recursive struct type
exist. The self-cycle test below is a *shape* judgement inside that semantics, not
an occurs check.

## Functions unify like arrays

When both operands are function types, the unifier descends the two signatures
positionally: the parameter pairs unify against each other — value against value,
type against type, attribute against attribute — and the two return type cells
unify. When the elements agree the two classes merge, the same merge two arrays
with equal elements make. There is **no host hook and no clone**: the old
`Program` policy had to ask its host *where* a signature lives and *whether it
may be written*, and it answered differently for a dynamic function and a frozen
module's, which is how a wrapper in an imported module ended up reported as an
undecided struct. The lowlevel now needs to be told nothing about function types.

The descent names the parameter **pair**, not its type slot alone, so a signature
carries attributes and can constrain the *values* that flow through it:
`?a: Int => ?a: Int` puts one cell in both positions, so unifying a function
against it makes the argument and the result the same cell. That capability is
argued from the representation rather than measured: no test in the workspace
writes a value-constraining signature yet.

Three shapes the arm has to tell apart, and the judgement is the invariant rather
than a roster:

- **Both sides a function type** — descend as above.
- **A *degenerate* function type** — a `[Function(fid), t]` pair whose type slot
  names a *different* function. It is not a self-cycle, and the positional match
  is the right answer for it; `examples/closure.lichen` builds one, because a
  lambda whose body returns a nested closure gives the outer function a return
  type that *is* the inner function's type node.
- **A function type against a self-cycle that is not a function type** — the
  universe `Type`, or a recursive struct type. This is refused, so
  `(\x. x) : Type` does not check. A *roster* of forbidden names would have to be
  extended by whoever invents the next cyclic type and would be wrong the day
  they forgot; the self-cycle test is the invariant.

### Frozen functions

A frozen (imported) function's template is immutable, so its signature is
**copied** into fresh dynamic leaves rather than read in place and bound — the
frozen original must never move. This is not a corner case: the whole `core`
prelude is a frozen module ([core-prelude](core-prelude.md)), and its functions'
type nodes carry a static self-cycle. A read that must not allocate
(`function_type_signature`, a `&self` query) reads the immutable template
instead.

One logical function can be named through several refs — a dynamic closure and
the frozen function it was materialized from, or two modules' re-exports of one
imported binding — so unifying values that name it through different refs must
merge, not conflict. `Module::function_identity` follows `Function::static_origin`
(dynamic) and `StaticFunction::origin` (static) to the one real function;
`function_identity_equal` compares identities. Without it, a re-export unified
against its original compared two `Function` values and conflicted.

## A struct type's nominal identity is pinned per written occurrence

A struct type expression compiles to the pair `[shape, kind]`, and its nominal
identity is a marker of the form

```text
wrapper = [ shape, kind ]
shape   = [ field types… ]
payload = [ id, names, names_in_order ]
marker  = [ payload, TypeStruct ]
kind    = [ marker, K ]
```

`Checker::struct_type_type` is the single construction point, and it
**deep-evaluates the marker** while the occurrence is being checked. That is what
makes one written occurrence one nominal type:

- The `id` is a `Fresh` node allocated per emitting site
  (`Checker::fresh_nominal_id`), and the name table is an arena payload. Both are
  computations the apply clone walk would otherwise copy per application — and a
  copied table is a *different* table that does not unify with the original. A
  node the deep pass has proved concrete is referenced in place by every clone,
  so the identity survives application; the same verdict freezes it
  non-undecided in a static module, so a persisted artifact bakes the identity
  rather than re-minting it per materialization.
- The **field types stay out of the identity** — they ride in the shape. An
  occurrence applied to two different arguments therefore stays one occurrence
  (`A Int` and `A Float` share the id and differ because their shapes differ),
  and two `struct<…>` written apart remain two declarations because each written
  occurrence allocated its own id. Without the pin, `A In` written twice was two
  nominal types, which made a type constructor that is not a function: a derived
  type could not be named by writing its expression, only by binding it once.

The identity matters wherever a type is built by one layer and named by another —
a kernel's JIT-built input type and the host's argument type are two evaluations
of one expression applied to one argument, and they must unify.

## What is open

- **The frozen written-arrow reduction is closed.** A written arrow annotation in
  a frozen module used to collapse the parameter's cell onto the `Function`
  marker; the imported wrapper and its local control now render identically and
  `crates/lichen-language/tests/frozen_function_type.rs` passes. A written arrow
  in a frozen module is therefore *not* a live cause of anything, and the two
  compute tests the old reading parked now run. What remains of that family is
  measured, with its reproductions, in
  [the kernel parameter's class](kernel-parameter-class.md).
- **The function arm is a second implementation of the array rule.** It descends
  with a fresh `path` and `steps`, has no early break, and records a failed child
  **twice** — once at the child and once at the function-type level — so the same
  message is printed twice with no descent path to say which side of the
  signature conflicted. Measured: a signature conflict reports two identical
  messages where the array arm reports one even two levels deep. The repair is to
  treat the signature as the two-element sequence `[parameter, return_type]` and
  run the array loop over it, parameterising only the element source — whose
  function side is fallible (materialization copies a frozen template, and a
  hand-built function may have no readable return type). It is a refactor of a
  hot path and is not done here.
- **The sub-typing question.** When two signatures agree their two function types
  merge into one class, and a class keeps **one** carrier, so it answers with
  whichever function the merge took. Whether two agreeing signatures are *the
  same type* or one a subtype of the other is not decided; a class carrying two
  identities is what answering that positionally before asking it looks like.
  Left deliberately to the next change rather than patched with a special case.
- **`?a: Int => ?a: Int` as source syntax.** Whether the type language spells a
  signature with attributes, and how a written `?a` in the two positions is made
  one cell, is argued from the representation but not measured — no workspace
  test writes one.
