# Common Programming Concepts

This chapter covers the concepts that every lichen program is built from:
how names are bound and scoped, the four atomic types, the operators and
their precedence, how functions are applied, and how `if` and blocks are
expressions. It is the longest chapter of the first part and the one to
come back to.

## Bindings and scope

### A binding gives a name to a value

A statement of the form `name = expression` binds the name. Every use of
the name afterwards is that same value, and evaluating it once is enough:

```lichen
answer = 1 + 2
answer * 10
```

```text
30: Int
```

A binding is not an assignment: there is no mutable variable to change
later. The name *is* the value's own node in the program's graph, and
every use of the name is that node.

### A block's names are in scope throughout the block

A binding is **block-wide** by default. The names of a block are entered
before any of its values are compiled, so a value may use a binding
written later, and two values may use each other:

```lichen
is_even = n => if n == 0 then 1 else is_odd (n - 1)
is_odd = n => if n == 0 then 0 else is_even (n - 1)
(is_even 10, is_odd 10)
```

```text
(1, 0): tuple{Int, Int}
```

Neither binding needs a forward declaration, and the two functions call
each other freely.

### The one self-reference is a call

A binding's name **is** its value's node, so a value may not use the
binding it binds as an input; that would make the value its own input.
The compiler refuses it by name:

```lichen
x = 1
x = x + 1
```

```text
error: `x` names itself in its own value: …
  --> 2:5
   |
 2 | x = x + 1
   |     ^
```

The message goes on to explain that a block-wide binding's name *is* its
value's own node, so the value would be its own input, and that the
rebinding should be written with `let`, which compiles the value before
the name enters scope.

The one reference a block-wide binding admits is the **recursive call**:
naming the binding as the head of an application inside a lambda body, as
`collatz` did in Chapter 2. That edge is a call rather than a plain value
read, so it is well defined.

### `let` rebinds sequentially

The restrictive form `let name = expression` compiles the value *before*
the name enters scope, so the name is visible only to later statements.
That is how a value is rebound in terms of the previous one:

```lichen
let x = 1
let x = x + 1
x
```

```text
2: Int
```

The second `x` is a new name that shadows the first, and its value reads
the first. `let` and `@cache` (below) may be combined: `@cache let x = …`.

### A repeated binding shadows the whole block

If one block binds the same name twice without `let`, the later binding
wins for *every* use in the block — including uses written before it —
because the names are entered before the values are compiled:

```lichen
x = 1
y = x
x = 2
(y, x)
```

```text
(2, 2): tuple{Int, Int}
```

This is the same block-wide visibility from the other side. When a
rebinding is meant, write `let`; when a name is meant to be new, bind it
in an inner block.

### An inner block shadows locally

A `{ … }` block introduces a scope. Its names shadow outer ones while the
block runs, and the outer names are back afterwards:

```lichen
x = 1
({ x = 2; x }, x)
```

```text
(2, 1): tuple{Int, Int}
```

### `@cache`

`@cache name = expression` marks a binding whose value is meant to survive
a rebuild. The compiler accepts the mark everywhere and it changes
nothing about evaluation; it exists for the incremental toolchain, which
identifies a retained cell by the binding's path from the program root and
can reuse it when an edit does not touch it
([incremental update](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/incremental-update.md)).

### Names

A name is a lowercase or mixed-case identifier (`x`, `id`, `n2`). The
language's keywords are reserved and cannot be names: `Int`, `Float`,
`string`, `Type`, `tuple`, `struct`, `array`, `table`, `set`, `let`,
`if`, `then`, `else`, `return`, `pub`, `int2float`, `float2int`,
`max`, `min`, `exp`, `sqrt`, `floor`, `sin`, `cos`, `ln`, and the `@`-led
words `@loop`, `@assert`, `@shallow`, `@gc`, `@in` and `@cache`. The underscore `_`
is not a name either; it is the inference placeholder of Chapter 5.

## The atomic types

lichen has four atomic types: `Int`, `Float`, `string`, and `Type`. The
first three are the types of literals; `Type` is the type of the type
values themselves.

### `Int`

