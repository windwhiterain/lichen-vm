# An undecided type cell: the four `examples` drifts

> **Status:** **three of the four fixed** (§10); the fourth is attributed and is a
> different mechanism. The suppressing condition is one line —
> `Module::expr_type_half` answers `None` for a **frozen** type half, so the
> `unify` binding an apply's result type cell to the callee's decided return type
> was never called. §9 traces it, §10 fixes it, §10.2 attributes what remains.
> **§10.2's account of the fourth is partly superseded** — §10.2b, after the
> measurement in [arithmetic-class-stated](arithmetic-class-stated.md); the
> attribution to `arithmetic_class` holds, the identification of the undecided
> cell and the "checker-side question" conclusion do not. §1–§8 are kept as the
> diagnosis that led there; where §10 answers one of their open questions, they
> say so. **The index-read route §10.2b ended on is measured and closed in
> [index-read-unify](index-read-unify.md)** — not a skipped unify, and not on the
> cell that route named.
>
> Companions: [examples-panic-diagnosis](examples-panic-diagnosis.md) §"What the
> runner says once past the panic", which first tabulated these four with the
> suite's strings; [raw-rendering-mark](raw-rendering-mark.md), which says what
> `raw` means and why it is not a defect; [class-channel](class-channel.md), the
> class vocabulary; [compile-order-class](compile-order-class.md) §4.1, the
> instrument-the-read method this note repeats;
> [operators](operators.md) §2, what a comparison is **specified** to yield;
> [operator-polymorphism](operator-polymorphism.md) §7.1, the routing's costs;
> [compound-element-slots](compound-element-slots.md) §"The read path", the
> index-read path family B is a case of.

## 1. The four instances

`cargo test -p lichen-compute --test lichen_compute examples` fails on `dev` with
these four, and the survey in [examples-panic-diagnosis](examples-panic-diagnosis.md)
records that a fifth example moving out of `examples/` does not change them. Each
program was also run through `cargo run -q -p lichen-compiler -- run <file>`:

| Example | Declared `output =` | CLI | Suite |
|---|---|---|---|
| `examples/assert.lichen` | `(1, 5): tuple{Int, Int}` | `(raw 1, 5): tuple{?a, Int}` | same |
| `examples/assert_in_function.lichen` | `1: Int` | `raw 1: ?a` | same |
| `examples/import/_.lichen` | `(42, 10, 7): tuple{Int, Int, Int}` | `(raw 42, 10, raw 7): tuple{?a, Int, ?b}` | same |
| `examples/lazy_infinite.lichen` | `(1, 2, 3): tuple{Int, Int, Int}` | `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}` | same |

**The CLI and the harness agree byte for byte on all four**, so the defect is in the
language and **not** in the suite's rendering. That rules out the one hypothesis
under which this would be a runner problem, and it means the `output =` declarations
are not stale *relative to the CLI* — they are what the language should print
(§5).

## 2. What is undecided, exactly

**The two spellings are one fact read twice, not two defects.** Both printers ask
the same question of the same node and both get `None`:

- `TypePrinter::node` (`crates/lichen-render/src/render/type_printer.rs`) — a node
  whose value slot is empty answers `self.class_name(node)`, which is the `?a`;
- `ValuePrinter::value` (`crates/lichen-render/src/render/value_printer.rs`) — the
  same `node_value` read on the same node, whose non-`Array` `else` answers
  `self.raw(value)`, which is the `raw 1`.

So `raw 1: ?a` is one empty cell, printed by the type printer and by the value
printer. [raw-rendering-mark](raw-rendering-mark.md) §5 already predicted this
("a `?a` type explains `raw 6`").

**It is not an undecided *class*.** Instrumenting the read the way
[compile-order-class](compile-order-class.md) §4.1 does — printing the node, its
representative, and every class member with its value — gives, for
`assert_in_function.lichen`:

```
PROBE tnode NodeId(53v1) empty; rep=Some(NodeId(53v1)); rep_value=None; members=[NodeId(53v1)=None]
```

The class has **one member, itself, with an empty value**. So this is not
[class-channel](class-channel.md)'s "a member that is neither reads nothing" and not
a decided class whose value failed to commit: there is no sibling holding `Int` that
could propagate here, and no class to be decided. The cell was **never written and
never joined to anything**. The same holds for all four — the undecided cells are
`53v1` (one), `37v1` (one), `102v1`/`142v1`/`195v1` (three), and `39v1`/`94v1` (two),
each a singleton class.

## 3. One cause or four?

**Not one cause: three.** The four share the read and the shape; the causes differ.
The discriminators, each measured by running one program through the CLI:

| program | prints |
|---|---|
| `n = 5` ; `n <= 5` | `raw 1: ?a` |
| `n = 5` ; `n < 5` | `raw 0: ?a` |
| `n = 5` ; `n == 5` | `1: Int` |
| `n = 5` ; `n + 1` | `6: Int` |
| `f = x => x + 10` ; `f 5` | `15: Int` |
| `f = x => x <= 10` ; `f 5` | `raw 1: ?a` |
| `f = (x : Int) => x <= 10` ; `f 5` | `raw 1: ?a` |
| `add [1, 2]` | `3: Int` |
| `sub [5, 2]` | `3: Int` |
| `mul [3, 4]` | `12: Int` |
| `less [1, 2]` | `raw 1: ?a` |
| `less_or_equal [1, 2]` | `raw 1: ?a` |
| `greater_or_equal [1, 2]` | `raw 0: ?a` |

Ruled out by measurement, each with the program that rules it out:

- **`@assert`.** `Checker::check_assert`
  (`crates/lichen-highlevel/src/checker/asserts.rs`) sets the assert's type cell to
  *the condition's*, which made the assert look like the discriminator. It is not:
  `n = 5` ; `n <= 5` contains no assert and prints `raw 1: ?a`. `check_assert` is
  how the two assert examples *inherit* the comparison's cell — it propagates the
  condition's answer, it does not decide one.
- **`@loop` / unmarked recursion.** The finding recorded one slice ago — that a
  marker is what gives an undecided trip count its `Int` — **does not transfer to
  any of these four.** `@loop f = x => [x, @shallow f (x + 1)]` … prints
  `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}`, identical to the unmarked program, and
  `@loop f = x => x <= 10` ; `f 5` prints `raw 1: ?a`, identical to the unmarked one.
  The marker is not the lever here.
- **`@shallow` by itself.** `xs = [1, @shallow [2, 3]]` ; `xs(0)` prints `1: Int`.
- **A stale declaration.** §1: the CLI prints the same string as the suite.
- **An annotation on the parameter.** `f = (x : Int) => x <= 10` ; `f 5` is still
  `raw 1: ?a`.

## 4. The three families, and which exprs differ

| | Instance | Trigger, as measured |
|---|---|---|
| **A** | `assert.lichen`, `assert_in_function.lichen` | a **routed comparison** — `< <= > >=`. `+ - * /` on the same operands decides. |
| **B** | `lazy_infinite.lichen` | a value read through a **lazy (`@shallow`) chain**. The same read on a literal binding decides. |
| **C** | `import/_.lichen` | a value read through a **frozen module's export**. The same function defined locally decides. |

Which exprs differ, concretely:

- **A vs A** — the two differ only in where the comparison sits (top level, inside a
  called function). Same mechanism; `assert_in_function` adds one hop and nothing
  else.
- **A vs B, and A vs C** — different triggers entirely. Nothing in A survives if the
  operator is arithmetic; nothing in B or C survives if the read stops going through
  the lazy chain or the frozen module. The shared part is only §2's read.
- **B vs C** — both are "a value that arrives through an indirection", and both
  contain elements that *do* decide (`geo.double 5` prints `10: Int` inside the same
  tuple that prints `(raw 42, …)`), so neither indirection is uniformly fatal. They
  are **not** established to be the same mechanism; see §6.

## 5. Verdict per instance

| Instance | Should print | Does print on `dev` | Verdict | After the fix (§10) |
|---|---|---|---|---|
| `assert.lichen` | `(1, 5): tuple{Int, Int}` | `(raw 1, 5): tuple{?a, Int}` | **defect**, family A, traced (§6.1, §9.5). [operators](operators.md) §2 specifies `Int`. | **fixed** |
| `assert_in_function.lichen` | `1: Int` | `raw 1: ?a` | **defect**, family A, traced (§6.1). Same cell as above, one apply away. | **fixed** — same condition, dynamic route |
| `import/_.lichen` | `(42, 10, 7): tuple{Int, Int, Int}` | `(raw 42, 10, raw 7): tuple{?a, Int, ?b}` | **defect**, family C — **not** a separate family; the same condition (§10.1). | **fixed** |
| `lazy_infinite.lichen` | `(1, 2, 3): tuple{Int, Int, Int}` | `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}` | **defect**, family B — attributed to `arithmetic_class`'s open class cell (§10.2). | **unchanged** |

None of the four should be re-declared to its current output. The suite's own
message offers that route "if that was intended", and for these four it is not:
[operators](operators.md) §2 tabulates a comparison's result as `Int`, and three of
the four examples already print `Int` for the *neighbouring* element of the same
shape (`5`, `10`). A declaration that matches today's output would record the defect
as intended.

## 6. What is traced, and what is not

### 6.1 Family A — traced to a join that does not happen

`routed_operator` (`crates/lichen-language/src/compile.rs`) sends `< <= > >=` — and
`+ - * /` — onto the frozen `core` prelude bindings, and deliberately does **not**
route `==`, `!=`, `%` or the bitwise trio, "their contract being a class the checker
already pins". So at the user site `n <= 5` is an `ExprKind::Apply`, and
`Checker::check_app` (`crates/lichen-highlevel/src/checker/lambda.rs`) mints a
fresh cell `c`, stores it as `state[e].ty`, and says in a comment that *"the runtime
apply binds it to the return expr"*.

Both ends of that promise were measured:

