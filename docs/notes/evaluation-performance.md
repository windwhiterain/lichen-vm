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

The `STAGES` environment variable adds the store's own two stages — `register_compute`
(the plugin's wrapper) and `prelude_import` (the core prelude) — which are two separate
compiles, so the fixed per-run cost is attributed rather than measured whole.

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

## Round 5: the clone walk reads a node once, and the class walk makes one pass

Timing `node_apply`'s segments rather than guessing found the opposite of what the
inclusive totals suggested: the per-visit *bookkeeping* is cheap — membership test 24–26 ns
(36 187 calls for `fib 10`), class low type 24 ns, the `origin` chain walk 23 ns,
`value_holds_foreign_function` 165 ns (5 738 calls) — and what the walk actually pays for
is the node's own facts read one lookup at a time. `node_apply` asked the slot table for
`self.nodes[node].value` twice in a row (a duplicated `or_else`), then again for
`.function`, `.operation`, `.evaluated_deep`, `.block` and `.origin`: six to eight lookups
per visit, 60 881 visits. They are now one read into locals.

`evaluate_class_first` had the same shape: a `class_members` walk to build a snapshot, then
a second pass over it re-reading every member. A member that does not owe an answer when
the snapshot is taken never owes one later — `value` is never cleared and `runned` only
grows — so the pass now pushes only the owing members, and the turn still re-reads
`visiting`, the one fact a nested read can change. One pass instead of two.

| reading | round 4 | round 5 | vs. the original |
|---|---|---|---|
| `recursion.lichen` | 33.5 ms | **32.9 ms** (median of 4) | 174 → 32.9 ms (5.3×) |
| its allocations | 34 681 | **34 671** | 754 827 → 34 671 (21.8×) |
| `mathlib.lichen` | 22.0 ms | **21.6 ms** | |
| corpus (46 programs) | ~488 ms | **~486 ms** | |

The corpus is flat: it is dominated by the compute examples, whose check is the checker's
graph construction, not the apply walk. The two readings that move are the apply-heavy
ones, which is where the lookups were.

Where `fib 10`'s remaining 33 ms goes, measured: `unify` 18 841 calls of which only 2 338
(12 %) are same-class; `add_node` 66 548 calls at ~85 ns; the class-first walk's ~400 000
member reads at 15–25 ns; the walk's own per-visit arithmetic under 6 ms in total.

## Round 4: the apply tail stops building two buffers per call

`apply_parameter_check` compared a wide pair by **collecting both sides into a fresh
`Vec<NodeId>`** on every apply, and its pattern walk started from a fresh
`HashSet` each time. A language pair is three wide (value, type, attributes), so every
apply took that path: two `Vec`s and one hash set per call, each growing by doubling as
the walk filled it.

Both buffers were unnecessary. `Module::array_items` hands back the payload's own
`&'static [ArrayItem]`, which the tail now indexes directly — no collection, and the
`unsafe` slice does not borrow the module, so the per-slot `as_dynamic`/`unify` calls keep
their `&mut self`. The visited set became a per-module stack, popped and pushed back empty,
like the class snapshots and clone buffers before it.

Measured on the same probe (release): `recursion.lichen` **40 394 → 34 681** allocations
and ~42 → **33.5 ms**; `mathlib.lichen` 85 411 → 83 541 and ~25 → **22 ms**. The suite
readings and all 45 example outcomes are unchanged.

| reading | round 3 | round 4 | vs. the original |
|---|---|---|---|
| `recursion.lichen` | 41.9 ms | **33.5 ms** | 174 → 33.5 ms (5.2×) |
| its allocations | 40 394 | **34 681** | 754 827 → 34 681 (21.8×) |
| its bytes allocated | 34 337 KiB | **31 176 KiB** | 126 372 → 31 176 KiB |
| `mathlib.lichen` | 25.1 ms | **22.0 ms** | |
| its allocations | 85 411 | **83 541** | |

## Round 4's rejected idea: a persisted prelude artifact

The prelude is compiled from source in every store that has no shared registry: 1.5 ms of
check plus 0.3 terms, 0.8 freeze. It was implemented as a cache — the frozen module written
beside a sidecar carrying its `direct` names and its source record, filed outside the
device's index because a published prelude artifact would change **every** dependent
package's identity (the artifact fold gives an unpublished dependency the all-zero
sentinel, and `language::persist` asserts that contract).