An `Int` is a machine-sized **unsigned** integer. Literals are
non-negative decimal digits, and a literal that overflows the machine
word is a lex error. Because the type is unsigned, subtraction wraps:

```lichen
(0 - 1, 7 / 2, 7 % 2)
```

```text
(18446744073709551615, 3, 1): tuple{Int, Int, Int}
```

`0 - 1` is the largest representable value, not -1. Division and
remainder are the unsigned ones, and the four order comparisons are
unsigned as well: `(0 - 1) > 1` is `1`. This is one machine word on every
backend, so the interpreter, the CPU JIT and the GPU JIT agree.

There is no unary minus, which is why subtraction is written `0 - 1`.
There is also no `Bool`: a comparison yields `1` for true and `0` for
false, and those two numbers are the only branches the language needs.

```lichen
(1 & 3, 1 | 2, 1 ^ 3, 2 > 1, 2 < 1)
```

```text
(1, 3, 2, 1, 0): tuple{Int, Int, Int, Int, Int}
```

`&`, `|` and `^` are bitwise, which over comparison results makes them
`and`, `or`, and exclusive or. `==` and `!=` are generalized: they
compare any two values of the same type, including type values and
strings.

A division by zero has no value. The interpreter records
`operator.divide_by_zero` and answers the **lazy marker**, which prints as
`none`:

```lichen
1 / 0
```

```text
none: Int
```

The computation is refused, but the program's type is still known —
`Int` — and evaluation continues lazily. The line above is the program's
standard output. The compiler also writes a **report** to standard error,
naming the reason:

```text
operator.divide_by_zero: the divisor of this division evaluated to 0, and there is no value for a division by zero
```

Chapter 6 explains what the marker means; for now, read `none` as "there
is no value here".

### `Float`

A `Float` is a 32-bit floating-point number. Literals have a decimal
point, and the type takes `+ - * /` and the four order comparisons, but
not `%`:

```lichen
(2.0 * 3.0, 2.0 / 4.0, 2.0 - 3.0, 2.0 > 1.0)
```

```text
(6.0, 0.5, -1.0, 1): tuple{Float, Float, Float, Int}
```

The comparison yields an `Int` like every other comparison. A negative
number is printed with a minus sign, but there is no unary minus to
*write* one: `0.0 - 1.0` is the spelling.

`exp`, `sqrt`, `floor`, `sin`, `cos` and `ln` are prefix keywords over one
operand, and `floor` yields a `Float`:

```lichen
(exp 0.0, exp 1.0, sqrt 4.0, floor 1.75, sin 0.0, cos 0.0, ln 1.0)
```

```text
(1.0, 2.718293, 2.0, 1.0, 0.0, 1.0, 0.0): tuple{Float, Float, Float, Float, Float, Float, Float}
```

`exp 1.0` is `2.718293`, not `e`: the language defines its transcendentals
(`exp`, `sqrt`, `sin`, `cos`, `ln`) as its own arithmetic sequences, and
every backend runs the same sequence, so the answer is reproducible
rather than platform-defined.

Floating point division by zero is not refused; it produces the IEEE
values, which the printer spells `inf` and `NaN`:

```lichen
(1.0 / 0.0, 0.0 / 0.0)
```

```text
(inf, NaN): tuple{Float, Float}
```

`Int` and `Float` are unrelated types. No operator mixes them — `1 + 1.5`
is `expected Int, found Float` — and nothing converts silently. The only
two crossings are the prefix keywords `int2float` and `float2int`, each of
which checks its operand against its own source class and yields its
target class:

```lichen
(int2float 3, float2int 3.7)
```

```text
(3.0, 3): tuple{Float, Int}
```

`int2float` rounds to the nearest `Float` (`int2float 16777217` is
`16777216.0`), and `float2int` truncates toward zero. `float2int` is
partial: a `NaN`, an infinity, or a negative number has no `Int` to
truncate to, and answers `none`:

```lichen
float2int (0.0 - 1.0)
```

```text
none: Int
```

The reason is reported on standard error:
`operator.out_of_range: this float2int operand is negative, and this
language's Int is unsigned, and there is no Int for it to truncate
toward`.

