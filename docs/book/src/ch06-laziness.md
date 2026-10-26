# Laziness, Sharing, and Scope

Two questions about any language's evaluation are *what* is computed and
*when*. lichen's answers are unusual: a program computes only the values
its answer needs, and it computes them as late as the answer allows. This
chapter is about that model — and about the one place where it does not
apply, which is checking.

## What is evaluated, and when

A lichen program is a graph. A binding's name is a node; a tuple element
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

- A tuple element is computed when it is read: `(1 / 0, 2)(1)` is `2`,
  and the unread element's refusal is not in the way.
- An array element is computed when it is read.
- An `if`'s untaken branch is an unread tuple element.
- A function body is computed when the function is applied, and a branch
  that selects by index computes only the selected element — Chapter 4's
  `fib` is the example: it terminates at `0` because the sum it does not
  select is never evaluated.

```lichen
((1 / 0) + 1, (1 / 0, 2)(1))
```

```text
(none, 2): Tuple
```

The first element is `none`: a refused computation's value (below). The
second is `2`, because the tuple's first element was never read.

## Checks are not lazy

The checker's job is to decide types, and it does that by evaluating the
program. So a read that is written where a value is needed is checked
there, even when the value would never be used:

```lichen
f := x => ([@any : Int, 3])[5]
```

```text
error: index 5 out of bounds (array length 3)
  --> 1:29
   |
 1 | f := x => ([@any : Int, 3])[5]
   |                             ^
```

The index is checked even though `f` is never applied and its body is
never evaluated by the program.

There is one exception, and it is the `if`: its two branches are a
tuple's elements, and only the taken one is read. A fault written in the
**untaken** arm is therefore not found:

```lichen
if 1 == 1 then 1 else 1 + "a"
```

```text
1: Int
```

So there are two different things that can go wrong, and they behave
differently:

- a **check** that fails — an out-of-bounds index, a type mismatch, a
  resolve error — is a diagnostic and stops the build;
- a **computation that is refused** — division by zero, a conversion with
  no answer — produces the lazy marker `none`, and evaluation continues.

The rest of this chapter is about the second kind, and about the
structures that need the first kind to be held back.

## Infinite structures and `@shallow`

An array's elements share one requirement, and the checker evaluates them to
find it. A definition that puts a call to itself in an array would
therefore be unfolded forever. The marker `@shallow` says the element's
value stays lazy in the checker's evaluation:

> **The array instance is not accepted yet.** The compiler refuses `T[1, 2]`
> with `expected Array, found Array` — Chapter 7 says where — so the
> programs in this section state the rule and the answers they are written
> to give.

```lichen
T := _x => [@any, 2]
nats := (n : Int) => T[n, @shallow nats (n + 1)]
ns := nats 0
(ns[1][0], ns[1][1][0], ns[1][1][1][0], ns[1][1][1][1][0])
```

```text
(1, 2, 3, 4): Tuple
```

`nats 0` is the array `[0, @shallow nats 1]`. Reading `ns[1]` gets the
element at position 1 — the marked call — and reading `[0]` of that gets
its first element, `1`; one more `[1][0]` walk one step further. The
stream has no end, and every read walks only as far as it needs.

Without the marker the same definition does not check: the checker
unfolds the element, and stops on its budget:

```lichen
T := _x => [@any, 2]
nats := (n : Int) => T[n, nats (n + 1)]
ns := nats 0
ns[1][0]
```

```text
error: the checker stopped on its apply-depth budget: nested applications exceeded 500 levels. The program may still terminate when run.
  --> 1:10
   |
 1 | T := _x => [@any, 2]
   |          ^
```

`@shallow` is what makes an infinite list expressible, and it appears
only inside an array instance's element.

The marker has two spellings. The bare `@shallow` keeps the whole value
subtree lazy, and `@shallow(n)` keeps the value slot shallow at each of
the first `n` levels of the element's type spine. The depth is a bound of
at least `1`; a depth of `0` marks no level, and is refused:

```lichen
T := _x => [@any : Int, 2]
T[@shallow(0) 5, 6]
```

```text
error: a `@shallow` depth of 0 marks no level — the mark is erased before the checker sees the element, so the position is checked exactly as if it were unmarked — write a depth of at least 1, or drop the marker entirely
  --> 2:3
   |
 2 | T[@shallow(0) 5, 6]
   |   ^
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

A binding is sharing, not copying. `x := 2 + 3` binds the name `x` to one
node, and every use of `x` reads that node; the value is computed once
and every use sees it. The same is true of a function's captured
environment: a closure points at the enclosing block's nodes rather than
duplicating them.

This is the property that Chapter 3's self-reference rule follows from: a
block-wide binding's name *is* its value's own node, so a value that read
the binding would be reading itself. It is also why a bound infinite
structure stays cheap: `ns` is one node, and the walks through it share
the prefix they have already built.

## Scopes and `@gc`

A program's blocks usually live as long as the program. `@gc e` gives `e`
a scope of its own, so that the nodes it builds can be released once its
value has been read:

```lichen
add := x => y => x + y
double := n => @gc { a := n + 1
                   b := add a a
                   b }
T := _x => [@any : Int, 2]
@gc T[double 3, double 7]
```

```text
[8, 16]: Array
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
- Tuples, arrays, `if` branches, and function bodies are the places a
  value can wait.
- Checking follows the values: a read written where a value is needed is
  checked there, but an `if`'s untaken arm is never read and so is never
  checked.
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
2. Explain why `(1 / 0, 2)(1)` is `2` but `f := x => ([@any : Int, 3])[5]`
   fails the build.
3. What does `(1 / 0) + 1` print, and why is that not an error?
4. Wrap a block in `@gc` and describe what the marker lets the program do
   that it could not do otherwise.
5. Try `[@any : Int, 2][@shallow(0) 5, 6]` and explain the refusal.
6. Write a function whose body is a large block, call it twice, and wrap the
   block in `@gc`. What is released, and when?
