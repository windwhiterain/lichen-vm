# Introduction

Welcome to *The lichen Programming Language*. This book teaches lichen
from nothing: how to install the compiler, how to write a program, and
how the language's model of values, types, and evaluation fits together.

## What lichen is

lichen is a small, lazy, pure programming language. A program is a
single expression built from literals, names, lambdas, applications,
tuples, arrays, structs, tables, and sets. The compiler evaluates the
program and prints its answer in the form

```text
value: type
```

For example, the program `1 + 2` prints `3: Int`. The first half is what
the program evaluates to; the second half is the type the value was
found to have.

What makes lichen different from most languages is that the second half
is not computed by a separate checker. Every value in the system is
represented as a pair of two graph nodes, its value and its type, and
**unification is a runtime operation**. Checking a program and running
it are the same pass. That has three consequences you will meet again
and again:

- **Types are ordinary values.** `Int`, `Float`, `string`, `Type`, function
  types, tuple types, and array types can be bound, passed to functions,
  returned from functions, and stored in data structures. The language has
  no separate type-level syntax with separate rules.
- **`Type : Type` holds.** The chain `value → type → kind → …` closes in a
  cycle at `Type`, which is the terminal of every chain rather than a
  supertype of everything. There is no subtyping and no kind system: a
  "kinding error" is an ordinary type error. The flip side is that whether
  a program terminates is the program's own responsibility.
- **Evaluation is lazy, and sharing is a graph.** A binding does not copy its
  value; the bound name *is* the value's node, and every use of the name is
  that same node. This is what makes recursion and first-class functions
  natural, and it is what makes an infinite structure expressible.

The language also has a compute sub-language for writing kernels, and a
type-level attribute system called *perspectives*; both are covered later
in the book, after the core is in place.

## Who this book is for

You should have written programs in some language before. lichen will
feel strange in places — there are no booleans, no comments, and no
mutable variables — but nothing in the book assumes prior knowledge of
lichen, and no chapter assumes the previous chapter's programs are still
open in another window.

The book is also for people who want to understand the *system*: the
compiler, the type checker, the package manager, and the compute layer.
The last part of the book covers building programs with all of those.

## How to read this book

The book is in four parts:

1. **Learning lichen** — install, run, and write real programs. This part
   ends with everything a first program needs.
2. **Values and types** — the model: the value/type pair, laziness, the
   compound forms (tuples, arrays, structs, tables, sets), and types as
   values.
3. **Types at work** — polymorphism, classes and refinements, recursive and
   dependent types, attributes, and diagnostics.
4. **Building programs** — modules and packages, verifying programs, kernels
   and the compute layer, the toolchain, advanced features, and a final
   project.

Read the parts in order the first time. Later, each chapter stands on its
own as a guide to the feature it names.

The book uses these conventions:

- Code in a `lichen` block is a complete program unless the text says
  otherwise. Save it to a file ending in `.lichen` and run it.
- Output in a `text` block is what the compiler printed, character for
  character.
- A `console` block contains a shell command and its output. `$` is the
  prompt; do not type it.
- Three dots (`…`) mean "genuinely elided text", and are explained in the
  surrounding prose. Nothing else is elided.
- `--> 2:9` means line 2, column 9.

## The state of the language

lichen is young and changes quickly. This book describes the behaviour of
the compiler in the same repository and was verified against it; the
[the language spec](../../language-spec.md) is the precise reference, and
the [notes](../../notes/) record the design decisions behind the
behaviour. If you find the compiler disagreeing with the book, trust the
compiler.

When you are ready, turn the page and install the compiler.
