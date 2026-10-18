# Polymorphism and Classes

lichen's functions are polymorphic by default: a function's parameter can
be decided afresh at every call. This chapter is about that mechanism,
the way a **class** states which types a function accepts, and the
annotation — the **refinement** — that writes such a contract.

## Every application is fresh

A function value is a template. Applying it instantiates the parameter and
the return type afresh, so one binding can serve different types:

```lichen
id = x => x
(id 5, id Int)
```

```text
(5, Int): tuple{Int, Type}
```

The same binding, at two different types, with no annotation anywhere.
This is automatic let-polymorphism: there is no `forall` and no
generalize/instantiate form, because the answer is a fresh cell at each
call. Chapter 5's `type_of` is the same idea in a nicer costume — one
function, used at `Int`, at `Type`, and at `array{Int, 2}`.

A template is only as open as its definition left it. A body that
constrains its parameter decides the template there, and then every call
must meet it:

```lichen
inc = x => x + 1
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
is `Int -> Int` by the time it is called. Openness is not a flag; it is
which cells the definition happened to decide.

## The operators' class

`+`, `-`, `*`, `/` and the comparisons accept more than one type, and they
are polymorphic in the same way. Both integers and floats work, but never
a mixture:

```lichen
(1 + 2, 1.5 + 2.5)
```

```text
(3, 4.0): tuple{Int, Float}
```

`1 + 1.5` is refused (`expected Int, found Float`): the two operands of one
operator share one class — an array's elements are one type — and the
literal `1` decides that class as `Int`.

What a class accepts is written with a **set of types**. The core prelude
(the bindings every program starts with) says it this way:

```lichen
Num = set(Int, Float)
in_num = t => t @in Num
```

`Num` is a set value whose members are the types `Int` and `Float`, and
`in_num` is a predicate that asks whether a type is one of them. This is
the class domain the arithmetic operators are declared over. Chapter 9
showed sets of types; here the set is used as a *contract*.

## Refinements

A **refinement** attaches a predicate to a value and requires the
predicate to answer `1`. Written on a value, it checks that value:

```lichen
small = v => v < 10
f = (x ! small) => x
f 5
```

The parameter `x` is refined by `small`, so the call checks `small x`;
it answers `1` and the call returns `5`. A value outside the refinement is
refused at the refinement:

```lichen
small = v => v < 10
f = (x ! small) => x
f 20
```

```text
error: assertion failed: expected 1, found 0
  --> 2:6
   |
 2 | f = (x ! small) => x
   |      ^
```

The refinement is checked through the same channel as `@assert`
(Chapter 14): a predicate that does not answer `1` fails the program, and
the diagnostic points at the refinement rather than at the call. The
failure sentence is the channel's; the core prelude's operators add the
class they expected (`does not satisfy {Int, Float}`), because their
contracts name the set.

### Refinements on types

Written **on a type**, inside the type position, the predicate receives
the *type*, not the value:

```lichen
Num = set(Int, Float)
in_num = t => t @in Num
keep = (x : (_ ! in_num)) => x
(keep 1, keep 2.5)
```

```text
(1, 2.5): tuple{Int, Float}
```

The annotation is `x : (_ ! in_num)`: the type position is the
placeholder, refined by `in_num`. The predicate's argument is the type the
parameter is used at — `Int` in the first call, `Float` in the second — and
both are members of `Num`. A class that is not admitted is refused:

```lichen
Num = set(Int, Float)
in_num = t => t @in Num
keep = (x : (_ ! in_num)) => x
keep string
```

```text
error: assertion failed: expected 1, found 0
  --> 3:14
   |
 3 | keep = (x : (_ ! in_num)) => x
   |              ^
