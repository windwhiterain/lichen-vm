# The Value and Its Type

Every expression in lichen evaluates to a **expr**: a value and its type.
The compiler prints both halves, and the earlier chapters have already
shown the shape of that output. This chapter is about what the expr
*is*: why types are values, how an expression's type is found, what an
open cell means, and why `Type : Type` makes lichen's type system
different from a conventional one.

This is the chapter to slow down for. The rest of the book builds on it.

## Every answer is an expr

A program's output is two things:

```lichen
(1 + 2, "a", Int)
```

```text
(3, "a", Int): Tuple
```

Each element is a value, and the tuple's type is `Tuple` — the shared
layout type (below). The compiler knows both halves for every expression
it evaluates. `3` is a value with type `Int`; `"a"` is a value with type
`string`; `Int` is a value with type `Type`.

The two halves are not computed by two systems. In lichen, checking a
program **is** evaluating it: the compiler builds a graph in which every
expression is an expr of two nodes — its value and its type — and
unification is what keeps the two consistent. That is why the type is
printed at all, and why a program's answer includes it.

## Values and types are the same kind of thing

A type is a value. `Int` is a value of type `Type`, and it can be bound,
passed, returned, and stored like any other value:

```lichen
t := Int
t
```

```text
Int: Type
```

The four atom names are values in exactly that sense:

```lichen
(Int, Float, string, Type)
```

```text
(Int, Float, string, Type): Tuple
```

## The compound forms have shared layout types

A **compound value** — a tuple, an array, a struct — does not carry its
shape in its type. The language has exactly three compound types, and
they are *layout* types, shared by every compound of that form:

```lichen
(Tuple, Array, Struct)
```

```text
(Tuple, Array, Struct): Tuple
```

`Tuple`, `Array` and `Struct` are ordinary values of type `Type`. Every
tuple's type is `Tuple`, every array's is `Array`, and every struct's is
`Struct`:

```lichen
type_of := x => {t := _; x: t; t}
(type_of (1, 2), type_of [@any : Int, 2], type_of {.x @any : Int})
```

```text
(Tuple, Array, Struct): Tuple
```

So where does the *shape* live? In the **value**. A tuple's elements are
the value's own elements; an array's requirement and elements are the
value's own slots; a struct's field names and values are the value's own
header and fields. The type says which layout the value has; the value
says what is in it. That is why a read of a compound takes its
requirement from the value's layout rather than from a type expression
(Chapter 7), and why two tuples of different shapes have the *same* type
and are told apart by their values.

A **requirement** is the other half of the story: a slot that states what
one position of a compound must hold. A tuple's requirements are written
as a tuple of them, `(@any : Int, @any : Int)`; an array's is a requirement
and a length, `[requirement, length]`; a struct's is
`{.name requirement, …}`. A **constructor** is a function that returns a
fresh requirement when it is applied — `T := _x => (@any : Int, @any : Int)`
— and the match operator `<:` applies it exactly once and unifies the
actual with what comes back: `actual <: T` is `actual = T _`. Each
application therefore yields fresh holes, and the constructor keeps its own.
Chapter 7 is about the three forms.

## Unification is how the type is found

A type is not declared for an expression and then checked against it.
The expression's type is **found** by unifying what the program demands
of it. Applying a function is the clearest case: the argument's type is
unified with the parameter's type, and the call's type is the function's
return type.

The identity function has an open parameter, and each application can
decide it differently:

```lichen
id := x => x
(id 5, id Int)
```

```text
(5, Int): Tuple
```

A mismatch is a failed unification, and the diagnostic names the two
types:

```lichen
5 : Float
```

```text
error: expected Float, found Int
  --> 1:1
   |
 1 | 5 : Float
   | ^
```

Read `expected` as "what the program requires here" and `found` as "what
the expression actually has". An annotation requires its left side to
unify with the type on its right, which is why `5 : Float` fails: the
value's found type is `Int`.

## Open cells

An expression whose type is not decided yet has an **open cell**, printed
`?a`, `?b`, and so on. Cells that are unified with each other are one
class and share one name:

```lichen
f := x => x
f
```

```text
?a => ?a: ?a => ?a
```

The same `?a` appears in both halves because it is one cell: the
parameter's type and the result's type are the same thing, and nothing in
the program has decided what that thing is. An open cell is not an error.
It is the honest answer "this program does not say", and it can be read
as the type's own inference state.

### The `_` placeholder

`_` is a **placeholder** in any position. In type position it is an open
cell to be inferred from context:

```lichen
(5 : _, (1 : _, 2))
```

```text
(5, (1, 2)): Tuple
```

The first `_` is decided as `Int` by the value, and the second by the
tuple's first element. In value position it is a typed hole — a place
for a value the context supplies:

```lichen
(5 : _, (1 : _, 2), (_ : Int))
```

```text
(5, (1, 2), none): Tuple
```