### `string`

A `string` is written `"…"` and is immutable and atomic. The quotes are
part of the printed form, there are no escape characters, and a string
literal may span lines:

```lichen
("hello", "line one
line two")
```

```text
("hello", "line one
line two"): tuple{string, string}
```

A string is not a container: `"abc"[0]` is refused (`expected TypeArray,
found Type`), there is no concatenation, and `+` on two strings fails the
numeric class the operators require. Equality works, because it is the
generalized equality over same-typed values:

```lichen
("a" == "a", "a" == "b")
```

```text
(1, 0): tuple{Int, Int}
```

### `Type`

The four atom names are values, and all four have type `Type`:

```lichen
(Int, Float, string, Type)
```

```text
(Int, Float, string, Type): tuple{Type, Type, Type, Type}
```

`Type : Type` holds — the chain closes at `Type` — and type values are
ordinary values: they can be bound, compared, applied, and returned.
Chapter 5 is about that model, and Chapter 10 shows the types that are
not atoms.

## Operators

The operators, from loosest to tightest, are:

| level | forms | associativity |
|---|---|---|
| lambda | `x => e` | right |
| annotations | `e : T`, `e # p`, `e ! p`, `e ? d`, `e ~ "text"` | right |
| function type | `T -> U` | right |
| table read | `t <== k` | left |
| comparison, membership | `<` `>` `<=` `>=` `==` `!=` `@in` | left |
| bitwise or | `\|` | left |
| bitwise exclusive or | `^` | left |
| bitwise and | `&` | left |
| sum | `+` `-` | left |
| product | `*` `/` `%` | left |
| prefix keywords | `@assert`, `@gc`, `int2float`, `float2int`, `max`, `min`, `exp`, `sqrt`, `floor`, `sin`, `cos`, `ln` | — |
| application | `f x` | left |
| postfix reads | `a[i]`, `a.name`, `X{n}`, `X::a`, `a(k)` | left |
| atoms | literals, names, `( … )`, `[ … ]`, `{ … }`, the type forms | — |

A few readings of the table are worth checking against the compiler:

```lichen
(1 + 2 * 3, 1 & 3 == 1, 1 | 2 ^ 3 & 1, int2float 1 + 2.0)
```

```text
(7, 1, 3, 3.0): tuple{Int, Int, Int, Float}
```

`*` binds tighter than `+`; a comparison is looser than the bitwise
operators, so `1 & 3 == 1` is `(1 & 3) == 1`; the three bitwise levels
nest `&` inside `^` inside `|`; and a prefix keyword binds tighter than
any binary operator, so `int2float 1 + 2.0` converts the `1` and then
adds.

Application is tighter than everything except the postfix reads.
`f 1 * 2` applies `f` to `1` and then multiplies. Left associativity is
uniform at each binary level:

```lichen
1 - 2 - 3
```

```text
18446744073709551612: Int
```

That is `(1 - 2) - 3`, with both subtractions wrapping.

Prefix keywords take a fixed shape. `@assert`, `@gc`, `int2float`,
`float2int`, `exp`, `sqrt`, `floor`, `sin`, `cos` and `ln` each take one
**application**; `max` and `min` each take two **atoms**. So `max a b` is
the maximum of the atoms `a` and `b`, and `max (a + 1) b` needs its
parentheses.

```lichen
f = x => x + 1
(f 1 * 2, max 1 2 * 3)
```

```text
(4, 6): tuple{Int, Int}
```

## Calling functions

A function is applied by writing it before its argument, separated by
whitespace: `f x`. Application is left-associative, and a function that
returns a function is applied again: `add 1 2` is `(add 1) 2`.

```lichen
add = x => y => x + y
add 1 2
```

```text
3: Int
```

A function takes exactly **one** argument; multiple parameters are written
as nested lambdas, which is called currying. Applying one argument to a
two-argument function is partial application, and it produces a usable
function:

```lichen
add = x => y => x + y
add1 = add 1
add1 2
```

```text
3: Int
```

A function can be passed to another function and returned from one:

```lichen
twice = f => x => f (f x)
inc = x => x + 1
twice inc 5
```

