# A function unifies by its own cells

> Status: **in progress.** Branch `feature/function-unify`. The value match has a
> `LowValue::Function` arm beside `Array`, so two functions unify by descending
> `[parameter, return]` and the `function_type_function` recogniser is no longer
> part of unification; a class pair met again on the descent now agrees instead of
> recording a conflict. The wildcard experiments (a lambda's value positions
> holding `@any`) are **rejected and reverted**, with the measurements below.
> One suite failure remains: the diagnostic-shape pin, which is a pin to move
> deliberately. The closure that read `none` for a captured parameter is **repaired** —
> the fix and its measurements are in §6 — and the pipeline test that pinned it passes.
> Measured on this branch after the repair: lowlevel 160/0, highlevel 86/1, and
> `lichen-language` 335/0 — the pipeline binary that pinned the capture loss included.
>
> **Checkpoint decision (with the maintainer): the branch stays unmerged until the
> repair lands.** The repair has landed; what remains before the merge is the diagnostic
> pin above, which pins a shape deliberately and therefore needs the maintainer's word
> before it is moved.
>
> This note is the branch's coordination document; it records the maintainer's
> proposals as they are made, and what each one measured.
>
> Points at: `crates/lichen-lowlevel/src/equality.rs` (the value match, the
> function arm, `function_cells`, `alias_read`, the `path` guard),
> `crates/lichen-lowlevel/src/function.rs` (`node_apply`'s wildcard rule),
> `crates/lichen-lowlevel/src/apply.rs` (`wire_apply_result`),
> `crates/lichen-lowlevel/src/evaluation.rs` (the `Index` arm and
> `evaluate_node_body`), `crates/lichen-lowlevel/tests/basic/equality.rs`,
> `crates/lichen-highlevel/tests/highlevel/wildcard_positions.rs`,
> `crates/lichen-language/tests/language/prelude_reads.rs`.
>
> Companions: [function-type-merge](function-type-merge.md) is the precedent — the
> function kind's element slots — and its *What is open* named both the repair this
> branch lands (the function arm as the array rule over `[parameter, return_type]`)
> and the carrier question the remaining failures are;
> [compound-element-slots](compound-element-slots.md) is the authority on `Any`'s
> four behaviours; [apply-clone-ownership](apply-clone-ownership.md) is the clone
> rule the wildcard filter had to fit; [lowlevel-vm](lowlevel-vm.md) holds the
> write and read rules; [eval-before-unify](eval-before-unify.md) is the premature
> answer the `none` failures look like.

## 1. The question

A lambda's term **is** its type (`f : f`): `check_lam` writes the same `[func_node,
pair]` shape for both. So applying a curried lambda to a variable inside another
lambda is refused by the value comparison, not by a type disagreement:

```lichen
f = n => (x => y => x) n
f 3
```

`dev` reports `expected ?a => Int, found ?a => Int` — the two sides print
identically. The two nodes are two **generations** of one closure: the apply mints
a fresh `FunctionId` per instantiation, `function_identity` compares ids (only a
`static_origin` is carried), and the reconciliation in `wire_apply_result`
(`unify(node, applied)`) therefore refuses a closure against the closure it is a
copy of. The same shape at top level is fine; **inside a lambda whose body is an
operation over a parameter** it is not.

## 2. What landed

| commit | change |
|---|---|
| `ba67cbe5` | `LowValue::Function` becomes an arm of the value match beside `Array`, descending `[parameter, return]`; the `function_type_function` gate, `unify_function_types` and the node-keyed `function_signature` are deleted |
| `b0e6b761` | a class pair met again on the descent **agrees** (`path.contains` answers true), because a function type's slot 1 is its own `↺`; the pinned lowlevel contract moves from `..._record_an_error_instead_of_looping` to `..._unify_instead_of_looping` |
| `70397dd5` | `crates/lichen-highlevel/tests/highlevel/wildcard_positions.rs` and `crates/lichen-language/tests/language/prelude_reads.rs` |
| `56b41d46`, `98b5b7bb`, `16a773db`, `f17e78c3` | the wildcard filters: `alias_read` skips a wildcard target (slot **or** class), the `Index` arm and `evaluate_node_body` answer undecided for one, the clone mints an undecided cell for a wildcard however it stands, and an apply that produced one answers undecided |

Measured on the 18-program matrix, dev against this branch: the refusals this
answers are gone and the answers agree with dev —

| program | dev | branch |
|---|---|---|
| `f = n => (x => y => x) n; f 3` | refused | `?a => Int: ?a -> Int` |
| `f = n => (x => y => x + y) n n; f 3` | refused | `3: Int` |
| `add = x => y => x + y; f = n => add n n; f 3` | refused | `6: Int` |
| `f = n => (x => x) n; f 3` | `3` | `3` |
| `f = n => (x => x + 0) n; f 3` | `3` | `3` |
| `f1 = x => y => [1, 2, x, y]; f1 3 4` | `[1, 2, 3, 4]` | `[1, 2, 3, 4]` |
| `a = 1; f1 = x => { b = 2; f2 = y => [a, b, x, y]; f2 }; f1 3 4` | `[1, 2, 3, 4]` | `[1, 2, none, 4]` |
| `mk = x => y => x; a = [mk 1, mk 2]; (a[0] 0, a[1] 0)` | refused | `(1, 1)` |

Suite state: `lichen-lowlevel` 160/0, `lichen-highlevel` 86/1, `lichen-language`
pipeline 140/1, `lichen-render`, `lichen-language-server` green. The two failures
are §5's.

## 3. The proposal that was rejected: `@any` in a function's value positions

**Proposed (maintainer).** A function's `parameter` and `return` value slots should
hold `@any`; the wildcard must be cloned, and the clone must be undecided.
**Then, as a boundary rule:** `@any` is confined to a function's own graph, and the
only way a function meets an expression outside it is the apply's clone — which is
what filters `@any`.

**Measured, and rejected.** With the shared wildcard node in `check_lambda`'s
parameter value cell and `signature_node`'s parameter/return value cells:

- One shared node aliases: `node_apply`'s wildcard rule mints one cell per node, so
  two `@any` positions in one instantiation become one cell and
  `f1 = x => y => [1, 2, x, y]; f1 3 4` answers `expected Int, found Int`. Minting per
  reach (no `remap` entry) fixes that.
- Every operation over an applied parameter then answers
  `this value is not a container — it has no element to read`: `f = n => (x => x + 0)
  n; f 3`, `f = n => (x => succ n)`, `add n n`. Suites: pipeline 139/2, a target
  12/10, a target 2/2, an assertion failure inside `core.lichen`.
- **The crossing is the prelude's operand array, not the clone.** The CLI compiles
  with the core prelude, so `x + 0` is core's `add`, which indexes its operand array;
  the element standing for the parameter is the wildcard leaf, and the library's read
  of it is not a container. A temporary trace (`add_node`, `set_node_value`,
  `propagate_class_value`, the clone's write) says `Any` reaches the failing node from
  a `write_node_value(node, Some(Any))` that is **neither** class propagation
  (`PROP-ANY` never fires) **nor** the clone (no clone carries it) **nor** the apply
  boundary (filtering it changes nothing). That writer is still unnamed.

The reader-side filters in §2's list came out of this and are **behaviour-neutral**
on the matrix; they state the wildcard's invariant ("answers no reader") rather than
repair a measured defect.

## 4. The proposal that stands: a function needs no `@any`

**Proposed (maintainer).** An undecided cell is already cloned by the apply, so a
function may not need `@any` at all.

**Measured, and confirmed.** Reverting the placement — `check_lam`'s parameter value
cell and `signature_node`'s two value cells go back to `fresh_cell()` — and keeping
everything else in §2:

| program | with `@any` | reverted | dev |
|---|---|---|---|
| `f = n => (x => x + 0) n; f 3` | error | `3` | `3` |
| `f = n => (x => succ n)` | error | `4` | `4` |
| `f = n => (x => y => x + y) 1 2; f 3` | error | `3` | `3` |
| `add = x => y => x + y; f = n => add n n; f 3` | error | `6` | refused |
| `f = n => (x => y => x) n; f 3` | ok | ok | refused |

Every "not a container" regression is gone and the fixes for the refusals stay. The
mechanism is `node_apply`'s `depends_on_parameter`: a node whose value is empty is
not *proven concrete*, so it is cloned per call already — the wildcard was never
what made per-call freshness.

## 5. What is open

- **A closure that reads `none` for a captured parameter — diagnosed and repaired.**
  `a = 1; f1 = x => { b = 2; f2 = y => [a, b, x, y]; f2 }; f1 3 4` **used to** answer
  `[1, 2, none, 4]` (dev: `[1, 2, 3, 4]`, pinned by
  `crates/lichen-language/tests/language/pipeline.rs::a_nested_function_captures_the_applied_outer_parameter`);
  it now answers `[1, 2, 3, 4]` and that test passes, the cause being the non-uniform
  copying recorded in §6. Every measurement below was taken **before** the repair.
  Every link of the chain is measured on this branch. **The process, in order:** check time
  gives each closure a class that already holds the template's function value. A call then
  copies the template into that call's nodes and binds the argument into the *copy's*
  parameter cell, while the template's own parameter cell is never written. The copying is
  not uniform — a node can be referenced in place in one walk and copied in another, and a
  call's walk runs nested walks that share its image map — so one class's images come out in
  different shapes, one of them still naming the template. Reconciling those images within
  the call walks into the two closure values, and the function kind compares only
  `[parameter, return]`, so it declares the template and the instance one value and merges
  their classes. The merged class keeps both values in its members, but a read answers the
  class root's own slot, which holds the template. The caller therefore applies the
  *template*, whose parameter cell is the one never bound: everything derived from the
  parameter stays undecided — what a reader sees as `none` — while constants the body writes
  itself survive. That one class then serves every later call, so each loses its own
  parameter-derived captures.
  - The inner apply computes and writes the minted generation:
    `WIRE node=78 applied=87 applied_slot0=Function(3) result_slot0=Function(3)`.
  - `unify_clone_groups` of that instantiation unifies two *clones*, and they are not
    the same shape: `GROUP-PAIR first=106 value_items=[91,106] clone=95
    value_items=[91,46]`. Both stand for the closure's self-referential pair (their
    templates 46 and 45 are one class), but only 106 is self-referential — **95's second
    element is the template node 46**. The two writes that made them, tagged with the
    instantiation that made each: `CLONE-WRITE applied=Function(2) epoch=24 template=45
    clone=95 items=[91,46]` and `CLONE-WRITE applied=Function(1) epoch=39 template=46
    clone=106 items=[91,106]`.
  - **Why two shapes exist: the clone decision is context-dependent, and one call's
    remap covers both contexts.** `node_apply`'s `depends_on_parameter` includes
    `value_holds_foreign_function(value, ctx.applied)`, and the closure inside the pair
    is `Function(2)` — which *is* the applied function in one context and a foreign
    closure in the other. `value_apply`'s closure branch builds the nested walk
    (`applied` = the closure, `closure_scope = Some(scope)`, `branch_top` = the fresh id)
    that **shares the parent's `remap`**, so the closure's scope is cloned first, in the
    context where that closure is not foreign, and the pair node is then reached again by
    the outer walk, where it is. Measured on the one node, every line carrying
    `remap=592092280928`: `DECIDE node=46 applied=Function(2) branch_top=Function(3)
    closure_scope=true epoch=24 depends=false` and `CLONE-WRITE applied=Function(2)
    epoch=24 template=45 clone=95 items=[91,46]` (nested walk: referenced in place, so the
    sibling clone's type slot kept naming the template), then `DECIDE node=46
    applied=Function(1) closure_scope=false epoch=39 depends=true` and `CLONE-WRITE
    applied=Function(1) epoch=39 template=46 clone=106 items=[91,106]` (outer walk:
    cloned, self-referential). One call, one remap, two treatments of one node.
  - The group therefore holds both shapes, and `regroup_clones`' key is sound where the
    capture is concerned: the capture's two template nodes are one class (`26->98 rep=49`,
    `49->100 rep=49`).
  - **How common the merge is, and what it does *not* explain.** Counting one run's merge
    log (433 merges for u3, 97 of them template-against-clone, 48 of those array-carrying),
    the merge is the walk's normal texture rather than an anomaly — and the count of
    *closure*-carrying ones does **not** separate a failing program from a correct one.
    Measured: `w2` (the capture-free control, which answers correctly) has 6, `w3` (loses
    one) 6, `u3` (loses one) 2, `n1`, `k3` and `q4` (all correct) 1 each, `v4` (correct) 0.
    What fits every measured program is **which captured cells the call must clone** — the
    ones `depends_on_parameter` is true for, i.e. the parameter itself or a cell that is not
    proven concrete — read by a closure whose class merged with its template's: u3 loses `x`
    while its `b = 2` survives, so the criterion is not "any captured cell read".
    `y2 = a = 1; f1 = x => { b = 2; f2 = y => [a, b, c, x, y]; c = 3; f2 }; f1 3 4` answers
    `[1, 2, 3, none, 4]` — both constant captures survive and only the parameter is lost —
    and `y1 = a = 1; f1 = x => { b = x + 1; f2 = y => [a, b, x, y]; f2 }; f1 3 4` answers
    `[1, none, none, 4]`, losing a captured local that must be cloned as well, so the rule is
    not parameter-specific. `dev` answers all three correctly (`[1, 2, 3, 4]`, `[1, 2, 3, 3,
    4]`, `[1, 4, 3, 4]`). Whole-log counts also vary a few percent between runs — the walk is
    hash-ordered — so they are indicative, not exact.
  - Its descent then compares a *clone* with a *template*: descending `[91,
    46]` against `[91, 106]` reaches element 1 = `(106, 46)`, and inside that pair
    element 0 = `(91, 44)` — the minted closure `Function(3)` against the template
    closure `Function(2)`. The recursion's own depth counter shows the nesting:
    `ARR depth=0 a=95[91,46] b=106[91,106]`, `ARR depth=1 a=46[44,46] b=106[91,106]`,
    `FN-ARM depth=2 a=44(Function(2) param=43 ret=57) b=91(Function(3) …)`, then
    `ARR depth=3 a=43[41,42] b=94[92,93]` for the cell interiors whose elements are the
    `92↔41`, `93↔42`, `94↔43` merges. The decision sits two levels inside the group
    pair, on the clone/template boundary.
  - **The `LowValue::Function` arm is what merges them.** Two functions are one value
    when their `[parameter, return]` cells agree, and a clone's cells agree with its
    template's by construction. `add_equality` then merges the classes, and **a class is
    multi-valued in its members' slots: what a reader gets is the *root's own slot*** —
    `class_value` walks to the root and returns its value — while `fill_class_holes` writes
    only into members that hold nothing. Measured on the decisive merge:
    `PRE-ADD a=44(origin=None) va=Function(2) b=91(origin=Some(44)) vb=Function(3)` gives
    `POST-ADD rep=38 rep_slot=Function(2) members=[38=F2 44=F2 71=F2 91=F3 88=F3]`. The class
    therefore carries the template's closure in its root and in two other members and the
    minted clone in two more, and answers the root — so the merge's argument order does not
    decide it, which is why every run answered the same way while the two sides swapped
    order between runs. The value's provenance is
    measured as well: every closure value is written into a template node during check —
    `SET-FN node=… origin=None epoch=5/8/10/… value=Function(1)/Function(2)/…`, epochs 5
    to 33 — while the walk that makes the clones runs from epoch 39, so the class's answer
    predates every clone. The class *membership* is a check-time merge too: the first
    `Function`-valued merge of the run is `EQ a=49(origin=None value=None)
    b=55(origin=None value=Function(2))` — the closure node joining an undecided node of
    its use site — which is why the class the clone later joins already answers
    `Function(2)`. (Node ids quoted in this note name one run; read them by
    `origin` and value, not by number.) Both structural claims were then re-run **twice**
    on the committed state with identical counts: two merges of decided closures per run,
    one of them with an `origin=None` side; in both runs the clone's slot keeps its own
    value (two of two); and 76 `SET-FN` writes per run over epochs 5 to 224, only two of
    which carry an origin — the check-time writes into template nodes are 74 of the 76.
  - Everything after that is consequence: the class answers `Function(2)`, and the caller's
    callee read resolves to it. Re-measured on the committed state (an apply's `check_app`
    leaves `state[e].val` empty, so the callee operand is an operation node):
    `CALLEE apply=NodeId(86) operand=NodeId(82) slot=Function(2) class=Function(2)` for the
    outer apply, against the inner one's `CALLEE apply=NodeId(78) operand=NodeId(29)
    slot=Function(1)`. The outer apply therefore instantiates the template, and both
    parameter checks are current-state measurements: the `f1 3` instantiation binds its own
    clone (`PARAMCHECK function=Function(1) node=78 param=97 argument=75` →
    `PARAM-BOUND … param_slot0_now=USize(3)`), while the instantiation the outer apply runs
    is the template's (`PARAMCHECK function=Function(2) node=86 param=113 argument=83` →
    `PARAM-BOUND … param_slot0_now=USize(4)`) — it binds only the closure's *own* parameter,
    so the captured cell is in none of its bindings and the answer loses exactly that
    position: `[1, 2, none, 4]`. The reason the loss lands on those positions is measured
    too: **the binding lands on the clone, and the template's own parameter cell is never
    bound** — `PARAM-BOUND function=Function(1) node=78 param=97 param_slot0_now=USize(3)
    template_param_slot0=undecided:NodeId(26)`, and `undecided:NodeId(41)` for the f2
    instantiation the outer apply runs — so the template instantiation computes `none` for
    everything derived from the parameter while whatever the body's own statements write
    stays: constant locals survive, the parameter and what depends on it do not.
  - **The mechanism predicts which positions lose their values.** Three programs written
    from the cause (not from the measured set) and then run: `a = 1; f1 = x => { b = 2;
    f2 = y => { c = 3; f3 = z => [a, b, c, x, y, z]; f3 }; f2 }; f1 3 4 5` answers
    `[1, 2, 3, none, none, 5]` — one lost cell per captured level, `x` and `y`; the same
    shape whose innermost body captures nothing (`[a, b, c, z]`) answers `[1, 2, 3, 5]`,
    correct; and the variant capturing only the middle parameter (`[a, b, c, y, z]`)
    answers `[1, 2, 3, none, 5]`, losing exactly `y`. So the loss tracks captured cells
    one per level, and the capture-free control is intact. All three are refusals on
    `dev`. The prediction extends to repetition and holds: `(f1 3 4, f1 5 6)` answers
    `([1, 2, none, 4], [1, 2, none, 6])` — each call loses *its own* capture — and
    `r = f1 3 4; f1 5 6` answers `[1, 2, none, 6]`, the observed call being the second
    one. The merge therefore poisons the class for every later call through it, which is
    what "the class answers the template" means once the class is shared.
- **One rule behind both merges.** The arm decides "same value" from `[parameter,
  return]` alone, and that is measured on both symptoms: for the capture loss the pair
  it declares equal is a clone against its template
  (`FN-ARM depth=2 a=44(Function(2) …) b=91(Function(3) …)`), and for the capture merge —
  `mk = x => y => x; a = [mk 1, mk 2]; (a[0] 0, a[1] 0)` answers `(1, 1)` where dev
  refuses — it is two *calls* of one function
  (`FN-ARM depth=1 a=Function(3) origin=Some(NodeId(30)) b=Function(4) …` under
  `mk 1` / `mk 2`). Code agrees in both cases because a closure's identity here is its
  code half; the captured environment lives outside the two cells the arm compares.
  **Verified by intervention, not only by correlation**: with the arm's `add_equality`
  gated on the two ids being equal — distinct closures then agree and keep their own
  classes — `a = 1; f1 = x => { b = 2; f2 = y => [a, b, x, y]; f2 }; f1 3 4` answers
  `[1, 2, 3, 4]` and `mk = x => y => x; a = [mk 1, mk 2]; (a[0] 0, a[1] 0)` answers
  `(1, 2)`, while every other measured program keeps its answer (including this
  branch's repaired refusals `n1`, `k3`, `k5`) and
  `a_nested_function_captures_the_applied_outer_parameter` passes. The gate was a
  diagnostic and is reverted; it decides nothing about what the repair should be.
  The merge is also **unique to the failing case in the measured set**: counting every
  merge of two decided closures, `u3` has two and one of them has an `origin=None` side
  (the template), while `v4`, `v5` and `q4` have none and `n1` (seven), `o2` (three) and
  `k3` (three) merge clones against clones only. So a clone-against-template merge is
  present exactly where the capture is lost, and taken away exactly there the answer
  returns.
  **A second diagnostic isolates the trigger.** With the closure test inside
  `depends_on_parameter` made context-free (`ctx.anchor` in place of `ctx.applied`), the
  capture loss also disappears (`[1, 2, 3, 4]`) while the capture merge does not move
  (`(1, 1)`) and no other measured answer changes. The two diagnostics therefore separate
  the two links: the context-dependent in-place decision is what makes the mixed shape,
  hence the merge that loses the capture; the comparison rule alone is what merges two
  calls' closures. Both gates were diagnostics and are reverted.
- **The same `none`, without a function merge.** `mk = x => y => x; f = n => mk n 1;
  f 3` (three spellings of it in the scratch set) answers `none` as well, and it is
  **not** this cause. Measured on the clone merges that carry a closure: u3 has a
  clone-against-template pair — `a=44(origin=None Function(2)) b=91(origin=Some(44)
  Function(3))` — while this program has none, every `Function`-valued side being a
  clone (`a=63(origin=Some(30) Function(4)) b=92(origin=Some(30) Function(4))` and
  friends). The diagnostic gate that repairs the capture loss above therefore leaves
  its answer unchanged, and its `none` comes from a different path.
- **`examples/gc.lichen` is also outside this cause**, and it was measured the same way:
  it makes six merges of decided closures and **none** of them has an `origin=None` side,
  and the gate that repairs the capture loss leaves its `[none, none]` unchanged (`u3` and
  `p1` do change under that gate, as controls). The README's `[8, 16]` and this `[none,
  none]` are therefore a different defect from the one this section measures.
- **The diagnostic shape.** `an_unannotated_lambda_has_an_undecided_arrow_type`
  pins the recorded value side of `(\x. x) : Type` to the function-type array; the
  arm now records the `Function` leaf, which is nearer the disagreement but is a
  pin to move deliberately.
- **The unnamed writer.** §3's `write_node_value(node, Some(Any))` is still to be
  named — tag `write_node_value`'s call sites, or take a backtrace, rather than
  adding another reader-side filter.

## 6. Repairs, and what each one is measured to do

### The root, at the design level

Three premises, each reasonable on its own, collide:

1. `f : f` — a lambda's term *is* its type. A closure is a self-referential pair
   `[function node, that pair]` inside a **class**, and the class carries the function
   leaf value, so "what type is this" and "which closure is this" share one structure.
2. The function kind's rule descends `[parameter, return]` — the **signature** — while a
   closure's *meaning* is its code **plus the environment of the call that made it**. The
   environment is not part of what is compared, so a template, any instance of it, and two
   calls of it are all "one value" by the rule.
3. A class answers **one** value, chosen by topology rather than by the reader's need: its
   members may each hold a different value, but a read answers the **root's own slot**, and
   the root is the node the check-time merge left in place. So an instance merged into its
   template's class is absorbed — its own, correctly bound value stays in its own slot and
   is never the one answered.

The machinery then makes the collision necessary rather than incidental: an apply must
instantiate, an instantiation must reconcile the images of a class within the call, and one
of those images *is* the template. In one sentence: **closures are unified by their code
and given one class-wide meaning, but a closure's meaning is its code plus the environment
of its call, and the environment is outside unification — so an instance that meets its own
template is absorbed into it and the environment that distinguished it is lost.** The
function arm applies a *type-level* rule (equal signatures) to a *value-level* question
(which closure), which is the category error underneath; the capture loss and the capture
merge are its two visible faces.

The cause above is measured, and one repair has **landed**; the rest are candidates with
what is already known about each. Of the candidates, B was run as a diagnostic (reverted),
so its effect on the programs is measured but never against the suites.

- **A. Make the clone decision context-free — landed.** The closure test inside
  `depends_on_parameter` is now judged against `ApplyCtx::anchor`, the call's applied
  function that nested walks already reuse unchanged, instead of `ctx.applied`, the
  function being walked.
  - Measured after landing: the pinned program answers `[1, 2, 3, 4]` and
    `a_nested_function_captures_the_applied_outer_parameter` passes; the whole family it
    belonged to answers correctly (`w1` `[1, 2, 3, 3, 4, 5]`, `w3` `[1, 2, 3, 4, 5]`, both
    calls of `x1`, `x2`, and the constant- and non-constant-capture discriminators `y1`
    and `y2`); lowlevel 160/0 and highlevel 86/1, the remaining red being the
    diagnostic-shape pin; and every other measured program keeps its answer, including
    this branch's repaired refusals.
  - What it does **not** repair: the capture merge and the `mk n 1` path, which are other
    causes (B below is the measured candidate for the former).
- **B. Let the function arm agree without merging when the two ids differ.** Two distinct
  closures would compare equal without their classes becoming one.
  - Measured (as the `a == b` gate, §5): the capture loss disappears (`[1, 2, 3, 4]`), the
    capture merge is repaired (`(1, 2)`), the pinned pipeline test passes, and every other
    measured program keeps its answer, including this branch's repaired refusals.
  - Pros: repairs both symptoms at the one place that decides them.
  - Cons: it changes what "two functions are one value" means — agreement without a shared
    class — so propagation through a function-valued class weakens, and the `f : f` cases
    this branch exists for would have to be re-measured against the suites; it also makes
    `add_equality`'s "where both sides are decided, they are one value" hold only per
    identity, which is a contract worth stating explicitly if it is chosen.
- **C. Let the Apply arm prefer the callee node's own slot.** Unmeasured.
  - Pros: it targets the harm where it happens — a reader taking a class-wide answer where
    the call's own instance matters — without touching unification.
  - Cons: it contradicts the class model ("a class answers its committed value"), so it
    needs its own decision record and permission before it is even tried.
- **D. Narrow `regroup_clones`' key to the template node instead of its class.** Unmeasured.
  - Pros: it stops exactly the pairing measured in §5 — the two clones whose templates are
    one class would land in separate groups.
  - Cons: reconciling a class's images in one call is what the regroup is for; narrowing the
    key would leave images unreconciled and plausibly break the `f : f` repairs, so it may
    trade one loss for another.
- **E. Compare captures in the function arm.** Unmeasured, and a design change.
  - Pros: it addresses the criterion itself rather than its consequence.
  - Cons: a capture environment is not part of the `[parameter, return]` pair, so it needs a
    representation; comparing captures by class would still call two calls' closures equal
    (the capture merge), and comparing them by instance would also block a clone from
    unifying with its template, which is what `f : f` needs — so as stated it is likely
    wrong.

The remaining candidates each need the same measurements before they are tried: the three
influenced suites against the recorded 160/0 and 86/1; the program set above against its
recorded answers; the branch's own repaired refusals (`n1`, `k3`, `k5`, `prelude_reads`,
`wildcard_positions`); and the diagnostic-shape pin, which must be moved deliberately
rather than by a candidate's side effect.

## 7. Reproductions

Scratch programs (gitignored, `C:\resource\lichen-vm\.scratch-core\`): `n1` (the
minimal refusal), `u3` (the capture loss), `p1` (the capture merge), `v1`–`v5`,
`k3`/`k5`/`k6`, `q1`/`q2`/`q4`/`q7`. The two committed test files are
`crates/lichen-highlevel/tests/highlevel/wildcard_positions.rs` (every parameter owns an
undecided value cell, and an applied parameter still computes) and
`crates/lichen-language/tests/language/prelude_reads.rs` (an operation over an applied
parameter records no read failure in the program's own pass; the CLI's own entry
accepts a compiled body).
