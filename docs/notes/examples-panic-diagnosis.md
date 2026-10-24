# The `examples` binary panics: `invalid SlotMap key used`

> Status: **historical — diagnosed, then fixed by disabling the read-path trigger.**
> The `examples` test binary in `lichen-compute` panicked at
> `crates/lichen-lowlevel/src/evaluation.rs:517` with `invalid SlotMap key used`,
> taking the whole binary down and masking the examples-level verdicts it would
> otherwise report. The panic was a **stale `NodeId`** — a node that `drop_block`
> had already removed — reached by a deep walk, which is the failure `drop_block`'s
> own `# Invariant` sentence names. It no longer reproduces: the maintainer ruled the
> engine-side collection trigger off until its owner repairs it
> ([gc-trigger-disabled](gc-trigger-disabled.md)), and
> `examples::every_example_runs_and_prints_what_it_declares` passes again
> (measured on the merged tip after the ruling). The chain below is the record of the
> measurement, and its line numbers are **as of that measurement** — the files have
> grown since, and where they did, the note kept the number rather than re-deriving
> it.
>
> The two probes named below are **not stale**: they are the survey that reopened
> this question for the collector's owner, and they stay while the repair is open.
>
> No production behaviour, no `Invariant`/`Safety` sentence, no example and no
> test expectation was changed to produce this. Two probe test files are left
> behind; see **What is left behind**.
>
> Measured, inferred and unknown are kept apart below, and each claim says which
> it is.

## The entry point

`NodeId` is a `slotmap::new_key_type!` key
(`crates/lichen-lowlevel/src/lib.rs:602`) indexing
`Module::nodes: SlotMap<NodeId, Node<P>>` (`crates/lichen-lowlevel/src/lib.rs:849`).
`invalid SlotMap key used` is slotmap's `Index` impl finding a slot that no longer
holds the key's version.

The site, in `Module::evaluate_node_deep_inner` (`evaluation.rs:501`), is the
first indexing of the walked node:

```rust
// A structural cycle is cut here: the node is being deep-evaluated by an outer frame and
// already holds its cached value.
if self.nodes[node].visiting
    && let Some(value) = self.nodes[node].value
```

## The call chain (measured, from `RUST_BACKTRACE=1`)

Read by hand before running, this looked like it went through
`render_build` (`crates/lichen-language/src/run.rs:149`). **Measurement corrected
that**: the panic happens during *compilation*, and `render_build` is never
reached. The measured chain, outermost first:

1. `crates/lichen-compute/tests/lichen_compute/examples.rs:42` — the single `#[test]` calls
   `readme::program_output(&file, &source)`.
2. `crates/lichen-tools/src/readme.rs:150` — `evaluate_raw(source, Some(file), &mut store)`
   over a fresh `new_store(None)` built at `readme.rs:149`.
3. `crates/lichen-language/src/run.rs:223` — `evaluate_raw` → `evaluate_raw_reporting`.
4. `crates/lichen-language/src/run.rs:247` — `compile_with_imports_at(..., Some(store.registry()), …)`.
5. `crates/lichen-language/src/lib.rs:231` → `lib.rs:281` (`compile_with_imports_at_with_cells`)
   → `lib.rs:323` (`build_report`).
6. `crates/lichen-highlevel/src/checker.rs:446` (`build_in_attr_native`) →
   `checker.rs:610` (`build_with`) → `module.evaluate_node_deep(checker.root_term, None)`.
7. `crates/lichen-lowlevel/src/evaluation.rs:447`/`:454` — the deep walk starts.
8. Down through `evaluate_node` → `evaluate_node_operation` → `function_apply`
   (`crates/lichen-lowlevel/src/function.rs`) → `evaluate_node_body` →
   **`Module::evaluate_block`** (`evaluation.rs:643`) — the `@gc` scope.
9. `evaluate_block` runs its **own** `evaluate_node_deep` (`evaluation.rs:648`),
   which descends into array items at `evaluation.rs:559`.
