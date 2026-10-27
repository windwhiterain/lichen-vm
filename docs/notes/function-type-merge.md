# Function types, and a struct type's nominal identity

> Status: **the `f : f` half is superseded** (the user's ruling, recorded
> below); the rest is current. A function value's own type is now the shared
> **`FunctionType` marker** — the spec's `Function` — and no longer the
> function itself. Two functions unify the way two arrays do. A signature is
> written by a **function template** — a real function value — and unified with
> plain `=`, which applies nothing; the thin arrow `->` is gone from the
> language. A struct's nominal identity is the **name table its value
> carries**, so one written occurrence is one type however many times it is
> evaluated.
>
> This note absorbs the still-true half of *"A function's type is the function
> itself"* — its **arrow** half, which a function template replaced — and of
> *"An applied struct type expression is not a function"*. Its `f : f` half is
> retired: see the ruling below.
>
> Points at: `crates/lichen-lowlevel/src/equality.rs` (the function/array unify
> arm, the function-type recognisers, the signature reads),
> `crates/lichen-highlevel/src/checker/lambda.rs` (`check_lam` and the wiring
> that makes a lambda's type its own expr),
> `crates/lichen-highlevel/src/checker/templates.rs` (the unify, and the
> direction it takes),
> `crates/lichen-highlevel/src/checker/structs.rs` (the struct value's header
> and its emitting sites), `crates/lichen-highlevel/src/shape.rs`
> (`low_type_of`'s function arm),
> `crates/lichen-render` (the `dom -> cod` spelling), and `crates/lichen-compute`
> (the kernel signature readers).
> Open questions are in [What is open](#what-is-open).

## The ruling: a function value's type is the `FunctionType` marker

**The user's ruling (authoritative).** The spec's `Function` section
(`docs/language-spec.md`) states a function value as
`@raw[@raw p => body, Function, ...]` and `Function` as
`@raw[@raw FunctionType, Type, ...]`. So:

- A function **value**'s own type is the shared **`FunctionType` marker** — the
  `Function` type constant — and **not** the legacy self-referential `f : f`
  (`[Function(fid), ↺]`). The type names no particular function, so the type
  chain never cycles at a function.
- A function's **identity** lives in its **value** (`LowValue::Function(fid)`),
  never in its type. The lowlevel reads it with `function_value_function` (a
  value) and `function_expr_function` (an expr's value half, the **direct**
  reader), with `function_apply_function` reading a deferred `Unify`'s expected
  operand for the apply side; the retired
  `function_type_function` / `is_function_type` / `static_function_type_function`
  are gone, and the marker predicate is the highlevel's
  `shape::is_function_type_any`.
- There is **no separate signature concept**: a function is unified with the
  template **directly**. The current spelling of a template is
  `_x : Int => _: Int`; the constraint is that unify, with `=` or `<:`.
  `unify_function_signatures` still compares two functions' parameter and
  return cells, and `Module::function_signature` reads a known function's
  signature.
- The retired **arrow term** (`[[domain, codomain], [FunctionType, K]]`, built
  by `Ctx::arrow`) goes with the representation it belonged to; a signature is
  a function template, not a type structure.

**What changed in the code.** `check_lam` builds the function's expr as
`[value, FunctionType, attrs…]` and makes the shared `Function` constant its
type; `function_pattern` (a fake function whose type was a self-cycle) is
replaced by that constant; the apply's fourth operand carries the callee's
**expr**, whose value half names the function, so the type side can be applied
where the value cannot. The **runtime** type application still runs — for a
callee whose value is not concrete it applies the signature the callee's expr
names, and a frozen callee goes through `static_function_type_apply` so its
codomain binds — but it no longer reads a function out of a **type**: the
identity comes from the expr. The type side of a call is also applied at check
time, by `apply_function_type`, from the callee the check names.

**How the constraint is stated.** A template against an actual whose value is
still undecided is a **direct unify** of the two exprs, deferred as a graph
statement over `[actual_term, expected_term]`: the actual's **type** is
unified with the shared `Function` type — never with the template, so no
type carries a template — and the parameter's later uses read the `Unify`
term, while the function's own parameter record is untouched, so the argument
is still bound before the body runs. The unify returns the actual's own expr,
so the value side applies the supplied function and not the template. The
type side reads the template through `function_apply_function`, which prefers
the term's computed value and falls back to the `Unify`'s **expected**
operand only while the actual is still undecided — a decided non-function, or
an `Error`, never falls back to it. `function_expr_function` is the **direct**
reader: the expr's own value half, with no template fallback, so a lowering
never routes to the template body.

**Why the runtime type application is restricted to the undecided callee.**
Beside a **concrete** value apply it instantiated the callee a second time,
next to the value side's own instantiation. Under the old representation the
two never shared the parameter cell (the cloned function type's self-reference
was *not* re-established in every clone, so the cloned type named no function
and the type application did not fire). Under the shared `FunctionType` marker
the clones are uniform, the two instantiations share the parameter cell, and
the value side's binding was **refused** — so a `@loop` recursion's state never
advanced and the loop spun. Measured: with the type application running beside
a concrete value apply, the loop test performed >2000 iterations without
terminating; restricted to the undecided callee — where the value side has no
instantiation of its own — `loop_run::the_loop_and_the_unroll_answer_the_same_value`
and `loop_run::a_marked_reduction_runs_a_count_the_expansion_cannot_afford`
pass. The type application itself is **not** retired: it still applies the
signature the callee's expr names, and a frozen callee goes through
`static_function_type_apply` so its codomain binds.

**The artifact decision.** The wire codec is unchanged, but an artifact's
*meaning* is not: a frozen module written by the old compiler carries the
retired self-referential function type, and a reader that met it would read a
function type where the new reader expects the shared marker. The artifact
format version therefore moves, so old artifacts are rebuilt rather than read
under the new meaning (`docs/notes/artifact-cache.md`).

## One representation: a function template is a function

**There is no thin arrow.** `A -> B` is not a token in the language, and the
written-arrow form is retired. A signature is stated by a **function
template**: a real function value written where a function would be, such as
`_x : Int => _: Int` — the parameter with its annotation, `=>`, and the
result. A constraint on a function is the ordinary unify, `f = (x : Int =>
_: Int)`, so a function is constrained by the same rule as every other
requirement: the actual's **signature** is unified with the template's, and the
actual's own body is kept
(`crates/lichen-highlevel/src/checker/templates.rs`).

Plain `=` is the spelling, not `<:`. The match operator applies its right
hand side exactly once, and a function template is not a constructor to apply:
it states a signature, not a requirement to be produced. A `<:` whose right
hand side is a function template would apply that template and unify the
actual with its **result**.

Because the template *is* a function, the one-representation property this
note records survives the change of spelling: there is no second shape for a
function's type, and a signature is never a distinct term.

A function value's *own* type is then the same thing, and it needs no second
node: `check_lam` binds the function expr's type cell to the expr itself, so a
function's type is the self-referential node — **superseded**

> **This subsection is history.** It describes the representation the user's
> ruling retired: a function's type was the self-referential node
>
> ```text
> [Function(fid), ↺]
> ```
>
> — value slot holds the function, type slot points back at itself — recognised
> as a type by that self-cycle (`Module::is_function_type`). The type is now
> the shared `FunctionType` marker, and the recogniser is the highlevel's
> `shape::is_function_type_any`; the identity is read from the **value**
> (`Module::function_value_function`, `Module::function_expr_function`).

Two consequences, and both are deliberate:

- **`f : f` is a consequence, not a rule anybody maintains.** The type chain
  closes at the function the way it closes at `Type`, so a function's type is the
  function (`overview.md`). — **superseded:** the chain does not close at a
  function any more; a function's type is the shared marker.
- **A function's signature is not in the type node.** It lives in the function
  template: the parameter is the `[value, type, attrs…]` expr the body binds and
  the apply clone walk clones (`Function::parameter`), and the return side is
  `Function::return_type` — the return's *type* cell, because `r#return` may be
  an unevaluated operation node whose own slots do not name the type. An apply
  clones those two cells fresh per call and never binds the template, so a
  function's signature is let-polymorphic: `f := x => x` accepts `1` and `1.5`
  in turn, and an annotation checks a clone rather than specialising the
  template. — **still true:** the signature lives in the function's own cells,
  and the type carries none of it.

The template's *return* position is an ordinary expression, which is how the
placeholder spells "a result of this type": `_x : Int => _: Int` states a
function of one `Int` whose result is a hole of type `Int`. The hole is filled
by the actual function the match supplies.


`Type` is the **terminal** of the chain, not a supertype: there is no subtyping
relation anywhere (`Int` is not `<: Type`; a compound type is typed by its kind),
and **there is no occurs check** — cyclic types unify (equi-recursive
behaviour), which is exactly what lets the universe and a recursive struct type
exist. The self-cycle test below is a *shape* judgement inside that semantics, not
an occurs check.

## Functions unify like arrays

When both operands are function types, the unifier descends the two signatures
positionally: the parameter exprs unify against each other — value against value,
type against type, attribute against attribute — and the two return type cells
unify. When the elements agree the two classes merge, the same merge two arrays
with equal elements make. There is **no host hook and no clone**: the old
`Program` policy had to ask its host *where* a signature lives and *whether it
may be written*, and it answered differently for a dynamic function and a frozen
module's, which is how a wrapper in an imported module ended up reported as an
undecided struct. The lowlevel now needs to be told nothing about function types.

The descent names the parameter **expr**, not its type slot alone, so a signature
carries attributes and can constrain the *values* that flow through it:
`?a: Int => ?a: Int` puts one cell in both positions, so unifying a function
against it makes the argument and the result the same cell. That capability is
argued from the representation rather than measured: no test in the workspace
writes a value-constraining signature yet.

Three shapes the arm has to tell apart, and the judgement is the invariant rather
than a roster:

- **Both sides a function type** — descend as above.
- **A *degenerate* function type** — a `[Function(fid), t]` expr whose type slot
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

## A struct's nominal identity is its written occurrence

> **The encoding below is the pre-template one.** A struct *type expression*
> is retired: a struct is now a **value**, and the identity rides in the
> **value's own header**, `[name table, nominal id, fields…]`
> (`crates/lichen-highlevel/src/checker/structs.rs`, `struct_value_node`).
> The rule this section records is unchanged: one written occurrence is one
> nominal type, and the name table is what carries it, because a table
> compares by **identity**. Measured: two struct constructors written apart
> produce instances that do not unify — `expected Table, found Table` — while
> two instances of one constructor do. See [value templates](value-templates.md)
> for the migration and the book's Chapter 8 for the surface.

A struct type expression compiles to the expr `[shape, kind]`, and its nominal
identity is a marker of the form

```text
wrapper = [ shape, kind ]
shape   = [ field types… ]
payload = [ id, names, names_in_order ]
marker  = [ payload, TypeStruct ]
kind    = [ marker, K ]
```

`Checker::struct_type_type` was the single construction point of that encoding;
today's is `Checker::struct_value_node`
(`crates/lichen-highlevel/src/checker/structs.rs`), and the `id` is the
**written occurrence itself** (`Checker::fresh_nominal_id` takes the
[`ExprId`]): the checker writes `TypeId(occurrence)` into the node rather than
allocating an operator to compute one. That is what makes one written occurrence
one nominal type:

- The id is a **build-time value**, so a clone copies the identity instead of
  re-minting it, and the checker never evaluates to establish it. It was a
  `Fresh` operator that read and incremented `HighGlobal::type_id_counter` at
  evaluation time, which made the identity depend on how far an evaluation
  happened to reach and forced the build phase to force the marker to pin it.
- The marker is still **deep-evaluated where it is constructed**, not in a later
  phase. Its name table is a `TableGet` operation, and a table compares by
  **identity**, so a cloned marker is a *different* table that does not unify with
  the original — a node the deep pass has proved concrete is referenced in place
  by every clone, and the same verdict freezes it non-undecided in a static
  module, so a persisted artifact bakes the identity rather than re-minting it per
  materialization. **Measured: the pin cannot be deferred.** Moving it to the
  pre-definition phase — recording each marker during the build and pinning them
  all afterwards, which is the shape a "build unifies, then evaluate" phase split
  wants — makes two applications of one struct constructor unify their name tables
  by identity and refuse a program that otherwise checks. The build's own
  subsequent statements read the struct type, so what they see depends on the pin
  having already run.
- The **field types stay out of the identity** — they ride in the shape. An
  occurrence applied to two different arguments therefore stays one occurrence
  (`A Int` and `A Float` share the id and differ because their shapes differ),
  and two `struct{…}` written apart remain two declarations because each written
  occurrence is a different expression. Without that, `A In` written twice was
  two nominal types, which made a type constructor that is not a function: a
  derived type could not be named by writing its expression, only by binding it
  once.

The identity matters wherever a type is built by one layer and named by another —
a kernel's JIT-built input type and the host's argument type are two evaluations
of one expression applied to one argument, and they must unify.

## What is open

- **The frozen written-arrow reduction is closed, and the arrow is gone.** A
  written arrow annotation in a frozen module used to collapse the parameter's
  cell onto the `Function` marker; the imported wrapper and its local control
  now render identically and
  `crates/lichen-language/tests/language/frozen_function_type.rs` passes. A written arrow
  in a frozen module is therefore *not* a live cause of anything, and the two
  compute tests the old reading parked now run. What remains of that family is
  measured, with its reproductions, in
  [the kernel parameter's class](kernel-parameter-class.md). The arrow form
  itself has since been **retired from the language**: a frozen module now
  states a signature with a **function template** (`_x : Int => _: Int`) and
  unifies it with plain `=`, so there is no written arrow left to be frozen.
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
  two sides are the parameter's `[value, type, attrs…]` expr and the return's term,
  so `?a: Int => ?a: Int` puts one cell in both positions and constrains the
  **values** passing through, not merely their types. The pattern the apply's
  function-ness guard builds is a real function for the same reason: a function type
  is the only function type there is, so no second shape is left for a pattern to
  wear. The thin arrow was later **removed from the language altogether**; the
  function template carries the same two sides, so the representation this
  measurement settled on is the one that survived.
- The function's own term is one node, not a separate type node beside the expr: a
  distinct `[Function(fid), ftype]` would be value-equal to the expr
  `[Function(fid), ftype]` and collide with it under the apply clone's topology
  re-establishment. The signature (parameter and return) lives in the function
  template, reached through `fid`, and unifying the type descends into those two
  cells directly. The pre-body expr — whose type slot held the placeholder `ty_cell`
  so the function-ness guard skipped during checking — is mutated in place into the
  self-referential `[Function(fid), ↺]`, the same shape as the universe
  `K = [Type, ↺]`: slot 0 the function's own value node, slot 1 the expr itself, so
  the type chain cycles at the function (`f : f : f : …`). — **superseded:**
  the pre-body expr is no longer mutated into a self-cycle; the function's type
  is the shared marker.
- A signature's `domain` and `codomain` compile before the shell opens: the two type
  expressions' nodes belong to the *enclosing* template and are cloned per call like
  any other node in scope, and the shell's own parent is that enclosing function,
  which is what keeps a signature written inside a lambda body re-instantiated per
  call rather than shared. A signature is never applied — it exists to be unified
  against — so its body is the bare return expr: no assert, no capture, nothing to
  run.
- A lambda's `parent` link is the frontend's decision, not the checker's: `parent`
  is the enclosing function, so a nested closure's nodes read as members of the
  enclosing template too while a sibling's do not — the mutual-recursion invariant.
  The link itself is the frontend's `ExprKind::Function::parent`, resolved in the
  checker through `Checker::function_of`; which function that *is* is decided where
  the lambda's syntax was compiled. The frontend's `fn_parents` invariant encodes
  the sibling rule.
- The function-ness guard refuses only a concrete non-function, and that is what
  makes it safe to run for every application: a concrete function type is the shared
  `FunctionType` marker, recognised by the marker its value slot holds, while
  undecided types (parameters, lambdas, call results) are left to the runtime apply,
  because unifying the shared cell here would chain the type cells of every use of a
  polymorphic value. A failed unify never merges classes, so the guard itself cannot
  chain either. — **superseded:** the guard now unifies the callee's type with the
  shared `Function` marker, not with a self-referential function pattern.
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
  `TypeStruct`-kinded expr, where equal ids unify, different ids do not, and an id
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
  given.
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
