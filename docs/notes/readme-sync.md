# README example sync

> Status: current
> Points at: `crates/lichen-tools/src/readme.rs` (the renderer),
> `crates/lichen-tools/src/bin/sync-readme.rs` (the on-demand command),
> `crates/lichen-language/tests/readme.rs` (the self-healing check), and the
> `examples/` tree itself. The `---…---` block *syntax* is the spec's business:
> [language-spec.md §2.2](../language-spec.md).

The top-level README's **Examples** section is not hand-written: it is rendered from
`examples/` — the **living spec** — so it cannot silently
drift from what the language actually prints. Each example is shown as its whole source
file, `---…---` block and all; the block's `output = "…"` metadata is the program's *real*
output, so the README carries verifiable truth rather than a promise, and each program's
own file documents exactly what it evaluates to.

The tooling lives in **`lichen-tools`**, a crate of its own: it is repository maintenance,
not a compiler surface, so an embedder of `lichen-language` links neither the generator nor
its `CARGO_MANIFEST_DIR`-relative tree walk. A run outside the repository — the
relative `examples/` absent — reports the unreadable path and exits non-zero.

## Why it exists

- The examples are the executable spec; a stale README would teach the wrong language.
- The `output =` metadata is computed, not asserted, so changing a program resyncs its
  documentation instead of leaving a hand-written claim behind.
- The renderer is exercised through the live example set rather than a unit-test fixture: the
  examples are a moving spec, so asserting the rendered blob in a unit test would force a test
  edit per add, rename or reorder.

## The three moving parts

1. **`sync_output_comments()`** — rewrites every example's `output = "…"` metadata to the
   program's actual output (appending it when the file has none). This is what keeps the
   `output =` in each file truthful.
2. **`render_examples()`** — walks `examples/` as a tree and renders each program
   (and each directory as one unit) into the markdown body between the
   `<!-- begin: examples -->` / `<!-- end: examples -->` markers.
3. **`replace_examples()`** — splices that blob into the README, replacing only the marked
   region so the heading and lead-in stay untouched.

## Directories and ordering

A directory is one unit. Its `_.lichen` program opens the section and carries the
directory's `order =`; its files and nested directories follow. Every entry sorts by its
`order = "N"` metadata (undeclared entries last, ties by name), so placement is declared
in the block, not hard-coded in the renderer.

Nested directories render the same way to any depth, one heading level deeper each time, and
every program renders as its whole source file. The programs run standalone wherever they
sit: a block's `name = import "path"` entries resolve relative to their own file.

The walk is bounded by `MAX_DEPTH`. A directory symlink pointing back into the tree would
recurse forever, so the walk that finds it is the one that must stop; the bound is generous
for `examples/` and exceeding it is reported, never a silent truncation.

## Keeping it in sync

- `cargo run -p lichen-tools --bin sync-readme` regenerates and writes the section on
  demand — run it right after changing an example to commit the result.
- `cargo test` self-heals the *generated* part: `tests/readme.rs` rewrites the README's
  example section in place on drift, so a stale README fixes itself on the next test run
  instead of failing the suite. A program's declared `output =` is not among the things it
  rewrites, and the next bullet says why.
- `sync-readme` is idempotent: running it on an already-synced tree produces the same
  README.
- The README's embedded example section is **derived documentation**, so `tests/readme.rs`
  rewrites it in place on drift instead of failing. An example's own `output = "…"`
  metadata is the opposite case — a claim about observable behaviour — so
  `tests/examples.rs` asserts it against the program's actual output and fails on a
  difference. That is why `readme::sync_output_comments` is called only by the `sync-readme`
  binary and never from `tests/readme.rs`: `cargo test` heals the generated section, not a
  program's declared output.
- A behaviour change therefore belongs in a reviewable diff: update the `output =` entry in
  the same commit, or run `sync-readme` on demand. A silently rewritten declaration would make
  a behaviour change look like a formatting change.
- `WORK_IN_PROGRESS` ignores examples one at a time, not the suite — every other program is
  still the living spec — and its entries name the example's **path relative to `examples/`**
  (e.g. `import/_.lichen`), because a directory's `_.lichen` face shares its basename with
  every other directory's: a basename list would skip or unskip the wrong one.

## Where the detail lives

The rendering rules and the marker logic are documented in the `readme` module's rustdoc
(`crates/lichen-tools/src/readme.rs`); this note is the "what/why" and the commands. The
`order` / `output` / prose metadata that feed this are covered in [packages.md](packages.md).