It works and it is faster **in a warm process**: the load is 0.47–0.75 ms against a
compile of 2.4 ms of measured stages. It does not move the shipped CLI, measured three
ways on `examples/array.lichen` (a 91-byte program, so the fixed cost dominates):

- prelude compiled vs loaded in one binary, interleaved, 8 runs: **20.15 → 15.81 ms**;
- the same binary against the pre-change binary, 15 runs: **16.76 vs 16.66 ms** median,
  14.80 vs 14.45 ms minimum — no difference;
- the compile's own timer reads 8.5–10 ms in a fresh CLI process, against 2.4 ms of
  stages measured in a warm one.

The difference between the first two readings is the finding: a fresh process pays the
prelude's page faults somewhere, and the checker it faults in is code the program's own
compile needs anyway. The win exists only for a store created in an already-warm process,
and no shipped path creates a second cache-backed store. The change was reverted rather
than landed, and the artifact's ~130 KB write, the identity ripple and the source-record
sidecar are the reasons a second attempt needs a buyer, not the code.

## Round 3: a merge of two decided classes has nothing to fill

Timing the entries rather than counting calls found the merge's own walk: `add_equality`
ran `fill_class_holes` on **every** merge where either side held a value, and for
`fib 10` that was 21 751 walks of up to a couple hundred members — **21.1 ms** of a
43 ms check, the single largest line item after evaluation itself.

The walk is a no-op in the common case. **A class that holds a value has no holes**: a
value write (`propagate_class_value`) and a hole fill both write *every* member, which is
precisely the invariant `unify_inner` already reads a member's slot by ("Ask each class's
value once, of the class: the representative may be a valueless node"). A merge of two
classes that both hold a value therefore has no empty slot to fill — and that is the
common case, since a comparison merges equal values. Only the merge with a class that
holds *nothing* has holes.

`add_equality` now fills only then. Measured on `fib 10`: 8 404 of 21 751 merges skip the
walk, and the merge's inclusive time falls 21.1 → **9.5 ms**; `recursion.lichen`
**49.6 → 41.9 ms** and `mathlib.lichen` **28.7 → 25.1 ms** (alternating A/B, medians of
five), the corpus 716 → 681 ms (45 programs — `gc.lichen` is dropped from this reading
because an unrelated regression panics in it, see below).

Two slot-table reductions ride along, both exact:

- `fill_class_holes` and `propagate_class_value` read the member's link and its slot
  through **one** `SlotMap` lookup instead of two;
- `evaluate_class_first` reads the three facts a member's turn depends on (visiting,
  operation, run stamp) through **one** lookup instead of three.

## Round 2: the frozen pair guard, the node slot, and the per-apply buffers

Three changes, each measured on its own, all on the same probe:

