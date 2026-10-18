# Structs and Records

A tuple groups values by **position**; a struct groups them by **name**.
That difference buys the language its nominal types: a struct type is a
new type, not a shape, and two struct types with the same fields are
still different types. This chapter is about struct types, their
instances, their reads, and the form that builds a struct from a block of
bindings — the **record**.

## A struct type

A struct type is written with a leading `.` before each field's name:

```lichen
struct{.x Int, .y Type}
```

```text
struct{.x Int, .y Type}: TypeStruct
```

The fields are named; there is no positional struct definition. A field
without a name is refused by name:

```lichen
struct{Int}
```

```text
error: a struct field must be named — write `struct{.name T}`
  --> 1:8
   |
 1 | struct{Int}
   |        ^
```

The type is a value whose type is the struct **kind**, `TypeStruct`
(Chapter 7's read table lists what that kind enables).

## A struct type is nominal

Each occurrence of the syntax is a **new** type. The occurrence gets a
fresh nominal identity, which the printer shows in diagnostics as a
`#` suffix. So two occurrences with the same fields do not unify:

```lichen
S = struct{.f Int}
S(.f 1) : struct{.f Int}
```

```text
error: expected struct{.f Int}#18, found struct{.f Int}#11
  --> 2:1
   |
 2 | S(.f 1) : struct{.f Int}
   | ^
```

The two types have the same fields and the same spelling, and they are
different types because they have different identities. Bind one
occurrence and the binding is the type:

```lichen
B = struct{.f Int}
[B(.f 1), B(.f 2)]
```

```text
[(1,), (2,)]: array{struct{.f Int}, 2}
```

Now both instances are of the *same* `B`, so they share an array. This is
what "nominal" means: a struct type's identity is its occurrence, not its
field list, and binding the type is how one identity is used in many
places.

A struct is not a tuple with names. A tuple type has no nominal identity,
and a struct instance does not unify with a tuple of its fields:

```lichen
S = struct{.a Int, .b Int}
S(.a 1, .b 2) : tuple{Int, Int}
```

```text
error: expected tuple{Int, Int}, found struct{.a Int, .b Int}#11
  --> 2:1
   |
 2 | S(.a 1, .b 2) : tuple{Int, Int}
   | ^
```

## Instantiating a struct

An instance is built by applying the struct type to its field values, in
the definition's order:

```lichen
S = struct{.x Int, .y Type}
S(.x 1, .y Int)
```

```text
(1, Int): struct{.x Int, .y Type}
```

The arguments may be named, and named arguments are reordered to the
definition's order, so the same instance can be written in any order:

```lichen
S = struct{.x Int, .y Type}
S(.y Int, .x 1)
```

```text
(1, Int): struct{.x Int, .y Type}
```

A positional argument fills the lowest-numbered field not already claimed
by a name, and the two forms mix. The parenthesis that builds an instance
is the **glued** one: `S(...)` with no space. A spaced `S (...)` applies a
function (Chapter 3).

A struct with one field is instantiated with the trailing separator,
`S(1,)`, because a glued parenthesis around one bare expression is the
tuple read (`S(1)` reads position 1 of the *type value*, and is refused
with `expected TypeTuple, found Type`). A struct with no fields is
`struct{}`, and its instance is written `E()`:

```lichen
E = struct{}
E()
```

```text
(): struct{}
```

The missing or extra argument is refused at the instance. A field left
unsupplied is refused, naming the field and the struct type:

```lichen
S = struct{.x Int, .y Type}
S(.x 1)
```

```text
error: missing field y in the struct type struct{.x Int, .y Type}#11
  --> 2:1
   |
 2 | S(.x 1)
   | ^
```

and an unknown name is refused at the argument:

```lichen
S = struct{.x Int, .y Type}
S(.z 1, .x 2)
```

```text
error: no field named z in the struct type struct{.x Int, .y Type}#11
  --> 2:6
   |
 2 | S(.z 1, .x 2)
   |      ^
```

Instances of a struct with a pinned field (`struct{.a 5 : Int}`, Chapter
10) are checked against the pin as well, so the field's value is part of
the type.

## Reading fields

A field is read by name with a dot:

```lichen
S = struct{.x Int, .y Type}
s = S(.x 1, .y Int)
(s.x, s.y)
```

```text
(1, Int): tuple{Int, Type}
```

The read resolves the name through the struct type's name table, and its
type is the field's. A miss is a diagnostic at the read, with a suggestion
when a field is close:

```lichen
S = struct{.x Int, .y Int}
S(.x 1, .y 2).xx
```

```text
error: no field with this name in the struct type struct{.x Int, .y Int}#11, did you mean 'x'?
  --> 2:1
   |
 2 | S(.x 1, .y 2).xx
   | ^
```

The container's kind must be a struct kind, so a tuple is refused by the
same rule as the positional reads:

```lichen
(10, 20).a
```

```text
error: expected TypeStruct, found TypeTuple
  --> 1:1
   |
 1 | (10, 20).a
   | ^
```

> **A read of a call's result.** A call's result is only as decided as the
> call expression's type: reading a field from it directly — `(f 1).y` — is
> refused where that type is not decided (`this field read cannot be
> checked: the container's type is not decided here — annotate the
> container's type`). Binding the result first does not decide it either;
> state the type on the call's result (`s = (f 1 : S); s.y`, or
> `(f 1 : S).y`). The restriction is the read's, not the value's.

