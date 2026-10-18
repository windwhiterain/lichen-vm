# Verifying a Program

lichen has no test framework in the language. Its idiom is simpler: a
program can declare what it prints, and a suite runs every example and
compares the declaration with the answer. This chapter is about that
suite — how an example states its promise, how the suite checks it, and
how the repository keeps the two in sync.

## A program's promise

An example's `---…---` header always carries the same three entries:

```lichen
---
  order = "02"
  output = "111: Int"
  doc = "Count the Collatz steps from 27 to 1."
---
collatz = n => if n == 1 then 0 else 1 + collatz [n / 2, 3 * n + 1][n % 2]
collatz 27
```

- `output` is what the program prints, as one line of `value: type`. It is
  the program's **promise**, and the suite compares against it exactly.
- `doc` is the program's prose, because the language has no comments.
- `order` places the file in the generated README section (a numeral; lower
  sorts first).

The file is otherwise an ordinary program. The promise is not decoration:
the suite fails when the program's answer disagrees with it.

## The suite

The examples live in [`examples/`](https://github.com/windwhiterain/lichen-vm/tree/dev/examples), and a test in the
compute crate walks the directory and runs every program:

```text
$ cargo test -p lichen-compute --test lichen_compute every_example_runs_and_prints_what_it_declares
running 1 test
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 243 filtered out; finished in 3.90s
```

The `filtered out` count is the rest of that crate's tests; the one test
that ran is the whole example suite. It requires every example to declare
an `output`, runs the program, and compares the printed line with the
declaration.

A mismatch is reported with both sides:

> an example's output no longer matches its `output =` declaration — the
> language's observable behaviour changed. If that was intended, update the
> declaration in the same commit (or run the sync-readme binary); otherwise
> it is a regression.

That message is the whole testing philosophy. When the answer moves, you
decide whether the move was **intended** — then the declaration is updated
with the behaviour — or a **regression** — then the compiler is fixed. A
declaration is never silently relaxed.

## The README's examples

The README's Examples section is generated from the same files: each
example appears with its header, its program, and its output. The
generator is a tool, not a test:

```text
$ cargo run -p lichen-tools --bin sync-readme
```

It rewrites each example's `output` to the program's actual answer and
splices the re-rendered section into the README, printing a line for each
thing it changed. On a clean tree it prints nothing. A second test (in the
compute crate) re-renders the section and rewrites the README if it has
drifted, with a note that it did.

So a change to an example touches three things: the file, its `output`,
and the README. The tool does the last two when asked, and the tests make
sure nobody forgets.

## Directories as examples

An example may be a directory. The directory's own program is `_.lichen`,
and the directory's `order` places the whole group:

```lichen math.lichen
succ = x => x + 1
add = (x : Int) => (y : Int) => x + y
```

```lichen _.lichen
---
  order = "5"
  math = import "math.lichen"
---
(math.succ 41, math.add 1 2)
```

The README renders the directory under one heading, with the directory's
program first and its files after it. The suite runs the directory's
`_.lichen`, and that program imports its siblings (Chapter 15), so the
whole directory is exercised through one entry.

## Writing your own example

To add an example to the suite:

1. Create the file — or the directory with a `_.lichen` — under `examples/`.
2. Give it a header with `order`, `doc`, and an `output` that is what the
   program prints. (`sync-readme` fills `output` in for you from a run.)
3. Run the suite test. It should report one more passing example.

A program outside the repository gets the same treatment without the suite:
run `lichen-compiler file.lichen`, compare the line with what you expect,
and keep the `output` in the header so the next reader can do the same.

## Assertions inside a program

The suite checks the answer a program prints. A fact a program must hold
*while running* is written with `@assert` (Chapter 14) or a refinement
(Chapter 11), so an internal invariant is checked by the program itself:

```lichen
f = x => { @assert (x > 0); x * 2 }
(f 1, f 5)
```

```text
(2, 10): tuple{Int, Int}
```

The two mechanisms cover different things and complement each other: the
declaration says what the program answers, the assert says what its
computations may assume.

## Where the suite lives

The test is in `crates/lichen-compute` because the shipping compiler
composes the compute sub-language, and many examples are kernels
(Chapter 17). The bare language is a crate underneath
(`crates/lichen-language`), and its tests check the language's own
behaviour; the examples are the **living specification** that crosses the
whole stack, from source text to a kernel on a backend.

## Summary

- An example declares `order`, `output`, and `doc` in its header; the output
  is a promise the suite checks exactly.
- The suite is one test in the compute crate; it walks `examples/`, requires
  an `output` from every example, runs each program, and compares.
- A mismatch means either the declaration is updated deliberately (with the
  behaviour change) or the compiler is fixed.
- `sync-readme` regenerates both the declared outputs and the README's
  Examples section; a test enforces the README's sync.
- A directory example is a `_.lichen` entry plus its siblings.
- `@assert` checks facts inside a program; the suite checks what it prints.

## Exercises

1. Write an example file with a header and check that the suite's test
   passes it.
2. Change the program so it prints something else, run the suite, and read
   the failure message.
3. Add a `doc` entry and say where it appears.
4. Make a directory with a `_.lichen` and a sibling module, and import the
   sibling.
5. Explain the difference between what `@assert` verifies and what the
   `output` declaration verifies.
