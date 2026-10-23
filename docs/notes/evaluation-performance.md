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

## Round 13: what a repeated apply costs, and what a plan cache would be worth

The question: one function applied many times — a recursion, or a `@loop`'s host loop, which
calls `instantiate` once per iteration — re-walks its template on every call. Could the walk's
answer be cached? A temporary counter-and-clock probe in `function.rs` and `apply.rs` measured
three shapes, with its counters reset at `evaluate_raw` and only the
outermost window counted (an import re-enters `evaluate_raw`, so a naive probe reads a window
that a nested compile zeroed mid-run — the first reading of these numbers was wrong for
exactly that reason).

| shape | applies | walk visits | clones/apply | wildcard cells/apply |
|---|---|---|---|---|
| `recursion.lichen` (`fib 10`) | 187 | 42 887 | 85.5 | 91 |
| a `@loop` summing 200 down (`20100: Int`) | 411 | 96 857 | 121.7 | 57 |
| `mathlib.lichen` | 137 | 20 417 | 45.3 | 32 |

**Applies never nest**: the probe counted `instantiate` entries inside another `instantiate`
and read **0** on all three shapes, so the recursion descends through the body's evaluation
and the windows are flat. Shares of each program's own window (30.4 / 52.3 / 46.2 ms):

| reading | `fib 10` | the `@loop` | `mathlib` |
|---|---|---|---|
| inside `instantiate` | 10.43 ms (34.3 %) | 22.86 (43.7 %) | 4.27 (9.2 %) |
| the clone walk | 5.09 (16.7 %) | 15.37 (29.4 %) | 3.57 (7.7 %) |
| — of it, value mapping | 4.97 (16.4 %) | 15.12 (28.9 %) | 3.51 (7.6 %) |
| the clone verdict | 4.32 (14.2 %) | 12.72 (24.3 %) | 3.37 (7.3 %) |
| `apply_parameter_check` | 7.00 (23.0 %) | 6.44 (12.3 %) | 1.01 (2.2 %) |
| — forcing the argument | 5.10 (16.8 %) | 2.57 (4.9 %) | 0.47 (1.0 %) |
| clone-group unification | 0.90 (3.0 %) | 4.96 (9.5 %) | 0.34 (0.7 %) |
| identity value mappings | 180 / 10 314 | 0 / 25 822 | 412 / 4 940 |

The **clone verdict** is what a plan would replace: template membership, the template-root
walk, and the `depends_on_parameter` answer. It is cacheable in principle — **the decision set
is stable after the first calls**: 3 distinct sets per function over the whole run, two flips,
both inside the first ten applies. The flips name their own cause: a template node's
`evaluated_deep` goes undecided → decided as the deep pass settles the template, which turns
"clones" into "shared" (and `member` changes for nodes next to the parameter).

Three measured reasons the win is bounded. **The walk's clock is value mapping**: the carried
answer's array is re-allocated with this call's cells, and it is an identity mapping in 1.7 %,
0 % and 8.3 % of mappings — so sharing the template's payload instead of rebuilding it buys
nothing. **The per-call cells are not removable** (the clones plus the wildcard cells above).
And **the verdict is ~106 ns of work per visit, of which ~50 ns is cacheable**: the
membership chain walk (~25 ns) and the origin chain walk (~23 ns) go, while
`class_low_shape` (~24 ns, the clone's own seed) and `value_holds_foreign_function` (~22 ns,
the carry rule) stay. That is 2.3 ms (7.7 %) of `fib 10` and 6.6 ms (12.6 %) of the loop
shape, plus the per-visit `remap` hash lookup a plan would turn into an index.

The same probe names two items that are *not* the traversal: `apply_parameter_check`'s forcing
of the argument costs 5.1 ms on `fib 10` (16.8 %) for 187 applies — 31 µs each, with no apply
nested inside it, so it is evaluation and class work; and the clone-group unification is
4.96 ms (9.5 %) on the loop shape, where the template's classes are big.

Verdict: a plan cache is feasible but bounded by the mapping and the cell creation it cannot
skip, and it needs a second execution path for a walk whose rules (owner tag, block images,
one closure per call, the two carry gates) are the whole subject of
[`apply-clone-ownership.md`](apply-clone-ownership.md). Its invalidation would have to track
the very thing the probe shows moving: a template node's `value`/`evaluated_deep`. Not built.

## Round 12: the allocations, finished — and the one that cannot be finished

Three allocation wins landed after round 11, all behaviour-preserving and all without a
measurable change in wall clock:

- **a clone adopts its class's low shape by refcount** (`Option<Arc<LowShape>>`): the walk
  boxed a copy of the shape for every shaped clone. `fib 10` 34 671 → 32 871 allocations.
