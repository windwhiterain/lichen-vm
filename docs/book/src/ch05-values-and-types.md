# The Value and Its Type

Every expression in lichen evaluates to a **pair**: a value and its type.
The compiler prints both halves, and the earlier chapters have already
shown the shape of that output. This chapter is about what the pair
*is*: why types are values, how an expression's type is found, what an
open cell means, and why `Type : Type` makes lichen's type system
different from a conventional one.

This is the chapter to slow down for. The rest of the book builds on it.

## Every answer is a pair

A program's output is two things:

```lichen
(1 + 2, "a", Int)
```

```text
(3, "a", Int): tuple{Int, string, Type}
```

Each element is a value, and the tuple's type is the tuple of their
types. The compiler knows both halves for every expression it evaluates.
`3` is a value with type `Int`; `"a"` is a value with type `string`;
`Int` is a value with type `Type`.

The two halves are not computed by two systems. In lichen, checking a
program **is** evaluating it: the compiler builds a graph in which every
expression is a pair of two nodes — its value and its type — and
unification is what keeps the two consistent. That is why the type is
printed at all, and why a program's answer includes it.

## Values and types are the same kind of thing

A type is a value. `Int` is a value of type `Type`, and it can be bound,
passed, returned, and stored like any other value:

```lichen
t = Int
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
(Int, Float, string, Type): tuple{Type, Type, Type, Type}
```

A *compound* type is a value too, but its type is its **kind** rather
than `Type`:

```lichen
(tuple{Int, Int}, array{Int, 2}, struct{.a Int})
```

```text
(tuple{Int, Int}, array{Int, 2}, struct{.a Int}): tuple{TypeTuple, TypeArray, TypeStruct}
```

The tuple type is a value whose type is `TypeTuple`; the array type's is
`TypeArray`; the struct type's is `TypeStruct`. A kind is not a separate
layer with separate rules — it is an ordinary type, and "is this a
kinding mistake?" is an ordinary type error. Chapter 10 is about the
compound type values and their kinds.

## Unification is how the type is found

A type is not declared for an expression and then checked against it.
The expression's type is **found** by unifying what the program demands
of it. Applying a function is the clearest case: the argument's type is
unified with the parameter's type, and the call's type is the function's
return type.

The identity function has an open parameter, and each application can
decide it differently:

```lichen
id = x => x
(id 5, id Int)
```

```text
(5, Int): tuple{Int, Type}
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
f = x => x
f
```

```text
?a => ?a: ?a -> ?a
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
(5 : _, [1, 2] : array{Int, _})
```

```text
(5, [1, 2]): tuple{Int, array{Int, 2}}
```

The first `_` is decided as `Int` by the value, and the second as the
length `2` by the array. In value position it is a typed hole — a place
for a value the context supplies:

```lichen
(5 : _, [1, 2] : array{Int, _}, (_ : Int))
```

```text
(5, [1, 2], none): tuple{Int, array{Int, 2}, Int}
```

The hole has no value of its own, so it prints as `none`; its type is
`Int` because the annotation says so. `_` is never a name: it cannot be
bound or used as a parameter (Chapter 3).

### The `raw` mark

When an answer's type is not decided by the program, the printed value
carries the `raw` mark:

```lichen
gcd = p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))
gcd (48, 18)
```

```text
raw 6: ?a
```

The value is `6`, but its type is an open cell, and the `raw` mark says so.
The mark is the printer's word for "this answer is not a decided type"; it
appears in the value half too, as in Chapter 4's `raw 3: raw[?a, ?b]`
where even the class of the arithmetic was open. A program that decides
every type prints neither `?` nor `raw`.

## Annotations state, they do not check

Every annotation is a unification. `e : T` requires the type of `e` to
unify with `T`; the value is `e`'s value and the type is `T`'s. So an
annotation can *state* a type that the program did not otherwise decide:

```lichen
inc = (x : Int) => x + 1
inc
```

```text
Int => Int: Int -> Int
```

or it can state a function's whole signature:

```lichen
(x => x + 1) : Int -> Int
```

```text
Int => Int: Int -> Int
```

and an annotation that disagrees with the value is refused where it is
written. Chapter 3's `5 : Float` is that refusal, and Chapter 10 shows the
annotations that carry more than a type: the perspective `#`, the
refinement `!`, and the doc `?`.

## Reading a type

Because types are values and unification is the mechanism, reading a
value's type needs no builtin. It is a function built from the two pieces
this chapter introduced — the placeholder and the annotation:

```lichen
type_of = x => {t = _; x: t; t}
(type_of (1), type_of Int, type_of [1, 2])
```

```text
(Int, Type, array{Int, 2}): tuple{Type, Type, TypeArray}
```

`t = _` makes an open cell. `x : t` unifies that cell with the parameter's
type. The body returns `t`, so the call's *value* is the operand's type
expression. The same function is used at three different types, because
each application instantiates the parameter fresh; that is the
let-polymorphism of Chapter 11 in its smallest form.

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

and why a compound type is typed by its kind rather than by `Type`:

```lichen
array{Int, 3} : Type
```

```text
error: expected Type, found TypeArray
  --> 1:1
   |
 1 | array{Int, 3} : Type
   | ^
```

`array{Int, 3}` is a value whose type is `TypeArray`, so annotating it
with `Type` is an ordinary type error. The flip side of this design is
that `Type : Type` admits recursive types and first-class types — and
that the decidability of a lichen program is the program's own
responsibility, not the type system's. Chapter 12 is about recursive
types; Chapter 13 records what a perspective adds to the pair.

## Summary

- Every expression evaluates to a pair, a value and its type; the compiler
  prints both.
- A type is a value; the atom types have type `Type`, and compound types
  have kind types such as `TypeTuple` and `TypeStruct`.
- An expression's type is found by unification, not declared and checked.
- An undecided type is an open cell named `?a`; the same name means one
  cell. `_` is a placeholder, and an answer whose type stays open prints
  with the `raw` mark.
- An annotation is a unification, and it can state what the program did not
  decide.
- `type_of` is an ordinary function: placeholder plus annotation.
- `Type : Type` holds and `Type` is the terminal of the chain; there is no
  subtyping, and a compound type is checked by its kind.

## Exercises

1. Write `type_of` from the chapter and read the types of `5`, `Float`,
   `(1, 2)`, `[1, 2]`, and `Type`.
2. Explain why `(1 : Type)` is refused, in terms of the chain of types.
3. Write `pair_up = x => (x, x)` and read its type with `type_of`. Which
   cell names appear in the printed type, and why?
4. Write a program that prints an open cell and one that prints a `raw`
   value. Explain the difference between the two marks.
5. Why is `array{Int, 3} : Type` a kinding error? What is that type
   value's type?