- **Inside `core.lichen` the callee's return type cell is decided.** Every
  `check_binop` runs once per operator, and the four comparisons are handed
  `NodeId(12v1)`, which **is** `int_type` — a node whose value is the `Int` type
  value. This is what [operators](operators.md) §2 specifies, and it is correct.
- **At the user site the apply's `c` does not reach it.** For `add [1, 2]` the same
  cell id `34v1` has `representative` `12v1` and prints `3: Int`; for
  `less_or_equal [1, 2]` its `representative` is **itself**, its value is empty, and
  it prints `raw 1: ?a`.
- **The value side is bound either way.** The `alias_read` trace — the function
  whose own doc calls it "a read of a pure cell is a reference", and which performs
  the join — reaches a `TypeValue(TypeInt)` target in both programs
  (`reader=76v1 target=79v1` for `add`, `reader=77v1 target=80v1` for
  `less_or_equal`). The cell `34v1` itself appears in **no** `alias_read` call in
  either program.

**So the mechanism is: for a routed comparison the apply's result type cell is never
joined to the callee's decided return type, so nothing carries `Int` into it, and
the empty cell then renders as `raw`/`?a` by §2.** This is the shape
[class-channel](class-channel.md) §1.1.3 measured for a merge — a member that is not
in the class reads nothing — except that here the class was **never formed at all**,
so no later write could have supplied the value either.

**Not traced:** *which* condition suppresses that join for the comparison callee.
The trace leaves two candidates standing and this slice stops before choosing
between them: the static-materializer path that carries a frozen callee's return
expr, and something keyed on the comparison's own result node rather than on the
callee. Instrumenting the apply that is supposed to perform the bind is the next
step.

### 6.2 Family B — characterised, not traced

Discriminator, measured: the same read decides when the container comes from a
literal binding and does not when it comes through a function application.
`xs = [1, @shallow [2, 3]]` ; `xs(1)` prints `[2, 3]: array{Int, 2}`, while
`f = x => [x, @shallow [x + 1, x + 2]]` ; `g = f 0` ; `g(1)` prints
`[raw 1, raw 2]: array{?a, 2}`. The undecided cell is the array's **element type
position**, and the read path is the one
[compound-element-slots](compound-element-slots.md) §"The read path" describes.

Ruled out: `@shallow` alone (§3), and `@loop` (§3). **Not traced:** the write that
should have carried a type into that element slot once the container resolved. That
note's own claim that `lazy_infinite` reads the declared form is **not true on
`dev`** — corrected there. **Answered, and not by that write:**
[index-read-unify](index-read-unify.md) §7 traced the chain and found the
container's type cell undecided *because the `@shallow` mark kept it so*, then
changed the mark by the user's decision — a `@shallow` position is a **value** mark
and says nothing about the element's type. `lazy_infinite` now reads its declared
`(1, 2, 3): tuple{Int, Int, Int}`, and this discriminator's two rows stay two rows:
`xs(1)` still prints `[2, 3]: array{Int, 2}` and `g(1)` still prints
`[raw 1, raw 2]: array{?a, 2}`, because that element is a literal and its `?a` is
`arithmetic_class`'s open class — §7.4 of that note states why that one *should*
stay undecided.

### 6.3 Family C — characterised, not traced

Discriminator, measured: the frozen-module boundary is necessary but not
sufficient. `math.succ 41` through an `import` prints `raw 42: ?a`, while
`succ = x => x + 1` ; `inc_twice = x => succ (succ x)` ; `inc_twice 5` prints
`7: Int` and `inc_twice 5` through the import prints `raw 7`. Inside the *same*
frozen module `geo.double 5` prints `10: Int` while `math.succ 41` does not, so the
boundary alone cannot be the whole cause.

Ruled out: the frozen status of the callee (a local `succ` decides), and `@loop`
(§3). ~~**Not traced:** what distinguishes `succ` from `double` across the same
boundary.~~ **Answered in §10.1:** it is the same distinction family A turns on —
`double`'s body is `math.add`, whose codomain is an **undecided open class cell**
(cloned per call, so bound), while `succ`'s body is a **comparison**, whose codomain is
the **decided** `int_type` (frozen, so refused). The frozen-module boundary was a red
herring: the discriminator is the *decidedness* of the codomain, not which module the
callee lives in. [class-channel](class-channel.md) §1.1.2 records the nearest prior
measurement — for `geo.double 5` the **value** survived and the **type cell did not
resolve** — and §1.1.3's refutation is the reason this note does not assume the
class value is merely misplaced: there, the structure was rewritten instead.

## 7. What a fix would touch

~~**This slice changes none of them.**~~ **Resolved in §10**, and not along the lines
this section guessed: the change is in `crates/lichen-lowlevel/src/resolve.rs` and in
the two bind sites that reach for the codomain, and
`crates/lichen-lowlevel/src/evaluation.rs` — the file named first, and the one another
session owns — **is not touched**.