- **a check's location holds its single step inline** (`LocPath::Empty | One | Many`): every
  check built its `Loc` with `vec![step]`. `mathlib` 82 572 → 80 085.
- **the lowering moves a program's statements** instead of deep-cloning them before lowering
  (the fresh compile path is the program's last reader; the session path, which keeps its
  program, passes a copy). `mathlib` 80 085 → 78 202, `fib 10` 32 598 → 32 397.

The census behind the last one, for `mathlib`'s 13 KiB source: **lex 1 545 allocations,
parse 22 986, resolve-and-lower 55 268**. So "the parser" was never one bucket: what is left
in resolve is the IR's own per-expression arenas and span table, and in parse it is small
`Vec`s growing from empty plus one `String` per name.

**The name `String` cannot be removed at an acceptable price, measured twice.** The lexer
allocates one per identifier token and the parser clones it into the AST, so dropping either
end halves the cost. Both routes were attempted and abandoned:

- **Borrowing the token's payload from the source** (`TokenKind::Name(&'a str)`) makes
  `Token`'s lifetime flow into `lex_resume`, which reuses the *previous* source's tokens in
  a list that must borrow the *new* one — the borrow checker refuses, and rebinding each
  reused token means re-lexing it (slower than the clone it saves) or teaching the token to
  rebind without the raw kind, which is forty match arms.
- **Making the token fieldless and reading the text from `range`** (the token already
  carries `range`) moves the problem to the parser, which then needs the source — and this
  parser is a chumsky combinator grammar whose every rule is a `fn rule<'a, Er>() -> impl
  Parser<'a, In<'a>, …>`; threading `&str` through all of them is a parser-wide rewrite for
  two thousand allocations on a 13 KiB file.

So the allocation thread ends here, on the measured floor: what is left per program is one
`String` per name, the small `Vec`s the grammar collects, and the IR's own arenas.

## Round 11: where a bigger refactor could pay, measured

This round asked a different question — not "what is left to tune" but "what is left to
restructure for a larger gain" — so it measured the candidates a refactor could target
instead of tuning any of them.

**First finding: the corpus is not a language metric.** The 46-program corpus is dominated
by compute work: `worldgen.lichen` spends **156 ms of its 183 ms inside the compute
operator's `run`** (17 calls, 9.2 ms each — wasmi executing the kernels, since the probe has
no device), `decode.lichen` 4 of 25 ms, `mathlib.lichen` 5 of 28. A corpus total that moves
by 2 % says almost nothing about the checker; the language-heavy programs are the ones that
do (`recursion.lichen` 25 ms, `mathlib` 21–28 ms).

**Second finding, corrected: the clone groups are small, and counting them needs a per-phase
reset.** The round's first pass counted unifications over the whole process — the prelude's
compile and the probe's second, check-only compile included — and read 18 841 of them, most
of which looked like the clone groups' pairwise unification. Resetting the counters at
`evaluate_raw` and dumping after it, `fib 10`'s run is **15 980 unifications**, of which the
clone groups are **1 083 pairs**; the check-only compile adds 2 861 unifications and 359
pairs. The rest are **answer commits** (one per computed node, `write_node_answer` joining a
reader into its class) and the apply's own signature and parameter checks — inherent to the
representation, not to the grouping.

So the class-level clone memo that round 11 proposed is worth **≈2 %**, not 30–40 %: 1 083
fewer unifications and 1 083 fewer clone paths out of a 22.6 ms run. It is not worth
touching cell identity for, and the maintainer's approval of it was given on the wrong
number; the correction is the point of this round.

**The per-pass visited memo was tried and does not pay either.** Recorded as a proposal by
round 11's table, it was implemented (a per-pass set of the nodes answered with themselves,
checked after the wildcard branch and filled at the two "reference as-is" returns) and
measured: `fib 10`'s allocations rose by 704 — a set per pass, growing — and the wall clock
did not improve; the hash lookups cost what the saved verdicts were worth. Reverted.

Measured ceilings for the safe alternatives, with the per-phase numbers:

| refactor | `fib 10` | `mathlib` | class |
|---|---|---|---|
| per-pass visited memo in the walk | **tried: +704 allocations, no gain** | — | rejected |
| chunked node storage (no growth memcpy) | 295 growth allocations, 25 MB moved; ≈1–2 ms | — | exact, container refactor |
| `Arc`-shared `LowShape` for clones | 2 702 boxes (8 % of allocations) | 1 159 | exact |
| parser allocation reduction | — | 22 986 allocations (20 106 ≤16 B), ≈0.8 ms | exact |
| checker `Loc` (`vec![step]` per build) | — | 2 235 allocations | exact |
| `unify` self-pair fast path | 11 of 15 980 | 71 of 4 667 | worthless |

Where `fib 10`'s 22.6 ms actually goes, per phase: the prelude compile 2.7 ms, `add_node`
5.1 ms, `unify` 4.5 ms (15 980 calls), the walk's own arithmetic ≈8 ms over 42 887 visits,
the answer-class walks and the rest ≈2 ms. The synthesis is unchanged from round 11 and is
now measured rather than argued: **no behaviour-preserving refactor of the language core
exceeds ≈10–15 %**, and the volume behind that floor is the node count, which only the
declined cell-identity changes reduce.

## Round 10: the node slot is 128 bytes, not 136

The peak is the node slot: `fib 10` ends with 65 391 nodes, and at 136 bytes each that is
8.4 MB of a 14.9 MB peak. Two fields were holding more bytes than their values need —
`runned: u32` is only ever 0 or 1 (six sites, all writes), and `depth: u32` is bounded by
`MAX_APPLY_DEPTH` (10 000, an order of magnitude under `u16::MAX`). Narrowed to `bool` and
`u16`, they free 5 bytes of field text, which is enough to cross the struct's 8-byte
alignment: **136 → 128 bytes (−5.9 %)**.

| reading | round 9 | round 10 |
|---|---|---|
| `Node<LangProgram>` | 136 B | **128 B** |
| `fib 10` peak / allocated | 14 868 / 31 144 KiB | **14 355 / 30 088 KiB** |
| `mathlib` peak / allocated | 9 002 / 27 052 KiB | **8 745 / 26 380 KiB** |
| corpus peak / allocated | 17 105 / 607 353 KiB | **16 593 / 603 356 KiB** |

Time and allocation counts are unchanged within noise, all suite readings are unchanged,
and the 46 example outcomes are the same eight red ones.

What is left in the 128 bytes, and why each is structural: `value: Option<P::Value>` is 40,
whose `Array` payload is a 32-byte `AnyHandle` (a static handle carries a module key, an
offset and a length beside the dynamic pointer); `equality: Meta<NodeId>` is 28 — parent,
next, tail and size, where `tail` is what makes a class merge splice two member lists in
O(1), and dropping it would make `union` walk the shorter list on every merge.

## Round 9: the example runner shares a registry too, and the server already did

`readme::program_output` — the tool behind `sync-readme` and the examples guard — built a
store per program, so each of `examples/`' 46 programs compiled the prelude again. It now
keeps one registry **per thread** and gives each call a store over it; the prelude and the
plugin wrapper compile once for the run. A thread-local rather than a `static`, because
`Registry<P>` is neither `Send` nor `Sync` by design (round 8): the cache is exactly as
wide as the thread that owns it.

Measured: `sync-readme` 110–118 ms → **80–84 ms**, and the README and example metadata come
out unchanged (the tool is idempotent; `git status` stays clean after a run).

**The server needed nothing: round 8's report was wrong about it.** `WorkerState` holds one
`registry: Arc<RwLock<Registry<P>>>` and every run's store adopts it
(`store.registry = Arc::clone(&self.registry)` before the analysis), so the prelude and the
plugin wrappers already compile once per worker; the *store* is per run, the registry is
not. A store is rebuilt each time because a document's imports must not be read through
another's — the same split this note uses for the CLI.

## Round 8: a directory run shares one registry

Each file of a directory run got a store of its own, and each store compiled the prelude
(1.5 ms of check plus 0.3 terms and 0.8 freeze) and — for the 22 examples that import it —
the compute wrapper (5.4 ms). On a 46-file run that is ~230 ms of a ~975 ms command, and it
is the same two modules every time.

The run now hands every store **one registry**, the compiled modules, while each store keeps
its own `packages`, `vendored`, `native` and device handle. Keys come from the device when
there is one; without one they come from `Registry::allocate_module_key`, a counter the
registry owns, because two stores sharing a registry must never be handed one key — that
move is what makes the sharing sound rather than lucky.

The change surfaced a real bug in the reuse path: `register_embedded` returned an
already-registered wrapper without filing it in the store's own `native` map, which is what
`import "compute.lichen"` resolves against. The first store of a run always compiled the
wrapper and filed it; every later store took the reuse path and then failed to find it,
falling through to a file read of `examples/compute.lichen`. The reused handle is now filed
the same way, which is where the "outputs identical" reading came from — the first version
of this change failed on every compute-importing example after the first.

| reading | before | after |
|---|---|---|
| `lichen-compiler examples` (46 files, quiet machine) | 975 ms | **405 ms** |
| the same, under load (median of five interleaved) | 1 190 ms | **749 ms** |
| its output | — | identical, 126 lines |
| single-file mode | — | identical |

It also removes `class_members`, dead since round 5 inlined its only caller.

**A process-global registry for the tools' example runner was tried and does not compile**:
`Registry<P>` holds raw arena pointers and is deliberately neither `Send` nor `Sync`, so it
cannot live in a `static`. The runner (`readme::program_output`, once per program) would
have to thread a registry through `render_examples`/`render_dir`, which is a signature change
through the render chain rather than a cache behind a `OnceLock`.

## Round 7: the check is the run, and it is three costs

Splitting the pipeline for the first time: for `fib 10` the frontend is **0.09 ms** and the
check is **22.8 ms of the 25.1 ms total**; for `mathlib.lichen` (13 KiB) the frontend is
1.66 ms and the check 13.5 ms of 21.7 ms. There is no separate "run" to optimise — the
checker is what evaluates, and the deep pass at the end finds everything decided.

Inside the check, timed at the entries (timers in place, so inflated by ~50 ns per call):

| entry | `fib 10` | `mathlib` |
|---|---|---|
| `add_node` | 6.0 ms / 79 162 calls (76 ns) | 3.7 ms / 44 181 (83 ns) |
| `unify` | 6.8 ms / 18 841 (359 ns, inclusive) | 2.4 ms / 4 667 (517 ns) |
| `unify_inner` | 10.1 ms / 49 245 (206 ns) | 5.3 ms / 16 117 (330 ns) |
| the clone walk's own arithmetic | ~7 ms | ~4 ms |
| the class walk | ~1 ms (round 6) | ~1 ms |
| its allocations | 34 671 | 83 532 |

Round 3's "`unify` is 14.8 ms" is **superseded**: the fill skip (round 3) and the narrowed
fill (round 6) took it to 6.8 ms. What is left of the check is proportional to two
semantic facts — the **number of nodes the representation creates** (79 162 for a six-line
program) and the **three-wide pair** every expression and value is stored as. Every exact
optimisation this note has measured is now a few percent at most; the levers that are left
are the two the maintainer declined in round 2 (sharing a materialized static read) and
round 5 (sharing a wildcard's cell), both cell-identity changes.

## Round 6: the merge fills the valueless side only

Round 3 stopped filling when both sides held a value. Timing the walk again showed the
half that was left is the bigger half: a merge of a *large* decided class with a *small*
empty one walked the large class to write nothing, because the fill ran on the merged class
after the union. **A class that holds a value has no holes** — the same invariant round 3
used — so only the valueless side has any, and it is now filled *before* the union, while
it is still the small side. The merged class ends uniform either way; nothing reads the two
classes in between.

Round 3's other half also lands here: `add_equality` reads the two roots once with
`disjoint::find` and passes them to the union, instead of reading each shape through
`class_root`'s uncompressed walk and letting the union find both again. That one is not
measurable on its own; it is a walk removed, not a walk made cheaper.

| reading | round 5 | round 6 | vs. the original |
|---|---|---|---|
| `recursion.lichen` min / median | 31.75 / 32.53 ms | **25.14 / 25.47 ms** | 174 → 25.1 ms (6.9×) |
| its allocations | 34 671 | **34 671** | 754 827 → 34 671 (21.8×) |
| `mathlib.lichen` min / median | 21.14 / 21.69 ms | **21.09 / 21.54 ms** | 25.1 → 21.1 ms |
| corpus (46 programs, median) | 591 ms | **580 ms** | 1 108 → 580 ms |

Ten interleaved runs per binary, minimum and median, because this machine's load moves the
median by ±20 % between minutes. `mathlib` is flat because its merges are small classes on
both sides; the corpus is flat-ish because it is compute-dominated.

**Round 5's "the class walk is the last compressible item" was wrong, and this corrects
it.** Skipping `evaluate_class_first`'s walk outright — an experiment, not a change —
moved `fib 10` by nothing measurable (31.8–33.4 ms with and without), so its ~400 000
member reads are ~3 ns each, not 15–25 ns: a class's members sit in the slot table's own
storage and are read out of cache. No owed counter is worth an invariant there.

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
member reads, which round 6 measured at ~3 ns each; the walk's own per-visit arithmetic
under 6 ms in total.

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
- **The class walk is not a target.** `fib 10` reads ~400 000 class members across 2 360
  class-first walks, and skipping the walk outright does not move its check (round 6): the
  members are the slot table's own storage, read out of cache at ~3 ns. Round 5 guessed
  15–25 ns and was wrong.
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
