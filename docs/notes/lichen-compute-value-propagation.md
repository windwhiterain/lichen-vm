# lichen: the value does not flow into a curried static-closure's parameter

**Resolved.** Both failure modes are fixed: A (§2, the baked-static no-capture
limitation, kept from the earlier session) and B (§4, the lazy host-operator
value arriving after the apply's parameter unify — fixed in §6 by the
`evaluate_node` postlude's class replication). The full suite is green
(§7). The analysis below is kept as the record of what the two bugs were and
how they were diagnosed.

In lichen's own vocabulary: `Module`/`Node`/`Function`, the `[value, type]`
pair, union-find classes (`Node::equality`), `unify`/`bind`/`force_pending`/
`alias_read`, the deep pass, `Parameterized`, and the static-module machinery.
The compute extension only adds the `Kernel` value and the `Jit`/`Launch`
operators; treat everything else as core lichen.

---

## 1. The motivating program

An imported (frozen) module `curry.lichen` exporting a curried function, applied
by an importer:

```
@{ add = import "curry.lichen" @}
add 5 3
```

with `curry.lichen` = `x => y => x + y`. `add 5` applies the imported `Function`
to `5` and must return a closure over `y` that carries `x = 5`; applying that
closure to `3` runs `x + y`.

Two things can go wrong, and they are separate:

- **A. The closure never captures.** A static function value is a *frozen
  template*; the static-materialization walk used to bake it, so the closure
  read an unbound static parameter. Fixed (§2). `add 5 3` now gives `8: Int`.
- **B. The capture exists but the value isn't concrete at the apply's bind.**
  A lazy value (computed by a host operator only when the closure forces it)
  arrives on the argument's value cell *after* the apply's parameter unify
  merged the classes, and was never replicated to the parameter's value cell.
  Fixed (§6), by replicating a newly-cached concrete value to the class's
  unbound pure-cell members in the `evaluate_node` postlude.

Note the boundary, verified directly: `add 5 3` **and** `add (make 5) 3` (where
`make = n => n`, so the arg is a lazy static-apply result) both give `8: Int`.
So B is **not** "any lazy value". B fires only when the argument's value cell
is still unbound at the moment the parameter is bound — which today happens
only for a value a **host operator** (`OperatorExt::run`) computes and writes
onto the class representative *after* the apply. The lone concrete trigger is
the compute `Kernel`; the mechanism is described generally below.

---

## 2. The fix for A (kept, regression-free)

`static_remap_value` now detects, during the materialization walk, a `Function`
value whose body (a static function in the *same* `StaticModule`) reaches the
**applied function's parameter** static node. When it does, instead of baking
it verbatim it creates a fresh **dynamic** `Function` in the apply's block and
walks the static function's scope through the apply's shared remap, so the
closure's read of the parameter rewrites to the **parameter's clone** — the node
the apply's parameter unify binds. The fresh `Function`'s own clones are tagged
with the fresh `FunctionId`; a free-variable read (a node owned by the
*enclosing* template) keeps its owner and is referenced in place, mirroring the
dynamic `value_apply` closure-clone branch.

To do this, `StaticFunction` gained a `nodes` field (its scope — the static
mirror of `Function::nodes`), populated at freeze
(`StaticModule::from_module_mapped`) and round-tripped through the artifact
codec (format version 2 → 3). The reachability test (does the static function's
return / parameter / asserts reach the applied parameter) is a plain walk.

Regressions avoided by two guards: a closure that does **not** reach the applied
parameter (e.g. `g = x => x + 1` riding inside a result pair) stays baked
static; and the applied function's **own** value node (the recursion
self-reference) stays baked too.

Verification: lowlevel `basic` 125/125, language + highlevel 191/191 pass; only
the two compute tests fail, on B.

---

## 3. The observed failure (B) — precise trace

`launch` = `StaticFunctionId(1)`, its inner closure = `StaticFunctionId(2)`.
The closure is re-homed as a dynamic `Function` (fresh `FunctionId`); its
`return` clone = `NodeId(95)`, its `parameter` clone = `NodeId(109)`. Its
`$launch` op = `NodeId(96)`, operand = `NodeId(97)`, operand items =
`[NodeId(98) (the kernel read), NodeId(107) (the argument)]`; `Node(98)` is an
`Index` read, `Index(cloned_parameter_pair, 0)`.

`launch` apply parameter unify:
- `cloned_param` = `NodeId(100)` (the parameter clone, a `[value, type]` pair):
  value cell `NodeId(101)`, type cell `NodeId(102)`.
