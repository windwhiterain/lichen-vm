# Polymorphism and Classes

lichen's functions are polymorphic by default: a function's parameter can
be decided afresh at every call. This chapter is about that mechanism,
the way a **class** states which types a function accepts, and the
annotation — the **refinement** — that writes such a contract.

## Every application is fresh

Applying a function instantiates its parameter and
the return type afresh, so one binding can serve different types:

```lichen
id := x => x
(id 5, id Int)
```

```text
(5, Int): Tuple
```

The same binding, at two different types, with no annotation anywhere.
This is automatic let-polymorphism: there is no `forall` and no
generalize/instantiate form, because the answer is a fresh cell at each
call. Chapter 5's `type_of` is the same idea in a nicer costume — one
function, used at `Int`, at `Type`, and at `Array`.

A function is only as open as its definition left it. A body that
constrains its parameter decides it there, and then every call must meet
it:

```lichen
inc := x => x + 1
inc 2.5
```

```text
error: expected Int, found Float
  --> 2:5
   |
 2 | inc 2.5
   |     ^
```

The body's `x + 1` ties `x` to the class of `1`, which is `Int`, so `inc`
takes an `Int` by the time it is called. Openness is not a flag; it is
which cells the definition happened to decide.

## The operators' class

`+`, `-`, `*`, `/` and the comparisons accept more than one type, and they
are polymorphic in the same way. Both integers and floats work, but never
a mixture:

```lichen
(1 + 2, 1.5 + 2.5)
```

```text
(3, 4.0): Tuple
```

`1 + 1.5` is refused: the two operands of one operator are a tuple, and
that tuple must be homogeneous — one class — and the literal `1` decides
the class as `Int`:

```lichen
1 + 1.5
```

```text
error: expected Tuple, found Tuple
  --> 1:1
   |
 1 | 1 + 1.5
   | ^
```

What a class accepts is written with a **set of types**. The core prelude
(the bindings every program starts with) says it this way:

```lichen
Num := set(Int, Float)
in_num := t => t @in Num
```

`Num` is a set value whose members are the types `Int` and `Float`, and
`in_num` is a predicate that asks whether a type is one of them. This is
the class domain the arithmetic operators are declared over. Chapter 9
showed sets of types; here the set is used as a *contract*.

## Refinements

A **refinement** attaches a predicate to a value and requires the
predicate to answer `1`. Written on a value, it checks that value:

```lichen
small := v => v < 10
f := (x ! small) => x
f 5
```

The parameter `x` is refined by `small`, so the call checks `small x`;
it answers `1` and the call returns `5`. A value outside the refinement is
refused at the refinement:

```lichen
small := v => v < 10
f := (x ! small) => x
f 20
```

```text
error: the refinement's predicate did not answer 1 — the value does not satisfy the refined type
  --> 2:7
   |
 2 | f := (x ! small) => x
   |       ^
```

The refinement is checked through the same channel as `@assert`
(Chapter 14): a predicate that does not answer `1` fails the program, and
the diagnostic points at the refinement rather than at the call. The
sentence names the refinement — `the refinement's predicate did not
answer 1 — the value does not satisfy the refined type` — where an
`@assert` reports the channel's plain `expected 1, found 0`. A class
contract can say more still: the core prelude's operators name the set the
value missed (`does not satisfy {Int, Float}`), because their contracts
name the class domain.

### Refinements on types

Written **on a type**, inside the type position, the predicate receives
the *type*, not the value:

```lichen
Num := set(Int, Float)
in_num := t => t @in Num
keep := (x : (_ ! in_num)) => x
(keep 1, keep 2.5)
```

```text
(1, 2.5): Tuple
```

The annotation is `x : (_ ! in_num)`: the type position is the
placeholder, refined by `in_num`. The predicate's argument is the type the
parameter is used at — `Int` in the first call, `Float` in the second — and
both are members of `Num`. A class that is not admitted is refused:

```lichen
Num := set(Int, Float)
in_num := t => t @in Num
keep := (x : (_ ! in_num)) => x
keep string
```

```text
error: the refinement's predicate did not answer 1 — the value does not satisfy the refined type
  --> 3:15
   |
 3 | keep := (x : (_ ! in_num)) => x
   |               ^
```