## Field types as values

A struct **type value** can be read by name with `::`. The read yields the
field's *type* as a value:

```lichen
S = struct{.x Int, .y Type}
S::x
```

```text
Int: Type
```

This is the type-level sibling of `.x`: where `.x` reads a field value
from an instance, `::x` reads a field type from the type value. The
container for `::` must be a struct **type value** — a struct instance is
refused — and the read yields the field type's kind, here `Type` for a
scalar field. Chapter 10 uses this form to read a type apart.

## Records: a struct from a block

A block whose last statement is a binding, with no tail and no `return`,
is a **record**: its value is an anonymous struct whose fields are the
bindings:

```lichen
{ a = 1; b = 2 }
```

```text
(1, 2): struct{.a Int, .b Int}
```

The value is printed as the positional tuple of the field values, and the
type carries the names. The fields follow these rules:

- a `name = expr` binding is a field;
- a `let` binding is never a field (it is block-local by construction);
- a bare expression is never a field (it is checked, and discarded);
- a `pub` mark selects: when any statement is `pub`, only the `pub` ones are
  fields.

```lichen
{ pub a = 1; b = 2 }
```

```text
(1,): struct{.a Int}
```

```lichen
{ let a = 1; b = 2 }
```

```text
(2,): struct{.b Int}
```

```lichen
{ 1; x = 2 }
```

```text
(2,): struct{.x Int}
```

A record's fields are read like any struct's, once the record is bound:

```lichen
r = { y = 1; z = 2 }
(r.y, r.z)
```

```text
(1, 2): tuple{Int, Int}
```

There is no empty record: `{}` is a parse error, exactly because a block
needs a statement to have a value.

## Record programs: a file as a struct

A program is a block (Chapter 1), so a file that ends in its bindings is a
record program: its value is the anonymous struct of the file's bindings.
That is how a lichen library file exports its names.

```lichen
succ = x => x + 1
double = (x : Int) => x + x
```

```text
(Int => Int, Int => Int): struct{.succ Int -> Int, .double Int -> Int}
```

A block that binds functions and is itself bound gives the same thing as
a value:

```lichen
m = { succ = x => x + 1; double = (x : Int) => x + x }
m.double 3
```

```text
6: Int
```

and `m` prints as a tuple of the exported functions with the struct type.
Chapter 15 builds on this form: an imported file is a package whose value
is its record.

## Summary

- A struct type is `struct{.name T, …}`; fields are named, and each
  occurrence of the syntax is a fresh nominal type.
- Bind a struct type to use one identity in many places; a struct type
  never unifies with a tuple of the same fields.
- An instance is `S(.x 1, .y Int)`; arguments may be named and are
  reordered; a one-field instance is `S(1,)`, an empty one is `E()`.
- Fields are read by name, `s.x`; the container's kind must be a struct
  kind, and a miss is diagnosed with a suggestion when one is close.
- `S::x` reads a field's type as a value from a struct type value.
- A block ending in bindings is a record: a struct whose fields are the
  bindings, selected by `let`, `pub`, and whether the statement is a
  binding. A program is a block, so a file ending in bindings is a record
  program.
- A struct instance prints as its positional tuple with the struct type.

## Exercises

1. Define `Point = struct{.x Int, .y Int}`, build a point, and read both
   fields. Annotate the instance with its type and confirm the answer.
2. Explain why `S(.a 1, .b 2) : tuple{Int, Int}` is refused, in terms of
   nominal identity.
3. Write a record block with a `let` and a `pub` binding and list which
   fields the record has.
4. Write a record program that exports two functions and read one of them
   through a block-bound name.
5. Write `mk = (x : Int) => S(.y x + 1, .z x + 2)` for a struct `S` and read
   the fields after binding the call's result. What does the same read
   answer without the binding?