```

The diagnostic points at the type position that carries the refinement.
This is where a **class contract** belongs: the function accepts any
member of a set of types, and the set is the interface.

A refinement written on a type survives wherever a type expression is
consumed, so it can refine one component of a compound type:

```lichen
Num = set(Int, Float)
in_num = t => t @in Num
f = (p : tuple{(_ ! in_num), (_ ! in_num)}) => p
f (1, 2.5)
```

```text
(1, 2.5): tuple{Int, Float}
```

A component outside the class is refused on its own position:

```lichen
Num = set(Int, Float)
in_num = t => t @in Num
f = (p : tuple{(_ ! in_num), (_ ! in_num)}) => p
f (1, "a")
```

```text
error: assertion failed: expected 1, found 0
  --> 3:31
   |
 3 | f = (p : tuple{(_ ! in_num), (_ ! in_num)}) => p
   |                               ^
```

The refinement is enforced where it was written: the second component's
annotation, not the tuple's.

A refinement does not change the value or its type. It is a predicate
that must hold, checked where the annotation is defined and — for an open
class — re-checked against each application's argument. The prelude's
operators use exactly this form to accept `{Int, Float}` and refuse
everything else.

## The prelude's contracts

The core prelude is where the surface language finds its arithmetic. It
binds `add`, `sub`, `mul`, `div`, `less`, `greater`, `less_or_equal` and
`greater_or_equal`, each over a **two-element array** whose element type is
refined by the class:

```lichen
add = operands => { operands : array{(_ ! in_num), 2}; operands[0] + operands[1] }
```

That one declaration is the whole contract, and it is checked three ways:

```lichen
add [1, 1.5]
```

```text
error: expected Int, found Float
  --> 1:9
   |
 1 | add [1, 1.5]
   |         ^
```

the two elements must share one class; a wrong length is refused against
the array type:

```lichen
add [1, 2, 3]
```

```text
error: expected array{Int, 2}, found array{Int, 3}
  --> 1:5
   |
 1 | add [1, 2, 3]
   |     ^
```

and a class the set does not admit is refused as the refinement. Among
the two diagnostics the operator reports, the class refusal reads:

```text
error: does not satisfy {Int, Float}
  --> …/builtin/core.lichen:3:56
   |
 3 | add = operands => { operands : array{(_ ! in_num), 2}; operands[0] + operands[1] }
   |                                                        ^
note: the failing use is here
  --> 1:5
   |
 1 | add ["a", "b"]
   |     ^
```

The path is elided — it points into the core prelude's materialized file.
The prelude is a **file** (Chapter 15), so its failure is reported at the
line that wrote the contract, with a `note` that names the failing use in
your program. That is the general shape of a contract failure: the
declaration states the rule, and the note points at the use that broke it.

## Summary

- Every application instantiates its function's cells afresh: one binding
  can be used at many types, with no annotation.
- A template is only as open as its definition left it; `x + 1` decides
  `Int`, so `inc 2.5` is refused.
- The arithmetic operators are polymorphic over `{Int, Float}` and refuse a
  mixture.
- A class is a set of type values; a refinement `e ! p` requires `p e` to
  answer `1`.
- A refinement on a type receives the type value; `x : (_ ! in_num)` is the
  class contract, and it is re-checked per application while the class is
  open.
- A refinement failure uses the assert channel's sentence and points at the
  refinement; the prelude's contracts name the class set and add a `note`
  at the failing use.
- The core prelude writes the operators' contracts in the language itself.

## Exercises

1. Write `id` and use it at `Int`, at `Type`, and at a tuple type. How many
   annotations did you need?
2. Explain why `(inc 1, inc 2.5)` is refused while `(id 1, id 2.5)` is not.
3. Write `Num = set(Int, Float)` and `in_num`, then a function that accepts
   `(x : (_ ! in_num))`. Which calls hold, and which are refused?
4. Write a refinement that accepts values below 10 and check that `9` holds
   while `20` does not.
5. Write `add [1, 2]`, `add [1.5, 2.5]`, `add [1, 1.5]`, and `add [1, 2, 3]`
   and explain each answer.
6. Refine one component of a two-element tuple type and check that a
   disagreeing component is refused.
