# A call's clones belong to the enclosing template

> Status: **current.** An apply's clones are stamped with the **enclosing
> template's** owner, not the callee's, so the template they sit in re-instantiates
> them on every call. A clone carries the template's answer only when that answer
> is a template fact, and only when its own elements are decided.
>
> This note supersedes the clone-walk reading of
> [`block-vs-lambda-struct-field-inference.md`](block-vs-lambda-struct-field-inference.md),
> which looked for the cause of the same disappearance in the checker's field-list
> check; that note is **closed** now (its named disagreement no longer reproduces),
> and it points here for the clone rules.
>
> Points at: `crates/lichen-lowlevel/src/scope.rs` (the lexical scope tree and
> the class scope summaries), `crates/lichen-lowlevel/src/function.rs`
> (`node_apply`, the dynamic clone rule), `crates/lichen-lowlevel/src/template.rs`
> (`clone_template`, the general-purpose clone walk),
> `crates/lichen-lowlevel/src/static_module/apply.rs` (`static_node_apply`, the
> frozen mirror), `crates/lichen-lowlevel/src/apply.rs`
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
- **A clone lands in the *image* of its template's block**, not in the apply's own
  target. A parentless block — the root, or a lambda body that outlives its enclosing
  scope — maps to the apply's target, as before; a `@gc` scope maps to a fresh child
  of its parent's image, so each instantiation gets its own and the read that ends the
  call collects that one. The memo is per template block per pass, so every clone of
  one scope shares one block. A node inside a scope is also **never referenced in
  place**: a scope's content is the call's, not the template's, so sharing it would
  let one call collect the template a later call still needs. See
  [gc-block](gc-block.md).

## 2. What a clone may carry

A clone inherits the template's answer, but only when that answer is a fact about
the **template** rather than about whichever call ran last. Two node axes decide,
and they are not to be conflated:

- `runned` — the source's **own operator** produced the value. Zero with a
  value present means a unification wrote it (an assertion, not a computation).
- `evaluated_deep` — the **deep pass** evaluated this node, which is what makes
  the answer a template fact every call shares.

**The author's ruling on presence** (`compile-order-class.md` §8): *what a template computes is the same
for every apply, and a generic or dependent template computes nothing decided* — so the gate is the deep
pass alone, `evaluated_deep`, and `runned` is a separate axis that does not decide carriage. A value a
unification wrote is carried for the same reason: no value is wrong, only a unify conflict is (ruling 7).

**The author's ruling on what the gate is for** (`compile-order-class.md` §8, ruling 7): *"啥叫错误的值？
没有值会是错误的。有的只有 unify 冲突。"* A carried value is never wrong — a disagreement with a later
computation surfaces as a unify conflict and is reported. So the question the gate answers is "is this a
**template fact**", not "might carrying it freeze a per-call answer". **The gate is implemented the way the ruling reads.** Both walks carry on `evaluated_deep` alone —
`function.rs`'s clone rule (`let template_answer = evaluated_deep.is_some();`) and the static walk
(`let carried = operation.is_none() || evaluated_deep.is_some();`) — so a unification-written value
travels with the clone, and whether its operator ran does not decide carriage.

A clone that fails either carries **no value at all**: a slot holding a value is a
slot a static reader (a backend compiling from the graph) reads as decided. An
answer whose own top-level elements are still open is the operator's result
structure, so the value still carries — mapping it is what puts this call's cells
in it — but the clone does **not** claim the operator's run. The claim follows the
mapped answer, not merely its presence:

