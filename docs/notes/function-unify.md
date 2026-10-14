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
    element is the template node 46**, because `node_apply(46)` returned it **in place**
    (`IN-PLACE node=46 reason=no-parameter mate=true`): 46 has a decided deep
    evaluation, is not the parameter, is not scoped and holds no foreign closure, so
    that reach treated it as not depending on the call, while its class-mate 45 was
    cloned.
  - The group unify therefore compares a *clone* with a *template*: descending `[91,
    46]` against `[91, 106]` reaches element 1 = `(106, 46)`, and inside that pair
    element 0 = `(91, 44)` — the minted closure `Function(3)` against the template
    closure `Function(2)`. The observed merge series is exactly that descent:
    `92↔41`, `93↔42`, `94↔43` (the two cells' interiors), then `91↔44`, then `106↔46`.
  - **The `LowValue::Function` arm is what merges them.** Two functions are one value
    when their `[parameter, return]` cells agree, and a clone's cells agree with its
    template's by construction. `add_equality` then gives the merged class its *one*
    value — the first committed, which for `{91, 44}` is the template's `Function(2)`,
    written during check.
  - Everything after that is consequence: the class answers `Function(2)`, the caller's
    callee read joins it (an apply's `check_app` leaves `state[e].val` empty, so the
    callee operand is an operation node), the outer apply instantiates the template, and
    the capture is cloned inside that *template* instantiation where nothing binds it —
    the parameter check binds the `f1 3` instantiation's clone (`cloned=97 …
    after=(98 slot=Some(USize(3)))`) while the executed body reads the template
    instantiation's clone (`109`, `origin=49`, empty).
  - `regroup_clones`' key is sound where the capture is concerned: the capture's two
    template nodes are one class (`26->98 rep=49`, `49->100 rep=49`).
- **A closure with a different capture merges.** `mk = x => y => x; a = [mk 1,
  mk 2]; (a[0] 0, a[1] 0)` answers `(1, 1)` where dev refuses: the signature descent
  sees `[parameter, return]`, and a capture is in neither cell, so two closures with
  different captures look equal. The same one-carrier question.
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
