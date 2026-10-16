# An undecided type cell: the four `examples` drifts

> **Status:** **diagnosis only; no production change.** The read is traced to the
> line, and the comparison family (2 of the 4) is traced to a join that does not
> happen. The frozen-module and lazy-chain families are **characterised by their
> discriminator and not traced** — §6 says what was ruled out and what was not.
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

## 4. The three families, and which pairs differ

| | Instance | Trigger, as measured |
|---|---|---|
| **A** | `assert.lichen`, `assert_in_function.lichen` | a **routed comparison** — `< <= > >=`. `+ - * /` on the same operands decides. |
| **B** | `lazy_infinite.lichen` | a value read through a **lazy (`@shallow`) chain**. The same read on a literal binding decides. |
| **C** | `import/_.lichen` | a value read through a **frozen module's export**. The same function defined locally decides. |

Which pairs differ, concretely:

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

| Instance | Should print | Does print | Verdict |
|---|---|---|---|
| `assert.lichen` | `(1, 5): tuple{Int, Int}` | `(raw 1, 5): tuple{?a, Int}` | **defect**, family A, traced (§6.1). [operators](operators.md) §2 specifies `Int`. |
| `assert_in_function.lichen` | `1: Int` | `raw 1: ?a` | **defect**, family A, traced (§6.1). Same cell as above, one apply away. |
| `import/_.lichen` | `(42, 10, 7): tuple{Int, Int, Int}` | `(raw 42, 10, raw 7): tuple{?a, Int, ?b}` | **defect**, family C, **not traced** (§6.3). |
| `lazy_infinite.lichen` | `(1, 2, 3): tuple{Int, Int, Int}` | `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}` | **defect**, family B, **not traced** (§6.2). |

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
apply binds it to the return pair"*.

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
pair, and something keyed on the comparison's own result node rather than on the
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
`dev`** — corrected there.

### 6.3 Family C — characterised, not traced

Discriminator, measured: the frozen-module boundary is necessary but not
sufficient. `math.succ 41` through an `import` prints `raw 42: ?a`, while
`succ = x => x + 1` ; `inc_twice = x => succ (succ x)` ; `inc_twice 5` prints
`7: Int` and `inc_twice 5` through the import prints `raw 7`. Inside the *same*
frozen module `geo.double 5` prints `10: Int` while `math.succ 41` does not, so the
boundary alone cannot be the whole cause.

Ruled out: the frozen status of the callee (a local `succ` decides), and `@loop`
(§3). **Not traced:** what distinguishes `succ` from `double` across the same
boundary. [class-channel](class-channel.md) §1.1.2 records the nearest prior
measurement — for `geo.double 5` the **value** survived and the **type cell did not
resolve** — and §1.1.3's refutation is the reason this note does not assume the
class value is merely misplaced: there, the structure was rewritten instead.

## 7. What a fix would touch

Named per family. **This slice changes none of them**; family A's is a decision
about the apply's bind, and families B and C are not located closely enough to name
a line.

- **A** — the runtime apply that is to bind an apply's result type cell to the
  callee's return pair, in `crates/lichen-lowlevel/src/evaluation.rs` (the apply
  arm: `wire_apply_result` / `function_apply`). **Another session owns
  `evaluation.rs` and garbage collection, so this is named and not touched.** The
  alternative is a checker-side decide at `check_app`, which would have to read the
  routed callee's return type — the frozen-module read that
  [class-channel](class-channel.md) §5.1.1 already had to teach one reader to do.
  **Either way it is the user's decision, not this slice's.**
- **B** — the index read's element-type carry, the path
  [compound-element-slots](compound-element-slots.md) §"The read path" names.
  The line is not located.
- **C** — the frozen-module carry of a return type across the module boundary;
  the file is not located. The nearest history is
  [class-channel](class-channel.md) §1.1.3.

## 8. Measured, inferred, unknown

- **Measured** — all four declared/CLI/suite strings; the CLI and the suite
  agreeing on all four; the singleton-class probe and the node ids; the whole of the
  §3 discriminator table, including the four `@loop`/`@assert`/`@shallow`/annotation
  refutations; that `check_binop` hands the comparisons a decided `int_type` inside
  `core.lichen`; that the apply's result cell joins that class for `add` and not for
  `less_or_equal`; that `alias_read` binds the `Int` value in both and names the
  result cell in neither.
- **Inferred** — that families B and C are both "an undecided cell an indirection
  failed to fill". Each has a discriminator and a minimal reproduction; neither has
  a located write.
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
