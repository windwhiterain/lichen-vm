# Laziness, Sharing, and Scope

Two questions about any language's evaluation are *what* is computed and
*when*. lichen's answers are unusual: a program computes only the values
its answer needs, and it computes them as late as the answer allows. This
chapter is about that model — and about the one place where it does not
apply, which is checking.

## What is evaluated, and when

A lichen program is a graph. A binding's name is a node; an array element
is a node; a function body is an expression. A node's value may not be
computed yet — it may be a **thunk**, an expression waiting to be
evaluated. Reading the node computes it, writes the result into the node,
and every later read gets the same result.

That is laziness: a value is computed on demand, at most once, and every
use shares the result. It is why lichen can have a value that is not
fully there yet, and why the language can express structures that never
end.

## The laziness is in the values

The places where a reading program does *not* need a value are the same
places where lichen does not compute it:

- An array element is computed when it is read: `[1 / 0, 2][1]` is `2`,
  and the unread element's refusal is not in the way.
- A tuple element is computed when it is read: `t = (1 / 0, 2); t(1)` is
  `2`.
- An `if`'s untaken branch is an unread array element.
- A function body is computed when the function is applied, and a branch
  that selects by index computes only the selected element — Chapter 4's
  `fib` is the example: it terminates at `0` because the sum it does not
  select is never evaluated.

```lichen
((1 / 0) + 1, [1 / 0, 2][1])
```

```text
(none, 2): tuple{Int, Int}
```

The first element is `none`: a refused computation's value (below). The
second is `2`, because the array's first element was never read.

## Checks are not lazy

The checker's job is to decide types, and it does that by evaluating the
program. So while the *values* are lazy, the *checker* evaluates the
program far enough to type every expression — including expressions whose
values would never be used. An error that is found while checking is
reported wherever it is written:

```lichen
f = x => [1, 2][5]
```

```text
error: index 5 out of bounds (array length 2)
  --> 1:17
   |
 1 | f = x => [1, 2][5]
   |                 ^
```

The index is checked even though `f` is never applied and its body is
never evaluated by the program. The same holds for an index inside an
untaken branch: the branch is a value the program would not need, but it
is still an expression the checker types.

So there are two different things that can go wrong, and they behave
differently:

- a **check** that fails — an out-of-bounds index, a type mismatch, a
  resolve error — is a diagnostic and stops the build;
- a **computation that is refused** — division by zero, a conversion with
  no answer — produces the lazy marker `none`, and evaluation continues.

The rest of this chapter is about the second kind, and about the
structures that need the first kind to be held back.

## Infinite structures and `@shallow`

An array's elements share one type, and the checker evaluates them to
find it. A definition that puts a call to itself in an array would
therefore be unfolded forever. The marker `@shallow` says the element's
value stays lazy in the checker's evaluation:

```lichen
nats = n => [n, @shallow nats (n + 1)]
ns = nats 0
(ns(1)(0), ns(1)(1)(0), ns(1)(1)(1)(0), ns(1)(1)(1)(1)(0))
```

```text
(1, 2, 3, 4): tuple{Int, Int, Int, Int}
```

`nats 0` is the array `[0, @shallow nats 1]`. Reading `ns(1)` gets the
element at position 1 — the marked call — and reading `(0)` of that gets
its first element, `1`; one more `(1)(0)` pair walks one step further.
The stream has no end, and every read walks only as far as it needs.

Without the marker the same definition does not check: the checker
unfolds the element, and the diagnostic repeats at every level it
reaches. `@shallow` is what makes an infinite list expressible, and it
appears only inside array literals.

The marker has two spellings. The bare `@shallow` keeps the whole value
subtree lazy, and `@shallow(n)` keeps the value slot shallow at each of
the first `n` levels of the element's type spine. The depth is a bound of
at least `1`; a depth of `0` marks no level, and is refused:

```text
error: a `@shallow` depth of 0 marks no level — …
  --> 1:2
   |
 1 | [@shallow(0) 5]
   |  ^
```