10. The panic at `evaluation.rs:517`.

Frames 22–36 of the backtrace are the give-away: `evaluate_block` sits *under* a
`function_apply`, so the scope being collected is the one inside an applied
function body.

## Stale, not foreign (measured)

Instrumented temporarily at the two sites and reverted; the tree carries neither
print today.

- `crates/lichen-lowlevel/src/gc.rs:265` (`drop_block`) logged
  `removed=NodeId(456v1) from block=BlockId(12v1) left=688` — the map went from
  **688** live nodes to **687**.
- The next walk logged `missing=NodeId(456v1) nodes_len=687`.

So the key dereferenced at `evaluation.rs:517` is **exactly the key `drop_block`
had just removed**, one statement earlier in the same run. Index 456 sits inside
a 687-entry map and carries version 1, so that slot was allocated once and
removed without reuse — a key from **this** module's own table, not another
map's. **Stale.**

`crates/lichen-lowlevel/src/gc.rs:232` states the contract this violates:

> **Invariant** — No live node outside the block still references one inside it:
> evaluating a surviving reference to a released block panics.

The panic *is* that sentence firing. The surviving reference is what the checker
leaves behind when an `@gc` scope inside an applied function is collected.

The only two `SlotMap` removals in the crate are `gc.rs:265` and
`ancestors.rs:49`; the latter is a `HashSet::remove` on the ancestor path, not a
node removal, so `drop_block` is the only candidate.

## Why the examples family, and not others (measured)

The runner's setup is **not** the cause, and this is the part the by-hand reading
got wrong:

- **No shared slot map across examples.** `program_output` builds a fresh
  `new_store(None)` per program (`readme.rs:149`), and every program compiles into
  its own `Module`.
- **No cross-example contamination.** `examples/gc.lichen` run **alone**, in a
  fresh process, through the real `program_output` path, still panics.
- **One example of forty.** A per-example survey of all 40 discovered examples
  found exactly one panic — `gc.lichen` — and 34 clean, with `compute_jit.lichen`
  skipped as work in progress.

The family differs because it runs **complete real programs** that combine an
`@gc` scope *inside a function body* with a store-backed compile. The lowlevel
`@gc` tests call `garbage_collect` directly on hand-built IR
(`crates/lichen-lowlevel/tests/basic/gc_*.rs`) and never produce this shape, which
is why the gap is unnoticed.

What the setup contributes, isolated one factor at a time:

| Program under test | Registry | Base path | Result |
| --- | --- | --- | --- |
| `gc.lichen` text | `None` (`testing::evaluate`) | none | **OK** |
| same text | `Some(store)` | `Some(file)` | **PANIC** |
| same text | `Some(store)` | none | **PANIC** |
| same text | bare `PackageStore::new()`, compute package **not** registered | `Some(file)` | **PANIC** |

So the trigger is the **program text plus any store** — not the base path, not the
import base, not the embedded compute package, and not the file on disk (the same
text inline as a string panics identically).

## Environmental, or a defect on `dev`? (measured: a defect)

It is a defect, not a missing template, prelude or fixture:

- It reproduces with a **bare** `PackageStore::new()` — the embedded compute
  package is never registered, so no fixture is missing.
- It reproduces from an **inline source string**, so the example file's presence
  in `examples/` is irrelevant (this is also why removing the newest example
  changes nothing).
- It reproduces in a **fresh process on the first compile**, so nothing is
  inherited from an earlier evaluation.
- `docs/notes/code-audit.md` carries **no `wontfix:D<n>`** over this behaviour, so
  no decision record makes this the accepted semantics.

## The smallest reproduction

One file, one process: `examples/gc.lichen` compiled through `program_output`.
There is **no test-name filter** that isolates it — `examples.rs` is a single
`#[test]` looping every example — and `--test-threads` cannot help, for the same
reason. The probe here reproduces it with `PROBE_EXAMPLE=gc.lichen`:

```
cargo test -p lichen-compute --test lichen_compute -- --exact lichen_compute::zz_examples_panic_probe::per_example_survey --nocapture
```

## What the runner says once past the panic

The per-example survey reproduced the runner's own comparison (declared
`output =` versus actual) and found **exactly the four drifts the panic was
masking**. They hold:

| Example | Declared | Actual |
| --- | --- | --- |
| `assert.lichen` | `(1, 5): tuple{Int, Int}` | `(raw 1, 5): tuple{?a, Int}` |
| `assert_in_function.lichen` | `1: Int` | `raw 1: ?a` |
| `import/_.lichen` | `(42, 10, 7): tuple{Int, Int, Int}` | `(raw 42, 10, raw 7): tuple{?a, Int, ?b}` |
| `lazy_infinite.lichen` | `(1, 2, 3): tuple{Int, Int, Int}` | `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}` |

34 examples matched their declaration and `compute_jit.lichen` is skipped by
`examples.rs:10`. The four drifts share one shape: an integer printed `raw` and
an element typed `?a` where `Int` is declared.

**The shape is one; the cause is not.** The shape is one *read*: the type printer
and the value printer ask the same empty cell the same question, and
`raw 1: ?a` is that one answer printed twice
([undecided-type-cell](undecided-type-cell.md) §2). Behind it are **three**
mechanisms, one per expr — a routed comparison, a lazy read, a frozen-module read —
measured with one program per discriminator and recorded with the CLI's output
alongside the suite's in [undecided-type-cell](undecided-type-cell.md) §1, §3 and
§4. The four are also **not** the `@loop` finding: the marker changes nothing for
any of them. Each program was additionally run through
`cargo run -q -p lichen-compiler -- run <file>` and the CLI agrees with this
suite byte for byte, so the defect is in the language and not in this runner's
rendering.

## When it started

**Bounded, not pinned.** `examples/gc.lichen` and the `@gc` feature were added by
the same commit, `9a5c2747 feat(gc): @gc e, an expression in a scope of its own`
(2026-10-12), which is 241 commits behind `dev`. The panic therefore cannot
predate that commit. Three later commits touch `@gc`
(`81a83df1`, `1f64e8b4`, `02d3f21b`) and are the only candidates for a regression;
narrowing to one of them is a bisect, which this slice did not run. **Unknown.**

## Measured / inferred / unknown

- **Measured** — the entry point and its measured call chain; the key is stale,
  named `NodeId(456v1)`, removed by `drop_block` from `BlockId(12v1)`; the
  trigger is the program text plus any store; `gc.lichen` panics alone in a fresh
  process; only 1 of 40 examples panics; the four drift names and their declared
  versus actual strings; the lower bound `9a5c2747` on the start.
- **Inferred** — that the surviving reference is one the checker leaves behind
  when an `@gc` scope inside an applied function is collected. The removal is
  measured; *which* edge still points at the released node is not.
- **Unknown** — which commit introduced the regression between `9a5c2747` and
  `dev`; the identity of the surviving edge; whether the four drifts and the panic
  share a cause (they look unrelated — the drifts are a rendering difference, the
  panic is a released-block reference).

## What is left behind

Two probe test files, both printing and asserting nothing, left in the tree while
the investigation is open:

- `crates/lichen-tools/tests/tools/zz_examples_panic_probe.rs` — the per-example
  survey: names the culprit and collects the drift verdicts. Honours
  `PROBE_EXAMPLE=<name>` to run one example alone.
- `crates/lichen-compute/tests/lichen_compute/zz_gc_panic_probe.rs` — the setup discriminator
  table above.

Neither changes a production file, an example, or a test expectation. The two
temporary instrumentation prints (`evaluation.rs:517` and `gc.rs:265`) were
reverted and are **not** part of this note's branch.
