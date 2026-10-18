# Types as Values

Chapter 5 established that a type is a value. This chapter takes that
seriously: types are built by functions, read apart with their own forms,
carried inside other types, and pinned to particular values. Everything
here follows from the same fact — a type is an ordinary node in the
program's graph.

## Types are ordinary values

A type value can be bound, passed, stored in a compound, and returned:

```lichen
[Int, Float]
```

```text
[Int, Float]: array{Type, 2}
```

The array's elements are type values, so its element type is `Type` and
its length is two. A struct can hold a type in a field (Chapter 8), and a
table can use a type as a key (Chapter 9). Indexing the array reads a type
back out:

```lichen
[Int, Float][0]
```

```text
Int: Type
```

## Types are built by functions

A type constructor is a function like any other. Here is a one-field box
type and a two-field pair type:

```lichen
Pair = T => U => tuple{T, U}
Pair Int string
```

```text
tuple{Int, string}: TypeTuple
```

`Pair` is an ordinary lambda. Its parameters are type values, its body is
a tuple type expression, and the result is a type value of kind
`TypeTuple`. The same function builds a different type when applied to
different arguments:

```lichen
Box = T => array{T, 3}
Box Int
```

```text
array{Int, 3}: TypeArray
```

and the result's kind is whatever the type expression's kind is. This is
why lichen does not need a separate type-constructor syntax: a
parametric type is a function, written and applied with the same forms as
any other function, and a partial application of one is a type
constructor with one parameter already supplied.

`type_of` (Chapter 5) is the smallest useful example: it is a function
whose result is the argument's type expression.

## Reading a type apart

A **type access** `X{n}` reads component `n` of a tuple **type value**:

```lichen
tuple{Int, string}{0}
```

```text
Int: Type
```

The index is a full expression, so a computed component works too:

```lichen
tuple{Int, string}{0 + 1}
```

```text
string: Type
```

The container must be a tuple type value: its type must be the tuple kind.
A tuple *value* is refused (`expected TypeTuple, found tuple{Int, Int}`),
and so is an array type value (`expected TypeTuple, found TypeArray`). An
index past the components is refused like any other out-of-bounds read:

```lichen
tuple{Int, string}{2}
```

```text
error: index 2 out of bounds (array length 2)
  --> 1:20
   |
 1 | tuple{Int, string}{2}
   |                    ^
```

A component that is itself a type value has the kind of that type
(Chapter 5's kinds), so `tuple{struct{.a Int}, Int}{0}` is a struct type
value whose type is `TypeStruct`.

The named sibling is `X::a` (Chapter 8): a struct **type value**'s field
type, read by name. `X{n}` and `X::a` are the type-level reads; `.name` and
`a(k)` are the value-level reads.

## Slots, and pinning a value

Every position of a compound type is a **slot**. A slot holds the value
position, the type position, and any attributes (Chapter 13). The braced
type forms write the slots directly, and a slot may state a value as well
as a type: `5 : Int` is a slot whose **value** is pinned to `5`:

```lichen
(5, 2) : tuple{5 : Int, Int}
```

```text
(5, 2): tuple{5 : Int, Int}
```

The pin is checked against the value that fills the position, and the
type that prints carries the pin — `tuple{5 : Int, Int}` — because the
pin is part of the type value. A different value is refused at the
position:

```lichen
(6, 2) : tuple{5 : Int, Int}
```

```text
error: expected 5, found 6
  --> 1:2
   |
 1 | (6, 2) : tuple{5 : Int, Int}
   |  ^
```

Because the pin rides in the type value, binding the type keeps it
holding:

```lichen
t = tuple{5 : Int, Int}
(5, 2) : t
```

```text
(5, 2): tuple{5 : Int, Int}
```

and the same spelling pins a set's member:

```lichen
set(5) : set{5 : Int}
```

```text
set(5): set{5 : Int}
```

with `set(6) : set{5 : Int}` refused at the `6`. An element with **no**
top-level `:` is a type expression whose value position is unconstrained;
`tuple{Int, Int}` accepts any two integers, while `tuple{5 : Int, Int}`
accepts only those with `5` first.

## The pinned length: a dependent array type

The length of `array{T, n}` is an expression, not a numeral, so it may be
a binding or a parameter:

```lichen
((n => ([1, 2, 3] : array{Int, n})) 3)
```

```text
[1, 2, 3]: array{Int, 3}
```

The annotation compares the literal's type `array{Int, 3}` with
`array{Int, n}`; the length read resolves to the binding `n`, and `n` is
pinned to `3`. The pin is enforced per application: applying the same
function to `2` is refused at the application, because the parameter now
carries a length its argument cannot meet. This is the language's
dependent-type mechanism — via laziness and pinning, not via a separate
type-level language.

The pin is the same mechanism as a pinned slot: a position of a type that
states a value. It is why `array{Int, n}` is a legal type expression whose
`n` is decided by the position that needs it, and why a read that supplies
a different length fails rather than silently resizing.

## Types are checked by their kind

A compound type value's type is its kind, never `Type`
(Chapter 5). So the forms for reading a type are forms for reading a value
of one particular kind, and a wrong container is an ordinary type error.
`tuple{Int, Int}` is `TypeTuple`, `array{Int, 3}` is `TypeArray`, a struct
type is `TypeStruct`, a table type is `TypeTable`, and a set type is
`SetType`. The one type that is not a value of a kind type is a function's:
its type is the function itself (Chapter 4), and Chapter 12 is about that
self-referential node.

## Summary

- Types are values: they can be bound, passed, stored, and returned, and an
  array of types has type `array{Type, n}`.
- A type constructor is an ordinary function; `Box Int` and `Pair Int
  string` are applications.
- `X{n}` reads component `n` of a tuple type value; `X::a` reads a struct
  type value's field type. The container's kind is required, and a
  computed index works.
- A compound type's positions are slots; `v : T` pins a slot's value, the
  pin rides in the type value, and a value that disagrees is refused.
- `array{T, n}`'s length may be an expression; an annotation pins it, and
  the pin is enforced per application.
- A type value's kind is what makes a type-level read meaningful, and a
  wrong kind is an ordinary type error.

## Exercises

1. Write `Pair = T => U => tuple{T, U}` and print `Pair Int string` and
   `Pair Type Int`.
2. Read the first and second components of `tuple{Int, string}` with
   `{0}` and `{1}`, and explain why `{2}` is refused.
3. Write a type `t = tuple{5 : Int, Int}` and check that `(5, 2) : t`
   holds while `(6, 2) : t` does not.
4. Write a function that takes a length `n` and returns `array{Int, n}`,
   then apply it and read the result's length from the printed type.
5. Explain the difference between `tuple{Int, Int}` and
   `tuple{5 : Int, Int}` in terms of what the first element may be.
6. Store three types in an array and read the middle one back.