1. **The path guard now names frozen refs.** `unify_inner` descends a pair of frozen
   nodes as *one bare value against another*, so the class-level `path` set — which is
   keyed on `NodeId` — cannot record it, and the same pair recurs level after level to
   the depth bound. `Module::unify_any_path` is the same guard keyed on `AnyNodeId`,
   carried only for the pairs a `Static` ref names (two dynamic ids are already the class
   guard's), and pooled on the module beside `unify_path`. The rule it applies is the one
   `path` already applies: a pair already on the descent is one structure, and the frame
   comparing it owns its interior, so the repeated visit adds nothing. Measured on
   `fib 10`: `unify_inner` **178 629 → 42 459** (−76 %), and `recursion.lichen`
   **53.4 → 43.4 ms**.
   *The maintainer approved this extension explicitly; the alternative that shares a
   materialized static leaf instead was declined, because a leaf is a cell a unification
   may write and sharing it changes cell identity.*
2. **The node slot is 144 bytes, not 160.** `Node::low_shape` was `Option<LowShape>` — 24
   bytes of a node paid for a slot that is `None` on nearly every node — and is now
   `Option<Box<LowShape>>`: the payload moves to the heap, where a shape is rare and
   already class-routed. Measured: `fib 10`'s peak live 15.77 → **14.86 MiB**, the
   corpus' 17.66 → **17.10 MiB**.
3. **The per-apply buffers are the module's.** Class-first member snapshots and array
   clone item buffers were a fresh `Vec` per read and per clone; both are now a stack the
   walk pops and pushes back empty, because both can re-enter themselves (a member's read
   starts another class walk; an array's items can hold another array). Measured on
   `fib 10`: **46 118 → 40 387** allocations; its bytes allocated 37.6 → 34.3 MiB.

| reading | round 1 | round 2 | vs. the original |
|---|---|---|---|
| `recursion.lichen` | 56 ms | **43 ms** | 174 → 43 ms (4.1×) |
| its `unify_inner` steps | 178 629 | **42 459** | 1 053 084 → 42 459 (24.8×) |
| its allocations | 63 206 | **40 387** | 754 827 → 40 387 (18.7×) |
| its peak live | 16 779 KiB | **14 865 KiB** | 16 789 → 14 865 KiB |
| corpus (46 programs) | 686 ms | **518 ms** | 1 108 → 518 ms (2.1×) |
| corpus bytes allocated | 821 468 KiB | **627 616 KiB** | 976 755 → 627 616 KiB |
| corpus peak live | 17 663 KiB | **17 097 KiB** | 17 661 → 17 097 KiB |

The corpus' allocation count is flat across round 2 (2.064 M → 2.061 M): its
allocation-heavy programs are the compute examples, whose cost is static-function
materialization and the checker's own graph, not the array-pair walk this round cut.

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

- **The per-run fixed cost is now the largest single line in the corpus reading, and it is
  not worth caching.** Phase timing separates `preprocess` (the store's own compiles) from
  the program's check: a store compiles the **core prelude** from source every time
  (1.5 ms of check plus 0.3 terms and 0.8 freeze for `core.lichen`), and a program that
  imports `compute.lichen` compiles that wrapper once per store too
  ([lazy-plugin-registration](lazy-plugin-registration.md) made it deferred, not cheap).
  Across 46 probe stores that is ~230 ms of the ~700 ms corpus total, but a persisted
  prelude artifact was measured and rejected: in a fresh process the compile's page faults
  are paid by the run anyway, so the CLI does not move (see round 4 above). The remaining
  lever here is a **shared registry** for the in-process multi-store cases (the example
  runner, a directory run, the server), which needs no persisted format.
- **The node count, not the node slot, is where the peak now is.** `fib 10` ends with
  65 391 live nodes for a six-line program: 16 130 are the clone walk's, 17 020 are fresh
  undecided cells minted for a shared wildcard node, and ~12 000 came from reading a
  frozen ref, which materializes a fresh leaf each time. Sharing a *decided* static read
  would cut most of that and the peak with it, but a leaf is a cell a unification writes,
  so it is a semantic change and is not taken.
- **The class walk is the last compressible item, and it needs a counter.** `fib 10`
  reads ~400 000 class members across 2 092 class-first walks — 15–25 ns each, because
  every member is a slot-table read of facts that mostly say "this member owes nothing".
  Skipping a walk whose class cannot owe needs a per-class flag maintained on creation and
  merge (the shape round 3 used for holes), and it only pays if most classes that are read
  hold no operation-bearing member; that has not been measured.
- **The clone walk itself** is no longer a mystery: its per-visit bookkeeping is under
  6 ms of `fib 10`'s 33 (round 5), so what is left of it is `add_node` (66 548 calls at
  ~85 ns) and the volume of nodes the representation creates. That volume is the same
  question as the peak above: sharing would change cell identity.
- **The upstream `gc.lichen` panic is not this note's, and it no longer reproduces.**
  `examples/gc.lichen` panicked with `invalid SlotMap key used` at
  `evaluate_pattern_argument_inner`, from a dynamic pattern id that was never in the slot
  table; it appeared at `fad1448e` (bisected: `93b4eb9c` is clean) and was masked by the
  examples guard stopping at `decode.lichen` first. At this commit the program runs to its
  declared answer, eight runs out of eight, both alone and in the corpus — but nothing here
  claims to have fixed it: the only changes since the reproduction are this note's walks,
  and a released-node read that a different allocation order can hide is still a released
  read. Its owner should confirm which it was before the guard stops masking it.
