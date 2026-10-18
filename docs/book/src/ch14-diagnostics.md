# Diagnostics and Assertions

Everything in the previous chapters was a correct program. This chapter is
about the other half of a language: what the compiler says when a program
is wrong, how to read it, and how a program states a fact it expects to
hold with `@assert`.

## The shape of a diagnostic

A diagnostic names what went wrong, points at the source, and marks the
column:

```lichen
1 + "a"
```

```text
error: expected Int, found string
  --> 1:5
   |
 1 | 1 + "a"
   |     ^
```

Read it as three parts. The first line is the message: what was expected
and what was found. The `-->` line is `line:column`. The caret is the
column, and the quoted source line lets you place it without opening the
file. A diagnostic may also carry `note` blocks, which point somewhere
else — a second location that explains the first (Chapter 11's prelude
failure is the example).

## The five stages

The compiler is a pipeline, and each stage has its own vocabulary. The
wording tells you which stage refused the program.

**Lexing** — a character the language does not accept:

```lichen
@
```

```text
error: unexpected character '@'
  --> 1:1
   |
 1 | @
   | ^
error: expected a separator, 'return', 'pub', '@loop', 'cache', 'let', a name, '@assert', '@gc', 'int2float', 'float2int', 'exp', 'sqrt', 'floor', 'sin', 'cos', 'ln', 'max', 'min', 'table', 'set', an expression, ':', '#', '!', '?', or '~', found the end of the program
  --> 1:2
   |
 1 | @
   |  ^
error: the program could not be parsed
  --> 1:2
   |
 1 | @
   |  ^
```

One character produced three diagnostics, and the reason is the pipeline's
next property.

**Parsing** — the tokens do not form an expression:

```lichen
3 :
```

```text
error: expected '@assert', '@gc', 'int2float', 'float2int', 'exp', 'sqrt', 'floor', 'sin', 'cos', 'ln', 'max', 'min', 'table', 'set', or an expression, found the end of the program
  --> 1:4
   |
 1 | 3 :
   |    ^
```

An unclosed delimiter is named as such: `(1 + 2` reports
`unclosed '(' opened on line 1`, followed by the list of tokens that could
have continued it.

**Resolving** — a name that is not in scope:

```lichen
y
```

```text
error: unresolved name 'y'
  --> 1:1
   |
 1 | y
   | ^
```

**Checking** — a type or attribute disagreement. This is the stage whose
messages quote types, and it is the one most diagnostics come from:

```lichen
5 : Float
```

```text
error: expected Float, found Int
  --> 1:1
   |
 1 | 5 : Float
   | ^
```

**Preprocessing** — a failure while reading the header block or importing a
package (Chapter 15). Its message names the file that could not be loaded.

The stages run in that order, and each one hands the next a partial
program.

## The pipeline recovers

A failed stage does not stop the pipeline. A lex error's character is
skipped and lexing continues, a broken statement is skipped and reported,
an unresolved name becomes an inert expression the checker ignores, and
the checker still runs on the partial program. So one run reports as many
problems as it can find:

```lichen
(y, z)
```

```text
error: unresolved name 'y'
  --> 1:2
   |
 1 | (y, z)
   |  ^
error: unresolved name 'z'
  --> 1:5
   |
 1 | (y, z)
   |     ^
```

```lichen
(1 + "a", 2 + "b")
```

```text
error: expected Int, found string
  --> 1:6
   |
 1 | (1 + "a", 2 + "b")
   |      ^
error: expected Int, found string
  --> 1:15
   |
 1 | (1 + "a", 2 + "b")
   |               ^
```

The lexer's recovery explains the rest of the `@` example's output: the
skipped character leaves a gap, the parser reports what it expected at the
gap, and the parse failure itself is reported once at the end of the
program. One bad character can produce a cascade, and the *first*
diagnostic is the one to fix.

## The vocabulary of a message

A few shapes recur, and knowing them makes a diagnostic much faster to
read:

- `expected A, found B` — the two types (or values) that disagreed. `A` is
  what the program requires at that position; `B` is what the expression
  actually has.
- `?a`, `?b` — open cells (Chapter 5). Cells in one unification class share
  a name, so two `?a`s in one message are the same cell.
- `…` — a cycle cut (Chapter 12), never a truncation.
- `struct{.x Int}#11` — a struct type's nominal identity (Chapter 8). Two
  `#id`s are two types, however identical their fields look.
- `raw` — the printer's mark for a value whose **decided** type cannot be
  read as a form (an undecided value prints its cell's `?a`, the same name
  its type shows); it is a defect, never an answer (Chapter 5).
