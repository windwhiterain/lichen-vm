# Types and Requirements as Values

Chapter 5 established that a type is a value. This chapter takes that
seriously: types are bound and passed, **constructors** are built by
functions, and a requirement may pin a value. Everything here follows from
the same fact — a type is an ordinary node in the program's graph.

## Types are ordinary values

A type value can be bound, passed, stored in a compound, and returned:

```lichen
S := _x => {.a _, .b _}
s := S{.a Int, .b Float}
(s.a, s.b)
```

```text
(Int, Float): Tuple
```

The compound's fields are type values, and reading a field reads a type
back out. An array of types has type `Array`, and a table can use a type
as a key (Chapter 9).

## Constructors are built by functions

A **constructor** is a function that returns a fresh requirement when it is
applied, and a constructor is an ordinary function. Here is a two-field
tuple constructor and a one-field box constructor:

```lichen
Expr := T => U => _x => (@any : T, @any : U)
Expr Int string
```

```text
?a => Tuple: ?b => Tuple
```

`Expr` is an ordinary lambda whose parameters are type values, and its body
is a constructor: applied once, it returns the tuple of requirements. The
same function builds a different constructor when applied to different
arguments:

```lichen
Box := T => _x => [@any : T, 3]
Box Int
```

```text
?a => Array: ?b => Array
```

and a struct constructor is the same shape with named fields:

```lichen
Pair := A => B => _x => {.a @any : A, .b @any : B}
p := Pair Int string
p{.a 1, .b "s"}
```

```text
(1, "s"): Struct
```

This is why lichen does not need a separate type-constructor syntax: a
parametric constructor is a function, written and applied with the same
forms as any other function, and a partial application of one is a
constructor with one parameter already supplied.

## A constructor is not read apart

A constructor's result can be printed, compared, passed and returned, but a
requirement's **components are not readable by a runtime access**: there is
no `X{n}` for a tuple requirement's component and no `X::a` for a struct
requirement's field. The rule is the language's own: a read reads a
**value**, and a requirement's component is not a value the program can ask
for at run time.

There are two replacements, and they cover the two ways a shape reaches a
function:

- when the shape arrives in a **parameter's type slot**, read the value's
  element and take its type — `type_of s.a`, not a read of the
  requirement's field type;
- when the shape arrives in a **value slot** and one of its components is
  needed, the program is higher-order: pass the shape's *parameters* and
  build the shape at the call site with a constructor, so the parameter is
  an ordinary value (a struct instance, a tuple) whose elements are
  readable.

```lichen
type_of := x => {t := _; x: t; t}
S := _x => {.a @any : Int}
s := S{.a 1}
type_of s.a
```

```text
Int: Type
```

A struct instance's field type is read from the **value**, never from the
constructor.

## Requirements, and pinning a value

Every position of a requirement is a **requirement**, and a requirement is a
slot: it holds a value position, a type position, and any attributes
(Chapter 13). `@any : Int` is the requirement "any `Int` will do"; the
slot's value position is left open, and its type position says `Int`.

A slot may state a value as well as a type, and then the value is
**pinned**: `(5 : Int)` is the requirement "exactly the value `5`, of type
`Int`". The constructor returns the requirement, so the match is written
against the constructor:

```lichen
(5, 2) <: (_x => (5 : Int, @any : Int))
```

```text
(5, 2): Tuple
```

The pin is checked against the value that fills the position, and a
different value is refused at the position:

```lichen
(6, 2) <: (_x => (5 : Int, @any : Int))
```

```text
error: expected 5, found 6
  --> 1:1
   |
 1 | (6, 2) <: (_x => (5 : Int, @any : Int))
   | ^
```

Because the pin rides in the requirement, a bound constructor keeps it
holding:

```lichen
T := _x => (5 : Int, @any : Int)
(5, 2) <: T
```

```text
(5, 2): Tuple
```

and the same spelling pins a set's member:

```lichen
set(5) : set{5 : Int}
```

```text
set(5): set{5 : Int}
```

with `set(6) : set{5 : Int}` refused at the `6`. A requirement with **no**
top-level `:` is a requirement whose value position is unconstrained;
`(@any : Int, @any : Int)` accepts any two integers, while
`(5 : Int, @any : Int)` accepts only those with `5` first.

## The pinned length: a dependent array requirement

The length of an array requirement `[requirement, length]` is an expression,
not a numeral, so it may be a binding or a parameter:

```lichen
f := n => _x => [@any : Int, n]
```

The instance's own element count pins the length cell, and the pin is
enforced per application: applying the same function to an argument whose
count the length cannot meet is refused at the application, because the
parameter now carries a length its argument cannot meet.

```lichen
f := n => _x => [@any : Int, n]
```

This is the language's dependent-type mechanism — via laziness and
pinning, not via a separate type-level language. The pin is the same
mechanism as a pinned slot: a position of a requirement that states a value.
It is why a length that is a parameter's is a legal requirement, and why a
read that supplies a different length fails rather than silently resizing.

## Types are checked by their layout

A compound's type is its layout — `Tuple`, `Array` or `Struct` — and never
`Type` (Chapter 5). A wrong container is an ordinary type error; no read
inspects a requirement's component at run time. A table's type is
`table{K, V}`, a set's is `set{E}`, and the one type that is not a value
of a kind type is a function's: its type is the shared `FunctionType` marker
(Chapter 4), and Chapter 12 is about a function's own cells.

## Summary

- Types are values: they can be bound, passed, stored, and returned, and an
  array of types has type `Array`.
- A **constructor** is a function that returns a fresh requirement when it
  is applied, and it is an ordinary function; `Box Int` and
  `Expr Int string` are applications.
- A requirement is not read apart: read a value's element and take its
  type (`type_of s.a`), or pass a shape's parameters and build the shape at
  the call site with a constructor.
- A requirement's positions are slots; `5 : Int` pins a slot's value, and a
  value that disagrees is refused.
- An array requirement's length may be an expression; the instance's count
  pins it, and the pin is enforced per application.
- A compound's type is its layout; a wrong layout is an ordinary type
  error.

## Exercises

1. Write `Expr := T => U => _x => (@any : T, @any : U)` and print
   `Expr Int string` and `Expr Type Int`.
2. Write a constructor `Pair := A => B => _x => {.a @any : A, .b @any : B}`,
   apply it at the call site, and explain why a generic function takes the
   element type rather than reading it out of a requirement.
3. Write a requirement `T := _x => (5 : Int, @any : Int)` and check that
   `(5, 2) <: T` holds while `(6, 2) <: T` does not.
4. Write a function that takes a length `n` and returns the requirement
   `[@any : Int, n]`, then apply it and read the result's length from the
   printed value.
5. Explain the difference between `(@any : Int, @any : Int)` and
   `(5 : Int, @any : Int)` in terms of what the first element may be.
6. Store three types in a tuple and read the middle one back.