The mark is a **value** mark, not a type mark: it says the element's
value may stay unevaluated, not that its type becomes unknown. The
precise rules for the two spellings are in the
[language spec](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/language-spec.md) §2.

## The lazy marker `none`

Some computations have no value. `1 / 0` is not an error the program can
recover from; it is a division with no answer, and the language's answer
is a marker:

```lichen
1 / 0
```

```text
none: Int
```

The marker carries a type, `Int`, because the division's type is known
even though its value is not. It also carries a *reason* — here
`operator.divide_by_zero` — which the compiler reports on standard error:

```text
operator.divide_by_zero: the divisor of this division evaluated to 0, and there is no value for a division by zero
```

A program that prints `none` did not fail; it printed the marker.

The marker flows through computations that read it:

```lichen
(1 / 0) + 1
```

```text
none: Int
```

and it is what an unfilled hole prints (Chapter 5's `_ : Int`):

```lichen
(_ : Int)
```

```text
none: Int
```

A refusal is not a diagnostic. The program above exits successfully; only
a failed *check* stops the build.

## Sharing

A binding is sharing, not copying. `x = 2 + 3` binds the name `x` to one
node, and every use of `x` reads that node; the value is computed once
and every use sees it. The same is true of a function's captured
environment: a closure points at the enclosing block's nodes rather than
duplicating them.

This is the property that Chapter 3's self-reference rule follows from: a
block-wide binding's name *is* its value's node, so a value that read the
binding would be reading itself. It is also why a bound infinite
structure stays cheap: `ns` is one node, and the walks through it share
the prefix they have already built.

## Scopes and `@gc`

A program's blocks usually live as long as the program. `@gc e` gives `e`
a scope of its own, so that the nodes it builds can be released once its
value has been read:

```lichen
add = x => y => x + y
double = n => @gc { a = n + 1
                   b = add a a
                   b }
@gc [double 3, double 7]
```

```text
[8, 16]: array{Int, 2}
```

`@gc e` is a prefix marker at the same level as `@assert`, so it takes a
whole application (`@gc f 41`) or a block (`@gc { … }`). It computes
nothing and changes no type: the scope's value is `e`'s value. What it
changes is the **lifetime**: `e` is compiled into a block of its own, and
once the parent reads the scope's value, the block is released, keeping
only what that value still reaches.

The trigger is a **shallow read**: the parent reads the scope's root, and
that is all. Nothing is evaluated just to decide whether the scope is
done, so a marker never makes an unread expression run. In the example,
each call's block can be released after its value is read; the outer
scope is released after the array is read.

A scope is safe to release because of a contract the language guarantees:
a value may only name nodes that outlive it — its own block, an enclosing
one, a parentless one, or a child scope's **declared root**. The compiler
builds cross-block references to satisfy that contract, so no released
scope leaves a dangling reference. The mechanism's note is
[gc-block](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/gc-block.md).

## Summary

- Values are computed on demand and at most once; a reading program
  computes only what its answer needs.
- Arrays, tuples, `if` branches, and function bodies are the places a
  value can wait.
- Checking is *not* lazy: an out-of-bounds index or a type error is
  reported wherever it is written, even where its value would never be
  used.
- `@shallow` keeps an array element's value lazy so an infinite structure
  can be written; its depth form keeps the first `n` levels shallow, and a
  depth of zero is refused.
- A refused computation answers the lazy marker `none`, with a recorded
  reason, and the program continues.
- `@gc` gives an expression a scope released on the parent's shallow read;
  it computes nothing and changes no type.
- Bindings share nodes rather than copying values.

## Exercises

1. Write an infinite stream of squares and read its first four values.
2. Explain why `[1 / 0, 2][1]` is `2` but `f = x => [1, 2][5]` fails the
   build.
3. What does `(1 / 0) + 1` print, and why is that not an error?
4. Wrap a block in `@gc` and describe what the marker lets the program do
   that it could not do otherwise.
5. Try `[@shallow(0) 5]` and explain the refusal.
6. Write a function whose body is a large block, call it twice, and wrap the
   block in `@gc`. What is released, and when?
