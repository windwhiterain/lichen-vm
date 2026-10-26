# Compound Values: Tuples and Arrays

The earlier chapters used tuples and arrays as if they had been
introduced. This chapter introduces them: how they are written, how their
elements are read, what their types say, and how a **requirement** states
what a compound's positions must hold.

The one idea to carry through the chapter is this: a compound's **type**
says which layout it has, and the compound's **value** says what is in
it. `Tuple` and `Array` are the two layout types; there is no
`tuple{…}` or `array{…}` type expression to learn.

## Tuples

A tuple is a fixed-length group of values written in parentheses:

```lichen
(1, "a", Int)
```

```text
(1, "a", Int): Tuple
```

The value is three values; the type is `Tuple`, the shared layout type
every tuple has. A tuple may group values of different types, which is
why it is the language's way to pass several arguments to a function: a
function takes exactly one argument (Chapter 4), so a tuple carries the
group.

A tuple has at least one element. Parentheses around one expression are
just grouping — `(5)` is the integer `5` — and the one-element tuple is
written with the trailing separator, `(5,)`:

```lichen
((5,), (5,)(0))
```

```text
((5,), 5): Tuple
```

There is no empty tuple: `()` and `(,)` are parse errors. The trailing
separator is the general rule — a tuple's elements are separated by the
same separator as statements (Chapter 3), and a trailing one is
allowed.

### Reading a tuple

A tuple's element is read by position with the adjacent single-expression
parentheses:

```lichen
t := (10, 20, 30)
(t(0), t(2), t(0 + 1))
```

```text
(10, 30, 20): Tuple
```

The index is a full expression, and the read is by position, not by name.
The form is chosen by syntax: `t(k)` with one bare expression inside is a
tuple read, and the **tuple layout** is required. An array is refused
(`expected Array, found Tuple`), and so is a struct (`expected Struct,
found Tuple`); Chapter 8 shows the struct read, which is by name.

A tuple's elements are lazy values like any other: reading one does not
read the others (Chapter 6).

### A tuple's requirements

A **requirement** states what one position of a compound must hold, and
`@any : T` is the requirement "any `T` will do":

```lichen
(@any : Int, @any : Int)
```

```text
(@any, @any): Tuple
```

Requirements are matched with `<:`, the match operator. Its right hand
side is a **constructor** — a function that returns a fresh requirement when
it is applied — and `<:` applies that constructor exactly once:

```lichen
T := _x => (@any : Int, @any : Int)
v := (1, 2) <: T
(T, v)
```

```text
(?a => Tuple, (1, 2)): Tuple
```

`actual <: T` is the same as `actual = T _`: the constructor is applied to a
placeholder, and the actual is unified with the requirement that comes back.
The constructor keeps its own holes and every match gets fresh ones; the
answer's first element is the constructor itself, printed as the function it
is.

A requirement that the actual does not meet is refused where the match is
written:

```lichen
(1, 2) <: (_x => (@any : Int, @any : Type))
```

```text
error: expected Type, found Int
  --> 1:1
   |
 1 | (1, 2) <: (_x => (@any : Int, @any : Type))
   | ^
```

A requirement is how a function says what it accepts, and the `<:` in a
parameter is the same rule applied to the parameter:

```lichen
T := _x => (@any : Int, @any : Int)
addpair := (p <: T) => p(0) + p(1)
addpair (1, 2)
```

```text
3: Int
```

The requirement states each position's type, and the read of a position
carries that position's type — which is what lets the arithmetic operators
see it.

## Arrays

An array is a fixed-length group of values of **one** type, and it is
written in two parts: the **template**, which says what an element must
be and how many there are, and the **instance**, which supplies the
elements.

The template is `[requirement, length]`:

```lichen
[@any : Int, 3]
```

```text
[?a, ?b, ?c]: Array
```

The template is itself an array value: three open slots waiting for
elements, with the element requirement kept beside them. The instance
writes the elements after it, and it applies its constructor once — the
same rule the tuple match uses:

```lichen
T := _x => [@any : Int, 3]
T[10, 20, 30]
```

Every element is unified with the **one** element requirement the
constructor returned, so the array is homogeneous by construction, and the
returned length is checked against the instance's element count.

There is no empty array: an array template's length is at least one, and
`[]` is a parse error.

> **The array instance is not accepted yet.** The compiler refuses
> `T[10, 20, 30]` with `expected Array, found Array`, and the plain
> template `[@any : Int, 3][10, 20, 30]` is refused as a call of a
> non-function. The rule above is the one the language states; the
> [constructor note](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/value-templates.md)
> records the measurement.

### Reading an array

An array's element is read with brackets, and the index is **comma-free** —
one expression and no separator. That is what tells a read apart from an
instance:

```lichen
a := [@any : Int, 3]
(a[0], a[2], a[1 + 1])
```

