# Compound Values: Tuples and Arrays

The earlier chapters used tuples and arrays as if they had been
introduced. This chapter introduces them: how they are written, how their
elements are read, what their types say, and what the two forms are not.

## Tuples

A tuple is a fixed-length group of values written in parentheses:

```lichen
(1, "a", Int)
```

```text
(1, "a", Int): tuple{Int, string, Type}
```

The value is three values; the type is the tuple of their types,
`tuple{Int, string, Type}`. A tuple may group values of different types,
which is why it is the language's way to pass several arguments to a
function: a function takes exactly one argument (Chapter 4), so a tuple
carries the group.

A tuple has at least one element. Parentheses around one expression are
just grouping — `(5)` is the integer `5` — and the one-element tuple is
written with the trailing separator, `(5,)`:

```lichen
((5,), (5,)(0))
```

```text
((5,), 5): tuple{tuple{Int}, Int}
```

There is no empty tuple: `()` and `(,)` are parse errors. The trailing
separator is the general rule — a tuple's elements are separated by the
same separator as statements (Chapter 3), and a trailing one is
allowed.

### Reading a tuple

A tuple's element is read by position with the adjacent single-expression
parentheses:

```lichen
t = (10, 20, 30)
(t(0), t(2), t(0 + 1))
```

```text
(10, 30, 20): tuple{Int, Int, Int}
```

The index is a full expression, and the read is by position, not by name.
The form is chosen by syntax: `t(k)` with one bare expression inside is a
tuple read, and the tuple *kind* is required. An array is refused
(`expected TypeTuple, found TypeArray`), and so is a struct instance
(`expected TypeTuple, found TypeStruct`); Chapter 8 shows the struct read,
which is by name.

A tuple's elements are lazy values like any other: reading one does not
read the others (Chapter 6).

### Tuple types are values

The tuple type is a value in the type language:

```lichen
tuple{Int, Int}
```

```text
tuple{Int, Int}: TypeTuple
```

It is a value whose type is the tuple **kind**, `TypeTuple`. Chapter 10 is
about reading such a type's components; here the point is that the type
of a tuple value and the type value itself are spelled the same way.

An annotation compares the value's type with the type expression:

```lichen
(1, 2) : tuple{Int, Int}
```

```text
(1, 2): tuple{Int, Int}
```

and a disagreement is an ordinary type error, including an arity
mismatch:

```lichen
(1, 2) : tuple{Int, Int, Int}
```

```text
error: expected tuple{Int, Int, Int}, found tuple{Int, Int}
  --> 1:1
   |
 1 | (1, 2) : tuple{Int, Int, Int}
   | ^
```

The `_` placeholder works in tuple types as it does everywhere
(Chapter 5): `(1, 2) : tuple{_, _}` is still `(1, 2) : tuple{Int, Int}`.

## Arrays

An array is a fixed-length group of values of **one** type, written in
brackets:

```lichen
[10, 20, 30]
```

```text
[10, 20, 30]: array{Int, 3}
```

The type is `array{T, n}`: the element type and the length. The length is
a value in the type, not a separate annotation — `array{Int, 3}` is the
array type of three integers, and Chapter 10 shows that the length may be
any expression.

An array is homogeneous. Its elements share one type, so a literal that
mixes types is refused at the element that disagrees:

```lichen
[1, "a"]
```

```text
error: expected Int, found string
  --> 1:5
   |
 1 | [1, "a"]
   |     ^
```

There is no empty array: `[]` is a parse error, and a literal has at least
one element. The same separator rules apply as for tuples.

### Reading an array

An array's element is read with brackets:

```lichen
a = [10, 20, 30]
(a[0], a[2], a[1 + 1])
```

```text
(10, 30, 30): tuple{Int, Int, Int}
```

The index is a full `Int` expression. The array *kind* is required, so a
tuple or a struct is refused by the same rule that refuses an array at a
tuple read:

```lichen
((1, 2)[0], [1, 2](0))
```

```text
error: expected TypeArray, found TypeTuple
  --> 1:2
   |
 1 | ((1, 2)[0], [1, 2](0))
   |  ^
error: expected TypeTuple, found TypeArray
  --> 1:13
   |
 1 | ((1, 2)[0], [1, 2](0))
   |             ^
```

Both reads name the kind they require, and both are refused where the
container is decided.

An index out of bounds is refused rather than answered. When the index is
known while checking, the refusal is at the read:

```lichen
[10, 20, 30][0 - 1]
```