- `argument` = `NodeId(62)` (the `[value, type]` pair of `k`): value cell
  `NodeId(51)`, type cell `NodeId(46)`.
- `unify(100, 62)` descends structurally to `unify(101, 51)` and
  `unify(102, 46)`.

After the unify:
- `Node(101)` (param value cell) = `Parameterized`.
- `Node(102)` (param type cell) = a concrete `Array` — **bound**.
- `Node(51)` (argument value cell) = the `Kernel` value — **bound**.
- `Node(46)` (argument type cell) = a concrete `Array` — **bound**.

So the **type cells** (`102`↔`46`) are bound to concrete `Array`s by the unify,
and the argument's **value** cell (`51`) is bound to the Kernel, but the
parameter's **value** cell (`101`) remains `Parameterized` even though
`unify(101, 51)` ran.

The `bind` log (a `bind` runs when one side is a pure unbound cell) for the
value cells:

```
bind: ra=NodeId(11) rb=NodeId(51)   va=Parameterized vb=None    -> nothing replicated
bind: ra=NodeId(98) rb=NodeId(101)  va=None        vb=Parameterized -> nothing replicated
bind: ra=NodeId(98) rb=NodeId(11)   va=None        vb=Parameterized -> nothing replicated
```

`Node(11)` is the union-find representative of `Node(51)`'s class at that
moment. The three binds merge the classes `{11, 51}`, `{98, 101}`, then
`{98}` into `{11}`, so `11/51/98/101` ultimately share one class — but **every
bind saw an unbound value on both sides**, so nothing was replicated;
`Node(101)` stayed `Parameterized`.

Then the closure applies (`function_apply` on the fresh `FunctionId`, `return` =
`Node(95)`, `parameter` = `Node(109)`). Its operand array `[Node(98), Node(107)]`
evaluates: `Node(98)` = `Index(Node(100), 0)` reads `Node(101)` =
`Parameterized`, so the operator under `run` is invoked with a `Parameterized`
operand and returns `Parameterized`.

---

## 4. Root cause

`Node(51)` ends up holding the `Kernel` value, and `Node(101)` was unified into
the same class, but `Node(101)` does not become that value — because **`Node(51)`
was unbound when the class merged**, and the Kernel value lands on `Node(51)`
only *after* those binds ran.

Why is `Node(51)` unbound at merge time?
- The argument `k` is the result of a static apply that produced the value via a
  **host operator** (`OperatorExt::run`). Its `wire_apply_result` stores the
  value on the apply result node and unifies that node with the applied return,
  so the value is on the class **representative**, not on `Node(51)`'s raw slot.
- `apply_parameter_check`'s `evaluate_pattern_argument` forces the argument's
  value cells, but `evaluate_node` on a **pure cell** (no operation, unbound
  value) returns `Parameterized`; it never follows the union-find rep to the
  cell's pending computation. So at the time the parameter unify runs, `Node(51)`
  still reads unbound.
- `bind` is the only place lichen replicates a class value to members (by
  walking the `meta().next` linked list). It therefore replicates nothing here.