- `note: the failing use is here` — a second location: the declaration is
  the first, your use is the second.
- `did you mean 'x'?` — a suggestion attached to a named-field miss
  (Chapter 8).
- `this value is not a function — it cannot be applied` — an application
  whose callee is not a function.
- `table lookup missed — no entry for key 9` — a lookup that has no entry
  (Chapter 9).

The rule behind the list is that positions and values are named, never
invented: a diagnostic quotes the source and the types the checker
actually holds.

## Asserts

`@assert e` is a **side constraint**: it requires `e` to evaluate to `1`,
and fails the program otherwise. Its value and type are `e`'s:

```lichen
n = 5
(@assert (n <= 5), n)
```

```text
(1, 5): tuple{Int, Int}
```

The first tuple element is the comparison's value, `1`; the assert checked
that it is `1` and passed it through. A condition that is not `1` fails:

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

`@assert` binds tighter than any binary operator and looser than
application, so `@assert f x` asserts the call and `@assert n > 0` asserts
`n` and then compares it with `0`. Parenthesize the condition.

A condition inside a function is checked per application, because the
parameter is decided per application. This holds:

```lichen
f = x => { @assert (x > 0); x }
f 1
```

```text
1: Int
```

and this does not:

```lichen
f = x => { @assert (x > 0); x }
f 0
```

```text
error: assertion failed: expected 1, found 0
  --> 1:12
   |
 1 | f = x => { @assert (x > 0); x }
   |            ^
```

The diagnostic points at the assert, not at the call, because the assert
is the thing that failed. An assert on a condition that is still
undecided when the definition is checked is not triggered then; the apply
clone re-checks it against each call's argument.

`@assert` and the refinement of Chapter 11 share one channel: both
require a condition to answer `1`. They differ in what they say. An
`@assert` reports the channel's plain sentence, `assertion failed:
expected 1, found 0`; a refinement names itself (`the refinement's
predicate did not answer 1 — the value does not satisfy the refined
type`), and a class contract names the set it expected (`does not satisfy
{Int, Float}`). The difference is in the spelling the condition was
registered with, and the position is always the condition's own.

## Errors, refusals, and the difference

Two things stop a build:

- a **diagnostic** from any stage, which is printed with a position and a
  message, and exits non-zero;
- nothing else. A program that reaches a value and prints it succeeds,
  even when the value is the lazy marker.

A refused computation is not a diagnostic (Chapter 6):

```lichen
(1 / 0, 1)
```

```text
(none, 1): tuple{Int, Int}
```

`1 / 0` has no value, so it answers `none` — with a reason the compiler
reports on standard error when the computation is forced — and the
program carries on. The distinction is sharp and worth remembering: a
failed *check* is the compiler refusing to build; a *refusal* is a value
that could not be computed.

## One honest limit

Almost every diagnostic has a position, but not all of them do. When the
failing expression sits inside a desugared branch — an `if`'s untaken arm,
for example — the position may not survive:

```lichen
if 1 == 1 then 1 else [1, 2][5]
```

```text
error: the build failed, but the failing check could not be attributed to an expression in this source
```

The program is still refused — correctly, because the index is out of
bounds — but the message says it could not be placed rather than guessing
a position. This is the compiler's honest failure, not its normal one.

## Summary

- A diagnostic is a message, a `line:column`, the source line, and a caret,
  and it may carry `note` blocks.
- The stages are preprocess, lex, parse, resolve and check; the wording says
  which one refused the program.
- The pipeline recovers, so one run reports as many problems as it can.
- The message vocabulary is `expected`/`found`, open cells `?a`, cycle cuts
  `…`, nominal ids `#id`, `raw`, notes, and suggestions.
- `@assert e` requires `e` to be `1`, passes its value through, binds
  between application and the binary operators, and a function's asserts
  are re-checked per application.
- A diagnostic stops the build; a refused computation is a `none` value and
  does not.
- One shape of check has no position, and the compiler says so.

## Exercises

1. Write a program with two independent type errors and check that both are
   reported.
2. Write an unresolved name, a parse error, and a type error, and identify
   each stage from its message.
3. Write a function whose assert holds at one argument and fails at
   another.
4. Explain the difference between `1 / 0` and `[1, 2][5]` in terms of what
   stops the build.
5. Read a diagnostic that contains `?a` and say which two positions share
   the cell.
6. Write an assert whose condition is a comparison and one whose condition
   is not, and explain which is the intended style.
