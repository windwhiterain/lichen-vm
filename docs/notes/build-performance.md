# Build performance: the chumsky `.boxed()` fix

> Status: historical — a past investigation, kept for context; not an authority on
> current behaviour.
> Points at: `crates/lichen-language/src/parse.rs`, `atom_parser()`.

A clean build of `lichen-language` once took ~10 minutes even though the workspace
dependencies compiled in ~15 seconds. The culprit was neither codegen nor the
dependencies: it was rustc's frontend spending ~9 minutes type-checking one large
`chumsky` parser-combinator type in `parse.rs`.

The fix was a single `.boxed()` call in `atom_parser()`, which erases the monolithic
parser type before it is threaded through the expression precedence chain. A clean
build dropped to ~8.5 seconds.

- More `.boxed()` calls elsewhere in `parse.rs` gave no meaningful gain; some made the
  build slightly worse.
- Conclusion: one strategic type-erasure call, not many — the minimal, focused fix.
