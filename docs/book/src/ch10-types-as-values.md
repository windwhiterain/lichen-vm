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
type and a two-field expr type:

```lichen
Expr = T => U => tuple{T, U}
Expr Int string
```

```text
tuple{Int, string}: TypeTuple
```

`Expr` is an ordinary lambda. Its parameters are type values, its body is
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

## Type values are not read apart

A type value can be printed, compared, passed and returned, but its
**components are not readable by a runtime access**: there is no `X{n}` for
a tuple type's component type and no `X::a` for a struct type's field type.
The rule is the language's own: a read reads a **value**, and a type's
component is not a value the program can ask for at run time.

There are two replacements, and they cover the two ways a type reaches a
function:

- when the type arrives in a **parameter's type slot**, read the value's
  element and take its type — `type_of s.a`, not a read of the type's field
  type;
- when the type arrives in a **value slot** and one of its components is
  needed, the program is higher-order: pass the type's *parameters* and build
  the type at the call site with a type function, so the parameter is an
  ordinary value (a struct instance, a tuple) whose elements are readable.

```lichen
S = struct{.a Int, .b string}
s = S(.a 1, .b "h")
type_of s.a
```

```text
Int: Type
```

A struct instance's field type is read from the **value**, never from the
type value `S`.

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
(Chapter 5). A kind says which type expression a value is, so a wrong
container is an ordinary type error; no read inspects a type value's
component at run time.
`tuple{Int, Int}` is `TypeTuple`, `array{Int, 3}` is `TypeArray`, a struct
type is `TypeStruct`, a table type is `TypeTable`, and a set type is
`SetType`. The one type that is not a value of a kind type is a function's:
its type is the function itself (Chapter 4), and Chapter 12 is about that
self-referential node.

## Summary

- Types are values: they can be bound, passed, stored, and returned, and an
  array of types has type `array{Type, n}`.
- A type constructor is an ordinary function; `Box Int` and `Expr Int
  string` are applications.
- A type value is not read apart: read a value's element and take its type
  (`type_of s.a`), or pass a type's parameters and build the type at the
  call site with a type function.
- A compound type's positions are slots; `v : T` pins a slot's value, the
  pin rides in the type value, and a value that disagrees is refused.
- `array{T, n}`'s length may be an expression; an annotation pins it, and
  the pin is enforced per application.
- A type value's kind is what makes a type expression a value of that kind,
  and a wrong kind is an ordinary type error.

## Exercises

1. Write `Expr = T => U => tuple{T, U}` and print `Expr Int string` and
   `Expr Type Int`.
2. Write `Expr = T => tuple{T, T}`, apply it to `Int` at the call site, and
   explain why a generic function takes the element type rather than reading
   it out of a tuple type value.
3. Write a type `t = tuple{5 : Int, Int}` and check that `(5, 2) : t`
   holds while `(6, 2) : t` does not.
4. Write a function that takes a length `n` and returns `array{Int, n}`,
   then apply it and read the result's length from the printed type.
5. Explain the difference between `tuple{Int, Int}` and
   `tuple{5 : Int, Int}` in terms of what the first element may be.
6. Store three types in an array and read the middle one back.
