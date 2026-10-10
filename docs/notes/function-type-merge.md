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

## Recovered measurements

- The merged lowering is the point rather than a tidying: `A -> B` used to compile
  to an arrow **term** `[[dom, cod], [FunctionType, K]]`, a second representation of
  a function's type beside the function's own — and a `[dom, cod]` shape has nowhere
  to hang an attribute, which is why [attributes](attributes.md) records
  "attributes do not flow through a function" as a non-goal. In the merged form the
  two sides are the parameter's `[value, type, attrs…]` pair and the return's term,
  so `?a: Int => ?a: Int` puts one cell in both positions and constrains the
  **values** passing through, not merely their types. The pattern the apply's
  function-ness guard builds is a real function for the same reason: a function type
  is the only function type there is, so no second shape is left for a pattern to
  wear.
- The function's own term is one node, not a separate type node beside the pair: a
  distinct `[Function(fid), ftype]` would be value-equal to the pair
  `[Function(fid), ftype]` and collide with it under the apply clone's topology
  re-establishment. The signature (parameter and return) lives in the function
  template, reached through `fid`, and unifying the type descends into those two
  cells directly. The pre-body pair — whose type slot held the placeholder `ty_cell`
  so the function-ness guard skipped during checking — is mutated in place into the
  self-referential `[Function(fid), ↺]`, the same shape as the universe
  `K = [Type, ↺]`: slot 0 the function's own value node, slot 1 the pair itself, so
  the type chain cycles at the function (`f : f : f : …`).
- A signature's `domain` and `codomain` compile before the shell opens: the two type
  expressions' nodes belong to the *enclosing* template and are cloned per call like
  any other node in scope, and the shell's own parent is that enclosing function,
  which is what keeps a signature written inside a lambda body re-instantiated per
  call rather than shared. A signature is never applied — it exists to be unified
  against — so its body is the bare return pair: no assert, no capture, nothing to
  run.
- A lambda's `parent` link is the frontend's decision, not the checker's: `parent`
  is the enclosing function, so a nested closure's nodes read as members of the
  enclosing template too while a sibling's do not — the mutual-recursion invariant.
  The link itself is the frontend's `ExprKind::Function::parent`, resolved in the
  checker through `Checker::function_of`; which function that *is* is decided where
  the lambda's syntax was compiled. The frontend's `fn_parents` invariant encodes
  the sibling rule.
- The function-ness guard refuses only a concrete non-function, and that is what
  makes it safe to run for every application: a concrete function type is the
  self-referential `[Function(fid), ↺]`, recognised by the two cells it has, while
  undecided types (parameters, lambdas, call results) are left to the runtime apply,
  because unifying the shared cell here would chain the type cells of every use of a
  polymorphic value. A failed unify never merges classes, so the guard itself cannot
  chain either.
- `TypeValue` and `TypeOperator` are the highlevel's own leaves: each layer provides
  a plain enum of its own variants — the lowlevel's `LowValue`/`LowOperator`, the
  highlevel's type values and type-level operators — and the composed vocabularies
  `HighProgramValue`/`HighProgramOperator` are flat unions, one `enum_ext!`
  invocation carrying each extension whole as one sibling variant, so a structural
  value sits one carry variant down (`HighProgramValue::LowValue(..)`) and nothing
  nests. Every `TypeValue` variant is a *type constant* whose own type is the
  canonical universe (`Type : Type`), which is what makes the composed vocabulary's
  literal build a one-arm answer for that whole branch. `TypeId` is the exception
  that is not a kind marker: a nominal type id living at `shape[0]` of a
  `TypeStruct`-kinded pair, where equal ids unify, different ids do not, and an id
  never unifies with the structural markers.
- The `TypeOperator` semantics the code now only names. `Add`/`Sub`/`Mul`/`Div` and
  the four order comparisons compute over one of the language's two scalar classes
  and never over a mixture: the classes do not convert
  ([floating-point](floating-point.md) §4.2), so the checker pins both operands to
  the class a concretely float operand selects, or to `Int` when neither operand
  names one. `Rem` and the bitwise trio are `Int`-only, because a float has no
  remainder and no bit pattern here. A comparison yields `USize(0/1)` whatever its
  operands are — no `Bool` value exists, and the result drives the lazy `Index`
  branch of an `if` directly — while the four arithmetic operators yield their
  operands' own class, which is what makes a float sum a `Float` again. An `Int` is
  machine-sized and **unsigned**, so `Div`/`Rem` are the unsigned operations and the
  four order comparisons are the unsigned ones when their operands are `Int`s;
  `Mul` and `Sub` wrap exactly as `Add` does. A float's arithmetic is IEEE instead,
  division by zero included, so nothing on the float side is refused.
  `Eq`/`Neq` are the generalized equality: they compare any two *same-typed* values
  whole — two `Int`s, two floats, or two type values, where `S::a == Int` is `1` —
  through the values' own `ValueExt::value_eq`, while a cross-type comparison is a
  check-time error because the checker unifies the operand types. For a float that
  relation is its 32 bits ([floating-point](floating-point.md) §3.1), which is why
  `0.0 == -0.0` is `0` and `NaN == NaN` is `1`.
