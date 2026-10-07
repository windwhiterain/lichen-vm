# Operator polymorphism: a refinement contract over a lichen dispatch

> Status: **Phases 0–3 landed** on `dev` (the refinement contract, the set value,
> `@in`, the **class refinement** `x : (_ ! in_num)`, the contract as the
> built-in [`core`](core-prelude.md) prelude, the static-signature instantiation a
> frozen module needs, and the **routing** — `+`, `-`, `*`, `/` and the four order
> comparisons lower onto the prelude's bindings).  The routing's three costs —
> which kernel-body shapes are refused by name, an open class's honest
> `raw[?a, ?b]` rendering, and a routed statement's missing value snapshot — are
> measured, closed and pinned in §7.1: **no test on `dev` is red for any of
> them**.
>
> The architecture was settled in discussion before any code: **an operator's
> contract and its implementation are separate artifacts, and the contract may be
> stricter than the implementation** (§2). The contract is a **refinement** (§3):
> one predicate function that must evaluate to `1`, carried in an ordinary
> **attribute** and enforced by the ordinary **assert** channel — on the operand's
> *value* by `e : T ! p`, or on its *class* by `x : (_ ! in_num)`. The
> implementation is an ordinary lichen function that selects a per-class leaf and
> applies it (§4). The panic arm is dead by construction (§5).
>
> **Rejected on the way here: the set as a *type*.** A kind marker usable in
> *type* position, whose
> members are type values, with a narrowing rule in `unify_inner`, cannot work,
> and each half of why is measured:
>
> - A set in *type* position has to commit a member at every use, and the unify
>   that does it is the **lowlevel's** (`apply_parameter_check`), so the rule
>   must be a `Program` hook consulted from `unify_inner` — which, being a
>   `Merge`, folds a per-occurrence domain node into the *canonical* class of
>   the member. `class_committed_value` then scans past the set, the deferral
>   path reconciles against it, and the domain node's own slot can be written
>   over. Every kernel test that reads a domain goes wrong, in ways that are
>   order-dependent and so differ between a fresh and an incremental build.
> - More fundamentally, a set that *is* a type makes **unification** the
>   enforcement mechanism. The contract is a fact about **values** — "this
>   operand's value is one of the numeric classes" — and a fact about a value is
>   checked by *evaluating* it, not by reconciling types. That is a refinement.
>
> A refinement needs no narrowing and no new type: the operand's cell stays an
> ordinary unbound `?a`, narrowed by the plain let-polymorphic cloning that
> already makes `x => x` polymorphic.
>
> Points at: `crates/lichen-highlevel/src/checker/operators.rs` (`check_binop`,
> the pin this removes), `crates/lichen-highlevel/src/attr.rs` (`AttrExt`, the
> carrier §3 takes `Perspective`'s shape from), `crates/lichen-highlevel/src/
> checker/asserts.rs` (`register_assert`, the enforcement `assert.rs` already
> provides), `crates/lichen-lowlevel/src/assert.rs` (`check_asserts`, the
> force-and-require-`USize(1)` discipline), `crates/lichen-highlevel/src/
> checker/indexing.rs` (`check_index`, whose bounds constraint is already a
> refinement in all but name), `crates/lichen-language/src/compile.rs` (the
> homogeneous branch-array desugar §4 replaces), `crates/lichen-compute` (the
> defaulting point), and `lichen-std/_.lichen` (the operators' end-state home).
>
> Companions: [operators](operators.md) (the operator set this edits),
> [floating-point](floating-point.md) §4.2 (the no-conversion rule this must
> not weaken), [type-of-in-std](type-of-in-std.md) (the precedent for moving a
> language form into the library),
> [defer-pending-type-forms](defer-pending-type-forms.md) (the deferred
> unification the dispatch relies on).

## 1. What is missing

`check_binop` decides the class of an arithmetic operation once, at check
time: a concretely `Float` operand selects `Float`, and **every other operand —
an `Int`, an undecided parameter — pins the whole operation to `Int`**. The
pin is what kills polymorphism:

```lichen
add = x => y => x + y   -- today: Int -> Int -> Int
add 1.5 2.5             -- today: expected Int, found Float
```

The language's let-polymorphism is structural — every application clones the
parameter cells fresh — so `x => x` is polymorphic precisely because nothing
ever binds its type cell. The operator pin binds it early. The goal: `+ - * /`
are polymorphic over the two scalar classes in a type-system-standard way,
**every instantiated use is class-concrete**, and a wrong-class use is a check
error, not a runtime accident.

## 2. The separation principle

The design's load-bearing idea, stated plainly because everything else follows
from it:

> **An operator's contract and its implementation are separate artifacts, and
> the contract may be stricter than the implementation.**

- The **contract** is a refinement: `+ : {a | a ∈ {Int, Float}} → …`. It is what
  rejects `add "a" "b"`, and it leaves the operand's *type* open, which is what
  makes the operator polymorphic at all.
- The **implementation** is a dispatch: an ordinary lichen function that asks
  the value's class which machine leaf to run. It is written for *every* class,
  including the ones the contract excludes — its last arm is a panic the
  contract proves unreachable.

This is the standard shape of a bounded-polymorphic builtin: Haskell's
`Num a => a -> a -> a` is a contract over a dictionary the class system
supplies; here the contract is a refinement whose predicate consults a **domain
value** (§3) and the dictionary is the operand's own type value read by the
dispatch (§4). The separation is what lets each half be simple: the contract
never sees the dispatch, the dispatch never restates the contract, and the
"impossible" arm needs no clever type — it needs a proof it is never taken,
which the contract *is*.

Two measured facts bound what the implementation can look like:

- **The runtime dispatch already exists.** `TypeOperator::run` picks integer
  or IEEE arithmetic from the operand *values*. A selection over
  `type_of x` is the same dispatch written in the language, one level up —
  which is the point: the semantics of `+` becomes library code, the same
  trajectory `type_of` took ([type-of-in-std](type-of-in-std.md)).
- **A homogeneous container cannot hold the dispatch.** `if` desugars to a
  branch array whose element type is one cell (`compile.rs`: "the branch
  array is homogeneous"), and a table literal unifies every entry's value
  into one cell (`check_table_term`). In `if t == Int then $iadd x y else
  $fadd x y` the two branches' results are an `Int` and a `Float`, and they
  meet in that cell. §4 removes the cell rather than loosening it.

## 3. The contract: a refinement on the operand's value

The constraint is not a type. It is a **refinement** — classical
`{v | p v}`, with the base type left as the ordinary inference cell and the
predicate `p` required to evaluate to `1`.

The attribute's slot holds **exactly one function**, an ordinary lichen value.
The surface spelling is the annotation chain's `!` piece (§3's last paragraph),
and the raw lowering of an annotated expression is the pair's third element:

```text
x : Int ! (v => v > 3)   ≡   [x, Int, v => v > 3]
                             └────┬────────┘ └──┬──┘
                          the pair's value,     the refinement slot
                          type, and tail
```

One function is enough because a conjunction is one function written with `*`
(`0 * 1 = 0`, so `*` is conjunction over this language's `0`/`1` scalars):
`x : Int ! (v => v > 3) ! (v => v < 10)` is `v => (v > 3) * (v < 10)`.

It is carried as an **attribute** (`AttrExt`), which is the one mechanism here
that already carries a non-equality constraint on an expression across every
journey the graph takes.  `Perspective` and `Doc` are the two proofs it works end
to end, and the refinement sits between them:

| | [`Perspective`](../../crates/lichen-perspective/src/perspective.rs) | [`Doc`](../../crates/lichen-doc/src/doc.rs) | refinement |
|---|---|---|---|
| slot holds | a lattice value (a thread count) | the doc's own pair | **one predicate function** |
| `combine` | `Gcd` — the meet, propagated from the children | a fresh unbound cell | **a fresh unbound cell: no propagation** |
| `missing_value` | `0`, the `gcd` identity | `None` (an unbound cell) | **`None` (an unbound cell)** |
| `share_missing_slot` | `true` (its absent value is concrete) | `false` | **`false`** |
| `unify_slots` | an equality unify of the two **slot values** (element 0) | relaxed, `is_subtype` always true (a later doc overrides) | **a plain unify of the two predicates — over-strict, by decision** |
| `is_label` | `false` | `true` | **`false`** |

The `missing_value` row is not a free choice, and it corrects an earlier draft of
this section (which said `Error`): an absent refinement has to be an **unbound
cell**, because the reconciliation is a *plain unify*.
[`AttrExt::share_missing_slot`](../../crates/lichen-highlevel/src/attr.rs)'s own contract
states why — a unify *writes* whichever side is unbound, so a concrete absent
value can be shared and an unbound one must not be — and a concrete absent value
is wrong on its own terms anyway, because `unify(Error, predicate)` conflicts and
no refinement could ever pass from one side to the other.  Being an unbound cell
is exactly what lets an annotation's predicate flow into an argument's slot,
which is the propagation the language already has for types ("`a : b; a : c`
makes `b` and `c` unify"): an unrefined expression costs a fresh cell and
nothing else.

**Why nothing propagates, and this is the load-bearing negative.**  The tempting
reading is Perspective's: derive an expression's refinement from its children's,
so that `x + y` carries `p x * q y` and a subtree accumulates one condition at
its root.  It does not hold here.  The frontend **folds** computations — an
`a == b` on known operands is folded away, a `let` is substituted, arithmetic on
constants is evaluated — so by the time a parent is built, the children that
produced its value may no longer exist as expressions at all.  A refinement that
survived that would have to be *inferred* rather than propagated, and inferring
a predicate for a computed value is a solver's job (the value is `3`; which
predicates does it satisfy?).  So propagation is dropped, and with it every
solver question: **a refinement is only ever where it was written**, and an
expression with no written refinement has none.

**Enforcement is one lowlevel `assert` per refined expression**, inserted by the
highlevel as it lowers that expression.  The condition is the refinement applied
to the expression's own value — `f value`, an ordinary lichen apply — and the
insertion is [`register_assert`](../../crates/lichen-highlevel/src/checker/asserts.rs) as it stands, so the
whole of the enforcement is already written and already has the right semantics:

- `check_asserts` force-evaluates the condition *ignoring laziness* and requires
  `USize(1)`; a condition that stays lazy is **pending**, not failed, so a
  refinement on an open parameter waits for a value.
- The entry is registered on the enclosing function, so the **apply clone
  re-checks the instantiated condition per call** — the refinement follows the
  argument without the checker.
- `AssertError`'s `{condition, template, value}` carries the provenance a
  diagnostic needs, and the condition node's runtime edge carries the span.
- It freezes with the function, so a persisted callee's refinement survives
  without the checker.

This is why the lowlevel needs **no new mechanism**, and why the earlier
"refinement record plus a `Program` hook" is not in this plan: the association
the lowlevel was missing ("which value does this constraint refine?") *is* the
condition node, because the highlevel has already applied the predicate to the
value and can attribute the span.  `check_index`'s bounds constraint
(`i < length`, registered on the parameter and re-checked per call) is already a
refinement in all but name; this generalises that path instead of opening a
parallel one.

**Reconciliation is a plain unify.**  Requiring the *same* refinement rather than
a weaker one is over-strict, and deliberately so: weakening needs implication
between predicates (`fact ⊨ requirement`), which is the subtyping this language
does not have.  A unify never wrongly *accepts*, so the strictness costs
expressiveness and buys soundness.

**Measured: the plain unify is of the two *predicates*, not of the two slots.**
`Refinement::unify_slots` mapped nothing and handed `check_unify` the two
`[value, type]` **slots**, while `Perspective::unify_slots` had always mapped
each through its element 0.  For an *absent* refinement that slot is
`[fresh cell, int]` (`AttrExt::missing_slot`'s default, and `int` is the shared
`Int` type expression), and the fresh cell is unified with the predicate's own
value node — that is how a refinement passes from an annotation to an argument's
absent slot.  The slot therefore *reads* as `[<the predicate function>, Int]`,
and unifying it against the other side's slot — the predicate's own type, the
self-referential `[Function(fid), ↺]` — compares two different shapes: the
positional descent walked into the cycle and refused a refinement that should
have passed.

| program | before | after |
|---|---|---|
| `5 : (_ ! (t => 1))` | refused — `expected ?a -> Int, found raw[Function, Int]` | checks and prints `5: Int` |
| `x : (_ ! in_num)` on a parameter | refused at every application | checks; `Int` and `Float` pass, `string` is refused |
| `membership::a_refinement_written_on_a_type_refines_the_class` | parked | passes |

**The function encoding was not at fault.**  The `[Function(fid), t]` the
diagnostic printed is that absent slot's `[value, type]` term pair, not a
leftover arrow or a degenerate function type; nothing about `[Function(fid), ↺]`
needs to change.  The one shape rule that was missing is now shared:
`slot_value_node` in `lichen-highlevel`'s `attr` module returns a slot's element
0 (or the slot itself when it is already bare), and both attributes call it — so
"compare the attribute's *value*, not its slot" is stated once, where the
attribute's semantics live.

**The surface spelling is `!`** — the annotation chain's fourth piece, beside
`# p` and `? d`: `x : Int ! p`.  `!` was the prefix assert, and the assert moved
to the keyword `@assert` (`@` being the language's keyword sigil, as in `@loop`)
in the same change, so the two are never confusable: one is a word at the `@`
sigil, the other marks an annotation's attribute.  `!` was chosen over the only
other free symbols (`'`, `` ` ``, `\`) and over a keyword because it is the
language's own "this must evaluate to `1`" mark — the refinement *is* an assert
about the value — and because it needs no new lexer token.  The right side is one
operand at the `->` level, like `#`/`?`, so the predicate is written explicitly:
`e : T ! (x => x > 3)`.

**Nothing unifies against a refinement.**  It is not in a type slot, no rule is
added to `unify_inner`, and the type cell stays open — that is the polymorphism.
The lowlevel apply does unify the parameter pair positionally, so the
parameter's live refinement slot must stay *unbound*, the same trick `check_lam`
already plays for a perspective: binding it would let the deep pass bake it and
make the apply enforce the declared refinement by equality.

The contract has **two halves**, and §2's separation principle is what keeps
them apart:

The contract that an *operator* carries has **two halves**, and §2's separation
principle is what keeps them apart:

- The **check** is the refinement: one predicate function in the attribute slot,
  enforced by the inserted assert. This is §3's subject.
- The **domain** is a *value*: a **set** of type values, `set{Int, Float}`.  A
  set is the language's own value form, not a type-level or domain-specific
  construct: its *value* is its members (an ordinary array node, no tag) and its
  *type* is `set<T>` — a kinded type whose shape is the element type **alone**,
  so a set has no length and is not an `array<T, n>`.  That is what makes "not a
  type" a property of the representation rather than a convention, and it is why
  a class domain needs no encoding of its own: `set{Int, Float}` is a plain
  binding a library can write, and the domain is simply that value.  It is what
  a reader that needs the **candidates** consumes — the kernel, which must answer
  an open domain with a class, and the diagnostics.  A reader that has to commit
  to one class takes the set's **first member** (`Int` here, the arithmetic
  operators' historical default); that is a convention of the reading, not a
  stored element.

  The *set kind* rather than the array kind is load-bearing, and the reason is
  soundness: a set-typed value can never flow into an `array<T, n>` parameter,
  so a set can never be indexed as an array inside a callee — where a
  value-level tag would be invisible and `s[0]` would read the tag.  A set's
  index read is refused at the *caller*, by the container unify
  ([`check_index`](../../crates/lichen-highlevel/src/checker/indexing.rs)), which
  is the same rule that refuses `t{i}` on an array.

The domain is *data*, and the predicate is the *function that consults it*: the
membership test reads the set. Nothing unifies against either, `+ : ?a -> ?a ->
?a` stays exactly that, and no rule is added to `unify_inner`.

The membership test itself is **representation-agnostic**, and that is a fix the
`@in` operator forced rather than a convenience.  A *source* set's members are
`TypeValue` nodes — the runtime's own value for a type constant — while the
domain the checker registers for the builtin operators is the array-encoded type
expression, and [`ValueExt::value_eq`] compares array *handles*: neither
representation matches the other, so a structural-only reader refused every
source-written domain.  [`set::contains`](../../crates/lichen-highlevel/src/set.rs)
therefore matches a member **by the class it denotes when it denotes one, and by
the language's own value equality otherwise** — the array encoding goes through
[`low_type_of`], a `TypeValue` compares by value (the variant is nominal, not
allocated), and an ordinary value (`set{1, 2}`) compares by value too.  Measured:
`type_of 5 @in Num` is `1`, `type_of "a" @in Num` is `0`, `2 @in set{1, 2}` is
`1` and `3 @in set{1, 2}` is `0`.

```lichen
-- lichen-std, end state
Num  = set{Int, Float}                  -- the domain VALUE: a set of type values
in_num = v => type_of v @in Num           -- one predicate, consulting the domain
is_int = v => type_of v == Int
iadd = …                                -- the machine leaves, today's TypeOperator
fadd = …                                --   split per class (see §6)
add  = x => y => { x : ?a{in_num}; y : ?a{in_num}      -- the contract
                   (fadd, iadd)(is_int (type_of x)) x y }
```

`Num` is writable, and so is the predicate — and the predicate goes **on the
type**, which is what makes it a class contract.  The contract takes **one
argument — the operand group, a 2-wide array** — because an array's elements are
one type (the operand *tie*), its length is the operator's *arity*, and its
element type is where the refinement goes:

```lichen
Num     = set{Int, Float}
in_num  = t => t @in Num             -- t is a *type value*
add     = operands => { operands : array<(_ ! in_num), 2>; operands[0] + operands[1] }
```

`(_ ! in_num)` inside the array type is the annotation chain with the refinement
written **on the type**: the checker applies the predicate to the type expression's
value — the class — instead of to the operand's value, so `in_num` receives a type
value and needs no type read of its own (§9 Phase 3 settled this; the
`type_of v @in Num` spelling of the first draft needs the read, and a read inside a
predicate is monomorphic, [type-of-in-std](type-of-in-std.md)).  Measured, with
exactly that `add`:

| program | result |
|---|---|
| `(add [1, 2], add [1.5, 2.5])` | `(3, 4.0)` — one argument, polymorphic across calls |
| `add [1, 1.5]` | refused, `expected Int, found Float` — the **tie**, from the array's element type |
| `add [1, 2, 3]` | refused, `expected array<…, 2>, found array<Int, 3>` — the **arity**, from the length |
| `add ["a", "b"]` | refused inside `core.lichen` — the **class** |
| `add` | `array<raw[?a, ?b], 2> -> raw[?a, ?b]` — the result's class is the operands' |

Three mechanism points fell out of the measurement, and all three are load-bearing:

- **The annotation names a type expression's *denotation*.**  An attribute-carrying
  type expression's term is the `[type, …, attribute]` group the attributes live
  in, and the type it names is the *annotated expression's own term* (a
  placeholder's cell pair, a type constant's `[marker, kind]`).  Binding the
  parameter's slot to the group instead is a type error (measured:
  `expected raw[?a, ?b, raw[Function, ?c -> Int]], found Int`), and binding it to
  the group's first slot alone loses the per-call re-check (measured: `f "a"` was
  then *accepted* — the class cell must stay reachable from the parameter pair for
  the apply clone to re-instantiate the condition).  The rule holds in *every* type
  position, which only became visible once a refinement sat inside a compound type:
  the tuple-type element / struct field / function-type side
  (`Checker::check_type_element`) and the array type's element
  (`Checker::check_array_type`) both took the expression's term as-is and leaked the
  group — `f = x : <(_ ! in_num), (_ ! in_num)> => x; f (1, 2)` was
  `expected <raw[?a, ?b, …], …>, found <Int, Int>`.
- **The refinement is enforced where it was written,** so a class refinement on a
  parameter rides the enclosing function and is re-checked per application, exactly
  as the value form is.
- **A hole bound at the top level is one cell for the whole program; a hole bound
  inside the function body is per call.**  `T = _` outside a lambda is shared, so
  `x : (T ! in_num); y : (T ! in_num)` ties every call together and the second
  `add 1 2` / `add 1.5 2.5` pair fails with `expected Int, found Float` — while the
  same tie written as an array element (or a body-local binding) re-instantiates per
  call.  That is why the contract is one parameter and not a shared top-level hole.

`@in` **landed** in Phase 3 as a keyword operator at the comparison level —
infix and left-associative, joining `@loop` and `@assert` at the `@` sigil
([operators](operators.md) §3; the language spec's *Membership*).  What is left
of Phase 3 is the leaf selection and the routing (§8.7, §9).

The refinement is an **attribute** on the type, so it is *shown* wherever the
annotated value is printed (`! in_num` beside the type — the spelling is §8.1) and
the parameter's own type cell stays the ordinary inference cell — that is the
polymorphism.

## 4. The dispatch: a hand-written dependent read

**Measured: the dependent read needs no new checking rule**, and **`if` must not
become it.**  The positional slot read `a(k)` already types as

```text
ty = Index(Index(type_of a, 0), k)          -- `slot_read`, checker/structs.rs
```

so indexing a **tuple** with a dynamic key *is* `if c then type_of b else
type_of a`:

```lichen
(10, "ten")(1 == 1)      -- measured: `"ten": string` — the taken branch's type
```

The first draft of this section proposed a rule for the same type slot while
`if` kept desugaring to an array; the rule is unnecessary.  Its *second* draft
then proposed desugaring `if` to that tuple read, and **that was tried and
rejected**: the branch unification an `if` performs is something the type system
**depends on**.  `check_array_term` unifies both branches into one element cell,
and a class question asked of the conditional reads that cell; a dynamic
condition makes the tuple read's type `Index(Index(branches, 0), c)`, which is
*never decided*.  Measured with the tuple desugar in place, inside a kernel:

```lichen
k = compute.jit (x => if x <= 3 then 10 else 20)
compute.launch k 2          -- rendered `10: ?a`; `if` renders `10: Int`
```

Two tests caught it (`jit_conditional_then`/`jit_conditional_else`), and the
kernel is where it matters most: a kernel body with an undecided type cannot be
lowered at all — the same specialize-before-JIT boundary as §8.4, reached from
the other side.  So:

- **`if` stays the unifying conditional** — `[e, t][c]`, one element cell, the
  branches' common type.  That is a *feature* of the type system, not a
  limitation of the desugar.
- **The dependent read is opt-in**: whoever needs it writes the tuple index
  `(e, t)(c)` themselves, which the language already supports and which needs no
  new rule.  It is the *conditional that does not unify*, and its type is the
  taken branch's.

What follows from that, and what the first draft got right:

- **An unselected arm's constraints do not fire — because the arm is an apply.**
  `check_app` performs **no** argument/parameter unify: it builds the `Apply`
  node and leaves the unify to the lowlevel `apply_parameter_check`, which runs
  per call site on the parameter clone.  A function body is a template and is
  never evaluated at definition, so an apply sitting in an unselected arm is
  never forced:

  ```lichen
  (fadd x y, iadd x y)(cond)      -- a hand-written dependent read
  (fadd, iadd)(cond) x y          -- select the leaf, then apply
  ```

  That is why no *branch-pending* deferral cause is needed (and with it, no
  deferral budget): the arm is an apply, and an unforced apply is already the
  existing behaviour.
- **The arms' types may still differ** — as a *tuple's* elements do, which is
  what makes the opt-in form expressible.  `iadd : Int -> Int -> Int` and
  `fadd : Float -> Float -> Float` can sit in one tuple; they cannot sit in one
  array element cell.

And the exhaustiveness arm: `panic` (the language's recorded-refusal channel,
the same one `operator.divide_by_zero` uses) has a free type cell that unifies
with anything, so a chain always has a last arm. Under the §3 contract that arm
is dead — see §5.

What the dependent read does **not** do: it does not constrain `?a` to
numerics.  `(1, "1")(c)` on a `string` operand still *checks* — the contract's
job is §3's, and this is the separation principle again: the read makes the
dispatch expressible, the refinement makes it safe.

## 5. The worked example

```lichen
add = x => y => { x : ?a{in_num}; y : ?a{in_num}
                  (fadd, iadd)(is_int (type_of x)) x y }
```

Measured on `dev` before any of this, and again with §6's operand tie plus the
§3 refinement condition landed, taken with `lichen-compiler`:

| program | today | landed |
|---|---|---|
| `add 1 2` | `3: Int` | `3: Int` |
| `add 1.5 2.5` | **fails**: `expected Int, found Float` at the `1.5` | **`4.0: Float`** |
| `add 1 1.5` | fails: `expected Int, found Float` at the `1.5` | **identical**, same text and position |
| `add "a" "b"` | fails: `expected Int, found string` | **refused**: `assertion failed: expected 1, found 0` at the `x + y` |
| `(add, add 1.5 2.5)` | — | `<?a -> ?a -> ?a, Float>` |

Three mechanism facts fell out of that measurement, and all three are
load-bearing:

- **The apply clone preserves equality classes.**  Tying the two operands into
  one class *in the template* therefore makes the arguments of a single
  application share one class, which is why `add 1 1.5` is still refused with
  today's exact message: the first argument commits the shared class and the
  second meets it.  So §8.3 is not a problem — "the operands are one class" is
  free, not a condition that has to be built and asserted.
- **An unknown class is not an error, it is an undecided value.**
  `TypeOperator::run` answers `Parameterized` for a pair it cannot compute, so
  before the refinement `add "a" "b"` *ran* and yielded an unresolved value.
  That is what the condition closes, and it is why the condition is registered
  as an assert *on the operand*: the assert is the only channel that turns
  "undecided" into "refused".
- **A condition undecided at definition time stays pending, and resolves per
  application.**  `add`'s condition names the parameter's *cell*, which is open
  while the definition is checked; the apply clone re-checks the instantiated
  condition against the argument, which is what makes one polymorphic definition
  refuse `"a"` and accept `1.5`.  No new machinery: it is
  `register_assert`'s documented behaviour, the same one `check_index`'s bounds
  constraint already relies on.
- **The pin was also hiding a printer bug.**  A type printer names an unbound
  cell by its **equality class** — `TypePrinter::class_name` keys its name table
  by the class representative — but `static_class_name` (a *frozen* module's
  cell) keyed by the ref alone, with no representative walk, and the lowlevel had
  no static representative query to walk with.  Under the pin every member of a
  class holds a committed value, so both cells printed `Int` and the difference
  was invisible; with the class open, an imported `?a -> ?a` printed `?a -> ?b`
  (`geo.double`'s hover) while the same type rendered dynamically printed
  `?a -> ?a` (the example's own `output =`).  Fixed by
  `Module::static_equality_representative` — the freeze keeps the class of a node
  whose own value is unbound *whole*, so following `parent` over the artifact's
  local ids is well defined — and the printer now mirrors `class_name` exactly.

- **Definition.** `x : ?a{in_num}` and `y : ?a{in_num}` put the predicate in the
  parameters' attribute slots; the *type* cells stay open, so the signature is
  `?a -> ?a` until a leaf's contract closes it (a leaf typed `?a -> ?a -> ?a`
  leaves it as it is). Each parameter's refinement is applied to that parameter's
  value and registered as an assert, which stays pending while the cell is open.
- **`add 1 2`.** Apply clones; `1` binds the clone's type to `Int`;
  `is_int (type_of x)` is `1`, so the `Index` takes `iadd`; `iadd 1 2` is
  `2: Int`, and the refinement `in_num` holds.
- **`add 1.5 2.5`.** The clone binds `Float`, the `Index` takes `fadd`, and the
  result is `3.0: Float`. **This is the feature**: today the pin refuses it.
- **`add "a" "b"`.** The refinement's condition resolves to `0` — a refusal
  naming the domain (`does not satisfy {Int, Float}`), which is §2's "the type is
  stricter than the implementation" made concrete. The dispatch never matters.
- **`add 1 1.5`.** The one case the refinement does **not** reproduce on its own,
  and it is worth stating because it is the price of dropping the set's type
  role. Today's refusal comes from the *pin*: `x + y` puts both cells in the
  canonical `Int` class, whose committed value every clone replicates — so the
  `y` clone starts at `Int` and `1.5` conflicts at the argument. A refinement
  binds nothing, and each application instantiates the parameter cell
  independently — that *is* let-polymorphism — so both clones satisfy `in_num`.
  Under the end state the refusal is the **selected leaf's own contract**:
  `iadd`'s parameter is `Int`, so `iadd 1 1.5` is refused at `iadd`. That is an
  apply-time (runtime-channel) refusal rather than a check error at the argument,
  and §8.4 records the choice.
- **`compute.jit (y => y + y)`.** The parameter's type cell is open, so the
  domain is undecided — but the refinement names the **set**, whose candidates
  are data. The kernel reads them and takes the default class, `Int`. Two things
  make that sound rather than merely test-preserving, and both are load-bearing:

  - The compiled class must be **committed into the signature**, not just used
    for lowering: `.sig` is `type_of f`, and `launch` gates the argument against
    the signature's domain lazily. A signature left open would admit a `Float`
    argument to an `Int` kernel — the default would be a guess the launch could
    not defend. Committed, the launch refuses it, which is §5's `add 1 1.5`
    refusal arriving through the other door.
  - Therefore `compute.jit` must write the defaulted class into the parameter
    cell (or build `.sig` from the classes it actually compiled), which is the
    "kernel default" this phase owes — and it is why the existing expectations
    (`.sig Int -> Int`) are the *correct* answer here rather than a spelling
    that happens to match.

  With the class concrete the `Index` selects one leaf and it lowers to
  `KernelBin::Add` — byte-identical kernels, no new optimizer.

## 6. The leaves

The dispatch selects a per-class machine **leaf** and applies it. Today's
`TypeOperator::Add` is *already* class-polymorphic at run time (it reads the
operand values), so two shapes work:

- **Keep the unified leaf**: `iadd` and `fadd` are both the existing operator;
  the selection is then the *specification* of the dispatch (and the place a
  future class plugs in), while `run` keeps doing what it does. Zero lowlevel
  change.
- **Split the leaf per class**: each leaf names its machine op, the `run` arm for
  a wrong-class leaf records a refusal, and the dispatch is load-bearing all the
  way down. Cleaner failure isolation, one more `TypeOperator` variant per
  operator per class (codec tags append-only, as ever).

Either satisfies the design; **the split is the honest end state** because it
gives each leaf a *concrete* parameter type (`Int -> Int -> Int`,
`Float -> Float -> Float`). That is what replaces the pin as the thing that
refuses a cross-class use — §5's `add 1 1.5` — and it is what makes the selected
leaf's contract enforceable by the ordinary apply, with no new rule.

The **`Int`-only** operators (`%`, the bitwise trio) are not polymorphic at all:
their domain is a single class, so they keep today's pin
(`unify(operand, int_type)`). A one-member domain is a pin, not a refinement —
no uniform mechanism is bought by pretending otherwise, and pinning is what makes
their *result* a machine scalar rather than an open cell. `==`/`!=` stay
unconstrained (the generalized equality).

## 7. Routing a surface operator to the function

How `a + b` reaches the std function, unchanged from the earlier analysis and
still phased: **R1** a prelude desugar (principled end state; needs the
language's first implicit import), **R2** an intrinsic registry (the checker
resolves `ir::BinOp` to the registered function value and checks an ordinary
apply; the emitter recognises "apply of an intrinsic whose dispatch selects one
leaf" so kernels see `KernelBin` as today), **R3** the checker's special case
stays but *reads the contract from the std binding* instead of restating
`{Int, Float}` in Rust (waypoint). The refinement and the domain value are
routing-agnostic: phases 0–2 below land the semantics under R3, and R2/R1 are the
"operators are library functions" end state whenever the prelude question is
answered.

### 7.1 Measured: the two things R1 needed, withdrawn twice, landed as the call form

R1 has been implemented twice — the resolver hands out the prelude's binders and
the lowering turns a routed `BinOp` into an apply of that binding with the operand
group `[l, r]` — and **withdrawn twice**, each time for a measured reason that is
not the routing's syntax.  The third attempt is the one that landed: the first
withdrawal's cause is fixed (`6e9c409`), and the three costs below are each
refused by name or asserted as the property they were about.

**First withdrawal: it regressed polymorphism.**  `f = x => x + x; (f 1, f 1.5)`
was `(2, 3.0)` and became `expected Int, found Float` at the second call — and the
same failure appeared with no operator at all (`f = x => add [x, x]`), while the
*same* apply at the top level (`(add [1, 2], add [1.5, 2.5])` = `(3, 4.0)`) was
fine.  It was neither the frozen concreteness flag nor the caller's apply node
being baked: a **residual clone made by a static apply carried no owner tag**, so
it was a member of no template; a caller's per-call clone walk re-instantiates only
template *members*, and a non-member is referenced in place — so the caller's open
parameter **type** was one module-global cell and the first call bound it for good.

That root is **fixed and landed** (`6e9c409`), by mirroring the owner tag the
dynamic path already stamps (`crates/lichen-lowlevel/src/static_module/apply.rs`;
`baked` clones stay untagged, keeping the concrete-leaf fast path).  Measured on a
hand-written static function that returns a structure with no operator involved
(`pair = x => [x, x]`, `f = x => p.pair x`): `([1, 1], [1.5, 1.5])` where it had
failed identically.  That fix stands on its own — it is what makes an apply of *any*
static function instantiate its signature per call.

**Landed: the routing is the call form**, and it carried three costs.  The resolver
hands out the prelude's binders and the lowering turns a routed `BinOp` into an
apply of that binding with the operand group `[l, r]` — so the surface operator
*is* the binding (its contract and its body), and the checker's builtin operator
remains only for the built-in module's own source (which has no prelude, so the
expansion terminates).  Measured: `1 + 2` is `3`, `1 + 1.5` is refused by the
*contract's* tie at the operator, `array<Int, add [1, 2]>` decides to
`array<Int, 3>` (so a routed operator still folds where a *value* is forced), and
`f = x => x + x; (f 1, f 1.5)` is `(2, 3.0)` — the polymorphism the first
withdrawal had cost.

The three costs, and what closed each.  **No test on `dev` is red for any of
them**: each is either refused by name and pinned, or asserted as the property it
was always about.

1. **A kernel body cannot call the binding.**  `y + y` lowers to
   `Apply(Static(<the prelude's add>), [y, y])`, which has no machine node behind
   it, so the shapes that need one *in an argument position* are refused by name
   and stay refused: an operator inside a cross-kernel call's argument
   (`k0 (x + 1)`) and one inside `compute.launch`'s argument.  Both refusals are
   pinned (`jit_an_operator_inside_a_cross_kernel_argument_is_refused`,
   `jit_an_operator_inside_a_launch_argument_is_refused`,
   `crates/lichen-language/tests/compute.rs`).  Everything else a kernel body
   needs works and is pinned green: the operator as the body's own result, one
   applied to a call's *result* (`k0 x + 1`), a helper's inlined body, and
   cross-kernel calls whose argument is read directly (`k0 x`, `k0 (x, 1)`,
   `k0 q`).  `examples/compute_jit.lichen` launches through the wrapper with the
   parameter read directly.  Widening this is the kernel workstream's
   specialize-before-JIT pass, and this apply is the shape that pass specializes:
   its inlined form (`operands = [y, y]; operands[0] + operands[1]`) was measured
   kernel-clean (`6: Int` inside `compute.jit`).
2. **An open class renders `raw[?a, ?b]`** — the operator's operand-group element
   is the placeholder's `[class, kind]` pair, so every arithmetic lambda's
   signature prints the mark the *refined* form printed all along
   (`x : (_ ! in_num)`).  That is the printer's **honest** reading of two cells the
   type chain never explained ([raw-rendering-mark](raw-rendering-mark.md) §2), not
   a new defect.  The expectations that pinned `?a -> ?a` are now **semantic**
   assertions, per [tests-do-not-render](tests-do-not-render.md): the
   imported-field hover asserts that the two sides are the *same* open cell and
   that both are named rather than how the pair is spelled
   (`crates/lichen-language-server/src/analysis.rs`), and the wrapper hover
   asserts that `.sig`'s domain and codomain are the wrapper's own two cells
   rather than the letters the checker numbered them
   ([checker-encoding-instability](checker-encoding-instability.md)).  The two
   `examples/import/*.lichen` `output =` declarations were re-pinned to the new
   reading in the same change, because an example's declared output is the
   language's observable behaviour and it is asserted as such; converting *those*
   to a semantic form is still the open half the rule records.  The one *real*
   printer defect this surfaced is fixed: a universe read out of a frozen module
   (a replica whose tail names the canonical self-loop in the module that wrote
   it) fell back to `raw[Int, Type]` where `<Int, Int>` was meant (`8805020`).
   The open class behind the mark is [type-of-in-std](type-of-in-std.md)'s defect.
3. **A routed statement's concrete value is not in the snapshot** —
   `Doc::statement_values` reports `None` for `y = x + 4` because the builtin
   operator folded it *eagerly* at check time while a call stays lazy (the
   snapshot deliberately never forces a value: a recursive binding must not
   diverge).  The value is still computed at run time; only the static snapshot is
   lost, and the tests now assert the type-only report rather than the old value
   (`crates/lichen-language-server/tests/statement_values.rs`).  Fixing it without
   the eager fold is what the *body-expansion* form would do — expand the binding's
   own body at the call site — which is recorded here as the open alternative, not
   as the landed shape.

So the chain's three steps: **static-signature instantiation** (landed,
`6e9c409`), **the routing** (landed, the call form), **kernel specialize** (the
kernel workstream's, for cost 1).

## 8. Open questions

1. *(Closed — **landed**.)* **The printer's spelling** of a refined cell.  The
   surface sigil is `!` (§3), so the readable form is `x : Int ! in_num`.  There
   is deliberately **no rule special to the refinement** here: the slot holds a
   *function value*, which is not printable on its own (the graph keeps a lambda
   as an opaque function, and a binding's name is resolved away), so the
   refinement is spelled through the general answer to "how is a value printed" —
   an attribute **naming** the value.

   Two general pieces, both landed:

   - [`AttrExt::label`](../../crates/lichen-highlevel/src/attr.rs) (default
     `None`) is "the **name** this attribute gives the value it attaches to", and
     `Doc` implements it for a **string** doc.  The name is bare; a labelled value
     *reads* as `?name` (the doc sigil), which is `render::value_label`'s
     spelling — so `f = (x => x) ? "fibo"; f` still prints `?fibo: ?a -> ?a`.  A
     *struct* doc describes instead, through `render`.
   - The obstacle to using that name *inside* an attribute's slot was the hook's
     **own signature**, and the fix is one parameter:
     `AttrExt::render(module, slot, attrs)` now receives the composed extension
     registry.  [`attr::pair_label`](../../crates/lichen-highlevel/src/attr.rs)
     is the shared reader: a pair's **arity** is in the graph but *which*
     attribute each of its tail slots belongs to is not (a one-entry tail is
     `[Doc]` or `[Perspective]`, and both are three elements long), so it asks
     **every** attribute of the composed set whether it names that slot, in
     canonical order, first answer wins — the same rule `render_attributes` uses,
     and sound because an attribute answers only about content it recognises as
     its own.

   `Refinement::render` is then three lines: `! ` plus the predicate's name.
   Measured — `in_num = (v => v > 0) ? "in_num"; 5 : Int ! in_num` prints
   `5 ! in_num: Int`; an unnamed predicate prints nothing (honest: a function
   value has no source form, and a made-up one could not be spelled back); a
   refinement on a *function* value prints `Function ! always: ?a -> ?a`.

   **What this does *not* reach, and why.**  The contract's `?a ! in_num -> ?a` is
   a *type* string, and a refinement is not in the type: it is the attribute of the
   *parameter expression* inside the lambda, so `f = x : _ ! in_num => x` prints
   `Int -> Int` with no refinement.  Showing it there needs the *lambda's* own
   rendering (its parameter's attribute is reachable in the template, not in the
   function type), which is a separate piece of work and no part of Phase 3.  The
   refinement **is** shown beside the type wherever the annotated *value* is what
   is printed, which is the operator's own end state (`add`'s body is refined, its
   *signature* is `?a -> ?a`).
2. **`Num`'s home**: std binding (the `type_of` precedent) vs keyword.
3. **The panic arm's spelling**: the recorded-refusal channel needs a
   value-level form a library function can write; today only builtins record.
   A `$panic` leaf with a free type cell is the minimal answer.
4. **Which channel refuses `add 1 1.5`.** §5 shows the refinement does not, by
   itself. The end state's answer — the selected leaf's concrete parameter type,
   refused by the ordinary apply — is an *apply-time* refusal
   (`DiagKind::Runtime`), where today it is a check error at the argument. That
   is the same channel `f = x => x + 1; f Type` already uses, so it is the
   language's existing answer to a per-call mismatch, but it is a visible change
   of diagnostic kind and position.
5. *(Closed — **landed**.)* **The refinement's diagnostic flavour.**  A
   refinement's failure no longer reads as `assert failed`.  The assert channel
   keeps its one shape, because an explicit `@assert e` means exactly that, so
   the *registration* now says how a failure reads:
   [`AssertSpelling`](../../crates/lichen-highlevel/src/diagnostic.rs) is
   `Condition` (an explicit assert, or a generated guard) or
   `Refinement { domain }` — plus `StructKind { container }` for the named
   read's container requirement, which rides the same channel
   (`docs/notes/eval-before-unify.md` §5.2); it is keyed by the **template**
   condition, which is
   what a per-call failure records.  `Diag` carries the spelling as
   `assert_spelling`, and
   [`crates/lichen-language/src/render.rs`](../../crates/lichen-language/src/render.rs)
   spells it in place of the generic wording — the domain is a class-set value,
   so the type printer's own `{Int, Float}` arm renders it.  Measured:
   `add "a" "b"` reports `does not satisfy {Int, Float}` at the operator.

   A refinement a **user** wrote still reads generically
   (`assertion failed: expected 1, found 0`), and that is not a gap: its
   predicate consults whatever it likes — `in_num` reads a set, `v => v > 3`
   reads a literal — so there is no domain to name.  Naming one is something
   only the *registrar* can do, which is why the spelling travels with the
   registration rather than being inferred at render time.
6. **One constraint slot per expression, at apply time — a pre-existing limit,
   now reachable.**  `Checker::check_ann` records a single `state[e].attr` (the
   last constraint attribute in canonical order) and `check_app`/`check_lam`
   re-check exactly one marker, so an expression spelled with *both* a
   perspective and a refinement reconciles both slots at the annotation but only
   the later one is re-validated against a provider.  It does not bite this
   feature — the refinement's *enforcement* is an assert and a parameter
   refinement rides the desugar, neither of which goes through that slot — but
   the operator's end state (`x : ?a ! in_num`) plus a perspective on one
   parameter would.  Generalising the slot to a per-marker set is the fix, and it
   is not this phase's.
7. *(Closed — **landed**.)* **A class domain's surface form.**  `Num` in §3's end
   state is a *value* a library writes, and the language now spells one:
   **`set{a, b, …}`** — a set of ordinary values, led by a word exactly like
   `table{…}`, because angle brackets are the spelling of an expression in *type*
   position and a set is not one.  `Num = set{Int, Float}` is a plain binding, so
   the contract can leave Rust (Phase 3, §9).

   The three options that were weighed, and why this one:

   - `domain<Int, Float>` — the first recommendation.  Rejected on the operator's
     own words: `<>` is the type-position spelling, and a set is a *value*.  It
     also had no answer for the encoding's `default` element.
   - `$domain(Int, Float)` — no new syntax, but `$` is the *plugin-private* sigil
     ("a normal file never lexes it as a valid call"), so it works only for a
     source the host embeds — the opposite of a std a *user* could write.
   - Reusing `{…}` — a bare glued `{` is a table lookup.  A *keyword-led*
     `set{…}` has no such ambiguity (`table{…}` is the precedent).

   Two consequences of "a set is a value", both landed:

   - **No tag, and a set is not an array.**  The set's *value* is its members and
     its *type* is `set<T>` (shape = the element type alone).  The separate kind
     is a soundness requirement, not a preference: with an array type, a set
     reaching an `array<T, n>` parameter and indexed inside the callee would read
     the members as an array — and a value-level tag could not be guarded there.
     `set{}`'s index read is refused by the ordinary container unify.
   - **The `default` element is gone.**  It was never read (`default_class` had no
     caller), and a general set has no privileged member.  A reader that must
     commit to a class takes the set's **first member** — which for
     `set{Int, Float}` is `Int`, the operators' historical default, visible in the
     source.
8. *(Closed — the two targets are green, and what remains is a recorded
   dependency.)* **The kernel boundary is not this feature's to fix.**  Making `+`
   polymorphic left a kernel body's class open, and a kernel lowered from a
   *template* has no class to compile — which is what made
   `a_kernel_value_and_type_render_by_name` render `.sig ?c -> ?c` with a `none`
   artifact and `runtime_only_package`'s `launch` gate unable to resolve the
   domain.  Both targets are **green**: the class a kernel's operands carry now
   reaches the read that consumes it (`c44aeb0`, `6be23a8`), and §7.1 cost 1
   records which operator shapes a kernel body may use and which are refused by
   name.  What is still owed is the widen-the-boundary work, and it is the
   specialize-before-JIT pass's
   ([kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6: "a kernel
   is never compiled from a template — at `jit`/`parallel` time the function is
   applied to a placeholder typed by the annotated domain, so every term's type
   cell is decided in the graph itself").

   The two halves meet at one interface: **the domain this note puts in the graph
   as a value is what types that placeholder.**  So this phase owes the domain
   and its readability, and owes *nothing* at the `jit` call site — writing a
   defaulted class into the parameter cell there would be both the other
   workstream's job and unsound (`f = y => y + y; k = jit f; f 1.5` must keep
   working: the cell is shared, the kernel is not).

   The `examples` target — this feature in public output — is green too: the
   declarations the routing changed are re-pinned in the same change, and §7.1
   cost 2 records why the reading is the printer's honest one.

(Closed since the first draft: the narrowing rule and its `Program` hook, and the
refinement *record* plus its `Program` hook — §3 rejects the set's type role, and
applying the predicate at lowering reduces enforcement to the assert channel
that already ships. The attribute's slot layout — one function, no propagation.
The branch-pending **deferral budget** — §4 removed the deferral cause rather
than budgeting for it.)

## 9. Phases

- **Phase 0 — the mechanism. Landed.** The class domain as a *value* — a set of
  type values, whose *value* is its members and whose *type* is `set<T>`
  (`crates/lichen-highlevel/src/set.rs`) — `TypeOperator::InDomain` as the
  structural membership test (`==` cannot do it: `value_eq` compares array
  *handles*, so a class node out of another module would compare unequal), and the
  condition registered by `check_binop` through `register_assert`.  Measured: no
  existing test regressed, and `add "a" "b"` went from *accepted* to refused.
  The domain's surface form (`set{Int, Float}`) landed in Phase 1.
- **Phase 1 — the contract on the builtin operators.** The operand tie, the
  domain condition, the refinement attribute, `!` on a parameter, the set value
  and its type, the static class-naming fix, and the diagnostic flavour are
  landed.  What remains is the printer's spelling (§8.1, which waits on the
  doc-overrides-a-value's-print mechanism) and the `Num`/std migration.  The
  kernel boundary (§8.4) is **not** in this phase: it is the
  specialize-before-JIT pass's, and the domain landed here is that pass's input.
  *The builtin operators are the user-visible feature.*
- **Phase 2 — the dependent read. Measured and *rejected as a desugar*.**  `if`
  keeps desugaring to `[e, t][c]`, because the branch unification it performs is
  what the type system depends on: with a dynamic condition the dependent form's
  type is never decided, and a kernel then cannot lower the body (§4 records the
  measurement and the two tests that caught it).  The dependent read is
  **opt-in** — `(e, t)(c)` — and needs no work: the positional slot read already
  types it as the taken branch's type.  Verified by hand: `(10, "ten")(1 == 1)`
  renders `"ten": string`.
- **Phase 3 — the implementation moves to std. Stages 1–2 landed.**  Landed: the
  membership keyword **`@in`** (the comparison rung, infix and left-associative,
  with the membership reader made representation-agnostic — §3 above); the
  **class refinement**, a refinement written inside a type position
  (`x : (_ ! in_num)`) whose predicate is applied to the type value, with the
  annotation naming the type expression's *denotation* (§3); and the **`core`
  prelude** — the contract is lichen source in a built-in module seeded into
  every program, so `add 1.5 2.5` is `4.0` in an otherwise empty file
  ([core-prelude](core-prelude.md)).  Measured through `lichen-compiler`:
  `(add 1 2, add 1.5 2.5)` is `(3, 4.0)`, `add "a" "b"` is refused, `Num` and
  `in_num` are in scope with no import, and a program's own `add` shadows the
  prelude's.

  What stays open: the **editor's jump** into the built-in file and the two
  diagnostics a cross-module failure still needs — the **domain spelling** (a
  refusal inside the prelude reads the assert channel's generic wording, not
  `does not satisfy {Int, Float}`, because the spelling and the domain node live
  in the built-in's build) and the **call site** (`AssertError` records the
  template it came from, not the apply that cloned it) —
  [core-prelude](core-prelude.md) §5; the **kernel side of the routing** (a routed
  operator is an apply of the `core` binding, so an operator *inside a kernel
  call's argument* is refused by name and waits on the kernel workstream's
  specialize-before-JIT pass to be folded back to a machine leaf — the surface
  routing itself is landed and the shapes a kernel body may use are listed in
  §7.1 cost 1); and the read's monomorphism
  ([type-of-in-std](type-of-in-std.md)), which is off the contract's path now that
  the class refinement needs no read and is visible as §7.1 cost 2's `raw[?a, ?b]`.