```text
7: Int
```

There is no multi-parameter lambda: `(a, b) => e` does not parse, because
`(a, b)` is always a tuple *value*. To pass several values, pass a tuple
and read it by position. A tuple parameter needs an annotation when the
body does arithmetic on its positions:

```lichen
addpair = (p : tuple{Int, Int}) => p(0) + p(1)
addpair (1, 2)
```

```text
3: Int
```

Without the annotation, the tuple's element types stay open, and the
answer prints with the `raw` mark (`raw 3: raw[?a, ?b]`): the value is
right, but the arithmetic's class was not decided until run time.
Chapter 5 explains why, and Chapter 10 shows what the annotation writes.

Parentheses group, and they also make a difference in application:

```lichen
add = x => y => x + y
(add 1 (2 + 3), add (1 + 2) 3)
```

```text
(6, 6): tuple{Int, Int}
```

An expression that is not an atom has to be parenthesized to become an
argument: `f x + 1` is `(f x) + 1`, while `f (x + 1)` passes `x + 1`.

## Expressions: `if` and blocks

### `if` is an expression

`if condition then a else b` is an expression like any other, and it can
appear anywhere a value can. Its condition is an **index**: the desugared
form reads position `0` or `1` of a two-element array, so the condition
must be an `Int` in that range.

```lichen
if 1 < 2 then 10 else 20
```

```text
10: Int
```

A comparison answers exactly `0` or `1`, which is why a comparison is the
natural condition. Any other `Int` fails:

```lichen
if 2 then 1 else 2
```

```text
error: index 2 out of bounds (array length 2)
  --> 1:4
   |
 1 | if 2 then 1 else 2
   |    ^
```

and a non-`Int` condition is refused for not being an index at all:
`if 1.5 then 1 else 2` reports `this value is not an index — an element can
only be read by position`.

Because the branches are an array's elements, they must have the same
type, and the untaken branch is never evaluated.

### Blocks are expressions

A `{ … }` block is an expression whose value is its tail. Its names are
local, it compiles to its tail's node, and it can be used anywhere an
expression can:

```lichen
{ a = 5; a * 2 }
```

```text
10: Int
```

An explicit `return expression` anywhere in the block pins the block's
value, so a value can be stated before the statements that follow it:

```lichen
{ a = 1; return a + 1 }
```

```text
2: Int
```

The block's bindings are local, and its value is the returned expression
rather than its final statement.

Because a block is an expression, it can be an argument, with no
parentheses needed:

```lichen
f = x => x + 1
f { a = 1; a }
```

```text
2: Int
```

A block whose last statement is a binding, and which has no `return`,
returns an anonymous **struct** of its bindings rather than a value.
Chapter 8 covers that form.

## Summary

- `name = expr` binds block-wide; a block's names are visible throughout it,
  forward and backward. `let name = expr` binds sequentially.
- A binding may name itself only as the head of a call.
- `Int` is unsigned and wraps; comparisons yield `0`/`1`; there is no
  `Bool` and no unary minus.
- `Float` is `f32` with its own operators and the two `int2float` /
  `float2int` crossings; `string` is atomic; the four atom names are
  values of type `Type`.
- Prefix keywords bind tighter than binary operators; application is
  tighter still; comparisons are looser than the bitwise operators.
- A function takes one argument; several are passed as a tuple or as
  nested lambdas.
- `if` is an index into two branches; a block is an expression whose value
  is its tail or its `return`.

## Exercises

1. Write a program that prints `(0 - 1 > 1, 0 - 1 < 1)` and explain both
   values in terms of unsigned arithmetic.
2. Write `average = a => b => (int2float a + int2float b) / 2.0` and
   evaluate it at `(3, 4)`. Why is the conversion needed on both operands?
3. Write a curried function `compose = f => g => x => f (g x)` and use it to
   compose `inc` with itself. (The standard library has one.)
4. Explain why `add 1 * 2` is not `add (1 * 2)`, and check your answer
   with the compiler.
5. Write a block expression that binds two numbers and returns their
   difference, and use it as the argument of a function.
