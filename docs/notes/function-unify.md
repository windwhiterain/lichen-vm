# A function unifies by its own cells

> Status: **in progress.** Branch `feature/function-unify`. The value match has a
> `LowValue::Function` arm beside `Array`, so two functions unify by descending
> `[parameter, return]` and the `function_type_function` recogniser is no longer
> part of unification; a class pair met again on the descent now agrees instead of
> recording a conflict. The wildcard experiments (a lambda's value positions
> holding `@any`) are **rejected and reverted**, with the measurements below.
> Two suite failures remain: one diagnostic-shape pin and one closure that reads
> `none` for a captured parameter.
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
> `crates/lichen-highlevel/tests/wildcard_positions.rs`,
> `crates/lichen-language/tests/prelude_reads.rs`.
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
| `70397dd5` | `crates/lichen-highlevel/tests/wildcard_positions.rs` and `crates/lichen-language/tests/prelude_reads.rs` |
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

- **A closure that reads `none` for a captured parameter.** `a = 1; f1 = x => { b =
  2; f2 = y => [a, b, x, y]; f2 }; f1 3 4` answers `[1, 2, none, 4]` (dev:
  `[1, 2, 3, 4]`, pinned by
  `crates/lichen-language/tests/pipeline.rs::a_nested_function_captures_the_applied_outer_parameter`),
  and `examples/gc.lichen` answers `[none, none]` where dev refuses it. Every link of
  the chain is measured on this branch:
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
  - **Why one walk embedded the template and the other did not: the clone decision is
    context-dependent.** `node_apply`'s `depends_on_parameter` includes
    `value_holds_foreign_function(value, ctx.applied)`, and the closure inside the pair
    is `Function(2)` — which *is* the applied function in one instantiation and a
    foreign closure in the other. Measured on the same node: `DECIDE node=46
    applied=Function(2) epoch=24 holds_foreign=false depends=false` (referenced **in
    place**, so the sibling clone's type slot kept naming it) against `DECIDE node=46
    applied=Function(1) epoch=39 holds_foreign=true depends=true` (cloned, so 106 is
    self-referential). One node, one template class, two shapes.
  - The group unify therefore compares a *clone* with a *template*: descending `[91,
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
    template's by construction. `add_equality` then merges the classes, and **which value
    a reader gets is decided by the merged class's representative, not by the merge's
    argument order**: measured, `PRE-ADD a=44 va=Function(2) b=91 vb=Function(3)` gives
    `POST-ADD rep=38 rep_class=Function(2) a_slot=Function(2) b_slot=Function(3)`. The
    representative is 38, a node that has carried the template's `Function(2)` since
    check time (its class already held 44 and 71), and `fill_class_holes` writes only
    into members that hold nothing — so the clone's own slot keeps `Function(3)` while
    the class answers `Function(2)`. That is why every run answered the same way even
    though the merge's two sides swapped order between runs. The value's provenance is
    measured as well: every closure value is written into a template node during check —
    `SET-FN node=… origin=None epoch=5/8/10/… value=Function(1)/Function(2)/…`, epochs 5
    to 33 — while the walk that makes the clones runs from epoch 39, so the class's answer
    predates every clone. (Node ids quoted in this note name one run; read them by
    `origin` and value, not by number.)
  - Everything after that is consequence: the class answers `Function(2)`, the caller's
    callee read joins it (an apply's `check_app` leaves `state[e].val` empty, so the
    callee operand is an operation node), the outer apply instantiates the template, and
    the capture is cloned inside that *template* instantiation where nothing binds it —
    the parameter check binds the `f1 3` instantiation's clone (`cloned=97 …
    after=(98 slot=Some(USize(3)))`) while the executed body reads the template
    instantiation's clone (`109`, `origin=49`, empty).
  - The two shapes come out of **one remap, one call**: `value_apply`'s closure branch
    builds a nested walk (`applied` = the closure being cloned, `closure_scope =
    Some(scope)`, `branch_top` = the fresh id) that **shares the parent's `remap`**
    (`function.rs`, the `inner` context it builds), so the closure's scope is cloned
    first — in a context where the pair's own closure *is* `ctx.applied` — and the pair
    node is then reached again by the outer walk, where it is foreign. Every line
    carries the one map: `DECIDE node=46 applied=Function(2) branch_top=Function(3)
    closure_scope=true epoch=24 depends=false` and `CLONE-WRITE applied=Function(2)
    epoch=24 template=45 clone=95 items=[91,46]` (nested walk, in place), then
    `DECIDE node=46 applied=Function(1) closure_scope=false epoch=39 depends=true` and
    `CLONE-WRITE applied=Function(1) epoch=39 template=46 clone=106 items=[91,106]`
    (outer walk, cloned), then `GROUP-PAIR function=Function(1) node=78 epoch=42
    first=95[91,46] clone=106[91,106]` — all with `remap=592092280928`. One call, one
    remap, two treatments of one node, which is why a single group unify holds both
    shapes.
  - `regroup_clones`' key is sound where the capture is concerned: the capture's two
    template nodes are one class (`26->98 rep=49`, `49->100 rep=49`).
  - **The mechanism predicts which positions lose their values.** Three programs written
    from the cause (not from the measured set) and then run: `a = 1; f1 = x => { b = 2;
    f2 = y => { c = 3; f3 = z => [a, b, c, x, y, z]; f3 }; f2 }; f1 3 4 5` answers
    `[1, 2, 3, none, none, 5]` — one lost cell per captured level, `x` and `y`; the same
    shape whose innermost body captures nothing (`[a, b, c, z]`) answers `[1, 2, 3, 5]`,
    correct; and the variant capturing only the middle parameter (`[a, b, c, y, z]`)
    answers `[1, 2, 3, none, 5]`, losing exactly `y`. So the loss tracks captured cells
    one per level, and the capture-free control is intact. All three are refusals on
    `dev`.
- **A closure with a different capture merges.** `mk = x => y => x; a = [mk 1,
  mk 2]; (a[0] 0, a[1] 0)` answers `(1, 1)` where dev refuses: the signature descent
  sees `[parameter, return]`, and a capture is in neither cell, so two closures with
  different captures look equal. The same one-carrier question.
- **One rule behind both merges.** The arm decides "same value" from `[parameter,
  return]` alone, and that is measured on both symptoms: for the capture loss the pair
  it declares equal is a clone against its template
  (`FN-ARM depth=2 a=44(Function(2) …) b=91(Function(3) …)`), and for the capture merge
  it is two *calls* of one function
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
- **The diagnostic shape.** `an_unannotated_lambda_has_an_undecided_arrow_type`
  pins the recorded value side of `(\x. x) : Type` to the function-type array; the
  arm now records the `Function` leaf, which is nearer the disagreement but is a
  pin to move deliberately.
- **The unnamed writer.** §3's `write_node_value(node, Some(Any))` is still to be
  named — tag `write_node_value`'s call sites, or take a backtrace, rather than
  adding another reader-side filter.

## 6. Reproductions

Scratch programs (gitignored, `C:\resource\lichen-vm\.scratch-core\`): `n1` (the
minimal refusal), `u3` (the capture loss), `p1` (the capture merge), `v1`–`v5`,
`k3`/`k5`/`k6`, `q1`/`q2`/`q4`/`q7`. The two committed test files are
`crates/lichen-highlevel/tests/wildcard_positions.rs` (every parameter owns an
undecided value cell, and an applied parameter still computes) and
`crates/lichen-language/tests/prelude_reads.rs` (an operation over an applied
parameter records no read failure in the program's own pass; the CLI's own entry
accepts a compiled body).
