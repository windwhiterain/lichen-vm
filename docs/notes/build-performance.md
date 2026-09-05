# Build performance: the chumsky `.boxed()` fix

> Status: current — the metrics below are from the present `lichen-language-parser` crate.
> Related, historical: the earlier fix (bottom) was for the pre-split `lichen-language`
> crate and is not an authority on current behaviour.
> Points at: `crates/lichen-language-parser/src/parse.rs`, `expression()`.

## Symptoms

A clean `cargo build --workspace` is dominated by one crate: `lichen-language-parser`
took **~96 s** of a ~118 s build. Every other `lichen` crate compiled in <3 s and
`chumsky` itself (the dependency) in ~6.8 s.

Decomposing the parser crate's own compile (Cargo `--timings`):

- frontend (type-check): **~92 s**
- codegen: **~4 s**

And `cargo check -p lichen-language-parser` (type-check only, no codegen) takes
**~86 s** — so the cost is rustc's *frontend* elaborating one enormous `chumsky`
parser-combinator type, not codegen and not the source size (`parse.rs` is only
~1 800 lines / ~90 KB).

## Root cause

`chumsky` builds the grammar as a combinator chain, and every `.then()` / `.or()` /
`.map()` / `.foldl()` / `.repeated()` / `.validate()` / `.recover_with()` wraps the
previous parser in another generic layer, threading `+ Clone` bounds. In
`expression()` the base atom parser is boxed (`atom_parser()` ends in `.boxed()`),
but the *operator precedence chain* built above it — `application → unary → arith →
cmp → arrow → annotation → lambda` — was **not** boxed. The accumulated combinator
type is therefore re-elaborated and re-normalised at every precedence level, and
rustc spends ~90 s doing it.

## Fix

Two strategic type-erasure calls, both in `expression()`:

1. Box the base of the operator precedence chain (`application`) so every higher
   precedence level threads a small `Boxed` type.
2. Box the whole precedence chain as it is handed back to `recursive` (the top of
   `expression`), so the recursive factory also sees a small type.

```rust
let application =
    atom.clone()
        .then(atom.repeated().collect::<Vec<_>>())
        .map(|(f, args)| { /* fold into Expr::Apply */ })
        .boxed();               // fix 1: base of the precedence chain
// … precedence chain (unary → arith → cmp → arrow → annotation → lambda) …
let parser = term4
    .then(/* `=>` chain */)
    .map(/* … */)
    .validate(/* … */)
    .boxed();                  // fix 2: top of the chain, handed to `recursive`
```

Both are pure type erasure — the parsed output (`Expr`) is unchanged, and all
`cargo test -p lichen-language-parser` cases (39) pass.

## Optimization boundary

Measured in the worktree (`CARGO_INCREMENTAL=0`, warm deps):

| state | `cargo check` | `cargo build` (parser alone) |
|---|---|---|
| original (no box) | ~86 s | ~96 s (cold, incl. deps) |
| + box `application` | ~6.8 s | — |
| + box `application` **and** top of `expression` | **~2.1 s** | **~6.5 s** |

The boundary is reached with the **two** boxes above.

- The marginal gain from a third box is small, and it is *not* free: boxing at the
  statement / block level (`statement_list`, `statement`, `block_body`) forces the
  generic `expr: impl Parser + Clone` parameter to carry `+ 'a`, and that bound
  cascades down through every combinator helper that threads `expr` (`block`,
  `block_statement`, the sub-parser helpers) — rustc then rejects the change (7
  `E0309` lifetime errors) unless the whole chain is annotated. The type-check win
  those boxes buy is only ~0.1–0.4 s, so the invasive lifetime churn is not worth it.
- After the two boxes the crate is no longer frontend-bound. The remaining ~6.5 s is
  mostly **codegen + debug info** (~4.4 s), not type-check (~2.1 s). `.boxed()` can
  only shrink the frontend, so it cannot reduce the crate below its codegen time —
  that is the hard floor for this grammar.
- To go lower you would have to cut codegen itself: a workspace profile with
  `debug = 0` / `debug = "line-tables-only"` (dev builds) — or, deeper, replace the
  chumsky combinator grammar with a hand-written recursive-descent parser (the
  direction the `frontend/chumsky` branch explores).

## Historical note (pre-split `lichen-language`)

A single `.boxed()` in the old `atom_parser()` once cut a ~10-min clean build to
~8.5 s, and additional boxes at the time gave no gain (some slightly worse). That
held for the old, flatter grammar; the re-grown grammar of the split parser crate
needs the two boxes above. The overall lesson is unchanged: box at the chokepoint
where the type is *threaded*, not everywhere.
