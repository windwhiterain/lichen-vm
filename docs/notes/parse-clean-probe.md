# Parse performance: the clean probe

> Status: current
> Points at: `crates/lichen-language-parser/src/parse.rs` (`parse_inner`,
> `GrammarError`), `crates/lichen-language-lex/src/lib.rs` (`describe_static`).

## Symptoms

Over the 32-file corpus (every `examples/*.lichen`, `lichen-std/*.lichen`, and
the three embedded sources; 8 995 bytes, 1 966 tokens), the front end's lex is
**0.13 ms** and the parse is **~15 ms**. Counting token-peek predicate
evaluations (every `token()`, `name()`, and literal filter) gives **48 017
evaluations for 1 966 tokens — 24.4 per token**.

The parser is one chumsky combinator grammar: each precedence level parses its
left operand and then *tries* its operator, and each atom attempt runs the whole
primary alternative set. A tried-and-failed branch does not merely fail: chumsky
builds a `Rich` error for it (with its expected-label set) and merges it into the
pass's "alt" error. On well-formed input all of those errors are discarded, and
they were almost the entire cost — **208 716 allocations, 21.8 MB, for one pass
over the corpus**.

## Why the grammar cannot be restructured away

The expected-label sets are part of the public diagnostics: a malformed input
renders the union of the labels of every branch that failed at the furthest
position, **in the order they were tried**. Rewriting the chain to stop
re-attempting levels therefore changes messages. Replacing the seven-branch
prefix `choice` with one table-driven `filter_map` dispatch (the planned
rewrite) compiles and parses, but changes 476 oracle lines:

```
before: expected '@assert', 'int2float', 'float2int', 'exp', 'sqrt', 'floor', 'max', 'min', 'table', 'set', or an expression, found '?'
after:  expected '@assert', something else, 'table', 'set', or an expression, found '?'
```

The labels *are* the record of the backtracking. Any rewrite that collapses a
`choice` is a diagnostics change, not an optimization.

### The same rewrite buys nothing once the probe is in

With the probe in place the discarded branches are already free, so the
table-driven prefix dispatch moved the corpus pass only from 2.06 ms to 1.96 ms —
inside the noise of a shared machine. There is no performance case for changing
the grammar at all.

## Fix: probe clean first, keep the rich pass for anything else

The grammar is generic over the error type it reports through
(`GrammarError`/`Ex`), so the same grammar is instantiated twice:

- a **clean probe** over the zero-sized `chumsky::error::EmptyErr`, whose
  discarded branches cost a cursor move and a `()` push, and
- the **diagnostic pass** over `Rich`, unchanged.

`parse_inner` runs the probe. If it yields a program *and* emitted no error, the
diagnostic pass would report nothing either, so the probe's program is the
result. Anything else — a failed parse, a recovery, a table-entry or lambda
validator message — falls through to the diagnostic pass, whose output is
byte-identical to the old single pass by construction.
`parse_statement_region_traced` probes the same way.

### Measurements

Alternating A/B, several rounds of five sub-rounds each (`CARGO_TARGET_DIR` in
the worktree, release, `-j 4`), old = `dev`, new = this change. The machine was
shared, so the ranges below span several rounds rather than one:

| phase | old | new | ratio |
|---|---|---|---|
| lex only (32 files) | 0.13–0.15 ms | 0.13–0.15 ms | 1.0x (unchanged code) |
| lex + parse (32 files) | 13.2–15.5 ms | 1.87–2.47 ms | **~6.5–7x** |
| parse only (pre-lexed) | 13.3–15.0 ms | 1.63–3.19 ms | **~5–8x** |
| malformed corpus (124 cases) | 10.5–11.6 ms | 8.0–9.8 ms | **~1.3x** |
| allocations (32 files) | 208 716 / 21.8 MB | 11 081 / 1.4 MB | **~19x** |
| peak live (32 files) | 176 594 B | 122 515 B | 1.4x |

Two smaller changes are part of the same number and stand on their own:

- the parser takes the token slice (`&'a [Token]`) instead of a cloning
  `chumsky::input::Stream`, halving the pass before this design (~1.85x);
- `TokenKind::describe_static` gives a parser label a `&'static str`, so a label
  no longer allocates a `String` (~27% of the remaining pass).

The malformed corpus runs the grammar twice, yet is still faster than the old
single pass: the probe is cheap and the rich pass is itself faster than the old
one. The peek count — 48 017, measured with a temporary inline counter that the
commit does not carry — is **unchanged**: this removes the *cost* of discarded
backtracking, not the backtracking.

## Acceptance

The oracle renders every parse error and the parsed program for all 418 cases
(32 corpus files, 262 string literals from the parser's own test module, 124
malformed snippets): **byte-identical** between `dev` and this change, including
the message above that the table-driven prefix dispatch breaks.

## The same trap in a plain cleanup

It is not only the precedence chain. Replacing the `=>` body rule's
`.repeated().collect::<Vec<_>>()` with `or_not()` — the obvious reading of "the
body is optional" — stops the chain after the first arrow, so on `x => => y` the
second arrow is left unconsumed and the diagnostic count goes from one to three:
`expected a separator or the end of the program, found '=>'` plus the
program-level message. The repetition is load-bearing for the label set, so it
stays and a `first_body` fold keeps the sequence while dropping all but the first
body (`parse.rs`). A cleanup is not exempt from this note's rule: what a failure
reports is part of the surface.
