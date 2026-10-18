# A First Program: the Collatz Conjecture

Rust's book starts with a guessing game. lichen has no input and no
console, so its first real program is a game against mathematics instead:
the Collatz conjecture.

The rule is simple. Start with a positive integer `n`. If `n` is `1`,
stop. If `n` is even, halve it. If `n` is odd, replace it with `3n + 1`.
The conjecture is that every starting value eventually reaches `1`;
nobody knows whether that is true, but it has been checked for an
enormous range of numbers. The program will count the steps.

```lichen
collatz = n => if n == 1 then 0 else 1 + collatz [n / 2, 3 * n + 1][n % 2]
collatz 27
```

```text
111: Int
```

The program takes 111 steps to get from 27 to 1. Let's take it apart.

## A binding of a function

`collatz = n => …` binds the name `collatz` to a function of one
parameter, `n`. There is no `fn` keyword, no return type, and no separate
function declaration: a function is an expression (`n => e`), and binding
it to a name is all it takes.

`n` is a normal name in the body. The body is everything after `=>`, up
to the end of the expression.

## There are no booleans

`n == 1` is a comparison. lichen has no `Bool` type: a comparison answers
`1` for true and `0` for false, and both are `Int`s like the operands.
Every operator in the language yields an `Int` except the floating-point
forms, which yield `Float` (Chapter 3). Programs branch by indexing on
that number.

## `if` is an expression over an index

`if cond then a else b` is an expression like any other; it is written in
the language as a shorthand for

```text
[b, a][cond]
```

That is: build the two branches as a two-element array and index it with
the condition. The order in the desugaring is `[else, then]`, because the
condition is `0` for false and `1` for true. Two consequences matter:

- The untaken branch is **never evaluated**, because an array's elements
  are lazy. `if 1 == 1 then 1 else 1 / 0` prints `1: Int`: position 0 of
  the desugared array — the division by zero — is never read.
- Both branches must have the **same type**, because they are elements of
  one array. A branch that disagrees is an array-element mismatch, not a
  special `if` error.

## The next step is an array lookup

`[n / 2, 3 * n + 1]` builds both possible successors of `n`: the even
successor in position 0 and the odd successor in position 1. Then
`[n % 2]` indexes it: `n % 2` is `0` for an even `n` and `1` for an odd
one. The glued brackets are the array read; a space before `[` would
apply the array as a function instead (Chapter 3).

`n / 2` is integer division, and `Int` is a machine-sized **unsigned**
integer, so `7 / 2` is `3` and `n % 2` is exactly the parity.

## The call is recursion

Inside its own body, `collatz` names itself — as the head of an
application. That is the one way a block-wide binding may refer to
itself: as a **call**, never as a plain value. lichen reads the call as a
recursive function, and the recursion stops when `n` reaches `1`.

Why only a call? Because a binding's name *is* its value's own node in
the program's graph. A value that named itself as a value would be its
own input; a call is an edge through a function, and it is well defined.
Chapter 3 spells the rule out and Chapter 4 explains what a function is.

## Counting the steps

The `1 +` in front of the recursive call adds the step just taken to the
count of steps the rest of the sequence needs. The base case, `n == 1`,
contributes `0` steps.

Try a few values:

```lichen
collatz = n => if n == 1 then 0 else 1 + collatz [n / 2, 3 * n + 1][n % 2]
(collatz 1, collatz 6, collatz 7)
```

```text
(0, 8, 16): tuple{Int, Int, Int}
```

`1` needs no steps; `6` takes 8; `7` takes 16.

## What else does the sequence do?

Counting is one thing; the sequence's **peak** is another. `peak n` is
the largest value the sequence reaches on its way to 1:

```lichen
peak = n => if n == 1 then 1 else max n (peak [n / 2, 3 * n + 1][n % 2])
peak 27
```

```text
9232: Int
```

`max` is a two-operand prefix keyword: `max a b` yields the larger of two
atoms, and it takes **atoms**, so the second operand is the parenthesized
recursive call. `max` yields the operands' own class, so `max` works on
integers and on floats, but never on a mixture.

> **Note**: bindings in a block are visible throughout the block, so a
> later binding can use an earlier one — and a binding can use itself, but
> only as a call. That is why `peak` can be defined in terms of `peak`
> without any forward declaration.

## Three kinds of mistakes

Even a small program can fail in three different ways, and the compiler
reports each differently.