```text
error: index 18446744073709551615 out of bounds (array length 3)
  --> 1:14
   |
 1 | [10, 20, 30][0 - 1]
   |              ^
```

The index is the unsigned `0 - 1`, so it is the largest machine integer —
a long way past the length. When the index is only known when the read is
evaluated, the same check runs then, and the diagnostic is still placed at
the read:

```lichen
at = (a : array{Int, 3}) => i => a[i]
at [10, 20, 30] 5
```

```text
error: index 5 out of bounds (array length 3)
  --> 1:29
   |
 1 | at = (a : array{Int, 3}) => i => a[i]
   |                             ^
```

The parameter's type states the length as `3`, so the read can be checked
at the moment the parameter provides the value. A length that is not yet
decided is pinned when the container arrives (Chapter 10).

An array's elements are lazy (Chapter 6), and its length is not readable
at run time: there is no built-in for it. Iteration over an array is
written by carrying the count in a recursion, which is why Chapter 4's
`apply_n` takes the count as an argument.

## Nested compounds

The two forms nest freely. A tuple may hold arrays and arrays may hold
tuples; a type says the same shape:

```lichen
xs = [(1, 2), (3, 4)]
(xs[0](0), xs[1](1))
```

```text
(1, 4): tuple{Int, Int}
```

```lichen
[[1, 2], [3, 4]][1][0]
```

```text
3: Int
```

A read may be chained only while the kind agrees at each step: the outer
`[1]` selects a row (an array), and the inner `[0]` reads its first
element. A tuple of arrays and an array of tuples are different types, and
`type_of` (Chapter 5) prints both without ambiguity:

```lichen
type_of = x => {t = _; x: t; t}
(type_of ([1, 2], "a"), type_of [(1, 2), (3, 4)])
```

```text
(tuple{array{Int, 2}, string}, array{tuple{Int, Int}, 2}): tuple{TypeTuple, TypeArray}
```

### The reads, in one table

The language has one read form per container kind, and the kind is what
chooses it. There is no runtime dispatch: a wrong kind is a check-time
refusal.

| container | type kind | read | see |
|---|---|---|---|
| tuple | `TypeTuple` | `a(k)` | this chapter |
| array | `TypeArray` | `a[i]` | this chapter |
| struct instance | `TypeStruct` | `a.name` | Chapter 8 |
| table | `TypeTable` | `t <== k` | Chapter 9 |
| set | `SetType` | `x @in s` | Chapter 9 |

The last column is the point: each read is a *different form* for a
*different kind*, and no container is indexable by more than one form.

## Tuples and arrays as values

Both forms are ordinary values: they can be bound, passed, returned, and
stored. A function returning a tuple is how a function returns several
answers:

```lichen
f = x => (x, x + 1)
f 1
```

```text
(1, 2): tuple{Int, Int}
```

and a function returning an array is the same shape with a homogeneous
result:

```lichen
f = x => [x, x, x]
f 7
```

```text
[7, 7, 7]: array{Int, 3}
```

A compound's *type* is fixed by its contents: `[1, 2]` is `array{Int, 2}`
wherever it is used, and `(1, 2)` is `tuple{Int, Int}`. That is why a
tuple with two different types cannot be stored in an array — an array's
elements must agree — while a tuple can hold different types precisely
because it is not homogeneous.

## Summary

- A tuple is `(a, b, …)`, at least one element; `(e,)` is the one-element
  tuple, and there is no empty tuple.
- A tuple's element is read by position, `t(k)`, and the read requires the
  tuple kind.
- An array is `[a, b, …]`, homogeneous, at least one element, and there is no
  empty array.
- An array's element is read by position, `a[i]`, with a runtime check
  when the index is not known while checking; the read requires the array
  kind.
- `tuple{T, …}` and `array{T, n}` are type values of the kinds `TypeTuple`
  and `TypeArray`; `type_of` reads both.
- The read form is chosen by the container's kind; a wrong kind is a
  check-time refusal, never a runtime dispatch.
- Tuples and arrays nest, and both are ordinary values.

## Exercises

1. Write a function that returns the first and last elements of an array as
   a tuple, and read the tuple's components.
2. Explain why `(1, 2)[0]` and `[1, 2](0)` are both refused, and why the
   diagnostics name different kinds.
3. Write `sum3 = t => t(0) + t(1) + t(2)` and apply it to `(1, 2, 3)`.
   What does the type of `t` need to be for the answer to be decided?
4. Write an array of arrays and read an element from the second row.
5. Look up the types of `[1, 2]`, `(1, 2)`, `[(1, 2)]`, and `([1], [2])`
   with `type_of`.