The hole has no value of its own, so it prints as `none`; its type is
`Int` because the annotation says so. `_` is never a name: it cannot be
bound or used as a parameter (Chapter 3).

### A `raw` mark is a defect

An **undecided** value prints the same cell name as its type: `_` in a
tuple prints `(?a, ?b): Tuple`, and two uses of one hole print
`(?a, ?a): Tuple`, so the answer's two halves share one table and show
the unification topology. `raw` is the printer's other fallback: a value
whose type the check **decided** but cannot read as a form (a kernel
handle the struct type never named, a bare `[in, out]` shape). The mark
is never an answer: it says the value/type expr cannot be printed, and a
correct program does not produce one. The strict read rule (Chapter 4)
refuses the reads that would have left a value undecided. Chapter 14
lists the mark among the printer's words for a defect.

## Annotations state, they do not check

Every annotation is a unification. `e : T` requires the type of `e` to
unify with `T`; the value is `e`'s value and the type is `T`'s. So an
annotation can *state* a type that the program did not otherwise decide:

```lichen
inc := (x : Int) => x + 1
inc
```

```text
Int => Int: Int => Int
```

or it can state a compound's layout:

```lichen
(1, 2) : Tuple
```

```text
(1, 2): Tuple
```

and a disagreement is an ordinary type error:

```lichen
(1, 2) : Array
```

```text
error: expected Array, found Tuple
  --> 1:1
   |
 1 | (1, 2) : Array
   | ^
```

Chapter 3's `5 : Float` is that refusal too, and Chapter 13 shows the
annotations that carry more than a type: the perspective `#`, the
refinement `!`, and the doc `?`.

## Reading a type

Because types are values and unification is the mechanism, reading a
value's type needs no builtin. It is a function built from the two pieces
this chapter introduced — the placeholder and the annotation:

```lichen
type_of := x => {t := _; x: t; t}
(type_of (1), type_of Int, type_of [@any : Int, 2])
```

```text
(Int, Type, Array): Tuple
```

`t := _` makes an open cell. `x : t` unifies that cell with the
parameter's type. The body returns `t`, so the call's *value* is the
operand's type expression. The same function is used at three different
types, because each application instantiates the parameter fresh; that is
the let-polymorphism of Chapter 11 in its smallest form.

The standard library ships this as `std.type_of`
([`lichen-std/_.lichen`](https://github.com/windwhiterain/lichen-vm/blob/dev/lichen-std/_.lichen)); Chapter 15 shows
how to import it. `type_of` is the source's own two-line definition, not a
special form.

## `Type : Type`, and no subtyping

Every value has a type, and every type has a type, and the chain closes
in a cycle at `Type`:

```lichen
Type : Type
```

```text
Type: Type
```

So `5 : Int`, `Int : Type`, `Type : Type`. The universe is the
self-referential node `K = [Type, K]`: `Type` is the terminal of every
chain, not a supertype of everything. There is no subtyping at all, which
is why a value of type `Int` is not accepted where a `Type` is expected:

```lichen
(1 : Type)
```

```text
error: expected Type, found Int
  --> 1:2
   |
 1 | (1 : Type)
   |  ^
```

The flip side of this design is that `Type : Type` admits recursive types
and first-class types — and that the decidability of a lichen program is
the program's own responsibility, not the type system's. Chapter 12 is
about recursive types; Chapter 13 records what a perspective adds to the
expr.

## Summary

- Every expression evaluates to an expr, a value and its type; the compiler
  prints both.
- A type is a value; the atom types have type `Type`.
- A compound's type is one of three shared layout types — `Tuple`,
  `Array`, `Struct` — and the *value* owns the layout: the elements, the
  array's requirement, the struct's field names.
- A **requirement** states what one position of a compound must be, and a
  **constructor** is a function that returns a fresh requirement when
  applied; `<:` applies the constructor once and unifies the actual with the
  result, while plain `=` applies nothing.
- An expression's type is found by unification, not declared and checked.
- An undecided type is an open cell named `?a`; the same name means one
  cell. `_` is a placeholder. An answer whose type stays open prints that
  cell's name in both halves — the topology, not a value; a `raw` mark is
  the separate symptom of a decided type the value cannot be read through
  (Chapter 14).
- An annotation is a unification, and it can state what the program did not
  decide.
- `type_of` is an ordinary function: placeholder plus annotation.
- `Type : Type` holds and `Type` is the terminal of the chain; there is no
  subtyping.

## Exercises

1. Write `type_of` from the chapter and read the types of `5`, `Float`,
   `(1, 2)`, `[@any : Int, 2][1, 2]`, and `Type`.
2. Explain why `(1 : Type)` is refused, in terms of the chain of types.
3. Write `expr_up = x => (x, x)` and read its type with `type_of`. Which
   cell names appear in the printed type, and why?
4. Explain why a `raw`-marked answer is a defect rather than a result, and
   name the annotation that removes one.
5. Why is `(1, 2) : Array` refused? What is the tuple's own type?