- **A** — named `evaluation.rs`, and **named wrong**: the bind lives in
  `static_module/apply.rs` and `function.rs`, and the refusal is one arm of
  `Module::expr_type_half` in `resolve.rs`. §7's framing — "a checker-side decide at
  `check_app`, which would have to read the routed callee's return type" — is not
  needed either: the runtime already read it and discarded it. **Fixed.**
- **B** — the index read's element-type carry. **Located in §10.2**: not the read, but
  `Checker::arithmetic_class`'s `else` branch returning the **shared open class cell**
  when neither operand states a class (`operators.rs:106`). **Not fixed**; it is the
  user's call whether that branch should decide.
- **C** — the frozen-module carry of a return type across the module boundary. **Not a
  separate line**: the same as A. **Fixed.**

## 8. Measured, inferred, unknown

- **Measured** — all four declared/CLI/suite strings; the CLI and the suite
  agreeing on all four; the singleton-class probe and the node ids; the whole of the
  §3 discriminator table, including the four `@loop`/`@assert`/`@shallow`/annotation
  refutations; that `check_binop` hands the comparisons a decided `int_type` inside
  `core.lichen`; that the apply's result cell joins that class for `add` and not for
  `less_or_equal`; that `alias_read` binds the `Int` value in both and names the
  result cell in neither.
- **Inferred** — ~~that families B and C are both "an undecided cell an indirection
  failed to fill"~~ **half refuted in §10.1**: C was the apply's missing join after all.
  Only B is "an undecided cell an indirection failed to fill", and its indirection is
  `arithmetic_class`'s open class cell, not a lazy chain (§10.2).
- **Unknown** — the commit that introduced any of the four. `9c563a3d` ("the raw
  fallback marks every reading it dumps") is the commit that made a scalar's dump
  *visible*, since before it the fallback spelled `1` correctly by accident, so that
  commit can have turned a pre-existing hole into a red suite without causing the
  hole. **Not bisected.**
- **Not this note's subject** — the pre-existing `invalid SlotMap key used` panic at
  `crates/lichen-lowlevel/src/evaluation.rs:517` is another session's, and this note
  attributes around it; the four drifts are reachable with the panic out of the way,
  which is how [examples-panic-diagnosis](examples-panic-diagnosis.md) measured
  them.

## 9. Reading half — where the join is, and the one condition that refuses it

**Read, then measured — §9.5 settles it.** §6.1 stops before naming the suppressing
condition; this section names the code the join passes through and the one conditional
in it that can fail, and §9.5 decides between the candidates.

### 9.1 What `check_app` does with the callee's return type: nothing

`Checker::check_app`
(`crates/lichen-highlevel/src/checker/lambda.rs:251`) is the whole checker half. It
`check_expr`s the callee and the argument, builds the argument expr, and then:

```rust
// The result's type cell rides in the apply's operand; the runtime apply
// binds it to the return expr.
let c = self.fresh_cell();
```

`c` is minted **fresh and unconstrained** — `fresh_cell`, never unified with anything
in `check_app`. It is then threaded into **both** operand arrays at index 2:

```rust
let value_operands = self.array_node(self.current_block, &[function_value, argument_expr, c, function_ty]);
let type_operands  = self.array_node(self.current_block, &[function_ty, argument_expr, c, function_ty]);
```

and stored as the expression's own type slot:

```rust
self.state[e].ty = Some(c);
```

**The checker now binds `c` at check time for a decided function type.** `check_app`
asks the engine for the call's own signature instance (`Module::apply_function_type`,
the `ParameterCheck::Skip` clone) and binds `c` to that instance's codomain, skipping
a callee still under check; an undecided callee keeps the runtime route alone. Until
that bind, the promise in the comment was entirely the runtime's, and §6.1's
"`check_app`'s `c` → the callee's decided return type" join was a missing checker
call — the checker never had it. The two runtime readers below remain, and the
checker's bind is the third route.

### 9.2 The readers of the return expr, and where each binds `c`

The apply encodes **two** applications — the value's `[function, argument, c, type]` and
the type's `[type, argument, c, type]` — and each reaches the bind at run time by a
different route; the checker adds its own bind for the decided function type.

| route | callee kind | the bind | file |
|---|---|---|---|
| the **checker's** instance bind, `apply_function_type` | `Dynamic`, a decided function type | `self.unify(cell, codomain)` where `codomain` is the per-call clone's (`ParameterCheck::Skip`) | `function.rs:apply_function_type`, `lambda.rs:check_app` |
| the **type** application, `apply_callee_type` → `apply_type` | `Dynamic` | `self.unify(cell, codomain)` where `codomain = self.expr_type_half(applied)` | `evaluation.rs:459`, `function.rs:139` |
| the **value** application, `static_function_apply`'s tail | `Static` (frozen) | `module.unify(cell, codomain)` where `codomain = module.expr_type_half(applied)` | `static_module/apply.rs:83-88` |

Both routes read the codomain the **same** way — `Module::expr_type_half` — and both are
the "the runtime apply binds it to the return expr" the checker defers to. The frozen
route's own comment names the reason it exists rather than reusing the type route:

```rust
// A static callee's type is static too, which the dynamic guard refuses.
if let Some(cell) = cell
    && let Some(codomain) = module.expr_type_half(applied)
{
    module.evaluate_node(Dyn(codomain), Some(block));
    module.unify(cell, codomain);
}
```

### 9.3 The condition that suppresses the join

**`expr_type_half` answers `None` for a frozen type half, and both routes skip the
unify on `None`.** `crates/lichen-lowlevel/src/resolve.rs:366`:

```rust
pub fn expr_type_half(&self, node: NodeId) -> Option<NodeId> {
    // SAFETY: `node` is a live node of `self`.
    let items = unsafe { self.array_items(node) }?;
    if !(2..=3).contains(&items.len()) {
        return None;
    }
    match items[1].node {
        AnyNodeId::Dynamic(r#type) => Some(r#type),
        AnyNodeId::Static(_) => None,
    }
}
```

`expr_value_half` (`resolve.rs:331`) is the sibling and carries the same `Static => None`
arm, so the two agree on the encoding; what they do **not** do is resolve a frozen item.
`item_of` (`resolve.rs:278`) and `array_items_of` (`resolve.rs:270`) are the same rule in
the same file, and `static_function_of_callee` (`resolve.rs:293`) is the one reader that
*does* follow a frozen item — through `static_read`, not through a `NodeId`.

**Why this is the discriminator, read from the clone walk.** A frozen function's return
expr is cloned by `static_node_apply`, and `static_remap_value`
(`static_module/apply.rs:192`) re-points an item at a per-call clone **only** under two
conditions:

```rust
AnyNodeId::Static(sref)
    if sref.module == ctx.module.key
        && (ctx.remap.contains_key(&sref.index)
            || ctx.module.nodes[sref.index.index].undecided()) => { /* clone */ }
node => node,
```

So an expr's type half arrives `Dynamic` **iff the frozen node was undecided**, and stays
a `Static` ref iff the frozen node was **decided**. The two families split exactly there,
from the two `check_binop` result arms (`crates/lichen-highlevel/src/checker/operators.rs:70`):

```rust
let result = match operator {
    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => arithmetic.expect("the arithmetic arm ran"),
    _ => self.int_type,
};
```

- `add`'s body is `operands[0] + operands[1]`, whose result is `arithmetic_class`'s
  **shared open class cell** (`operators.rs:145`, `left_ty` when neither operand states a
  class). That cell is *undecided* in the frozen module → cloned → element 1 is `Dynamic`
  → `expr_type_half` answers `Some` → `c` is unified with it → `c` is in the class of the
  cloned operand cell, which the parameter check has just bound to `Int`.
- `less_or_equal`'s body is `operands[0] <= operands[1]`, whose result is `self.int_type`
  **itself** — a *decided* node of the frozen module → not cloned → element 1 is a
  `Static` ref → `expr_type_half` answers `None` → **the `unify` never runs**.

This is the shape §2 measured and §6.1 could not see: the class is not refused, and the
cell is not unconstrained after the join — **the join call itself is never reached**, and
the `?a` cell is the one the frozen module's decided `int_type` would have filled.

### 9.4 The candidates this narrows to, and what distinguishes them

| | candidate | evidence that would confirm | evidence that would refute |
|---|---|---|---|
| **K1** | `expr_type_half`'s `Static => None` refuses the bind; the codomain is `Static` for a comparison and `Dynamic` for `add` | the frozen expr's element 1 is `Static` for `less_or_equal` and `Dynamic` for `add` | element 1 is `Dynamic` in both, and the codomain is empty for another reason |
| **K2** | `static_remap_value` declines to clone, so the decided codomain stays a frozen ref — K1 seen from the other side, its cause rather than its symptom | the frozen `int_type` node reports `undecided() == false` | the frozen node reports `undecided() == true` and yet the item is `Static` |
| **K3** | an ordering fault — the codomain is read before it is decided | `expr_type_half` answers `Some` but the codomain's value is `None` at the unify and decided afterwards | the codomain is decided at the moment of the read |

K1 and K2 are the same event read from two ends and would be settled by one measurement;
K3 is excluded by the order of the statements (`evaluate_node(Dyn(applied))` runs before
`expr_type_half`) **if** that measurement shows the codomain decided at the read.

**The fix's home, if K1 holds, is not `evaluation.rs`.** `expr_type_half` is in
`resolve.rs`; the frozen route's bind is in `static_module/apply.rs`. Both are outside
the file the §7 route-naming reserved, so the choice §7 left to the user does not have
to be made again.

### 9.5 The measurement — the join call is never reached

One probe in `static_function_apply`'s tail, printing the cloned return expr's element 1
and which arm of `expr_type_half` answers, run through the CLI on each example and on
`add [1, 2]` for the contrast. `int_type` is the frozen module's **static index 11** in
every program below, so `[1] Static(…, index 11)` is the decided `int_type` of §6.1.

