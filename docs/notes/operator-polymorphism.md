# Operator polymorphism: a refinement contract over a lichen dispatch

> Status: **current.** `+`, `-`, `*`, `/` and the four order comparisons are
> polymorphic over `Int` and `Float`: every instantiated use is class-concrete and
> a wrong-class use is a check error, not a runtime accident. The contract is a
> **refinement** — one predicate function that must evaluate to `1`, carried in an
> ordinary attribute and enforced by the ordinary assert channel. The
> implementation is an ordinary lichen function that selects a per-class leaf. The
> domain the predicate consults is a **set value**, `set{Int, Float}`. The surface
> operators **route** onto the prelude's bindings
> ([core-prelude](core-prelude.md)), so a program's `+` *is* the library binding.
> The routing's three costs are measured, closed and pinned in §7.1 — no test is
> red for any of them.
>
> Points at: `crates/lichen-highlevel/src/refinement.rs` (the attribute),
> `crates/lichen-highlevel/src/set.rs` (the set value and its membership read),
> `crates/lichen-highlevel/src/checker/operators.rs` (`check_binop`),
> `crates/lichen-highlevel/src/checker/asserts.rs` (`register_assert`),
> `crates/lichen-highlevel/src/program.rs`
> (`TypeOperator::InDomain` / `IsStructType`), `crates/lichen-lowlevel/src/assert.rs`
> (forcing a condition and requiring `USize(1)`),
> `crates/lichen-language/src/core.lichen` and `compile.rs` / `resolve.rs` (the
> prelude and the routing), `crates/lichen-render` (the `! name` spelling and the
> `{Int, Float}` domain spelling).
>
> Companions: [core-prelude](core-prelude.md), [operators](operators.md) (the
> operator set), [attributes](attributes.md) (the attribute machinery a refinement
> is carried by), [floating-point](floating-point.md) §4.2 (the no-conversion rule
> a contract must not weaken), [type-of-in-std](type-of-in-std.md) (the read's
> monomorphism), [kernel-class-crossing-fixes](kernel-class-crossing-fixes.md)
> (the kernel-side specialize pass the routing's one cost waits on).

## 1. What is missing

`check_binop` used to decide an arithmetic operation's class once, at check time:
a concretely `Float` operand selected `Float`, and every other operand — an
`Int`, an undecided parameter — pinned the whole operation to `Int`. The pin is
what killed polymorphism:

```lichen
add = x => y => x + y   -- used to be Int -> Int -> Int
add 1.5 2.5             -- used to fail: expected Int, found Float
```

The language's let-polymorphism is structural — an apply clones the parameter
cells fresh — so `x => x` is polymorphic precisely because nothing ever binds its
type cell. The operator pin bound it early. The goal: `+ - * /` are polymorphic
over the two scalar classes in a type-system-standard way, **every instantiated
use is class-concrete**, and a wrong-class use is a check error.

Two measured facts bound what the implementation can look like:

- **The runtime dispatch already exists.** `TypeOperator::run` picks integer or
  IEEE arithmetic from the operand *values*. A selection over the operand's type
  value is the same dispatch written in the language, one level up — which is the
  point: the semantics of `+` becomes library code, the same trajectory `type_of`
  took.
- **A homogeneous container cannot hold the dispatch.** `if` desugars to a branch
  array whose element type is one cell, and a table literal unifies every entry's
  value into one cell. In `if t == Int then iadd x y else fadd x y` the two
  branches' results are an `Int` and a `Float`, and they meet in that cell. §4
  removes the cell rather than loosening it.

## 2. The separation principle

The design's load-bearing idea, stated plainly because everything else follows
from it:

> **An operator's contract and its implementation are separate artifacts, and the
> contract may be stricter than the implementation.**

- The **contract** is a refinement: a predicate over the operand's class. It is
  what rejects `add "a" "b"`, and it leaves the operand's *type* open, which is
  what makes the operator polymorphic at all.
- The **implementation** is a dispatch: an ordinary lichen function that asks the
  value's class which machine leaf to run. It is written for *every* class,
  including the ones the contract excludes — its last arm is a panic the contract
  proves unreachable.

This is the standard shape of a bounded-polymorphic builtin: Haskell's
`Num a => a -> a -> a` is a contract over a dictionary the class system supplies;
here the contract is a refinement whose predicate consults a **domain value**
(§3) and the dictionary is the operand's own type value read by the dispatch
(§4). The separation is what lets each half be simple: the contract never sees the
dispatch, the dispatch never restates the contract, and the "impossible" arm needs
no clever type — it needs a proof it is never taken, which the contract *is*.

## 3. The contract: a refinement

A **refinement** is classical `{v | p v}`: the base type stays an ordinary
inference cell, and the predicate `p` must evaluate to `1`. It is carried as an
ordinary **attribute** (`AttrExt`) — the one mechanism that already carries a
non-equality constraint on an expression across every journey the graph takes —
and enforced through the ordinary **assert** channel, so the checker never names
a concrete attribute.

### 3.1 The attribute

The attribute's slot holds **exactly one function**, an ordinary lichen value:

```text
x : Int ! (v => v > 3)   ≡   [x, Int, v => v > 3]
                             └────┬────────┘ └──┬──┘
                          the pair's value,     the refinement slot
                          type, and tail
```

One function is enough because a conjunction is one function written with `*`
(`0 * 1 = 0`): `x : Int ! (v => v > 3) ! (v => v < 10)` is
`v => (v > 3) * (v < 10)`.

| | `Perspective` | `Doc` | refinement |
|---|---|---|---|
| slot holds | a lattice value | the doc's own pair | **one predicate function** |
| `combine` | `Gcd`, propagated from children | a fresh undecided cell | **a fresh undecided cell: no propagation** |
| `missing_value` | `0` | an undecided cell | **an undecided cell** |
| `share_missing_slot` | `true` | `false` | **`false`** |
| `unify_slots` | equality of the two slot values | relaxed | **a plain unify of the two predicates — over-strict, by decision** |

The `missing_value` row is not a free choice: an absent refinement has to be an
**undecided cell**, because the reconciliation is a *plain unify* and a unify
writes whichever side is undecided. A concrete absent value would conflict with
every predicate, so no refinement could ever pass from one side to the other.
Being an undecided cell is exactly what lets an annotation's predicate flow into
an argument's slot — the propagation the language already has for types.

**Nothing propagates, and this is the load-bearing negative.** The tempting
reading is Perspective's: derive an expression's refinement from its children's,
so `x + y` carries `p x * q y`. It does not hold here, because the frontend
**folds** computations — an `a == b` on known operands is folded away, a `let` is
substituted, arithmetic on constants is evaluated — so by the time a parent is
built the children that produced its value may no longer exist as expressions. A
refinement that survived that would have to be *inferred*, and inferring a
predicate for a computed value is a solver's job. So **a refinement is only ever
where it was written**, and an expression with no written refinement has none.

**Reconciliation is a plain unify of the two *predicates*, not of the two
slots.** An absent refinement's slot has the default `[fresh cell, int]` shape,
and unifying that slot against the other side's slot compares a term pair with the
predicate function's own self-referential type — two different shapes. The shared
shape rule is [`slot_value_node`](../../crates/lichen-highlevel/src/attr.rs): compare
the attribute's *value*, not its slot, stated once where the attribute's semantics
live. Requiring the *same* refinement rather than a weaker one is over-strict, and
deliberately so: weakening needs implication between predicates, which is the
subtyping this language does not have. A unify never wrongly *accepts*.

**Enforcement is one lowlevel `assert` per refined expression**, inserted by the
highlevel as it lowers that expression. The condition is the refinement applied to
the expression's own value — `f value`, an ordinary lichen apply — and the
insertion is [`register_assert`](../../crates/lichen-highlevel/src/checker/asserts.rs)
as it stands:

- `check_asserts` deep-evaluates the condition and requires `USize(1)`. A
  condition that stays undecided — an open parameter, a computation behind a lazy
  marker — is **pending**, not failed, so a refinement on an open parameter waits
  for a value.
- The entry is registered on the enclosing function, so the **apply clone
  re-checks the instantiated condition per call**: the refinement follows the
  argument without the checker. A parameter refinement is a general desugar,
  `x ! p => e` to `x => { x ! p; e }`, which is why it needs no IR field and gets
  per-application re-checking for free.
- `AssertError`'s `{condition, template, value}` carries the provenance a
  diagnostic needs, and the assert freezes with the function, so a persisted
  callee's refinement survives without the checker.

Two measured facts bound the mechanism. **An unknown class is not an error but an
undecided value**: `TypeOperator::run` answers `Parameterized` for a pair it
cannot compute, so before the refinement `add "a" "b"` *ran* and yielded an
unresolved value — the assert is the only channel that turns "undecided" into
"refused". And **the apply clone preserves equality classes**, so tying the
operands in the *template* is what makes one application's arguments share a
class: `add [1, 1.5]` is refused with the expected text, and no extra condition is
needed for "the operands are one class".

The lowlevel needs no new mechanism: the association the lowlevel would otherwise
be missing ("which value does this constraint refine?") *is* the condition node,
because the highlevel has already applied the predicate to the value and can
attribute the span. `check_index`'s bounds constraint is a refinement in all but
name; this generalises that path rather than opening a parallel one.

**The surface spelling is `!`** — the annotation chain's fourth piece, beside
`# p` and `? d`: `x : Int ! p`. The prefix assert is the keyword `@assert` (`@`
being the language's keyword sigil, as in `@loop`), so the two are never
confusable. `!` is the language's own "this must evaluate to `1`" mark — the
refinement *is* an assert about the value — and it needs no new lexer token. The
right side is one operand at the `->` level, like `#`/`?`, so the predicate is
written explicitly: `e : T ! (x => x > 3)`.

The refinement is an **attribute** on the type, so it is *shown* wherever the
annotated value is printed (`! in_num` beside the type — the spelling is §8.1) and
the parameter's own type cell stays the ordinary inference cell. That is the
polymorphism.

### 3.2 The domain: a set value

The predicate's domain is **data**: a **set** of type values, written
`set{Int, Float}`.

- A set's *value* is its members — an ordinary array node, **no tag**.
- A set's *type* is `set<T>`, a kinded type whose shape is the element type
  **alone** (`TypeSet`), so a set has no length and is not an `array<T, n>`.

The separate kind is a soundness requirement rather than a convenience: an
array-typed set passed into an `array<T, n>` parameter and indexed inside the
callee would read its element 0, and a value-level tag is invisible there.
`set{Int, Float}[0]` is refused by the ordinary container unify, at the caller. A
set's members are homogeneous, like an array literal: `set{1, "a"}` is refused.
`set{…}` is led by a word exactly like `table{…}`, because angle brackets are the
spelling of an expression in *type* position and a set is not one; `set` is
therefore a reserved word, the form's one breaking change.

There is **no privileged `default` member**. A reader that must commit to one
class takes the set's **first member** — for `set{Int, Float}` that is `Int`, the
arithmetic operators' historical default, visible in the source. That is a
convention of the reading, not a stored element.

Membership is
[`TypeOperator::InDomain`](../../crates/lichen-highlevel/src/program.rs) (codec tag
17), a **structural** test: `==` cannot do it, because `TypeOperator::Eq` goes
through `ValueExt::value_eq`, which for an array is *handle* identity, so a class
node out of another module would compare unequal.

The reader is **representation-agnostic**, and that is a fix `@in` forced rather
than a convenience. A *source* set's members are `LowValue::TypeValue` nodes —
the runtime's own value for a type constant — while the domain the checker
registers for the builtin operators is the array-encoded type expression, and
`ValueExt::value_eq` compares array handles: neither representation matches the
other, so a structural-only reader refused every source-written domain.
[`set::contains`](../../crates/lichen-highlevel/src/set.rs) therefore matches a
member **by the class it denotes when it denotes one** (the `low_type_of` decode,
which also accepts a bare marker, because a source type constant *is* its marker
leaf) **and by the language's own value equality otherwise**.

### 3.3 `@in`, the membership predicate

`value @in set` is the language's membership test: a keyword at the `@` sigil and
the language's only **infix keyword**. It sits at the comparison rung,
left-associative, so `x @in S == 1` is `(x @in S) == 1`, and it yields the same
`0`/`1` a comparison yields. Its test is `TypeOperator::InDomain`, so `@in` and
the builtin contract consult the domain through the identical operator.

- Its **right operand is pinned to a set** (`Guard`, the same container pin
  `e[i]` applies): with `Num = 7`, `Int @in Num` is refused with
  `expected set<?a>, found Int`.
- Its **left operand is deliberately unconstrained** — no unify against the set's
  element cell. A membership test is a fact about a *value*, and a check-time
  unify would write the argument's class and make the predicate mono-class:
  measured, `in_num 1` then `in_num 1.5` refused with `expected Int, found Float`
  while the tie existed. The asymmetry is the price: `5 @in Num` is accepted and
  answers `0` (the value `5` is not the type `Int`), because a mismatched left is
  not an error.

### 3.4 The class refinement

A refinement may be written on a **type**: `x : (_ ! in_num)` puts the predicate
inside the type position, and the checker applies it to the **type value** — the
class — instead of to the operand's value. That is what makes a class contract
expressible with no type read:

```lichen
in_num = t => t @in Num          -- t is a *type value*
add    = operands => { operands : array<(_ ! in_num), 2>; operands[0] + operands[1] }
```

The prelude's contract takes **one argument — the operand group, a 2-wide
array** — because an array's elements are one type (the operand *tie*), its
length is the operator's *arity*, and its element type is where the refinement
goes. Measured with exactly that `add`:

| program | result |
|---|---|
| `(add [1, 2], add [1.5, 2.5])` | `(3, 4.0)` — one argument, polymorphic across calls |
| `add [1, 1.5]` | refused, `expected Int, found Float` — the **tie**, from the array's element type |
| `add [1, 2, 3]` | refused, `expected array<…, 2>, found array<Int, 3>` — the **arity**, from the length |
| `add ["a", "b"]` | refused inside `core.lichen` — the **class** |
| `add` alone | `array<raw[?a, ?b], 2> -> raw[?a, ?b]` — the result's class is the operands' |

Three mechanism points are load-bearing:

- **An annotation names a type expression's *denotation*.** A type expression
  that carries attributes has a *pair* as its term, and binding the parameter's
  type slot to that pair — the obvious reading — is a type error. The type it
  names is the **annotated expression's own term** (`Checker::type_denotation`),
  and the attribute is enforced by its own assert instead. Taking the group's
  first slot alone is the opposite trap: the signature reads clean but the class
  cell is no longer reachable from the parameter pair, so the apply clone stops
  re-instantiating the condition and `f "a"` is *accepted*. A refinement written
  on a type must keep the placeholder's `[shape, kind]` pair. The rule holds in
  every type position — a tuple/struct field, a function-type side, and an array
  type's element.
- **The refinement is enforced where it was written**, so a class refinement on a
  parameter rides the enclosing function and is re-checked per application,
  exactly as the value form is: an open class is re-checked per call
  (`(f 5, f 1.5)` pass, `f "a"` is refused), and a concrete one is decided at the
  definition.
- **A hole bound at the top level is one cell for the whole program; a hole bound
  inside the function body is per call.** `T = _` outside a lambda is shared, so
  `x : (T ! in_num); y : (T ! in_num)` ties every call together and the second
  `add 1 2` / `add 1.5 2.5` pair fails with `expected Int, found Float` — while
  the same tie written as an array element (or a body-local binding)
  re-instantiates per call. That is why the contract is one parameter over an
  operand array and not a shared top-level hole.

An open class's annotated type is the placeholder's `[shape, kind]` pair of
cells, so its printed signature carries the printer's honest raw mark
(`raw[?a, ?b] -> …`, [raw-rendering-mark](raw-rendering-mark.md)); a concrete
class prints `Int -> …`.

## 4. The dispatch: a hand-written dependent read

**Measured: the dependent read needs no new checking rule**, and **`if` must not
become it.** The positional slot read `a(k)` already types as
`Index(Index(type_of a, 0), k)`, so indexing a **tuple** with a dynamic key *is*
`if c then type_of b else type_of a`:

```lichen
(10, "ten")(1 == 1)      -- measured: `"ten": string` — the taken branch's type
```

Desugaring `if` to that read was tried and **rejected**: the branch unification an
`if` performs is something the type system **depends on**. `check_array_term`
unifies both branches into one element cell, and a class question asked of the
conditional reads that cell; a dynamic condition makes the tuple read's type
`Index(Index(branches, 0), c)`, which is *never decided*. Measured with the tuple
desugar in place, inside a kernel:

```lichen
k = compute.jit (x => if x <= 3 then 10 else 20)
compute.launch k 2          -- rendered `10: ?a`; `if` renders `10: Int`
```

The kernel is where it matters most: a kernel body with an undecided type cannot
be lowered at all. So:

- **`if` stays the unifying conditional** — one element cell, the branches'
  common type. That is a *feature* of the type system, not a limitation of the
  desugar.
- **The dependent read is opt-in**: whoever needs it writes the tuple index
  `(e, t)(c)`, which the language already supports and which needs no new rule. It
  is the *conditional that does not unify*, and its type is the taken branch's.

What follows from that:

- **An unselected arm's constraints do not fire — because the arm is an apply.**
  `check_app` performs **no** argument/parameter unify: it builds the `Apply`
  node and leaves the unify to the lowlevel parameter check, which runs per call
  site on the parameter clone. A function body is a template and is never
  evaluated at definition, so an apply sitting in an unselected arm is never
  forced:

  ```lichen
  (fadd x y, iadd x y)(cond)      -- a hand-written dependent read
  (fadd, iadd)(cond) x y          -- select the leaf, then apply
  ```

  That is why no *branch-pending* deferral cause is needed: the arm is an apply,
  and an unforced apply is already the existing behaviour.
- **The arms' types may still differ** — as a *tuple's* elements do, which is
  what makes the opt-in form expressible. `iadd : Int -> Int -> Int` and
  `fadd : Float -> Float -> Float` can sit in one tuple; they cannot sit in one
  array element cell.

And the exhaustiveness arm: `panic` (the language's recorded-refusal channel) has
a free type cell that unifies with anything, so a chain always has a last arm.
Under the §3 contract that arm is dead — see §6.

What the dependent read does **not** do: it does not constrain `?a` to numerics.
`(1, "1")(c)` on a `string` operand still *checks* — the contract's job is §3's,
and this is the separation principle again: the read makes the dispatch
expressible, the refinement makes it safe.

## 5. The worked example

The contract's end state, as the prelude writes it:

```lichen
Num     = set{Int, Float}
in_num  = t => t @in Num
add     = operands => { operands : array<(_ ! in_num), 2>; operands[0] + operands[1] }
```

| program | with the contract |
|---|---|
| `add [1, 2]` | `3: Int` |
| `add [1.5, 2.5]` | `4.0: Float` — **this is the feature**; the pin refused it |
| `add [1, 1.5]` | refused, `expected Int, found Float` — the operand tie |
| `add ["a", "b"]` | refused, `does not satisfy {Int, Float}` — the class |
| `add` | `array<raw[?a, ?b], 2> -> raw[?a, ?b]` |

The refinement binds nothing and each application instantiates the parameter cell
independently — that *is* let-polymorphism — so `add [1, 1.5]` is refused by the
array's element tie rather than by a pinned class, and §8.4 records which channel
refuses a cross-class use under the split-leaf end state.

**`compute.jit (y => y + y)`.** The parameter's type cell is open, so the domain
is undecided — but the refinement names the **set**, whose candidates are data.
The kernel reads them and takes the default class, `Int`. Two things make that
sound rather than merely test-preserving:

- The compiled class must be **committed into the signature**, not just used for
  lowering: the signature is the wrapper's own `.I`/`.O` (the wrapper unifies
  `f: I -> O`, so `.I` is the function's domain), and `launch` gates the argument
  against `.I` lazily. A signature left open would admit a `Float` argument to an
  `Int` kernel — the default would be a guess the launch could not defend.
- Therefore the default must be written where the kernel's signature is built (or
  `.I`/`.O` built from the classes actually compiled). Writing it into the
  parameter cell at `jit` is *unsound*: that cell is shared while the kernel is
  not, and `f = y => y + y; k = compute.jit f; f 1.5` must keep working.

With the class concrete the dispatch selects one leaf and it lowers to the
machine binop — byte-identical kernels, no new optimizer.

## 6. The leaves

The dispatch selects a per-class machine **leaf** and applies it. Today's
`TypeOperator::Add` is *already* class-polymorphic at run time (it reads the
operand values), so two shapes work:

- **Keep the unified leaf**: `iadd` and `fadd` are both the existing operator; the
  selection is then the *specification* of the dispatch, while `run` keeps doing
  what it does. Zero lowlevel change.
- **Split the leaf per class**: each leaf names its machine op, the `run` arm for
  a wrong-class leaf records a refusal, and the dispatch is load-bearing all the
  way down.

Either satisfies the design; **the split is the honest end state** because it
gives each leaf a *concrete* parameter type (`Int -> Int -> Int`,
`Float -> Float -> Float`). That is what replaces the pin as the thing that
refuses a cross-class use, and it is what makes the selected leaf's contract
enforceable by the ordinary apply, with no new rule.

The **`Int`-only** operators (`%`, the bitwise trio) are not polymorphic at all:
their domain is a single class, so they keep the pin. A one-member domain is a
pin, not a refinement — no uniform mechanism is bought by pretending otherwise,
and pinning is what makes their *result* a machine scalar rather than an open
cell. `==`/`!=` stay unconstrained (the generalized equality).

## 7. Routing a surface operator

`+`, `-`, `*`, `/` and the four order comparisons **route** onto the `core`
prelude's bindings. The compiler records the prelude's binders by name, and a
surface operator lowers to `Apply(<the prelude's binding>, <the operand group>)`
— one `array<_, 2>`:

```lichen
1 + 2        -- lowers to add [1, 2], the binding's contract and body
```

The built-in module's own source has no prelude (the prelude is what it is), so
its body keeps the machine operators and the routing terminates; `==`/`!=` are
unconstrained and `%`/the bitwise trio are `Int`-only, so none of them is routed.
The refinement and the domain value are routing-agnostic: the semantics landed
first under the checker's special case, and the routing then made the operator
*the binding*.

The routing needed one thing first: **a static (frozen) apply must instantiate
its signature per call.** It did not — a residual clone made by a static apply
carried no owner tag, so it was a member of no template and a caller's per-call
clone walk referenced the module-global cell in place; the first call bound an
open class for good. The fix mirrors the owner tag the dynamic path stamps; the
clone-ownership rule is [apply-clone-ownership](apply-clone-ownership.md). That
fix stands on its own: it is what makes an apply of *any* static function
instantiate its signature per call.

### 7.1 Measured: the routing's three costs

The three costs, and what closed each.  **No test on `dev` is red for any of
them**: each is either refused by name and pinned, or asserted as the property it
was always about.

1. **A kernel body cannot call the binding.**  `y + y` lowers to
   `Apply(Static(<the prelude's add>), [y, y])`, which has no machine node behind
   it, so the shapes that need one *in an argument position* used to be refused by
   name.  **`k0 (x + 1)` is emitted now**: the identity of the operator is a
   *static* fact of the frozen callee, so `Lower::apply` reads it out of that
   callee's body (`Module::static_function_compute_operator`) when the residual is
   missing — pinned green by
   `jit_an_operator_inside_a_cross_kernel_argument_is_emitted`
   (`crates/lichen-language/tests/compute.rs`), and see
   [loop-conversion §8.5](loop-conversion.md) item 4 for the measurement and for
   what it did not fix.  One inside `compute.launch`'s argument is still refused,
   and this note does not claim a test for it — no such test exists on `dev`.
   Everything else a kernel body
   needs works and is pinned green: the operator as the body's own result, one
   applied to a call's *result* (`k0 x + 1`), a helper's inlined body, and
   cross-kernel calls whose argument is read directly (`k0 x`, `k0 (x, 1)`,
   `k0 q`).  `examples/compute_jit.lichen` launches through the wrapper with the
   parameter read directly.  Widening this is the kernel workstream's
   specialize-before-JIT pass, and this apply is the shape that pass specializes:
   its inlined form (`operands = [y, y]; operands[0] + operands[1]`) was measured
   kernel-clean.
2. **An open class renders `raw[?a, ?b]`** — the operator's operand-group element
   is the placeholder's `[class, kind]` pair, so every arithmetic lambda's
   signature prints the mark the *refined* form printed all along. That is the
   printer's **honest** reading of two cells the type chain never explained
   ([raw-rendering-mark](raw-rendering-mark.md) §2), not a new defect. The
   expectations that pinned `?a -> ?a` are now **semantic** assertions, per
   [tests-do-not-render](tests-do-not-render.md): the imported-field hover asserts
   that the two sides are the *same* open cell and that both are named, and the
   wrapper hover asserts that the wrapper's `.I`/`.O` are its own two cells. A
   frozen cell is named by its equality class rather than by its ref
   (`Module::static_equality_representative`), a printer fix the operator pin had
   been hiding — a pinned class renders its committed value, so both members of a
   class printed `Int` and the difference was invisible. The one *real* printer
   defect this surfaced is fixed: a universe read out of a frozen module fell back
   to `raw[Int, Type]` where `<Int, Int>` was meant. The open class behind the
   mark is [type-of-in-std](type-of-in-std.md)'s defect.
3. **A routed statement's concrete value is not in the snapshot** —
   `Doc::statement_values` reports `None` for `y = x + 4` because the builtin
   operator folded it *eagerly* at check time while a call stays lazy (the
   snapshot deliberately never forces a value: a recursive binding must not
   diverge). The value is still computed at run time; only the static snapshot is
   lost, and the tests assert the type-only report. Fixing it without the eager
   fold is what the *body-expansion* form would do — expand the binding's own body
   at the call site — recorded here as the open alternative, not the landed shape.

So the chain's three steps: **static-signature instantiation** (landed), **the
routing** (landed, the call form), **kernel specialize** (the kernel
workstream's, for cost 1).

## 8. Open questions

1. *(Closed — landed.)* **The printer's spelling** of a refined cell. The surface
   sigil is `!` (§3), so the readable form is `x : Int ! in_num`. There is
   deliberately **no rule special to the refinement**: the slot holds a *function
   value*, which is not printable on its own, so the refinement is spelled through
   the general answer to "how is a value printed" — an attribute **naming** the
   value. [`AttrExt::label`](../../crates/lichen-highlevel/src/attr.rs) is "the
   **name** this attribute gives the value it attaches to" (bare; the value reads
   as `?name` through `render::value_label`), and
   [`attr::pair_label`](../../crates/lichen-highlevel/src/attr.rs) is the shared
   reader: a pair's arity is in the graph but *which* attribute each tail slot
   belongs to is not, so it asks every attribute of the composed set whether it
   names that slot, in canonical order, first answer wins. `Refinement::render` is
   `! ` plus the predicate's name. Measured: an unlabelled predicate prints
   nothing (honest: a function value has no source form). **What this does not
   reach**: the contract's `?a ! in_num -> ?a` is a *type* string, and a
   refinement is not in the type — it is the attribute of the *parameter
   expression* inside the lambda, so `f = x : _ ! in_num => x` prints `Int -> Int`
   with no refinement. Showing it there needs the *lambda's* own rendering.
2. **`Num`'s home**: a std binding (the `type_of` precedent) or a keyword.
3. **The panic arm's spelling**: the recorded-refusal channel needs a value-level
   form a library function can write; today only builtins record. A `$panic` leaf
   with a free type cell is the minimal answer.
4. **Which channel refuses `add 1 1.5`.** §5 shows the refinement does not, by
   itself. The end state's answer — the selected leaf's concrete parameter type,
   refused by the ordinary apply — is an *apply-time* refusal
   (`DiagKind::Runtime`), where today it is a check error at the argument. That is
   the same channel `f = x => x + 1; f Type` already uses, so it is the language's
   existing answer to a per-call mismatch, but it is a visible change of diagnostic
   kind and position.
5. *(Closed — landed.)* **The refinement's diagnostic flavour.** A refinement's
   failure does not read as `assert failed`. The assert channel keeps its one
   shape, because an explicit `@assert e` means exactly that, so the
   *registration* says how a failure reads:
   [`AssertSpelling`](../../crates/lichen-highlevel/src/diagnostic.rs) is
   `Condition`, `Refinement { domain }` — plus `StructKind { container }` for the
   named read's container requirement, which rides the same channel
   ([eval-before-unify](eval-before-unify.md) §5.2) — and it is keyed by the
   **template** condition, which is what a per-call failure records. The renderer
   spells it in place of the generic wording, and the domain is a class-set value,
   so the type printer's own `{Int, Float}` arm renders it. A refinement a **user**
   wrote still reads generically: its predicate consults whatever it likes, so
   there is no domain to name, and naming one is something only the *registrar*
   can do.
6. **One constraint slot per expression, at apply time — a pre-existing limit,
   now reachable.** `Checker::check_ann` records a single `state[e].attr` (the
   last constraint attribute in canonical order) and `check_app`/`check_lam`
   re-check exactly one marker, so an expression spelled with *both* a perspective
   and a refinement reconciles both slots at the annotation but only the later one
   is re-validated against a provider. It does not bite this feature — the
   refinement's enforcement is an assert and a parameter refinement rides the
   desugar — but the operator's end state plus a perspective on one parameter
   would. Generalising the slot to a per-marker set is the fix.
7. *(Closed — landed.)* **A class domain's surface form**: **`set{a, b, …}`** — a
   set of ordinary values, led by a word exactly like `table{…}`, because angle
   brackets are the spelling of an expression in *type* position and a set is not
   one. `Num = set{Int, Float}` is a plain binding, so the contract can leave Rust.
   The three options weighed: `domain<Int, Float>` (rejected: `<>` is the
   type-position spelling, and a set is a *value*); `$domain(Int, Float)` (no new
   syntax, but `$` is the *plugin-private* sigil, so it works only for a source
   the host embeds); and reusing `{…}` (a bare glued `{` is a table lookup). Two
   consequences of "a set is a value" are in §3.2.
8. *(Closed — the two targets are green, and what remains is a recorded
   dependency.)* **The kernel boundary is not this feature's to fix.** Making `+`
   polymorphic left a kernel body's class open, and a kernel lowered from a
   *template* has no class to compile. Both targets are **green**: the class a
   kernel's operands carry now reaches the read that consumes it, and §7.1 cost 1
   records which operator shapes a kernel body may use and which are refused by
   name. What is still owed is the widen-the-boundary work, and it is the
   specialize-before-JIT pass's
   ([kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6: "a kernel is
   never compiled from a template — at `jit`/`parallel` time the function is
   applied to a placeholder typed by the annotated domain"). The two halves meet
   at one interface: **the domain this note puts in the graph as a value is what
   types that placeholder.** So this feature owes the domain and its readability,
   and owes *nothing* at the `jit` call site — writing a defaulted class into the
   parameter cell there would be both the other workstream's job and unsound.

### Closed since the first draft

The narrowing rule and its `Program` hook, and the refinement *record* plus its
`Program` hook — §3 rejects the set's type role, and applying the predicate at
lowering reduces enforcement to the assert channel that already ships. The
attribute's slot layout: one function, no propagation. The branch-pending
**deferral budget** — §4 removed the deferral cause rather than budgeting for it.

### Rejected, with the measurement

- **The set used as a *type*** — a kind marker in *type* position whose members
  are type values, narrowed in `unify_inner`. A set in type position has to commit
  a member at every use, and the unify that does it is the lowlevel's, so the rule
  would have to be a `Program` hook — which, being a merge, folds a
  per-occurrence domain node into the member's *canonical* class, after which the
  committed value is scanned past the set. Six test targets fail,
  order-dependently. More fundamentally, a set that *is* a type makes
  **unification** the enforcement mechanism, and the contract is a fact about
  values: a fact about a value is checked by *evaluating* it, not by reconciling
  types. That is a refinement, and it needs no narrowing and no new type.
- **Desugaring `if` to the dependent read** — §4 records the measurement.
- **Branch-pending deferral.** Not needed: an arm is an apply, `check_app` does
  no check-time argument unify, and a template's body is never evaluated at
  definition, so an unselected arm's constraints never fire.

## 9. The migration to the prelude

**Phase 1 — the contract on the builtin operators. Landed.** The operand tie, the
domain condition, the refinement attribute, `!` on a parameter, the set value and
its type, the static class-naming fix, and the diagnostic flavour. *The builtin
operators are the user-visible feature.*

**Phase 3 — the implementation moves to std. Stages 1–2 landed.** The membership
keyword **`@in`**; the **class refinement**, a refinement written inside a type
position whose predicate is applied to the type value; and the **`core` prelude**
— the contract is lichen source in a built-in module seeded into every program, so
`add [1, 1.5]` is refused by the prelude's own tie and `add [1.5, 2.5]` is `4.0`
in an otherwise empty file ([core-prelude](core-prelude.md)). The **routing** is
landed as the call form (§7), so the surface operator is the binding; the
checker's builtin remains only inside the built-in module's own source.

What stays open after the migration: the kernel side of the routing (§7.1 cost
1); the read's monomorphism ([type-of-in-std](type-of-in-std.md)), which is off
the contract's path now that the class refinement needs no read and is visible as
§7.1 cost 2's `raw[?a, ?b]`; the two cross-module diagnostics (a refusal inside
the prelude reads the generic wording, and the diagnostic names the built-in's
line rather than the application — [core-prelude](core-prelude.md) §5); and the
editor grammar, which still spells the assert as the prefix `!`, has no `set{…}`
and has no `@in`.

## Recovered measurements

- `%` and the bitwise trio are `Int`-only because the language has no float bit
  pattern (a float has no remainder either), so a float operand is a check error for
  those four operators and for them alone.  Their domain is a single class, so there
  is no polymorphism to keep open — unlike the arithmetic and comparison operators,
  which share the `{Int, Float}` domain refinement.
- When neither operand of a scalar operation has stated a class, the two type cells
  are unified into one open class and a refinement `left ∈ {Int, Float}` is
  registered as an assert on it.  That domain is a *set's value* — the members
  themselves — which is exactly what `Num = set{Int, Float}` lowers to, so the check
  reads the same graph a library-written contract will.
- A class conversion is the one operator kind typed as the other class: the operand
  is checked against the direction's **source** class (`Int` for `int2float`,
  `Float` for `float2int`) and the result's type is its **target**, so this is the
  only expression form whose type is not its operand's.  A wrong-class operand is
  the same refusal every other operator issues — the diagnostic names the class it
  expected, and nothing here converts silently.  Only a class the operand already
  *states* is unified, because a unify binds every cell the operand's class shares
  and that class may be one a kernel body's other cells read: pinning the index a
  float is written beside to `Int` would refuse the very program these two words
  exist to write.  An undecided operand therefore stays undecided, and the value
  that arrives at the other class answers the lazy marker in `TypeOperator::run`
  rather than a guess here — a weaker message than a parameter pinned at its apply,
  paid for by the conversion being usable where the classes are not yet decided.
- `float2int`'s partiality is not a check-time fact: in range is a fact about the
  *value*, not about its type, so the interpreter records `OUT_OF_RANGE` and answers
  the lazy marker while `check_convert` does not check it.
- Operator routing is syntactic, never a runtime kind dispatch: the frontend picks
  the operator from the spelling.
  - `a(k)` (adjacent single-expression paren) is the positional slot read; `a(1,)`,
    `a(1,1)` and the two zero-field spellings `a()` / `a(,)` are struct
    instantiation, mirroring the tuple grammar's `()` unit vs `(,)` empty tuple; a
    spaced paren is function application.  (The two zero-field spellings are the
    one part no note recorded.)
  - `a.name` is the named field read, `a[i]` the array read, `t{k}` the table lookup
    (the *adjacent* brace is the distinction), `X<e>` the raw positional component
    read and `X::a` the raw named one.
  - A struct *instance* reads a field **value** with `.a`; a positional component of
    a type-as-value reads with `X<e>`; a named one with `X::a`.
  - The refusals follow from the same principle: a struct type reaching `a(k)` is
    refused like any other non-tuple, and a set reaching `s[i]` is refused because
    the read pins its container to an array type.
- `attr::pair_label` exists because no schema tail is needed and none is readable: a
  pair's **arity** is in the graph, but *which* attribute each of its tail slots
  belongs to is not — a one-entry tail is `[Doc]` or `[Perspective]`, and both are
  three elements long.  So the search asks **every** attribute of the composed set
  whether it names that slot, in canonical order, first answer wins — the same rule
  and the same order `lichen_render::render_attributes` uses — and it is sound
  because an attribute answers only about content it recognises as its own (a string
  doc, `AttrExt::label`).  The answer is `None` when the pair is not an array,
  carries no attribute, or carries none that names it, and a *static* (frozen) slot
  is skipped: an attribute reads a dynamic node, and inventing a name for a frozen
  one is worse than silence.
- `Refinement::constraint` reads the slot's element 0 — the predicate expression's
  own value node — with no re-wrapping, so the node identity is preserved and the
  apply's subject is the very function the user wrote.  An apply node **is** its
  return `[value, type]` pair, so the condition's own value is that pair's element 0,
  the same read the checker's `value_of` builds.  Registering the pair itself would
  hand the assert channel an array, which is neither `1` nor *lazy*, and it would
  report a failure even while the applied value is still open.
- The refinement's absent form is an undecided cell by intent rather than a
  convenience: the fresh cell `combine` returns is the per-site no-refinement
  marker, per-site precisely because a unify may bind it.  A *static* predicate is a
  frozen node of another module, but the annotation's own slot is always built in
  this module, so that case cannot arise and staying silent is the conservative
  answer rather than inventing a node.
- `set::contains` is both the class-domain read and the source form's meaning
  (`value @in set`).  A set of ordinary values (`set{1, 2}`) is compared with
  `ValueExt::value_eq`, which for a machine scalar *is* the value itself, so
  `2 @in set{1, 2}` holds and `3 @in set{1, 2}` does not; a member the low type
  vocabulary cannot classify stays on this side too, which is what keeps a `string`
  type a non-member of a class domain (`add "a" "b"`).  `members` returns `None` when
  the value is not an array, so it is not a set's value at all — a set's membership
  is decided by its type, and the reader is for a caller that already knows it holds
  one (a class domain).  A member and the tested value are the same member when
  **both** denote a class, and value-equal otherwise; a node with no value at all
  (an undecided cell, or a node that is not a value) is never a member.
- Cost 1's kernel behaviour in detail: the checker peels a call result via
  `Index(apply, 0)`, which the JIT looks through to emit the kernel call directly,
  and a bare `k x` apply leaves a direct kernel apply's codomain `?a` — the checker
  only resolves it via `$launch` — while the wrapper form `compute.launch k0 x`
  *does* give `Int`.
- Cost 3 is pinned by `tests/statement_values.rs`:
  `statement_values_report_type_and_concrete_value` and
  `statement_at_finds_the_containing_statement`; the same read-only rule reports a
  lazy binding such as `paradox` as `None` rather than forcing it.