**A type error.** Call `collatz` with a string:

```lichen
collatz = n => if n == 1 then 0 else 1 + collatz [n / 2, 3 * n + 1][n % 2]
collatz "twenty-seven"
```

```text
error: expected Int, found string
  --> 2:9
   |
 2 | collatz "twenty-seven"
   |         ^
```

The message names what the function's parameter requires and what the
argument is. The header shows `line:column` — line 2, column 9 — and the
caret points at the argument. A type error is found before any value is
printed, so the program never produces output.

**A broken promise.** `@assert e` requires `e` to evaluate to `1`, and
fails the program if it does not:

```lichen
n = 0
@assert (n > 0)
n
```

```text
error: assertion failed: expected 1, found 0
  --> 2:1
   |
 2 | @assert (n > 0)
   | ^
```

`@assert` is a prefix operator, and it binds tighter than comparison but
looser than application: `@assert n > 0` asserts `n`, not the comparison,
and then compares its value with `0`. Parenthesize the condition.
Chapter 14 covers assertions and the rest of the diagnostic vocabulary.

**Non-termination.** `collatz 0` never reaches `1`, so it never finishes:

```lichen
collatz = n => if n == 1 then 0 else 1 + collatz [n / 2, 3 * n + 1][n % 2]
collatz 0
```

```text
error: the checker stopped on its apply-depth budget: nested applications exceeded 500 levels. The program may still terminate when run.
  --> 2:1
   |
 2 | collatz 0
   | ^
```

The checker evaluates the program in order to type it, so it stops a
recursion that does not stop for it at 500 nested applications. Read the
second sentence carefully: the checker does not claim the program would
fail at run time. It claims only that it will not keep unfolding the
program at check time. Chapter 6 explains what evaluation does and when,
and why a non-terminating program is the program's own business.

## The answer has a type, and so does the function

The program printed `111: Int`: the count is an `Int`. The function's own
answer is a function value, and its two halves say so:

```lichen
collatz = n => if n == 1 then 0 else 1 + collatz [n / 2, 3 * n + 1][n % 2]
collatz
```

```text
Int => Int: Int -> Int
```

The type half is `Int -> Int`: a function from `Int` to `Int`. The value
half is printed as `Int => Int` — the same function, shown as the
signature it implements. A function with no decided parameter type prints
as `?a => ?a: ?a -> ?a`; the `?a` names an open cell, and Chapter 5
explains those. An annotated function states its signature up front:

```lichen
collatz = (n => if n == 1 then 0 else 1 + collatz [n / 2, 3 * n + 1][n % 2]) : Int -> Int
collatz
```

```text
Int => Int: Int -> Int
```

Same answer here — the recursion had already decided the parameter's type
— but an annotation is how a function with no such use states its
signature. Chapter 12 explains why a recursive function in particular may
need one.

## Making it an example

The repository's `examples/` directory is a checked suite: every file
declares the output it should produce, and the suite compares the
declaration with what the program prints. A program in that form begins
with a header:

```lichen
---
  order = "02"
  output = "111: Int"
  doc = "Count the Collatz steps from 27 to 1."
---
collatz = n => if n == 1 then 0 else 1 + collatz [n / 2, 3 * n + 1][n % 2]
collatz 27
```

`output` is the promised line, `order` places the file in the suite, and
`doc` is where the explanation lives, because the language has no
comments. Chapter 16 shows the suite and how to add to it.

## Summary

- A function is a value: `n => e`, bound with `=`.
- A comparison yields `0` or `1`; `if` is the lazy index of a two-element
  array, so only the taken branch runs.
- `[a, b]` builds an array, `[i]` reads position `i`, and `n / 2` is
  unsigned integer division.
- A binding may name itself as the head of a call, which is recursion.
- `@assert e` fails the program unless `e` evaluates to `1`.
- The compiler prints `value: type`, and a function's value half shows its
  signature.

## Exercises

1. What is `collatz 97`? Confirm your answer with the compiler.
2. Write `odd_steps n`: the number of *odd* values the sequence passes
   through on its way to 1. (Hint: count `n % 2` at each step.)
3. Explain why `if 1 == 0 then 1 else 2` prints `2: Int`, and why
   `[2, 1][1 == 0]` prints the same thing.
4. Write a program that prints `collatz` itself, and read both halves of
   its output.
5. Give `peak` an annotation of type `Int -> Int` and confirm the output
   does not change.
