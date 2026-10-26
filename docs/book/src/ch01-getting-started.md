# Getting Started

This chapter takes you from an empty directory to a running lichen
program. You will install the compiler, run your first expression, and
learn what the compiler prints and why.

## Installing the compiler

lichen is written in Rust. If you do not have a Rust toolchain, install
one with [rustup](https://rustup.rs); the workspace builds with a current
stable toolchain.

From a checkout of the lichen repository, build everything:

```console
$ cargo build
```

This builds the workspace and leaves the compiler at
`target/debug/lichen-compiler` (`lichen-compiler.exe` on Windows). If you
would rather have it on your `PATH`, install it instead:

```console
$ cargo install --path crates/lichen-compiler
```

Either way, the command is `lichen-compiler`. Ask it for its version:

```console
$ lichen-compiler --version
lichen-compiler 0.1.0
```

> **Note**: the version line uses the executable's own file name, so on
> Windows it reads `lichen-compiler.exe 0.1.0`. The repository also builds
> two programs you will meet in later chapters: the package manager
> `lichen` (Chapter 15) and the language server `lichen-language-server`
> (Chapter 18). The compiler is all you need for now.

## Your first program

Create a file named `first.lichen` containing exactly this line:

```lichen
1 + 2
```

Run it:

```console
$ lichen-compiler first.lichen
3: Int
```

That is the whole interaction: a source file, one line of output, and a
zero exit status. The output is the answer **and its type**, written
`value: type`. `3: Int` reads "the value `3`, of type `Int`".

Why does the compiler print the type? Because lichen does not check
types in a separate pass before running the program. Every expression
evaluates to a *expr*: a value and its type. Evaluation is what decides
both, and unification is what keeps them consistent. The type is part of
the program's answer, so the compiler shows it to you. (Chapter 5 is
about this model in full.)

## Statements, separators, and the tail

A program is a **block**: a sequence of statements followed, optionally,
by a tail expression. A statement is either a binding (`name := expr`) or
a bare expression. The block's value is its tail, so the following program
has the value of `x + y`:

```lichen
x := 2
y := 3
x + y
```

```text
5: Int
```

A bare expression in a non-final position is a statement like any other:
it is checked and evaluated, and its value is discarded.

Statements are separated by a newline, a semicolon, or a comma. All three
lex to the same separator, so these programs are identical:

```lichen
x := 2; y := 3; x + y
```

```lichen
x := 2, y := 3, x + y
```

The quantity of separators never matters: blank lines and trailing
separators are tolerated. The end of the file is not itself a separator;
it simply ends the block. What a separator *cannot* do is appear in the
middle of an expression. An expression ends where a separator begins, so
a lambda body has to start on the same line as its `=>`. Chapter 3 goes
through the consequences.

A binding is not an assignment. `x := 2` gives the name `x` the value of
the expression `2`, and every use of the name is that same value, shared
as one node in the program's graph. A name may be bound again in the same
block only with care, and a function may call itself only as a call;
Chapter 3 states the rules.

> **Note**: the binding operator is `:=`, and `=` is a different thing
> entirely: `=` *unifies* two expressions, asking them to agree
> (Chapter 5). The two never overlap — a name on the left of `:=` is a
> binding, and `=` is written between two expressions.

A program that ends in a binding, with no tail expression, is a **record
program**: its value is an anonymous struct of its bindings.

```lichen
a := 1
b := 2
```

```text
(1, 2): Struct
```

That shape is how a lichen library file exports its names; Chapter 8
covers it.

## There are no comments

lichen has no comment syntax. Not a line comment, not a block comment:
the lexer never skips text. Prose about a program lives in a
`---...---` block at the top of the file, which the compiler removes
before lexing:

```lichen
---
  output = "3: Int"
  doc = "The smallest lichen program: an expression, and the compiler
  prints its value and its type."
---
1 + 2
```

The block is metadata and is never part of the program. The entries that
matter to the toolchain are:

- `doc` — prose about the program. Tools show it; the language ignores it.
- `output` — the value the program is expected to print. The example suite
  checks it against what the program actually prints (Chapter 16).
- `order` — where the file sorts in the example suite.
- `name = import "path"` — make another program available as a package
  (Chapter 15).
- `name = depend "url"` — declare a git dependency the package manager
  fetches.
- `name = plug "url"` — declare a native plugin that extends the compiler.

The header is optional, and there can be at most one, before any code. A
file may also begin with prose of its own before the header — a markdown
title, say — because everything before the opening `---` is ignored. The
precise syntax is in the [language spec](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/language-spec.md), §2.2.

## Running a whole directory

If the path you give the compiler is a directory, it runs every `.lichen`
file directly inside it, in file-name order, and prints one line per file:

```console
$ lichen-compiler my-programs
a.lichen: 2: Int
b.lichen: 4: Int
```

The directory is not searched recursively, and only `.lichen` files are
picked up. A file that fails makes the exit status non-zero; the files
that can run still run. This is how the repository's own `examples/`
directory works (Chapter 16).

## Summary

- A lichen program is a block: statements, then an optional tail. Its value
  is the tail's value.
- A binding is `name := expr`; `=` is the unify of two expressions.
- The compiler prints `value: type`, because every expression is an expr.
- Newlines, semicolons, and commas are all the same separator.
- There are no comments; prose lives in the `---...---` header block.
- A directory path runs every program in it.

## Exercises

1. Run `9 * 9` and explain both halves of the output.
2. Write a program with two bindings and a tail that uses both, once with
   newlines and once with commas. Confirm the output is the same.
3. Add a `doc` entry to your program's header and check that the output is
   unchanged.
4. Make a directory with two programs, run the directory, and check the
   exit status with `$?` in a POSIX shell (or `$LASTEXITCODE` in
   PowerShell).