- `int2float` moves an `Int` into the float class: every `Int` up to `2^24` maps to
  exactly one `f32`, and beyond that the float's 24-bit significand keeps the
  magnitude and drops the low bits — the value's own fact, not a refusal, since an
  `Int` is machine-sized and an `f32` is not. `float2int` truncates toward zero
  (`3.7` → `3`, `-3.7` → `-3`) and is the one partial operator in this vocabulary: a
  `NaN`, an infinity, a negative (the language's `Int` is unsigned) or a magnitude
  at or past the machine integer has no answer, and `OUT_OF_RANGE` records that and
  leaves the result lazy — the `DIVIDE_BY_ZERO` shape, a run-time refusal rather
  than a check error, because the operand is a runtime value the checker cannot see.
  The checker pins the operand to the direction's source class and the result is the
  target class, so a conversion is the *only* expression whose type differs from its
  operand's.
- `InDomain` is the refinement's membership test, and it exists as an operator
  rather than as a combination of `==` for one measured reason:
  `TypeOperator::Eq` compares through `ValueExt::value_eq`, which for an array is
  *handle* identity, so a structurally identical class node out of another module
  (an imported artifact's `int` type) would compare unequal and the refinement would
  refuse a value it should admit. It decodes structurally, the same way
  `shape::low_type_of` does. A side whose class is still undecided leaves the whole
  operator lazy, which is what keeps a refinement on an open parameter **pending**
  rather than failed until an application supplies the class.
- `IsStructType` recognises the `TypeStruct` tag (`[payload, TypeStruct]`, via
  `shape::is_struct_type_any`) over an operand `[type value, universe]`: the value
  to judge and the checker's canonical universe node, which the reader needs to
  recognise the kind's universe slot. Like `InDomain`, a still-undecided operand
  leaves the answer lazy rather than `0` — a question about a type nothing has
  decided yet has no answer. The named field read `a.name` states its container
  requirement with this operator: the container's type is not decided at check time
  (a parameter, a call result), so the condition is registered on the **assert
  channel**, which re-checks it per apply clone exactly as `InDomain` does
  ([eval-before-unify](eval-before-unify.md) §6.2 option 1). A decided container
  needs none of this — the read judges it where it is.
- The low-type transfer's three answers (`TypeOperator::low_type`): every comparison
  produces a `USize` — lichen has no `Bool` value, so a comparison result *is* a
  machine scalar — and so do the `Int`-only operators and the generalized equality,
  whose result is a scalar even when its operands are not. The four arithmetic
  operators produce their operands' own class: `Float` when either operand's low
  type is a float, else `USize` — which is what keeps a float-valued expression out
  of a backend, since the kernel domain walk refuses a `Float` leaf
  ([floating-point](floating-point.md) §3.8), while leaving an operand whose type is
  still undecided on the `USize` transfer a pre-apply template has always been
  given. `Fresh` produces a nominal type id, which the low type vocabulary has no
  shape for, so it declines.
- The two run-time refusals, and why only the interpreter refuses:
  `DIVIDE_BY_ZERO` and `OUT_OF_RANGE` are recorded through the lowlevel's general
  extension channel because `Module::eval_errors` is a closed enum of *structural*
  value facts (an out-of-bounds index, a table miss), and a divisor that evaluated
  to zero or a float with no `Int` to truncate toward is neither. The operator's
  answer is the lazy marker, which is what every other refused computation in this
  language answers, so the program reports the undecided result and the record says
  why. `DIVIDE_BY_ZERO` is **`Int`-only**: a float `Div` has the IEEE answer — an
  infinity or a `NaN`, both ordinary float values — so no float divisor is recorded.
  `OUT_OF_RANGE`'s four shapes are named rather than lumped together because the fix
  differs for each: a `NaN` came from a refused float computation, an infinity from
  an overflowing one, a negative needs the value re-derived (the `Int` is unsigned)
  and an out-of-range magnitude has no machine integer at all. `Int2Float` never
  refuses; every `Int` has a float (the nearest one, past `2^24`). Only the
  interpreter refuses, because a kernel has already left this crate: a JIT'd integer
  division traps (wasm) or is undefined (SPIR-V), and a JIT'd float→int conversion
  is wasm's `i32.trunc_f32_s` (a trap out of the whole invocation) or SPIR-V's
  `OpConvertFToU` (undefined for an out-of-range operand). A guard in either
  direction would need a branch, which the GPU backend's straight-line body — the
  thing that makes its uninitialised output buffers sound — does not allow.
- The static self-reference of a function type (`slot1_is_self` /
  `static_function_type_function`): a materialized static function-type copies the
  frozen node's value, so its slot 1 points at the frozen node's own cycle rather
  than back at the copy. That is the static form of the relation the dynamic arm
  reads as "one class": a class has one value, and a member carries it
  ([class-channel](class-channel.md)).
