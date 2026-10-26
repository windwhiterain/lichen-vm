# Recursive and Dependent Shapes

The last two chapters of this part are about the two things lichen's type
system does that a conventional one does not: a value may be
**infinite**, and a result may depend on a **value**. Both come from the
same source — unification, with no occurs check — and both are why the
language says the program, not the type system, is responsible for
decidability.

## Unification has no occurs check

Unifying two types in lichen is comparing their cell structures, and a
cell may be compared with a structure that contains it. There is
deliberately no occurs check: a cyclic type is a legal type. The universe
itself needs one — Chapter 5's `Type : Type` closes a chain in a cycle —
and so does every recursive shape.

The immediate consequence is that *recursive shapes need no declaration
form*. A constructor can refer to another constructor, and two constructors
can refer to each other, and the result is an ordinary cyclic value:

```lichen
A := _x => {.v @any : Int, .next @any : Struct}
B := _x => {.v @any : Type, .next @any : Struct}
a := A{.v 1, .next b}
b := B{.v Int, .next a}
a
```

```text
(1, (Int, (1, …))): Struct
```

`a` holds a `b`, which holds an `a`; the *value* `a` holds `b`, which
holds `a`. The graph has a cycle, and the printer says so rather than
unfolding forever. The two constructors are written apart and each is its
own nominal type (Chapter 8); the cycle is in the values, which is where
the compiler reads a struct's shape from.

A single binding cannot refer to itself as a value, so a directly
self-referential constructor is refused (Chapter 3's rule). A recursive
shape is written as a cycle between bindings — two is enough, as above —
and a constructor that applies itself is cut by the checker's
apply-depth budget (Chapter 4). The mutual form is the one that works.

## The printer cuts a cycle

A `…` inside a printed type or value is a **cycle cut**: the walk reached
a node it had already printed and stopped there. It is not a truncation
of a longer finite thing; it is the finite notation for something that
does not end.

In the example above, the cut is at the second `(1, …)`: the walk from
`a` went through `b` and back to `a`, so it stopped. A cut is per walk,
so a cycle of any length terminates, and two cycles that share no node
each get their own cut. This is also why the universe's own self-loop
prints at all: the type chain closes at `Type`, and a printed type that
walked into it would cut there.

## Recursive functions, same shape

A function's type **is** the function (`f : f`, Chapter 4): an applied
function's value and type are the same self-referential node. That is the
same pattern as the universe's cycle and the same pattern as `A`/`B`.

A recursive function's result is as decided as its signature. When the
signature is stated, the recursive calls read it and the result follows:

```lichen
T := _x => (@any : Int, @any : Int)
gcd := (p <: T) => ((if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))) : Int)
gcd (48, 18)
```

```text
6: Int
```

Without the constructor the function's own cells stay open, and Chapter 4's
refusal is what the checker answers: a read states its requirement from
the type of its container, and `p` has none. The constructor is not a
separate check; it is the function's parameter and return cells, stated by
hand.

## A result that depends on a value

A result may depend on an argument's **value**, because the checker
evaluates the program to decide types. The trick is a tuple whose
positions hold different things:

```lichen
a := x => (1, Int)(x)
(a 0, a 1)
```

```text
(1, Int): Tuple
```

`a 0` reads position 0 of `(1, Int)`, which is the value `1`, so the
call is `1 : Int`. `a 1` reads position 1, which is the type `Int`, so
the call is `Int : Type`. The same function produced a value at one
argument and a type at the other, and the checker tracked both because
the read is performed while checking.

The function's own printed type cannot state this: `a` alone is
`?a => ?b: ?a => ?b`. The dependency lives in the *evaluation*, not in
the function's signature — which is the point. lichen gets dependent
typing through evaluation and laziness, not through a separate dependent
type language. When the argument is concrete, the result is decided; when
it is not, the result stays open (`?b`).

The same mechanism pins an array requirement's length (Chapter 10):
`[@any : Int, n]`'s `n` is a value, and the instance's element count pins
the value the length must equal. A dependent type is a requirement with a
value in it, and the value is decided by unification.

## The price

Equi-recursive unification and first-class types buy a great deal: no
occurs check, no recursive-type declaration, no separate kind system, and
a universe that is an ordinary value. The price is stated in the project's
README and worth repeating:

> the cost is that the decidability of a lichen program is your
> integration's responsibility, e.g. encode your type-system's universes
> into lichen.

There is no algorithm that decides every lichen program's termination,
and the type system does not pretend otherwise. A program may have an
infinite value, a cyclic value, or no answer at all; the checker's budgets
are safety valves, not proofs (Chapter 4). What the language guarantees is
the consistency of every `value : type` expr it produces, given consistent
inputs.

## Summary

- Unification has no occurs check: cyclic shapes are legal, and the
  universe itself is a cycle at `Type`.
- Recursive shapes are written as cycles between bindings; a single
  binding cannot name itself, and a self-applying constructor is cut by
  the apply-depth budget.
- The printer cuts a cycle with `…`: a finite notation for an infinite
  value, never a truncation.
- A function's type is the function itself, which is the same
  self-referential shape; a constrained recursive function has a decided
  result.
- A result may depend on an argument's value, because checking evaluates;
  a concrete argument decides the answer, an open one leaves it open.
- A dependent array length is the same mechanism as a pinned requirement.
- The price of all of it is that decidability is the program's own
  responsibility.

## Exercises

1. Write two struct constructors that refer to each other, and a value that
   cycles through them. Read a field and explain where the printer cut.
2. Explain why a constructor that names itself is refused while the `A`/`B`
   pair is not.
3. Write `gcd` with and without its constructor and compare the two outputs.
4. Write `a := x => (1, "a", Int)(x)` and call it at each of the three
   indices. What type does each call have?
5. Explain what `?a => ?b: ?a => ?b` says about the function `a`, and why
   the signature cannot be more specific.
6. Write an array requirement whose length is a parameter and pin it with an
   instance; explain where the pin is enforced.
