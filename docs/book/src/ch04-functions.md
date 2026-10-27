# Functions and Closures

In lichen a function is not a special kind of declaration; it is a value
like a number. You can bind it, pass it to another function, return it
from one, and store it in a compound value. This chapter covers functions
as values, the closures they carry, recursion, and higher-order
functions.

## A lambda is a value

A function is written `parameter => body`:

```lichen
inc := x => x + 1
inc
```

```text
Int => Int: Int => Int
```

The text `x => x + 1` is a **lambda**: `x` is its parameter and `x + 1`
its body. The body is everything to the right of the `=>` up to the end
of the expression, so `x => x + 1` is `x => (x + 1)`. To limit the body,
parenthesize it.

A lambda is an expression, so it can be a whole program:

```lichen
x => x + 1
```

```text
Int => Int: Int => Int
```

A function's value is printed as its **signature** — the parameter's
type, `=>`, the result's type — and its type is the shared `FunctionType`
marker, which names no function.

The identity function shows what the compiler does with a function whose
parameter has no decided type yet:

```lichen
id := x => x
id
```

```text
?a => ?a: ?a => ?a
```

`?a` is an **open cell**: a type not yet decided. It appears here because
nothing has said what the identity function's parameter is. Chapter 5
explains the cell model; for now, `?a` is the compiler saying "this is
one type, and it is not known yet".

## A function's value and its type

The output `Int => Int: TypeFunction` has two halves, and they say different
things. A function's **value** is the function, and it prints its signature
(`Int => Int`); its **type** is the shared `FunctionType` marker
(`TypeFunction`), which names no function — so the type chain never cycles at
a function, and a signature is stated by a **function template** instead.

The consequence is that annotating a function is not adding a check
beside it. The annotation writes the function's own cells:

```lichen
inc := (x : Int) => x + 1
inc
```

```text
Int => Int: Int => Int
```

A whole signature is stated with a **function template**: a parameter with
its type, `=>`, and the result. The template is written where a function
would be, and it is unified with plain `=`:

```lichen
inc := (x : Int) => x + 1
inc = _x : Int => _: Int
```

```text
Int => Int: Int => Int
```

The template `_x : Int => _: Int` says: a function whose parameter is
annotated `Int`, and whose result is a placeholder of type `Int`. The `=`
asks the actual's **signature** to agree with it — and the actual's own body
is kept; only its signature is constrained. There is no separate
function-type syntax, and no thin arrow: `->` is not a token in the
language.

Plain `=` is the spelling because `=` applies nothing. The match operator
`<:` applies its right hand side exactly once, and a function template is
not a constructor to apply: it states a signature, not a requirement to be
produced. Chapter 7 and Chapter 12 show the constructor form for the other
compound layouts.

A function's parameter is instantiated fresh at every application. That
is what lets one binding be used at two different types:

```lichen
id := x => x
(id 5, id Int)
```

```text
(5, Int): Tuple
```

The binding `id` has one open parameter, and each application decides it
again, at `Int` in the first use and at `Type` in the second. This is
automatic let-polymorphism, and Chapter 11 is about it.

## Closures

A function body can see the names of the block it was written in. When
the function is returned from that block, it keeps them:

```lichen
a := 1
f1 := x => {
    b := 2
    f2 := y => (a, b, x, y)
    f2
}
f1 3 4
```

```text
(1, 2, 3, 4): Tuple
```

`f1 3` runs the block: it binds `b := 2`, builds `f2`, and returns it.
`f2`'s body reads `a` from the enclosing program, `b` from the block, and
`x` from `f1`'s parameter; the call `f1 3 4` supplies `y`. The result is
the four captured values in the order the tuple lists them.

Every call of `f1` builds its own environment, so the closures it returns
are independent. This is ordinary partial application: applying one
argument to a two-argument function leaves a function that has captured
the first.

```lichen
make := a => b => a + b
add2 := make 2
add2 3
```

```text
5: Int
```

A closure does not copy its environment. The captured names are the same
nodes the surrounding program uses, shared rather than duplicated; in a
language with no mutation the difference is invisible, but it is what
keeps laziness and memory use sane (Chapter 6).

## Recursion

A binding may name itself as the head of a call. That is recursion, and it
is the only self-reference the language admits in a value (Chapter 3).

```lichen
fact := n => if n == 0 then 1 else n * fact (n - 1)
fact 5
```

```text
120: Int
```

The body reads `fact` only to call it, so the definition is well founded:
each call has its own parameter cell and its own body.

Recursion can select the branch lazily. This `fib` builds both futures of
`x` as a two-element tuple and reads the one the comparison selects:

```lichen
fib := x => (fib (x - 1) + fib (x - 2), x)(x <= 1)
fib 10
```

```text
55: Int
```

For `x <= 1` the index is `1`, so position 1 — the value `x` — is read
and the sum in position 0 is never evaluated; for larger `x` the index is
`0` and the sum is read. Without that laziness, `fib 0` would call
`fib (0 - 1)` and never stop.

Two functions may call each other, because a block's names are in scope
throughout it:

```lichen
is_even := n => if n == 0 then 1 else is_odd (n - 1)
is_odd := n => if n == 0 then 0 else is_even (n - 1)
(is_even 10, is_odd 10)
```