| program | the apply's result cell (`c`) | expr element 1 | `expr_type_half(applied)` | the cell's class after the bind |
|---|---|---|---|---|
| `add [1, 2]` → `3: Int` | `NodeId(34v1)` | `Dynamic(NodeId(50v1))`, decided | **`Some(NodeId(50v1))`** | joins, `Int` |
| `assert.lichen` → `(raw 1, 5)` | `NodeId(37v1)` | `Static(ModuleKey(0), index 11)` | **`None`** | singleton `37v1`, empty |
| `assert_in_function.lichen` → `raw 1` | `NodeId(83v1)` | `Static(ModuleKey(0), index 11)` | **`None`** | singleton `83v1`, empty |
| `assert_in_function.lichen` → `raw 1` | `NodeId(154v1)`, `227v1` | `Static(ModuleKey(0), index 11)` | **`None`** | singleton, empty |

**`34v1` and `37v1` are the very ids §6.1 named**, so this is that measurement, not a
second one: the same cell `34v1` that joins `int_type`'s class for `add` stays a
singleton for `less_or_equal`, and the difference is entirely whether
`expr_type_half` answers.

**Which candidate this settles.** K1 and K2 are the same event read from two ends, and
the probe shows both halves: element 1 is `Dynamic` **iff** the frozen node was
undecided, exactly `static_remap_value`'s two-clause guard. K3 is **refuted** — where
the read is taken at all, the codomain is already decided (`[1] Dynamic(NodeId(50v1))
decided=true`), so this is not an ordering fault. The condition is:

> **A decided return type is carried into the clone as a frozen `Static` ref, and
> `expr_type_half` answers `None` for a frozen type half, so
> `static_function_apply`'s `unify(cell, codomain)` is never called.**

The `?a` cell is a singleton with an empty value because the one call that could have
filled it was skipped — not because a class was refused and not because the value was
mislaid. §6.1's "the static-materializer path that carries a frozen callee's return
expr" is this: the materializer carries it, and the reader declines it.

### 9.6 B and C do not fail the same way — measured, and §10 corrects this

The same probe on the other two examples shows the opposite of family A: **every** apply
whose bind *does* run ends with its cell in a class whose every member is decided.

| example | binds that ran | the cell's class after the bind |
|---|---|---|
| `import/_.lichen` | `126v1 → 154v1`, `194v1 → 265v1`, `333v1 → 345v1`, `382v1 → 394v1`, `68v1 → 255v1`, `528v1 → 556v1`, `492v1 → 588v1` | **all members decided** (`m=…:true` throughout; the largest class has 22) |
| `lazy_infinite.lichen` | `269v1 → 328v1`, `307v1 → 397v1`, `376v1 → 466v1` | **all members decided** |