The diagnostic points at the type position that carries the refinement.
This is where a **class contract** belongs: the function accepts any
member of a set of types, and the set is the interface.

A refinement written on a type survives wherever a type expression is
consumed, so it can ride on one requirement of a compound's **constructor**:

```lichen
Num := set(Int, Float)
in_num := t => t @in Num
Pair := _x => (@any : (_ ! in_num), @any : (_ ! in_num))
f := (p <: Pair) => p
f (1, 2.5)
```

```text
(1, 2.5): Tuple
```

The requirement `@any : (_ ! in_num)` asks for any value whose **type** is
a member of `Num`, so the two positions accept an `Int` and a `Float`
together. What the match compares is the shape — the requirement's own
cells — so a constructor states a contract without becoming a second check
on every use; the class *refusal* is the prelude's operators' business, and
it is shown below.

A refinement does not change the value or its type. It is a predicate
that must hold, checked where the annotation is defined and — for an open
class — re-checked against each application's argument. The prelude's
operators use exactly this form to accept `{Int, Float}` and refuse
everything else.

## The prelude's contracts

The core prelude is where the surface language finds its arithmetic. It
binds `add`, `sub`, `mul`, `div`, `less`, `greater`, `less_or_equal` and
`greater_or_equal`, each over a **two-element tuple** whose positions are
refined by the class:

```lichen
let Pair := _x => (_ ! in_num, _ ! in_num)
add := operands => { operands <: Pair; operands(0) + operands(1) }
```

`Pair` is a constructor, and `operands <: Pair` applies it once: the two
positions it returns are the requirements the operands must meet.

That one declaration is the whole contract, and it is checked three ways.

A class the set does not admit is refused as the refinement. Among the
diagnostics the operator reports, the class refusal reads:

```text
error: does not satisfy {Int, Float}
  --> …/builtin/core.lichen:4:40
   |
 4 | add := operands => { operands <: Pair; operands(0) + operands(1) }
   |                                        ^
note: the failing use is here
  --> 1:5
   |
 1 | add ("a", "b")
   |     ^
```

The path is elided — it points into the core prelude's materialized file.
The prelude is a **file** (Chapter 15), so its failure is reported at the
line that wrote the contract, with a `note` that names the failing use in
your program. That is the general shape of a contract failure: the
declaration states the rule, and the note points at the use that broke it.

The two operands of one operator must also share a class, and a tuple
whose elements disagree is refused by the operator itself:

```lichen
add (1, 1.5)
```

```text
error: expected Tuple, found Tuple
  --> 1:5
   |
 1 | add (1, 1.5)
   |     ^
```

and the operands' count is the prelude's two, so a third is refused the
same way.

## Summary

- Every application instantiates its function's cells afresh: one binding
  can be used at many types, with no annotation.
- A function is only as open as its definition left it; `x + 1` decides
  `Int`, so `inc 2.5` is refused.
- The arithmetic operators are polymorphic over `{Int, Float}` and refuse a
  mixture.
- A class is a set of type values; a refinement `e ! p` requires `p e` to
  answer `1`.
- A refinement on a type receives the type value; `x : (_ ! in_num)` is the
  class contract, and it is re-checked per application while the class is
  open.
- A refinement failure names the refinement and points at it; a class
  contract names the class set, and the prelude adds a `note` at the failing
  use.
- The core prelude writes the operators' contracts in the language itself.

## Exercises

1. Write `id` and use it at `Int`, at `Type`, and at an array. How many
   annotations did you need?
2. Explain why `(inc 1, inc 2.5)` is refused while `(id 1, id 2.5)` is not.
3. Write `Num := set(Int, Float)` and `in_num`, then a function that accepts
   `(x : (_ ! in_num))`. Which calls hold, and which are refused?
4. Write a refinement that accepts values below 10 and check that `9` holds
   while `20` does not.
5. Write `add (1, 2)`, `add (1.5, 2.5)`, `add (1, 1.5)`, and
   `add (1, 2, 3)` and explain each answer.
6. Refine one requirement of a two-element tuple constructor and check which
   calls hold.