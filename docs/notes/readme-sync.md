# README example sync

> Status: current
> Points at: `crates/lichen-language/src/readme.rs` (the renderer), `src/bin/sync-readme.rs`
> (the on-demand command), `tests/readme.rs` (the self-healing check), and the
> `examples/programs/` tree itself. The `@{…@}` block *syntax* is the spec's business:
> [language-spec.md §2.2](../language-spec.md).

The top-level README's **Examples** section is not hand-written: it is rendered from
`crates/lichen-language/examples/programs/` — the **living spec** — so it cannot silently
drift from what the language actually prints. Each example is shown as its whole source
file, `@{…@}` block and all; the block's `output = "…"` metadata is the program's *real*
output, so the README carries verifiable truth rather than a promise, and each program's
own file documents exactly what it evaluates to.

## Why it exists

- The examples are the executable spec; a stale README would teach the wrong language.
- The `output =` metadata is computed, not asserted, so changing a program resyncs its
  documentation instead of leaving a hand-written claim behind.

## The three moving parts

1. **`sync_output_comments()`** — rewrites every example's `output = "…"` metadata to the
   program's actual output (appending it when the file has none). This is what keeps the
   `output =` in each file truthful.
2. **`render_examples()`** — walks `examples/programs/` as a tree and renders each program
   (and each directory as one unit) into the markdown body between the
   `<!-- begin: examples -->` / `<!-- end: examples -->` markers.
3. **`replace_examples()`** — splices that blob into the README, replacing only the marked
   region so the heading and lead-in stay untouched.

## Directories and ordering

A directory is one unit. Its `_.lichen` program opens the section and carries the
directory's `order =`; its files and nested directories follow. Every entry sorts by its
`order = "N"` metadata (undeclared entries last, ties by name), so placement is declared
in the block, not hard-coded in the renderer.

## Keeping it in sync

- `cargo run -p lichen-language --bin sync-readme` regenerates and writes the section on
  demand — run it right after changing an example to commit the result.
- `cargo test` self-heals: `tests/readme.rs` resyncs the README and the `output =`
  metadata in place on drift, so a stale README or stale metadata fixes itself on the
  next test run instead of failing the suite.

## Where the detail lives

The rendering rules and the marker logic are documented in the `readme` module's rustdoc
(`src/readme.rs`); this note is the "what/why" and the commands. The `order` / `output` /
prose metadata that feed this are covered in [packages.md](packages.md).
