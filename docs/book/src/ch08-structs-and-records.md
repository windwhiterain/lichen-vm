# Structs and Records

A tuple groups values by **position**; a struct groups them by **name**.
That difference is what the language's nominal types are built from: a
struct's identity is its own name table, and two structs written apart
are two types however alike they look. This chapter is about struct
constructors, their instances, their reads, and the form that builds a
struct from a block of bindings — the **record**.

## A struct constructor

A **constructor** is a function that returns a fresh requirement when it
is applied, and a struct's requirement is written with a leading `.` before
each field's name, each field carrying a **requirement** — the value the
field must hold:

```lichen
S := _x => {.x @any : Int, .y @any : Type}
S
```

```text
?a => Struct: ?a => Struct
```

The constructor is an ordinary function, so it prints as one; what it
returns is the struct value, and that value's type is the shared layout type
`Struct` (Chapter 7's read table lists what that layout enables).
`@any : Int` is the requirement "any `Int` will do"; a field may require a
concrete value instead, as Chapter 10's pinned slots do.

The fields are named; there is no positional struct definition.

## A struct's identity is its own name table

Each written occurrence of a struct constructor is a **new** type. The
occurrence carries a name table, and that table is the identity: two
constructors written apart do not unify, however identical their fields
look:

```lichen
S1 := _x => {.x @any : Int}
S2 := _x => {.x @any : Int}
a := S1{.x 1}
b := S2{.x 2}
a = b
```

```text
error: expected Table, found Table
  --> 3:6
   |
 3 | a := S1{.x 1}
   |      ^
```

The message names the thing that differs — the **name table** — because
the table is where the fields' names live, and a table compares by
identity rather than by content. That is what makes a struct nominal: a
struct's type is not its field list, it is the occurrence.

Bind one occurrence and the binding is the type, so one identity can be
used in many places:

```lichen
S := _x => {.x @any : Int}
a := S{.x 1}
b := S{.x 2}
(a, b)
```

```text
((1,), (2,)): Tuple
```

Both instances are of the *same* `S`, so they share a tuple. This is what
"nominal" means, and it is why the language can tell a struct apart from
a tuple of the same fields:

```lichen
S := _x => {.x @any : Int}
a := S{.x 1}
a : Tuple
```

```text
error: expected Tuple, found Struct
  --> 2:6
   |
 2 | a := S{.x 1}
   |      ^
```

## Instantiating a struct

An instance applies the constructor **once** and fills the struct value it
returns, written after the constructor with the same `.name` spelling:

```lichen
S := _x => {.x @any : Int, .y @any : Type}
S{.x 1, .y Int}
```

```text
(1, Int): Struct
```

There is no second application. `S` is a constructor, `S{…}` applies it to a
placeholder and fills what comes back, and `(S _){…}` would apply the
returned struct value a second time, which is not the constructor form.

The instance's fields are checked against the requirements the constructor
returned as they are given, so a value that does not meet the requirement is
refused at the field:

```lichen
S := _x => {.x @any : Int}
S{.x "s"}
```

```text
error: expected Int, found string
  --> 2:6
   |
 2 | S{.x "s"}
   |      ^
```

A field left unsupplied is refused, naming the field:

```lichen
S := _x => {.x @any : Int, .y @any : Int}
S{.x 1}
```

```text
error: missing field y in the struct type Struct
  --> 2:1
   |
 2 | S{.x 1}
   | ^
```

and an unknown name is refused at the field:

```lichen
S := _x => {.x @any : Int}
S{.y 1}
```

```text
error: no field named y in the struct type Struct
  --> 2:6
   |
 2 | S{.y 1}
   |      ^
```

A struct with no fields is written `{}`, and its instance is `S{}`:

```lichen
S := _x => {}
S{}
```

```text
(): Struct
```

## Reading fields

A field is read by name with a dot:

```lichen
S := _x => {.x @any : Int, .y @any : Type}
s := S{.x 1, .y Int}
(s.x, s.y)
```

```text
(1, Int): Tuple
```

The read resolves the name through the **value's own name table**, and
its type is the field's. A miss is a diagnostic at the read, with a
suggestion when a field is close:

```lichen
S := _x => {.x @any : Int, .y @any : Int}
a := S{.x 1, .y 2}
a.xx
```

```text
error: no field with this name in the struct type Struct, did you mean 'x'?
  --> 3:1
   |
 3 | a.xx
   | ^
```

The container's layout must be `Struct`, so a tuple is refused by the
same rule as the positional reads:

```lichen
(10, 20).a
```

```text
error: expected Struct, found Tuple
  --> 1:1
   |
 1 | (10, 20).a
   | ^
```

> **A read of a call's result.** A call's result is that call's own instance
> of the callee's signature, so a read from it — `(f 1).x` — is checked
> whenever `f`'s function type is decided. When the callee's type is not a
> function type at the call, the result stays undecided and the read is
> refused (`this field read cannot be checked: the container's type is not
> decided here — annotate the container's type`); state the layout on the
> call expression then (`s = (f 1 : Struct); s.x`, or `(f 1 : Struct).x`).
> The restriction is the read's, not the value's.

There is no `::` form: a struct's field names live in the **value**, and a
type value has no fields to read. Read the field's value and take its
type instead — `type_of s.x` (Chapter 10).

## Records: a struct from a block

A block whose last statement is a binding, with no tail and no `return`,
is a **record**: its value is an anonymous struct whose fields are the
bindings:

```lichen
{ a := 1; b := 2 }
```

```text
(1, 2): Struct
```

The value is printed as the positional tuple of the field values, and the
field names live in its own name table. The fields follow these rules:

- a `name := expr` binding is a field;
- a `let` binding is never a field (it is block-local by construction);
- a bare expression is never a field (it is checked, and discarded);
- a `pub` mark selects: when any statement is `pub`, only the `pub` ones are
  fields.

```lichen
{ pub a := 1; b := 2 }
```

```text
(1,): Struct
```

```lichen
{ let a := 1; b := 2 }
```

```text
(2,): Struct
```

```lichen
{ 1; x := 2 }
```

```text
(2,): Struct
```

A record's fields are read like any struct's, once the record is bound:

```lichen
r := { y := 1; z := 2 }
(r.y, r.z)
```

```text
(1, 2): Tuple
```

There is no empty record: `{}` is a parse error, exactly because a block
needs a statement to have a value.

## Record programs: a file as a struct

A program is a block (Chapter 1), so a file that ends in its bindings is a
record program: its value is the anonymous struct of the file's bindings.
That is how a lichen library file exports its names.

```lichen
succ := x => x + 1
double := (x : Int) => x + x
```

```text
(Int => Int, Int => Int): Struct
```

A block that binds functions and is itself bound gives the same thing as
a value:

```lichen
m := { succ := x => x + 1; double := (x : Int) => x + x }
m.double 3
```

```text
6: Int
```

and `m` prints as a tuple of the exported functions with the struct
layout. Chapter 15 builds on this form: an imported file is a package
whose value is its record.

## Summary

- A struct **constructor** is a function that returns a fresh requirement
  when applied: `S := _x => {.x @any : Int}`. Each field is named, and each
  written occurrence is a fresh identity.
- The identity is the value's own name table, so two constructors written
  apart do not unify; bind a constructor to use one identity in many places.
- An instance is `S{.x 1, .y Int}`: it applies the constructor once and
  fills the struct value it returns. Every field must be supplied and must
  meet the requirement.
- Fields are read by name, `s.x`; the container's layout must be
  `Struct`, and a miss is diagnosed with a suggestion when one is close.
- There is no `::` read: the names live in the value, so read the value
  and take its type.
- A block ending in bindings is a record: a struct whose fields are the
  bindings, selected by `let`, `pub`, and whether the statement is a
  binding. A program is a block, so a file ending in bindings is a record
  program.
- A struct value prints as its positional tuple with the `Struct` layout.

## Exercises

1. Define a constructor `S := _x => {.x @any : Int, .y @any : Int}`, build
   an instance, and read both fields.
2. Explain why two constructors written apart do not unify, in terms of the
   name table.
3. Write a record block with a `let` and a `pub` binding and list which
   fields the record has.
4. Write a record program that exports two functions and read one of them
   through a block-bound name.
5. Write `mk = (x : Int) => S{.y x + 1, .z x + 2}` for a constructor `S` and
   read the fields after binding the call's result. What does the same
   read answer without the binding?
