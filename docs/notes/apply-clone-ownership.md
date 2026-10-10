# A call's clones belong to the enclosing template

> Status: **current.** An apply's clones are stamped with the **enclosing
> template's** owner, not the callee's, so the template they sit in re-instantiates
> them on every call. A clone carries the template's answer only when that answer
> is a template fact, and only when its own elements are decided.
>
> This note supersedes the clone-walk reading of
> [`block-vs-lambda-struct-field-inference.md`](block-vs-lambda-struct-field-inference.md),
> which looked for the cause of the same disappearance in the checker's field-list
> check; that note's field-list divergence is a separate, still-open question, and
> it now points here for the clone rules.
>
> Points at: `crates/lichen-lowlevel/src/function.rs` (`node_apply`, the dynamic
> clone rule), `crates/lichen-lowlevel/src/static_module/apply.rs`
> (`static_node_apply`, the frozen mirror), `crates/lichen-lowlevel/src/apply.rs`
> (`answer_elements_are_undecided`, the shared policy).

## 1. The rule

A value that travels through a wrapper function used to disappear. The
reproductions are one shape — a call whose *result* the enclosing body goes on to
use, where the callee's body is materialized inside the caller's body:

```lichen
id = x => x
f  = x => id x
f 1                     -- read `parameterized: ?a`, should read `1: Int`

add    = x => y => x + y
double = x => add x x
double 5                -- read `parameterized`, should read `10`
```

The cause is the clone's **owner tag**. An apply clones the nodes of the callee's
template whose value could differ per call, and every clone it makes belongs to
the template the *call* sits in — the applying node's own owner. That is what
makes the enclosing template re-instantiate them on its next call. Stamping a
clone with the **callee's** id instead leaves it looking like another function's
instance, so the enclosing template references it in place for every call: the
first call's parameter cells are then everyone's, and the value is never rebuilt.

- **The dynamic walk** stamps a plain apply's clones with the apply node's owner
  (`ApplyCtx::tag`). A **closure walk** is the one exception: a node of the
  closure's own scope joins the fresh closure id (its template reads as members of
  that id), while a capture — a member of the enclosing template cloned through
  the closure's edges — keeps the source's own owner, because re-cloning it under
  the fresh id would tear the captured value out of the enclosing instance the
  walk already built.
- **The static mirror** stamps a **residual** clone with the caller's tag too,
  because an unowned residual is referenced in place forever: the first call's
  argument binds its cell for every later call. A **baked** clone (the solved
  value, final for this call) stays unowned, so a genuinely concrete leaf keeps
  its fast path.
- The clone's `origin` is recorded separately — the node it instantiates — so a
  runtime failure that names a clone can be attributed to the argument the caller
  passed.
- **Template membership is the chain test, or a scope hit.** A node belongs to the
  template when its `Function::parent` chain reaches the applied function — **or**
  when it is inside a closure branch whose scope is hit. The chain test alone does not
  cover a closure that arrived through a unification (a parameter bound to a function
  value): it is walked under the *enclosing* anchor, and its own nodes' chains are
  rooted at the original id, whose parent chain need not reach that anchor, so they
  read as outside the template and the fresh closure's scope would be shared across
  calls. A closure's own scope is always in its own template, so membership is the
  chain test or a scope hit.

## 2. What a clone may carry

A clone inherits the template's answer, but only when that answer is a fact about
the **template** rather than about whichever call ran last. Two node axes decide,
and they are not to be conflated:

- `runned` — the source's **own operator** produced the value. `false` with a
  value present means a unification wrote it (an assertion, not a computation).
- `evaluated_deep` — the **deep pass** evaluated this node, which is what makes
  the answer a template fact every call shares.

A clone that fails either carries **no value at all**: a slot holding a value is a
slot a static reader (a backend compiling from the graph) reads as decided. An
answer whose own top-level elements are still open is the operator's result
structure, so the value still carries — mapping it is what puts this call's cells
in it — but the clone does **not** claim the operator's run. The claim follows the
mapped answer, not merely its presence:

```rust
runned = mapped.is_some_and(|value| !answer_elements_are_undecided(value));
```

The test is **one level deep by design**: a structure whose own elements are
decided is a fact a clone may answer with, however open its interior is (a struct
type's field cells are bound by the enclosing call's checks, and carrying that
answer is what stops a second generation of holes being minted). A template that
ran and could not decide maps to `None` and leaves the clone undecided with an
empty slot, which is the state of a node that has not run.

Two more carry rules:

- **A function id is never carried.** It is a per-call allocation, not a value the
  operator computed from its operand, and mapping it mints a *second* per-call
  closure beside the one the element walk already cloned — the two then meet in a
  unification as two different functions. One closure per call is enforced by a
  `minted` table (source `FunctionId` → fresh `FunctionId`), the closure-level
  counterpart of the node-level remap.
- **An answer that holds a foreign closure is not carriable either.** The
  concreteness proof cannot see through a function's body, so a value *containing*
  a closure is as per-call as a bare one. The carry side consumes the same
  recursive predicate as the bake side (`value_holds_foreign_function`), and the
  attribution happens at the clone decision, where the scope is known: a dynamic
  function counts as foreign unless it is the applied function (the recursion
  point, which must stay in place or a clone would mint a second function where
  the body means one) or an enclosing function, which every call shares. An
  answer holding a foreign closure carries nothing; the clone re-runs the operator
  and mints this call's closure.

**The same rule is the clone rule, not the dynamic clone's rule.** A frozen node
carries the same two fields the static walk reads (`runned` and `evaluated_deep`),
and the static materialize walk asks the same question through the same shared
policy. Its capture test asks whether a closure's body reaches any `undecided`
node **outside its own template scope**: an own-scope open cell re-opens per call
through the residual clone rule and is not a capture, while a captured open cell is
outside the scope by definition. No equality-class walk is needed, because the
body reaches the captured cell through operation operands and array items already.

## 3. Measuring it

Two entry points share one renderer, and they build different graphs:
`run::evaluate(source)` compiles bare source with an empty store, while
`run::evaluate_raw(source, Some(path), &mut store)` preprocesses and compiles with
the registry — which is what the compiler and the test harness use. Same source,
both entries, after the rule:

```text
curried  bare (compile)         : 10: ?a
curried  registry (evaluate_raw): 10: Int
```

A probe that reports `?a` where `lichen-compiler` reports `Int` is reporting its
own entry point, not the language. Before the rule both printed `parameterized`,
so the value loss was real on both paths; only the *type name* is resolved by the
registry path alone.

## 4. What it does not fix

The rule removes one generation of re-computed cells, and the field-list
divergence of
[`block-vs-lambda-struct-field-inference.md`](block-vs-lambda-struct-field-inference.md)
still stands underneath it. A callee that **is** re-run per call — a program that
binds its argument before the instantiate, say — still mints a fresh generation of
field cells, and the checker's field-list constraint does not follow it: the
constraint is part of the *check*, not part of the *expansion*, while macro
expansion re-runs the callee's body per call but never re-checks the fields
against the call's arguments. Making the constraint part of the per-call expansion
is that note's open direction (a).