**This reading was half wrong, and §10.1 says which half.** It is right for B: the cells
that print `?a` in `lazy_infinite.lichen` are decided, and the `?a` sits elsewhere
(§6.2's element slot). It is **wrong for C**: the cells that print `?a` in
`import/_.lichen` were decided too — but only because the two applies whose bind
matters, `math.succ` and `geo.inc_twice`, **never reached the probe at all**, the bind
having been skipped on the very `expr_type_half = None` arm this section measures
elsewhere. The probe could not see them, and "every bind that ran was decided" was read
as "nothing is missing".

The transferable lesson is about the method: a probe placed **after** the branch that
can fail reports only the calls that passed it. §9.5 escaped this by also printing the
cell's own class and the arm taken; §9.6 did not, and the inference went through
unchecked.


## 10. The fix, and what each instance does after it

**One condition, three of the four instances, fixed.** The change is three files in
`lichen-lowlevel`, and the same two lines on each of the two routes §9.2 named.

> **A decided return type is carried through the clone walk as the frozen `Static` ref
> it was, and `Module::expr_type_half` answers `None` for a frozen type half, so the
> `unify(cell, codomain)` that binds an apply's result type cell to the callee's
> decided return type was never called.**

`crates/lichen-lowlevel/src/resolve.rs` — the expr's type half as the reference the
expr itself holds, frozen or not. `expr_type_half` now reads through it, so the width
rule stays in one place and **its `# Invariant` is unchanged, word for word**:

```rust
    pub fn expr_type_half(&self, node: NodeId) -> Option<NodeId> {
        match self.expr_type_item(node)? {
            AnyNodeId::Dynamic(r#type) => Some(r#type),
            AnyNodeId::Static(_) => None,
        }
    }

    /// The **type** half of a `[value, type]` expr node, as the expr's own reference.
    pub fn expr_type_item(&self, node: NodeId) -> Option<AnyNodeId> {
        // SAFETY: `node` is a live node of `self`.
        let items = unsafe { self.array_items(node) }?;
        if !(2..=3).contains(&items.len()) {
            return None;
        }
        Some(items[1].node)
    }
```

`crates/lichen-lowlevel/src/static_module/apply.rs` — the frozen route's bind:

```rust
            // A static callee's type is static too, which the dynamic guard refuses.
            if let Some(cell) = cell
                && let Some(codomain) = module.expr_type_item(applied)
            {
                // A decided codomain stays the frozen ref the clone walk carried; a
                // leaf in this block gives the bind its value.
                let codomain = module.as_dynamic(codomain, block);
                module.evaluate_node(Dyn(codomain), Some(block));
                module.unify(cell, codomain);
            }
```

`crates/lichen-lowlevel/src/function.rs` — `apply_type`, the dynamic route's bind, the
same two lines. **§7's route choice did not have to be made:** the fix is in
`resolve.rs` and at the two bind sites, and
`crates/lichen-lowlevel/src/evaluation.rs` — the file another session owns — **is not
touched**. `as_dynamic` is the crate's own instrument for this ("a static ref
materialized into a leaf"), the same one `static_function_signature` documents and
`check_app`'s guard already uses, so no second reading of an expr's encoding is added.

### 10.1 Per instance, before and after

| example | declared `output =` | before | after | mechanism |
|---|---|---|---|---|
| `assert.lichen` | `(1, 5): tuple{Int, Int}` | `(raw 1, 5): tuple{?a, Int}` | **`(1, 5): tuple{Int, Int}`** — matches | the comparison's apply bound on the **frozen** route, `static_function_apply`; §9.5 measured its `expr_type_half` as `None` |
| `assert_in_function.lichen` | `1: Int` | `raw 1: ?a` | **`1: Int`** — matches | the same condition on the **dynamic** route: `f`'s return expr is `[Dynamic(112v1), Static(ModuleKey(0), index 11)]`, so `apply_type`'s `expr_type_half` was `None` for cell `53v1` |
| `import/_.lichen` | `(42, 10, 7): tuple{Int, Int, Int}` | `(raw 42, 10, raw 7): tuple{?a, Int, ?b}` | **`(42, 10, 7): tuple{Int, Int, Int}`** — matches | **family C was this condition too**, on both routes: `math.succ`'s and `geo.inc_twice`'s codomains were both carried frozen |
| `lazy_infinite.lichen` | `(1, 2, 3): tuple{Int, Int, Int}` | `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}` | `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}` — **unchanged** | §10.2: a different mechanism, in `arithmetic_class` |

**§4's "B vs C are not established to be the same" is settled, and against the note's
guess.** C was not a family of its own: it was the same missing join, reached through
two frozen modules and two hops, and §6.3's open question is answered in §6.3 above.
**B is the only family left, and it is genuinely different.**

### 10.2 Family B — attributed, and it is not the apply's join

> **Partly superseded by [arithmetic-class-stated](arithmetic-class-stated.md) §4.**
> The control table below and the `71v1`/`66v1` reading both reproduce; the sentence
> "what is undecided is the **array's element type slot**" does **not**, and neither
> does the conclusion that B is a checker-side question. That note measured the class
> of every cell in this family and then ran the decision this section left open. It
> is kept here as the diagnosis that led there, with §10.2b carrying what the
> measurement changed.

The join **runs** on `lazy_infinite.lichen`: measured, `inf = f 0`'s result cell `71v1`
sits in a class whose representative `66v1` is decided. What is undecided is the
**array's element type slot** — §6.2's own read — and the discriminator is
`Checker::arithmetic_class`'s `stated` test
(`crates/lichen-highlevel/src/checker/operators.rs:106`):

```rust
let stated = self.class_is_stated(left_ty) || self.class_is_stated(right_ty);
```

A **stated** operand takes the branch that unifies both operands with `int_type` and
returns `self.int_type`, decided at **check** time, before any run. An **unstated** one
— a function parameter — takes the `else` branch, returns `left_ty`, **the shared open
class cell**, and gates the class on a refinement assert that only resolves at run time.
Measured, one program each:

| program | prints |
|---|---|
| `x = 0` ; `xs = [x, @shallow [x + 1, x + 2]]` ; `xs(1)` | `[1, 2]: array{Int, 2}` — stated operand, decided at check time |
| `f = x => [x, @shallow [x + 1, x + 2]]` ; `g = f 0` ; `g(1)` | `[raw 1, raw 2]: array{?a, 2}` — unstated operand, open class cell |
| `f = x => [x, @shallow [x, x]]` ; `g = f 0` ; `g(1)` | `[0, 0]: array{Int, 2}` — a bare read of `x` decides, so `@shallow` is **not** the discriminator |
| `f = (x : Int) => [x, @shallow [x + 1, x + 2]]` ; `g = f 0` ; `g(1)` | `[raw 1, raw 2]: array{?a, 2}` — an annotation does not reach it |
| `f = x => [x, [x + 1, x + 2]]` ; `g = f 0` ; `g(1)` | `error: expected Int, found array{?a, 2}` — **not** a drift; dropping `@shallow` does not produce the declared output either |

So B's undecided cell holds an **open class the run never closes**, not a declared type
a join failed to carry. **§10's fix does not apply to B and was not expected to.** B is
a checker-side question — whether `arithmetic_class`'s `else` branch should decide when
neither operand states a class — and it is the user's call, as §7 framed A's. **Not
fixed here.** §6.2's "not traced: the write that should have carried a type into that
element slot" is now **located**: it is the class commit `arithmetic_class` defers to
its refinement assert, not a write a reader skipped.

### 10.2b What the decision measurement changed

[arithmetic-class-stated](arithmetic-class-stated.md) ran the question this section
left open. Three of its findings reach here.

**The array's element type slot is decided; the apply's own cell is not.** The class
of `lazy_infinite.lichen`'s element cell `32v1` is `22v1`, **fifteen members, all
fifteen decided**. The two `f`-apply result cells `43v1` and `51v1` are **undecided
singletons**, and so are the three cells that print `?a`/`?b`/`?c` — `102v1`,
`142v1`, `195v1`, which is the set §2 already named. §9.6's probe read the *array
element* class (`269v1`, `307v1`, `376v1` and their partners are all members of `22v1`)
as a decided *apply* cell; the error is §9.6's own lesson, one section further on.

For the table above the shape is the reverse — there the element cell `29v1` and the
two apply cells `44v1`/`62v1` are **one** undecided class of three, which is why this
section's sentence fits the controls and not the example.

**`arithmetic_class`'s open branch does not run in the user's program.** `+ - * / <
<= > >=` route onto the frozen prelude, so the branch fires while `core.lichen` is
being checked and **zero** times in the user's own check. `max`/`min` are the one
user-code path to it. `arithmetic_class` was therefore the right place to have looked
and the wrong place to have left the question.

**So B is not a checker-side question.** Deciding the `else` branch — unifying
`left_ty` with `int_type` and returning `int_type` — was measured: it leaves
`lazy_infinite.lichen` byte for byte where it was, and costs twelve `examples` (ten
panicking, two drifting) plus the `Float` add, the `Float` maximum and the
polymorphic `x + y`. The remaining carry was re-homed to the index read's own, from
a container element cell that is already decided:
`Checker::check_index`'s `elem_cell` (`crates/lichen-highlevel/src/checker/indexing.rs:70-83`),
[compound-element-slots](compound-element-slots.md) §"The read path". That is a
`lichen-lowlevel` question and a separate slice. **§7's framing — a checker-side decide
— is retired for B.**

> **Superseded by [index-read-unify](index-read-unify.md).** The re-homing above
> names the wrong cell: `check_index`'s `lazy_index_path` `elem_cell` is built
> **zero** times across the examples suite, and the `?a` cells of
> `lazy_infinite.lichen` are minted by `checker/structs.rs`'s `slot_read` lazy arm.
> No unify is skipped, and the decided cell the carry was supposed to come from is
> not on this path. That note measures the chain and says what would have to change
> instead; §7's framing is retired for B **for the second and final time**, and B is
> not a checker-side question by either route.

§10.2's history is kept above; nothing in it was deleted, because the control table it
recorded is what the measurement was checked against, and it reproduces.

### 10.3 What the fix does not change

`examples/import/math.lichen` and `examples/import/geometry.lichen` print byte for byte
what they printed before, so no other example's `output =` moved:

```
math.lichen      (Int => Int, raw[?a, ?b] => …): struct{.succ Int -> Int, .add raw[?a, ?b] => …}
geometry.lichen  (raw[?a, ?b] => …, Function): struct{.double raw[?a, ?b] -> …, …}
```

Those two already **declare** `?a` in their `output =`. That records a polymorphic
lambda's exported signature (`math.add = x => y => x + y` has no stated operand) — the
same open class B is about, at the level of a module's type — not this defect, so it is
left as it stands rather than re-declared.

### 10.4 Suite attribution

`cargo test -p lichen-compute --test lichen_compute` on this branch and on a worktree
checked out at `dev` give the **same 15 failures by name, 203 passed / 15 failed / 2
ignored** — so the change introduces none and clears none outside `examples`, whose own
failure goes from four drifts to one (`lazy_infinite.lichen`).

## Coordination

Two commits landed on `dev` while this note was being written, both in the same
neighbourhood as the missing join: `86ce6b6f` (*highlevel: an engine-side unify is located
through the descent it recorded*) and `973d7132` (*highlevel: a conflict keeps the values
its error already carries*). **Neither was this join** — both are in the checker, and
§9.1 measures that `check_app` never held the join at all: the cell it mints is bound by
the **runtime**, in `lichen-lowlevel`.

So the choice §7 left open — a *checker-side decide at `check_app`* or a *runtime bind* —
did not have to be made, because **both halves of it were wrong**: the checker never had
the call to delete, and the runtime had it but skipped it on one arm. The fix is neither
route, and it touches neither `crates/lichen-highlevel/src/checker/` nor
`crates/lichen-lowlevel/src/evaluation.rs`. **Family B (§10.2) is the remaining open
item**, and it *is* a checker-side decision the user should make.

`crates/lichen-lowlevel/src/evaluation.rs` was not edited and
`crates/lichen-lowlevel/src/gc.rs` was not touched.