```text
(?a, ?b, ?b): Tuple
```

The elements read here are the template's own holes, because the instance
that would fill them is not accepted yet (above).

`a[i]` reads; `a[i, j]` and `a[i,]` instantiate the constructor `a` is
written on. The array **layout** is required, so a tuple or a struct is
refused by the same rule that refuses an array at a tuple read:

```lichen
t := (10, 20, 30)
t[0]
```

```text
error: expected Array, found Tuple
  --> 1:6
   |
 1 | t := (10, 20, 30)
   |      ^
```

An index out of bounds is refused rather than answered. When the index is
known while checking, the refusal is at the read:

```lichen
a := [@any : Int, 3]
a[3]
```

```text
error: index 3 out of bounds (array length 3)
  --> 2:3
   |
 2 | a[3]
   |   ^
```

When the index is only known when the read is evaluated, the same check
runs then, and the read answers the lazy marker.

An array's elements are lazy (Chapter 6), and its length is not readable
at run time: there is no built-in for it, and the length lives in the
template rather than in the value. Iteration over an array is written by
carrying the count in a recursion, which is why Chapter 4's `apply_n`
takes the count as an argument.

## Nested compounds

The two forms nest freely. A tuple may hold arrays and arrays may hold
tuples:

```lichen
xs := [@any : Tuple, 2]
xs[0]
```

```text
?a: ?a
```

A read may be chained only while the layout agrees at each step: the
outer `[0]` selects a row (an array), and the inner `(0)` reads the
tuple's first element. Here the row is still a hole, so the inner read
has no layout to take:

```lichen
xs := [@any : Tuple, 2]
xs[0](0)
```

```text
error: this tuple read cannot be checked: the container's type is not decided here — annotate the container's type
  --> 2:1
   |
 2 | xs[0](0)
   | ^
```

A tuple of arrays and an array of tuples are different *values*, and
`type_of` (Chapter 5) prints their layouts without ambiguity:

```lichen
type_of := x => {t := _; x: t; t}
(type_of ((1, 2), "a"), type_of [@any : Tuple, 2])
```

```text
(Tuple, Array): Tuple
```

### The reads, in one table

The language has one read form per layout, and the layout is what chooses
it. There is no runtime dispatch: a wrong layout is a check-time
refusal.

| container | layout | read | see |
|---|---|---|---|
| tuple | `Tuple` | `a(k)` | this chapter |
| array | `Array` | `a[i]` | this chapter |
| struct | `Struct` | `a.name` | Chapter 8 |
| table | `table{K, V}` | `t <== k` | Chapter 9 |
| set | `set{E}` | `x @in s` | Chapter 9 |

The last column is the point: each read is a *different form* for a
*different layout*, and no container is indexable by more than one form.

## Tuples and arrays as values

Both forms are ordinary values: they can be bound, passed, returned, and
stored. A function returning a tuple is how a function returns several
answers:

```lichen
f := x => (x, x + 1)
f 1
```

```text
(1, 2): Tuple
```

and a function returning an array is the same shape with a homogeneous
result:

```lichen
f := x => [@any : Int, 3]
f 7
```

```text
[?a, ?b, ?c]: Array
```

A compound's **layout** is fixed by its form, not by its contents: every
tuple is a `Tuple` and every array an `Array`, wherever it is used. The
shape is in the value, and the value is what the elements are read from.

## Summary

- A tuple is `(a, b, …)`, at least one element; `(e,)` is the one-element
  tuple, and there is no empty tuple. Its type is the shared layout type
  `Tuple`.
- A tuple's element is read by position, `t(k)`, and the read requires the
  tuple layout.
- A **requirement** states what one position must be — `@any : T` being
  "any `T` will do" — and a **constructor** is a function that returns a
  fresh requirement when applied. `<:` applies the constructor once and
  unifies the actual with the result; plain `=` applies nothing.
- An array template is `[requirement, length]`, and an array instance
  applies its constructor once and fills the returned element requirement.
  The instance is not accepted by the compiler yet. An array's type is
  `Array`.
- An array's element is read by position, `a[i]`, with a comma-free
  index; the read requires the array layout.
- The read form is chosen by the container's layout; a wrong layout is a
  check-time refusal, never a runtime dispatch.
- Tuples and arrays nest, and both are ordinary values.

## Exercises

1. Write a function that returns the first and last elements of an array as
   a tuple, and read the tuple's components.
2. Explain why `(1, 2)[0]` and a tuple read on an array are both refused,
   and why the diagnostics name different layouts.
3. Write `sum3 = t => t(0) + t(1) + t(2)` and apply it to `(1, 2, 3)`.
   What does the type of `t` need to be for the answer to be decided?
4. Write an array of arrays and read an element from the second row.
5. Write a tuple constructor, match two different values against it, and
   check that the constructor still reads the same afterwards.