```text
(1, 0): Tuple
```

### A recursive function may need its shape stated

A recursive function's result is often only as decided as the function's
own signature. This `gcd` says nothing about the shape of `p`, so the
checker cannot read it:

```lichen
gcd := p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))
gcd (48, 18)
```

```text
error: this tuple read cannot be checked: the container's type is not decided here — annotate the container's type
  --> 1:8
   |
 1 | gcd := p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))
   |        ^
error: this tuple read cannot be checked: the container's type is not decided here — annotate the container's type
  --> 1:8
   |
 1 | gcd := p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))
   |        ^
error: this tuple read cannot be checked: the container's type is not decided here — annotate the container's type
  --> 1:8
   |
 1 | gcd := p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))
   |        ^
error: this tuple read cannot be checked: the container's type is not decided here — annotate the container's type
  --> 1:8
   |
 1 | gcd := p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))
   |        ^
error: this tuple read cannot be checked: the container's type is not decided here — annotate the container's type
  --> 1:8
   |
 1 | gcd := p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))
   |        ^
```

A read states its requirement from the type of its container, and `p`
has none here: nothing in the program has said what its elements are,
and a recursive function's type is the shared `FunctionType` marker, which
names no function. A
**constructor** states the shape — here a tuple constructor whose two
positions require an `Int` — and the result follows from it:

```lichen
T := _x => (@any : Int, @any : Int)
gcd := (p <: T) => ((if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))) : Int)
gcd (48, 18)
```

```text
6: Int
```

The `@any : Int` is the *requirement* for one position: any `Int` will
do. The `<:` applies the constructor exactly once and unifies the
parameter with the requirement that comes back, so the argument is checked
against a fresh requirement each time.

The annotation on a recursive function is a constraint on the function's
own cells, not a check performed beside the recursion; Chapter 12
explains why that distinction is what makes the self-reference well
founded.

### The checker unfolds the recursion

To type a program, the checker evaluates it, so a recursion that does not
stop for it is cut off:

```text
error: the checker stopped on its apply-depth budget: nested applications exceeded 500 levels. The program may still terminate when run.
```

The budget bounds **nesting**, not recursion in general: it counts how
deeply applications nest, so a recursion that never deepens the nest is
not caught by it. It is a safety valve, not a termination checker.
Chapter 6 is about the distinction between what the checker decides and
what run time does.

## Marks on a binding

Two marks may precede a binding's name. `@cache` marks a retained cell
(Chapter 3), and `@loop` marks a recursion the compiler may compile as a
loop rather than unroll:

```lichen
@loop fact := n => if n == 0 then 1 else n * fact (n - 1)
fact 5
```

```text
120: Int
```

The mark changes no answer. It is a hint to the loop-conversion pass
([the note](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/loop-conversion.md)), which turns an eligible
recursion into a loop nest when the shape allows it.

## Higher-order functions

A function that takes a function is ordinary:

```lichen
twice := f => x => f (f x)
inc := x => x + 1
twice inc 5
```

```text
7: Int
```

Composition is the same shape:

```lichen
compose := f => g => x => f (g x)
inc := x => x + 1
compose inc inc 5
```

```text
7: Int
```

Functions can be stored in a compound, because a compound is an ordinary
value:

```lichen
inc := x => x + 1
fs := (inc, inc)
fs(0) 5
```

```text
6: Int
```

The tuple's positions hold the two functions, and the read takes the first
of them.

There is no map over an array. An array's length is part of its
**requirement**, and the language has no runtime read of that length, so
iteration over an array is written by carrying the count in a recursion:

```lichen
apply_n := n => f => x => if n == 0 then x else apply_n (n - 1) f (f x)
inc := x => x + 1
apply_n 3 inc 0
```

```text
3: Int
```

The standard library ships `id`, `succ`, `add`, `double`, `compose`,
`inc_twice` and `type_of`; Chapter 15 shows how to import it.

## Summary

- A function is an expression, `x => body`, and a value like any other.
- A function's type is the shared `FunctionType` marker; the **value**
  prints the signature, written with `=>`.
- A signature is stated with a **function template** — `_x : Int => _: Int`
  — and unified with plain `=`, which applies nothing; there is no thin
  arrow.
- An application instantiates the parameter fresh, so a binding is
  automatically polymorphic.
- A closure keeps the names visible where it was written; partial
  application builds one.
- Recursion is a self-call; a recursive function whose result stays open
  states its signature with a constructor.
- `@loop` marks a recursion the compiler may compile as a loop; it changes
  no answer.
- Functions are values: they can be arguments, results, and compound
  elements.

## Exercises

1. Write `apply_n n f x`, which applies `f` to `x` `n` times, and check
   `apply_n 3 inc 0`.
2. Write `compose3 = f => g => h => x => f (g (h x))` and use it.
3. Explain why `fib 0` terminates, in terms of what the tuple's elements
   evaluate.
4. Remove the constructor from the `gcd` above and compare the output.
5. Write `@loop` and unmarked versions of `fact` and confirm they agree.
6. Write a function that returns a function of its own, and explain what
   the returned function captured.