Once the closure body runs and forces the operation, the host operator produces
the value and it lands on the class **representative** — but nothing routes that
value into the already-merged members. lichen's "a value became concrete after
the class was bound" replication exists only for *pending computations*
(`force_pending`, `force_operand` on an operation's operand chain); a lazy host
operator value arriving after the merge has no such path.

In short: **the argument's runtime value is computed by a host operator only when
the closure forces it; the apply binds the parameter before that value is
concrete; `bind`-time replication can't reach the parameter's value cell; and
nothing later replicates the late-arriving value to it.** lichen's own design
comment near the `Index`/`TableGet` reads ("a read of a pure cell is a
reference… joining the reader to the cell's class lets a later bind reach it
through replication") points at the intended mechanism; the missing piece is the
"later bind" actually carrying the value into the already-merged class.  (The
"value is on the class representative" claim in the first bullet is the *initial*
hypothesis — §6 pinpoints the actual write to `evaluate_node`'s postlude, which
caches on `Node(51)` and leaves the rep unbound.)

---

## 5. Methods tried, and why each failed (in lichen's terms)

1. **Make the closure dynamic (fix A)** — §2. Correct and kept. Removes the
   baked-static no-capture limitation; does not touch B.
2. **Force the argument before the parameter bind** — insert
   `evaluate_node_forced(argument)` in `apply_parameter_check` before `unify`.
   Failed: `evaluate_node_forced` forces an operation's **operand chain**, but a
   *pure cell* has no operation, so the pass reads the cell's raw slot and
   returns `Parameterized`; it cannot reach the host operator's computation,
   which lives on a separate node visible only through the class rep. The value
   stays lazy. (Reverted.)
3. **Read through the class representative in `evaluate_node`** — when a node's
   own value is unbound and it is a pure cell, consult
   `equality_representative(node)` and return (and cache) that rep's value.
   No effect: when the closure's `Index(parameter_pair, 0)` is read, the
   captured cell's rep does not hold the value either, because the value reaches
   `Node(51)` via a path (`wire_apply_result` + a lazy host `run`) that does not
   `bind`-replicate onto the already-merged members, so there is no concrete
   value on the rep to read through. Reverted — a broad core-semantics change
   with real regression risk and no effect here.
4. **Replicate a cached value to the class in `evaluate_node`'s postlude** —
   §6. **The fix.**  Correct and kept. A value isn't written onto the rep-only:
   when `evaluate_node` caches a concrete value on a class *member*, it walks
   the class's `meta().next` linked list and sets the value on every unbound
   pure-cell member (the same linked-list replication `bind`/`force_pending`
   use), so a late-arriving host-operator value reaches the cells (and the rep)
   bound before it was concrete. Passes the full suite.

All experimental edits to `apply.rs`, `evaluation.rs`, `equality.rs` were
reverted; the tree stands at the §2 state (fix A) **plus** the §6 postlude fix
(fix B).

---

## 6. The fix (the crux resolved)

The exact write that gives `Node(51)` its value **after** the apply binds is the
`evaluate_node` **postlude** (`if !Parameterized { self.nodes[node].value =
Some(value) }`), which caches a computed value on the evaluated node but does
**not** replicate it to the class. `wire_apply_result` is not the culprit: its
`unify(node, applied)` is a `bind`, which *does* replicate when the value side
is a pure cell, but here the value is already concrete on both of those nodes
by the time that `unify` runs (the affected member is a different node joined
through the pattern walk).

The chain that breaks, traced concretely:
- `launch`'s parameter unify merges the argument's value cell `Node(51)` (a
  pending `Jit` op, unbound) with the parameter's value cell `Node(101)`. Both
  read unbound, so neither `bind` replicates anything (`Node(101)` stays
  `Parameterized`).
- Evaluating the argument (during `apply_parameter_check`'s
  `evaluate_pattern_argument`) forces the `Jit` op; the `Kernel` value arrives
  on `Node(51)` through the `evaluate_node` postlude — a **direct write** that
  skips class replication, so `Node(11)` (the class rep) and `Node(101)`
  (the captured parameter cell) never see it.
- A later `bind` merges the remaining reader (`Node(98)`) into the class, but
  reads only the rep's value (`Parameterized`); the member's concrete `Kernel`
  is invisible to it.

The fix routes the late-arriving value through the class-replication path. In
`evaluate_node`'s postlude, after caching a concrete value on `node`, find the
class representative and walk the `meta().next` linked list, setting the value
on every **unbound pure-cell** member (`operation.is_none()`). Only the
`compute`/lazy host-operator scenario changes semantics; a singleton class is a
no-op, and a pending-computation member keeps its own operation (matching
`force_pending`, so an unresolved computation is never overridden). This is the
evaluation-side counterpart of `bind`'s and `force_pending`'s replication, and
it restores the design comment's promise: *a value that becomes concrete after
the class was bound reaches every member bound before it.*

With the fix, the launch apply's final merge reads the rep's value (`Kernel`),
replicates it onto the parameter's value cell, and the closure's
`$launch(k, a)` runs against a concrete kernel.

---

## 7. Reproduce / state

- `cargo test -p lichen-language --test compute -- jit_then_launch_scalar`
  → now `"6: Int"` (was `"parameterized: Int"`). All four compute tests pass.
- To see the trace, temporarily print: in the operator `run`, the `operand`
  before the `Parameterized` early-return; in `static_function_apply`,
  `cloned_param`/`argument` and their `array_items` values; in
  `StaticModule::static_remap_value`, each `Function` value's
  `StaticFunctionRef`, `same_module`, `== applied`, `reaches_parameter`.
- Full suite green: lowlevel `basic` 125/125; language + highlevel 191/191;
  `cargo test --workspace` exits 0.

Current modified files (uncommitted): the compute extension refactor
(highlevel `checker/ir/lib/native`; language
`ast/compile/compute/lex/lib/package/parse/persist/run`, `tests/compute.rs`),
plus fix A (lowlevel `lib.rs`, `static_module.rs`) **and** fix B (lowlevel
`evaluation.rs`). HEAD is `495e8bf ir rework`.
