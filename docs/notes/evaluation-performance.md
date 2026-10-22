# Evaluation performance: the unification walk

> Status: current — the walk allocates nothing per array pair, keeps its recursion
> buffers, and cuts two frozen cycles it used to descend to the depth bound.
> Points at: `crates/lichen-lowlevel/src/equality.rs` (`unify`, `unify_array_items`,
> `layout_len`/`layout_entry`, `fill_class_holes`, `propagate_class_value`),
> `crates/lichen-lowlevel/src/evaluation.rs` (`evaluate_class_first`),
> `crates/lichen-lowlevel/src/ancestors.rs` (`AncestorPairs::clear`), and the probe
> `crates/lichen-tools/examples/core_profile.rs`.

## Symptoms

The probe is the measurement rig: `cargo run --release -p lichen-tools --example
core_profile` compiles and runs every `examples/*.lichen` in one process behind a
counting allocator, and prints per program the wall clock, the allocation count, the
bytes allocated, the peak live bytes, and the store's own share. The run's phases are
split by compiling the same source a second time without imports, which is the check
alone.

Baseline on this machine (dev at `847c40d5`):

| reading | value |
|---|---|
| corpus (46 programs, one process) | **1 108 ms**, 5 125 581 allocations, 976 755 KiB allocated, peak 17 661 KiB |
| `recursion.lichen` (`fib 10`, 106 bytes) | **174 ms**, 754 827 allocations, 126 372 KiB, peak 16 789 KiB |
| the same program's *check* | 7.8 ms of the 174 |
| the per-run fixed cost (`new_store`) | 4.2–5.5 ms, 18 703 allocations, 6 747 KiB |

So a six-line recursive program spent 166 of its 174 ms computing, and one eight-character
expression made **754 827 allocations**. The check was not where the time went; the run
— the deep pass forcing the program — was.

## Where the allocations were

Instrumenting the evaluator's hot paths with temporary counters (the probe's numbers are
the record; the counters themselves are not in the tree) put 465 811 of the 754 827
allocations inside `Module::unify`, and an exact-size histogram named them: **414 794 of
8 bytes and 205 559 of 24 bytes**, which is 82 % of the run.

The site is `unify_array_items`. Every array pair that carries an attribute layout (every
language slot does) allocated **three** `Vec`s per comparison:

- `array_layout(&a)` and `array_layout(&b)` copied the layout out of the registry
  (`Vec<u32>` of the layout's length — 8 bytes for a two-attribute slot), and
- `canonicals` collected both layouts' union, sorted it, and deduplicated it (24 bytes).

Unification descends an array pair's positions and attributes on every apply, so a
program's allocation count was a multiple of its unification steps.

Three smaller sites had the same shape: `propagate_class_value` and `fill_class_holes`
collected a class's members into a fresh `Vec` before walking them, `evaluate_class_first`
did the same for its snapshot, and each `unify` call built a fresh path set
(`AncestorPairs`) and step stack (`Vec<UnifyStep>`) and grew them again from empty.

## The frozen cycle that never cut

The counters also exposed a second cost, larger than the allocations: **1 053 084
`unify_inner` calls for `fib 10`**, of which **781 750 sat at depth ≥ 15** of a bound of
64.

The descent was a pair of *frozen* nodes: `[value, type, attributes]` arrays whose type
slot is a function type, and a function type is the function itself. The recursion guard
is a set of node **pairs on the current path**, and it cannot see a static ref: `Side::of`
turns a static id into a `value` with no node, so the pair `(static ref, static ref)` has
no name. Each level of the descent re-read the same frozen node into the same pair again,
so nothing matched, and the comparison ran to `MAX_VALUE_DEPTH` before the bound gave it
the benefit of the doubt. Every apply paid it.

## The fixes

None of them changes what the walk decides:

1. **The layout merge allocates nothing** (`unify_array_items`, `layout_len`,
   `layout_entry`). Both layouts are already sorted, and an entry's slot is its own
   position in the payload, so the union is merged in place: two cursors, one entry read
   per step, no `Vec` and no binary search.
2. **One payload compared with itself answers at once.** `a == b` on the two `ArrayValue`
   handles is the same frozen structure read through two paths; there is nothing to
   descend and no dynamic class to merge.
3. **Two views of one frozen node are one value.** In the array descent, a pair of equal
   `AnyNodeId::Static` refs returns without recursing. That is what cuts the function-type
   cycle above: the coinductive reading — a node compared with itself agrees — is the one
   `path` already applies to node pairs, extended to the refs it cannot name.
4. **The recursion buffers are the module's** (`unify_path`, `unify_steps`,
   `class_snapshot`). `unify` takes them with `mem::take`, clears content and keeps
   capacity, and puts them back; `evaluate_class_first` does the same for its member
   snapshot. A nested walk takes an empty one and leaves the holder's own to the holder.
5. **Two class walks read the member list in place** (`propagate_class_value`,
   `fill_class_holes`): the body only writes value slots, so the list cannot change under
   it and no snapshot is needed.
6. The apply path's `DBG_ARG` environment probe — a lookup on **every** apply — is gone.

## Measurements

Alternating A/B against a worktree at the baseline commit, release, same machine
(`core_profile`, medians of five sub-runs each):

| reading | before | after | |
|---|---|---|---|
| `recursion.lichen` | 174 ms | **56 ms** | 3.1× |
| its allocations | 754 827 | **63 206** | 11.9× |
| its bytes allocated | 126 372 KiB | **44 273 KiB** | 2.9× |
| corpus (46 programs) | 1 108 ms | **686 ms** | 1.6× |
| corpus allocations | 5 125 581 | **2 477 884** | 2.1× |
| corpus bytes allocated | 976 755 KiB | **821 468 KiB** | 1.2× |
| corpus peak live | 17 661 KiB | 17 663 KiB | unchanged |
| corpus nodes built | 102 623 | 102 623 | unchanged |

The check phase is unchanged (~81 ms for the corpus): the win is in the run, which is the
deep pass forcing the program. Node counts identical means the graphs built are the same
graphs — this is a walk that stops paying for what it already knew, not a change of
semantics.

## What is knowingly unchanged

- **The 8 red examples are pre-existing.** At the baseline commit, `decode`,
  `decode_loop`, `generate`, `kvcache`, `stencil`, `topk`, `torus` and `worldgen` fail
  their kernel-body lowering (`a kernel body reached a node with neither a value nor an
  operation`); the probe reports the same 8 before and after, and the owner of that work
  is the parallel `fix/selector-clone` branch. The examples' declared-output guard
  (`crates/lichen-tools/tests/tools/examples.rs`) is red at the baseline for the same
  reason.
- **The check phase's own cost** — the checker building and unifying the program — is
  untouched by this note. It is the larger half only for programs that do not run.

## What is left

- **The deep chains that remain**: 178 629 `unify_inner` calls for `fib 10`, of which
  111 250 are still at depth ≥ 15. The surviving pair is *(dynamic node, frozen ref)*,
  which no current guard names; a path guard over `AnyNodeId` pairs would cut it. It is
  a candidate, not a landed change.
- **The per-run fixed cost** was the next target, and it landed:
  [lazy-plugin-registration](lazy-plugin-registration.md) makes the compute sub-language's
  wrapper compile on the import that names it, which took a 91-byte program from 6.9 ms to
  2.4 ms and its peak live from 3 727 KiB to 1 017 KiB.