```rust
let claimed = mapped.is_some_and(|value| !answer_elements_are_undecided(value));
self.stamp_node_ran_if(clone, claimed);
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
  counterpart of the node-level remap. The **frozen** pass keeps the same rule: a
  frozen closure is re-homed once per pass (`static_module/apply.rs`'s `minted`), so
  the closure's own value node — which its scope holds — resolves to the closure it
  names.
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

## 4. The scope rule is class-wide

A clone decision is a fact of the **equality class**, not of the node the walk
reached. A class is cloned only when **every** member of it lies inside the
scope of the function whose body the walk is instantiating; a class that
reaches outside that scope is **referenced as a whole**. A clone therefore
never splits a class, and the verdict is the same for every member of one
class.

- **The scope of a function** is a node of the immutable lexical scope tree
  (`scope.rs`): a function's scope hangs under the scope of the function it is
  written inside, and the tree is built once, when the function is created.
  The tree outlives the function: a summary that names a scope keeps naming the
  same lexical place after the function itself is collected.
- **The scope of a walk** is the scope of the function whose body the walk
  instantiates: the applied function's for an apply, the closure's own for the
  walk that mints a closure from its template. A capture — a cell owned
  outside that scope — is therefore **referenced**, not cloned, which is the
  rule the user's `@clone` examples ask for (`x => @clone y => x+1` references
  `x`; `x => @clone y => y+1` clones `y`).
- **The summary of a class** is the nearest common ancestor of the owner scopes
  of its members, held by the class's representative. A member with no owner —
  a top-level cell, or one whose owner was collected — names the **virtual
  root**: it is outside every scope, so the class that holds it is external and
  is referenced whole.
- **The verdict** is one ancestor test: the scope root must be an
  ancestor-or-self of the class's summary. The scope tree carries a
  binary-lifting table, so the test is `O(log F)` in the number of functions
  and independent of the class's size; the clone walk caches the verdict per
  class, so the hot path is `O(1)` per node.

The rule is maintained at every write that can move it: a node's creation (its
own owner's scope), an owner retag (the class is recomputed from its members),
a class union (the two summaries merge), the collector's class rebuild (the
survivors are recomputed), and the freeze (the artifact records every frozen
node's owner and every frozen function's parent, so a load rebuilds the frozen
scope tree and every class's summary from the artifact's own tables).

### Implementation boundaries exposed by the virtual root

The virtual-root rule is exact. A top-level cell deliberately has no owner
(`Checker::alloc_node`), so unifying it with a function-owned cell can make the
whole class external. That is a sharing decision the implementation must
respect, not a reason to make unowned members neutral. Two measurements
separate implementation defects from that rule.

**A reference still participates in the parameter check.** In
`apply_unify_binds_an_undecided_argument_into_the_param_class`, the parameter
belongs to `f`, a constant has no owner, and the two are unified before `f` is
applied to an unowned, undecided argument. The class is therefore external and
the parameter image is the original parameter cell. The old apply skipped the
check whenever the remap contained no clone for the parameter: the call
returned `1` but left the argument undecided. The fix stores `parameter_image`
and runs `apply_parameter_check` against it whether it is a clone or a
reference. This test and `apply_time_conflict_records_an_error` pass without
changing the external verdict.

**The recursive expansion was not a loop-backedge contamination.** The old
function-value type was the self-referential `f : f` expr. Its class contained
both function-owned cells and the unowned type cell reserved for a top-level
recursive binding, so the strict verdict referenced one polymorphic function
type class at every unrolled application. A managed trace measured more than
534,900 applies with depths oscillating around 54–62 instead of reaching the
500 budget; marked-loop iterations advanced, and no external parameter root
occurred. Restoring the old owner-chain membership made the loop finish in
0.05s, and narrowing that exception to the function type expr alone was
sufficient; the function value and the shared `Int` type-index node were not
the cause.

The user's ruling removed that obsolete representation: a function value's
type is the shared Unit `FunctionType`, and a function constraint is a direct
`Unify` of the two exprs, deferred in the graph when the actual is undecided
([function-type-merge](function-type-merge.md)). With that implementation in
place, both `loop_run::the_loop_and_the_unroll_answer_the_same_value` and
`loop_run::a_marked_reduction_runs_a_count_the_expansion_cannot_afford` pass in
0.11s total under the strict class rule. No unowned semantics, assertion, loop
count, or apply-depth budget changed.

### The frozen path

The frozen artifact carries the same facts: every frozen node's owner and every
frozen function's parent. Loading validates those function links, rebuilds the
scope tree, and computes each class summary on its representative. An unowned
or invalid owner absorbs into the virtual root exactly as in the dynamic path.
Version 25 retires artifacts without this metadata.

A frozen open cell is read-only, so *reference* cannot mean writing to the
artifact. Materialization uses one canonical dynamic **outer instance** for an
external open cell per top-level pass and shares that mapping with nested
re-homes; it does not allocate a fresh `materialize_leaf` at each encounter.
An instance created for an external verdict starts **unowned**: tagging it with
the nested caller would make its later class summary internal and re-instantiate
the capture per call. If the same source image is later reached from an
enclosing scope whose verdict is inside, the image is reused and receives that
enclosing owner; a nested external verdict never strips an existing outer
owner. Inside-scope cells are instantiated per call, while an external class is
never made internal by a nested owner retag. `static_node_apply`, static signature
materialization, and `template_map_ref` all use the same class verdict, and the
static parameter check consumes the parameter image whether it is a clone or
an outer-instance reference.

When a dynamic template walk judges a frozen class, only the template scope's
**own** function can supply the counterpart scope, through a `static_origin` in
the same frozen module. No origin, or an origin in another module, means
external. The walk does not fall back to a dynamic ancestor's origin: that
ancestor is not the lexical scope being instantiated and would wrongly make a
nested wrapper's captures internal.

## 5. What it does not fix, and what closed underneath it

The rule removes one generation of re-computed cells. A callee that **is** re-run
per call — a program that binds its argument before the instantiate, say — still
mints a fresh generation of field cells, and the checker's field-list constraint
does not follow it: the constraint is part of the *check*, not part of the
*expansion*. That was the field-list divergence of
[`block-vs-lambda-struct-field-inference.md`](block-vs-lambda-struct-field-inference.md),
which is now **closed**: the disagreement it named no longer reproduces
(re-measured on `dev@1afc7d93`, its two programs both print `(Int,):
struct{.x Type}`), the hole-field examples are expected by the maintainer's
ruling, and its per-call-expansion direction (a) was carried to an `Instantiate`
operator and then withdrawn, the operator gone. What remains here is the
generation rule, not a checker constraint that follows it.

## Recovered measurements

- The definition pass pins what a clone copies, and every function body is walked
  once even when the function is never applied, so its apply-time checks fire. A
  body the pass never computed keeps every operation node undecided, so each
  application clones them all and silently re-runs per-application work — a
  body-local struct type's name table is the sharpest example, because a table
  compares by identity and a cloned one is a different table.
- Two facts the function walk depends on: a body ending in a call resolves its
  result cell before the root pass walks the function's type spine, or the root
  pass reads a cell that is still undecided; and the walk's order against the root
  pass is irrelevant, because reads alias their target cells (the lowlevel `Index`
  arm), so bindings propagate class-wise however they happen. The walk is skipped
  when a checker-side unify (an annotation, a guard) has already failed, because
  the build is rejected either way.
- Assert conditions are reachability entry points in their own right: the return's
  concreteness proof does not reach the body's assert conditions, so each gets its
  own proof. A condition that does not read the parameter is per-call-invariant, so
  the apply references it in place instead of cloning and re-registering it; a
  condition that does read the parameter stays undecided and clones per call.
- Untagged nodes are shared, not cloned: the clone walk's membership test
  (`Function::nodes` / `Node::function`) decides sharing, and an untagged node is
  referenced in place. Allocating the two shared index constants, the kind markers
  and the shared missing slots before any function exists is what makes them behave
  like the old per-occurrence nodes observed through one node instead of several.
- The verdict that pins a struct type's identity payload is the same verdict that
  freezes it non-undecided in a static module, so a solved artifact bakes the
  identity in rather than recomputing it at load time (the identity pinning itself
  is [function-type-merge.md](function-type-merge.md)'s subject).
- The declared parameter attribute (`x # n => e`) is a **template** constraint: the
  apply's check compares each argument's attribute against the declared value node
  (`Checker::function_param_attr`, then the attribute's `AttrExt::unify_slots`).
  The live attribute cell in the parameter expr is deliberately left **undecided**:
  binding it to the declared value would let the deep pass bake it (it is a
  concrete value), so the per-apply clone would reference the template's cell
  instead of resetting it, and the lowlevel apply's positional unify would enforce
  the declared perspective as equality against the argument, defeating the
  attribute's subtype relaxation. Kept undecided it is a fresh per-apply clone that
  binds the argument's actual perspective, exactly like the value and type cells,
  so the body's return reads the caller's perspective and `f (5 # 4)` yields
  `5 # 4`; the declared value itself stays only in `Checker::function_param_attr`.
- The function shell exists before any of its nodes, and its stack entry goes in
  with it: the allocation helper tags and registers *every* node against the
  function currently being built, so the parameter's cells and expr are this
  template's from the start and nothing has to be moved, re-tagged or overwritten
  afterwards. The clone walk's membership test (`finish_function` asserts it) is
  satisfied by construction rather than by a later repair pass.
- The checker is the only place that knows an application's argument structure (its
  expression's source span), and the argument **edge** — this apply op node to the
  argument — is unique per application even when the argument node itself is
  shared, so a runtime parameter-check failure can be attributed to the argument's
  span regardless of node sharing. The lowlevel records only the apply node on
  failure; the diagnostics read this edge.
- `Function::parent` is the explicit link the checker hands to `begin_function`: it
  keeps sibling functions' template scopes disjoint while absorbing truly-nested
  closures into their parent's template. It is carried in the IR because a
  `Function` node is allocated *after* its body, so the enclosing function's id is
  not known to the checker at that point; it is `None` for a top-level function and
  for the mutual-recursion sibling case (the frontend's `fn_parents` invariant).
- `IR::repoint` is the frontend's alias fixup: a block-wide binding whose value is a
  bare name (`c = b`) aliases its target, so the uses that captured the binding's
  reserved placeholder are re-pointed to the aliased node, and the placeholder
  keeps no references and leaves the block-root set. The alias target is a block
  root already (a forward alias is another binding's placeholder) or a
  `let`/statement value no new cycle can form through. It is a no-op when
  `from == to` — the degenerate self-alias `a = a` — because the placeholder must
  stay referenced and block-rooted.
- A body read of `ins(i)` compiles to a **bare cell**: no operation, no subscript,
  and not even a member of the parameter's class. Nothing in the unapplied body
  links a read to a slot, and nothing states the arity either; the apply settles
  which read is which slot. Were it settled by read order, a function reading its
  parameter back to front would take its two arguments swapped, and a caller
  passing `(4, data)` would get a dispatch over four elements reading the *number*
  as a buffer — a silently wrong answer rather than a refusal.
- A deferred named instantiation's supplying key (`s(.x 1)` with `s` a parameter)
  is duplicated per apply, and the checker never saw that node, so the refusal is
  recorded on a clone that cannot name its own template: the caret comes from the
  clone's template origin (`Module::node_origin`), the argument node the checker
  *did* attribute, so both the unknown-field and the mismatched-type refusal point
  at the argument the caller wrote inside the lambda body. A bare name's use *is*
  its binder's own expression, so `f (S)` points the caret at `S`'s binding, the
  same place a runtime parameter-check failure of a name argument points. The
  imported file's own `.x 1` is not reachable: an ordinary package keeps no source
  record.
- A deferred instantiation written in an *imported* body has its cells in the
  **frozen** module, not in the importer. A per-apply clone that reused the first
  call's bound cells would contaminate the second, so two calls resolving
  *different* struct types through one imported function would contaminate each
  other; leaving the parameter's type open and letting the caller supply it is what
  makes each call decide the instantiation for itself.
- Two block-wide bindings that call each other get **sibling** template scopes,
  which are disjoint: an apply clone inside `f` references `g` in place rather than
  cloning it per level. That is what lets a mutual recursion descend and terminate
  with exactly two function templates existing. The checker totalizes the same
  cycle separately (no stack overflow, no diagnostics), so the check-time and
  run-time halves are two separate mechanisms for one shape.
- A self-reference is a carry rule like any other, keyed on the same concreteness
  answer: a function value node proven concrete — it does not depend on the
  parameter — is referenced in place by the recursion, so one `FunctionId` serves
  every level. With `evaluated_deep` still `None` the clone rule cannot carry it,
  so each recursion level mints its own function clone, homed on the calling block.
- Nesting adds one step: a nested function's value node is a member of the
  **enclosing** function's template, but its internals are not, and the `parent`
  link is what joins them — the body clone folds that scope in, which is also what
  rewrites a capture when the nested body reads the outer parameter directly.
  Without the fold the capture would stay bound to the template's parameter; with
  it, `f(x) = g` returns a closure whose `g` reads this call's `x`.
- `disjoint` (the class union-find) keeps each set as a `parent`-pointer tree plus
  a singly-linked member list headed by the representative, so walking `next` from
  a root visits every node of the set. `find` compresses paths without touching the
  list, `union` splices the two member lists with O(1) pointer surgery, and neither
  allocates: the metadata lives inside the caller's nodes and the links are the
  union-find's state, not the caller's, so a node changes sets only through the
  module's operations. `find`'s recursion depth is bounded by the path length.
  `Meta::new` is the one way to build a `Meta` whose links did not come from this
  module: freezing a solved module remaps a live node's links onto local indices,
  and the artifact decoder reads them back.
