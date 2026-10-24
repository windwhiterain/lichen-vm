# Heavy values across edits: keys as the judge, propagation as the accelerator

> Status: **landed for the mechanism and for T1/T2 values; T3 is decided and not built.** This
> note records a decision that replaces one half of
> [incremental-update](incremental-update.md): that note's **"no keys"** ruling is
> **withdrawn**, and a content key now decides reuse. What the decision does *not* touch is
> stated in §5, and what it does not *solve* is in §7.
>
> What is built, in one line each: the per-binding key and its oracle (§9.9, §9.10), the
> `Portable`/`ReadOnly` freeze modes (§9.4), the compute value's artifact form (§9.6), a cell
> that reloads into an empty registry (§9.12), cells that persist across processes with
> reclamation and two guards (§9.13, §9.15), a cache scoped by the vocabulary (§9.16), and the
> measurements that decide each of them (§9.1–§9.14). What is not built is named in §13.
>
> This is the plan for two application domains — data-science dependency graphs and
> procedural-content dependency graphs — where a long-lived graph is edited by an agent and
> some nodes carry heavy computed data. The mechanism that already exists is
> [incremental-update](incremental-update.md)'s: occurrence-path identity, the `@cache` mark,
> per-cell freeze into a `StaticModule` closure, a `CellStore`, and dirty propagation over the
> resolver's read graph. **This note does not restate any of that**; it records only what the
> key ruling changes.
>
> Points at: `crates/lichen-language/src/{cells,cellcache,key,dirty,session,compile}.rs`,
> `crates/lichen-language/src/lib.rs` (`freeze_cells`), `crates/lichen-language/src/resolve/content_key.rs`,
> `crates/lichen-lowlevel/src/registry.rs` (`freeze_closure_mapped`, `reserve_cell_key`),
> `crates/lichen-lowlevel/src/static_module/freeze.rs` (`FreezeMode`),
> `crates/lichen-registry/src/device.rs` (`artifact_hash`, `verify`, `publish`),
> `crates/lichen-compute/src/compute.rs` (the value codec),
> and the notes [incremental-update](incremental-update.md), [artifact-cache](artifact-cache.md),
> [incremental-evaluation](incremental-evaluation.md), [code-audit](code-audit.md) (`D1`, `D15`).

## 1. The ruling, and what it replaces

The ruling is one sentence: **keys are allowed, and the key is the judge while dirty
propagation is only the accelerator.** Its two halves are separate decisions and both matter:

- **Keys are allowed.** The earlier ruling forbade identity derived from content. It is
  withdrawn. This note therefore does not argue for or against keys; it designs with them.
- **Propagation stays.** The statement-level fixpoint in `dirty.rs` is kept, initially
  beside the key path. The relation is **master/slave**: the key decides whether a value
  is reused, and propagation only decides which positions are worth asking about. This is
  stated as a standing contract because two mechanisms that both claim to decide dirtiness
  is the "second source of truth" that `incremental-update.md` §4.5 rejected recorded reads
  for. A reader must never have to reconcile two answers: if propagation and the key
  disagree, **the key wins and propagation is the bug**.

What the ruling does not change: identity is still an occurrence path (or, later, a graph
node id) for *retention*, and §2.4's argument about content keys is still right — see §6.

## 2. The key, precisely

Every binding gets a key. `@cache` decides which keys are *retained*, not which exist. That
is what lets a marked binding's key be compared against an unmarked dependency's without
the dependency having to be marked too.

```
K_in(b) = H( KEY_FORMAT_VERSION
           , vocabulary_key                 // the plugin set: value/operator leaves
           , source_identity                // the file id, for imports and cross-file reads
           , resolved_structure(b)           // b's own AST, reads named by occurrence path
           , [ K_out(d) for d in reads(b) ]  // direct reads, in the resolver's order
           )
K_out(b) = H( the value b evaluated to )
```

Five properties are contractual, and each one is a decision rather than an accident:

- **`K_in` folds the *direct* reads, and each read contributes its `K_out`.** A transitive
  fold would rehash a shared subexpression once per path to it; this shape costs one hash
  per binding per build, which is the same order as the resolve walk that already runs
  (see §5).
- **A binding's own `@cache` mark is NOT in `K_in`.** Marking or unmarking a binding does not
  change the computation, so it must not move the key. This is deliberately *unlike* the
  build content key, where `@cache` **is** included (`KEY_FORMAT_VERSION` 2) because the
  lowering consumes it. Two keys, two questions; conflating them would make toggling a mark
  invalidate values it did not change.
- **`K_in` is over the resolved structure, and a read is named by its occurrence path — not by
  its `BinderId`.** This is a correction: the build content key writes binders by id, which is
  right for a whole file (any change should move it) and wrong for a binding (binder ids are
  numbered in traversal order, so inserting a statement before a binding renumbers it and a key
  written by id would change for a binding nothing touched). The occurrence path is the identity
  that outlives the insertion (§2.2 of [incremental-update](incremental-update.md)), so it is
  the one a key folds. Spans are dropped, as in the build key. Two consequences, both wanted:
  editing whitespace or moving a binding without changing resolution does not move any key, and
  a resolution that changes under unchanged text *does* move it — the read now names a different
  path. Built as `lichen_language::key` (§9.9).
- **`vocabulary_key` participates** because a `jit`/`parallel` leaf's meaning is the
  plugin's, and the artifact store already scopes per plugin set for exactly that reason
  ([artifact-cache](artifact-cache.md), "the artifact store is scoped per plugin set").
- **A direct self-reference folds to a constant.** A recursive binding reads itself, so
  recursing to build its key has no base and refusing has no use: the first realistic pipeline
  measurement showed the key inert on **all seven** cells, because every stage reads one
  recursive `step`. A *longer* cycle still refuses (`None`), because the members' keys would
  then depend on traversal order. §9.11 is the measurement.
- **`K_out` is computed after evaluation, from the value.** It is what makes backdating
  free; §4.
- **A read of an import folds the imported package's artifact hash.** An import's binding is
  not a statement of this file, so it has no `K_out` here; what it has is a content identity —
  its source folded with its own dependencies — and that is what the key folds. `EXTERNAL` (a
  constant) stays as the fallback for a binder with no digest. §9.17 is the measurement that
  made this necessary, and the cell record's dependency hashes are its evidence on load.
- **A statement's own nested binders are its content, not its reads.** `dirty.rs`'s walk
  records every `Name` use in a subtree, including a lambda body's locals, and folding those as
  inputs refused the key of every binding whose value holds a lambda — which is every kernel of
  a compute application (§9.17). A read the same statement declares at any depth is subtracted
  before the key folds, so a lambda's parameter and locals stay part of the hashed expression
  and stop being mistaken for inputs.

## 3. Three consumers, three costs

The split between `K_in` and `K_out` is not elegance. It is what makes the three things a
caller can ask for have three different prices, so each can be paid separately:

| what the caller wants | needs | cost |
|---|---|---|
| **reuse within one process** | `K_in` vs the stored `K_in` | one hash per binding per build |
| **backdating** (a recomputed value that did not change keeps its consumers clean) | `K_out` | one hash over each recomputed value |
| **persistence across a restart** | `K_out` + a serializable form | the value hash, plus the codec |

This is the sharpest consequence of the ruling, and it re-prices plan stages that were
planned around a recompute heap:

- **Backdating is not a stage any more.** `incremental-update.md` §4.4 says it "is not a
  local addition to `dirty.rs`" and needs "a recompute heap, not a filter on the
  propagation", because it assumed values must be *compared* after recomputation. With
  `K_out` folded into consumers' `K_in`, a consumer whose dependency's value did not change
  has an unchanged `K_in` and is never asked. The hash does the work the heap would have
  done, and `ValueExt::value_eq`'s allocation-identity semantics for arrays (the reason a
  value comparison was not even defined) stop mattering.
- **Persistence stops being a policy question.** A stored cell is validated by recomputing
  its `K_out`, exactly as `DeviceRegistry::verify` validates a package. The
  `[0; 32]` placeholder `freeze_cells` passes today is where this lands.

## 4. Why the two-program propagation argument is subsumed

`incremental-update.md` §4.2 states that **both** the previous and the current program must
be propagated over, and that "either alone is unsound, and neither is a superset of the
other". That argument is correct for propagation and **does not survive into the key
route** — because the two mechanisms ask different questions:

> Propagation **detects a change from a delta**, so it needs both deltas.
> A key **compares closures**, so it needs only the current one; the previous one is the
> stored digest, not the previous program.

Concretely, the three cases §4.2 separates are all key movements under §2's rules: a read
that *disappeared* changes `resolved_structure`; a read that *appeared* adds an entry to the
read list; a resolution that moved under unchanged text changes the recorded `BinderId`.
So the window seeding keeps its value as an accelerator (§5) and loses its status as a
soundness obligation. The note's warning "dropping either propagation is unsound" becomes
"a key that omits a read is unsound", which is a narrower and checkable obligation — §11.

## 5. What propagation still buys, and honestly how little

Propagation answers "what did this edit reach" in O(edit); a key fold is O(program) because
every key must be re-derived to know which moved. That is a real difference in principle.

**It is close to zero here**, and the reason is worth recording: **resolve is already
whole-program per build.** Only lex and parse are incremental (via the splice); resolution
re-derives the whole program every compile. So the key fold is the same order as work that
already happens, and the O(edit) advantage would only appear once resolve is also
incremental. Therefore:

- Propagation is kept as an accelerator (its window is already computed, and it costs
  nothing extra to seed).
- **It must not be the judge.** Where they disagree, §1 says the key wins.
- If a measurement ever shows the key fold dominating a build, the answer is to make
  resolve incremental, not to promote propagation back into a judge.

## 6. What this does not reopen: §2.4's argument about content identity

`incremental-update.md` §2.4 rejects a content key as identity, with the argument that "a
hash answers 'is this the same?' only by comparing content, so it cannot distinguish 'the
same node, moved' from 'a different node' without also hashing the context, which is the
path again".

That argument was about **the identity of a retention point the user marked**, and it
remains correct for that job: a `@cache` cell is still identified by its occurrence path, and
still dropped when the path moves. The key introduced here answers a different question —
**"is this the same computation?"** — and for that question content identity is exactly
right, including its one consequence that would have been a defect for retention: two
bindings with identical structure and identical dependency keys get the **same** key and
share one cache entry. For a value cache that is deduplication, a bonus.

The consequence that does need care is not correctness: a shared entry has **one owner**,
so byte accounting and the "why did this recompute" report must attribute it once. §10.

## 7. What keys do not solve: the three value tiers

Both target domains' heavy data is the data that is hardest to persist. The tiering is by
value shape, and the key ruling moves two of the three:

| tier | value shape | in-process reuse | backdating | persistence | why |
|---|---|---|---|---|---|
| **T1** | a solved host value (scalar, string, host array, table, struct) | yes | yes | **yes** | `LowValue` codec tags `0,1,5,6,8`; the arena is already flat bytes |
| **T2** | a packed host buffer `Buffer(AnyHandle<[u8]>, ScalarClass)` | yes | yes | **yes, lifted (§9.6)** | the bytes are in the block arena, so the codec files them like an array: tag, element class, relocated handle; framing is a constant 179 bytes |
| **T3** | device-resident `DeviceBuffer` / `Graph` / `Kernel` / `ParKernel` | **yes** | **at a cost** | **decided: materialize (§12), not built** | the id names no content, so it cannot be in the bytes; the cell reads back to the host on freeze and persists as T2's `Buffer` |

**T3 is not moved by the key ruling, and the reason is worth stating exactly.** `ResidentId`'s
own contract is "never compared across backends and never persisted"
(`lichen-kernel-ir`). `KernelId` and `GraphId` are already content-addressed — `D15` did that
precisely so the derived-module cache could hit — but a device buffer has no content form, so
there is nothing stable to fold.

Two consequences, and the second is the one that is easy to miss:

- A T3 cell cannot be persisted, so a GPU pipeline still does not survive a restart. That is
  a materialisation/sidecar problem, not a key problem, and it is unchanged from before the
  ruling.
- **`K_out` for a T3 value costs a device→host read.** Hashing a buffer means reading it
  back. So the price of the sidecar and the price of the digest are *the same price* — the
  readback — which means T3's backdating and T3's persistence should be decided together,
  and both should be opt-in per cell: a cell that declares no digest keeps in-process reuse
  and gives up backdating and persistence, which is a coherent choice for a value that is
  expensive to read back and cheap to recompute.

## 8. The work budget: found, measured, and removed by decision

This was not in the plan until a probe found it. `WorkBudget`'s default was
`apply_depth_limit: 500` / `apply_total_limit: 2_000`, and **every language-level caller got
it**: `build_in_attr_native` is what `compile`, `BufferSession`, `compile_with_cells` and the
language server all reach, and it passed `WorkBudget::default()` literally. The lowlevel's own
`Module::MAX_APPLY_TOTAL` was **100 000**, so the checker overrode the VM's bound by a factor
of fifty, and `reset_apply_budget` had no caller — the count was cumulative over the build.

Two measured consequences, and then the decision they forced:

- **A heavy node overran it almost immediately.** The naive `fib` this probe uses costs
  ~2·fib(n+1)−1 applications — plain double recursion; `[a, b][cond]` is a *lazy* conditional,
  so it is not charged for both arms — and `fib 16` is ~3 193 applications against a 2 000
  budget.
- **An overrun cost the whole program its cache, not part of it.** `freeze_cells` returns on
  `!build.ok`, and a `NonTerminating` budget refusal made `build.ok` false. Measured: one
  `fib 16` cell gave `build ok false`, 1 diagnostic, **`frozen 0`, `retained 0`**.

### 8.1 The decision, and what it removes

**Decided: the cumulative bound is removed.** `WorkBudget` keeps one field,
`apply_depth_limit`; `BudgetExhausted::ApplyTotal`, `Module::apply_total_limit`,
`Module::apply_total`, `Module::MAX_APPLY_TOTAL` and the counter charge in `apply.rs` and
`loop_run.rs` are gone, as is `reset_apply_budget` (which had no caller). The depth guard
stays, because it is also the stack guard.

**What it buys, measured**: the shape that used to be refused — `cheap` 8 plus **4 cells of
`fib 16`**, ~12 800 applications — now gives `build ok true`, 0 diagnostics, **`frozen 12`,
`retained 12`**. Heavy host-side work is possible for the first time, which is what both
domains need for a fold that is naturally recursive.

### 8.2 What it costs, stated plainly

- **The only guard against a divergence that does not nest is gone.** Two tests were the
  evidence and both were deleted with it: `a_marked_loop_that_never_reaches_its_base_is_refused_by_the_work_budget`,
  whose own comment was *"The work budget turns an endless loop into a diagnostic instead of a
  hang"*, and `flattened_recursion_records_the_total_apply_budget`, whose program *"stays at
  depth 1, invisible to the depth guard"*. An endless `@loop` now hangs the compiler — and the
  language server's compile worker with it, since that worker catches panics but not hangs.
- **Nothing bounds a heavy compile any more.** The same shape costs **80 s in a debug build**
  (measured under `HEAVY16`). A release build is faster by a constant factor, not
  asymptotically, and a data-science program's real work is larger.
- **The depth guard remains** — 500 at the checker, 10 000 as the module default — and it
  catches a *nesting* divergence only. A recursion that widens instead of deepening is
  exactly the shape it cannot see.

The honest replacement would be a **wall-clock deadline recorded as a refusal**, which
separates "this is too much work" from "this does not terminate" — the conflation of those two
in one counter was the original defect. §13 records it as open.

## 9. Recovered measurements

One process, wall clock, on a measurement branch. §9.1 is a **debug** build, so its
absolute milliseconds are inflated and its ratios are the reading. §9.2 has **both** builds,
because its headline is the one that had to survive a release compile.
Probes: `crates/lichen-language/examples/{cache_timing,pipeline_edits}.rs` (temporary; delete
when this closes).

**Shape**: 8 cheap `@cache`d bindings in a chain plus 4 heavy ones calling a naive `fib 10`,
with a tail that reads every one. Frontend versus check: **2.38 ms against 149.43 ms**, so
the check is 98.4 % of a build and a key fold that costs "the same order as resolve" is
competing with under two percent of it.

| edit | fresh ms | session ms | ratio | build | reused | frozen | dropped |
|---|---|---|---|---|---|---|---|
| cheap seed `c0` | 151.42 | **1.95** | 78× | new | 4 | 8 | 8 |
| heavy `h0` depth+1 | 199.92 | 86.22 | 2.3× | new | 11 | 1 | 1 |
| append a cached statement | 188.65 | **6.37** | 30× | new | 12 | 1 | 0 |
| whitespace only | 195.97 | **0.41** | 478× | reuse | 0 | 0 | 0 |
| tail only | 194.29 | **1.47** | 132× | new | 13 | 0 | 0 |

The table is one rerun; a first run read the same within noise (e.g. 156.93 / 1.95 on the
first row), and the ratios are what it is for.

Three readings, and the third is a caveat rather than a win:

- **The reuse is real and it is exactly the intended cone.** Editing `c0` recomputes the
  eight cheap cells and leaves all four heavy ones alone; editing `h0`'s depth touches that
  one cell and its readers.
- **Session cost is flat in the number of heavy nodes while a fresh compile is linear**:
  **87 / 88 / 89 ms against 57 / 87 / 189 ms** for 1 / 2 / 4 heavy cells. The checker still
  walks the whole IR and only the cell bodies are skipped, so the win comes from what a cell
  *contains* — the mechanism's intended granularity, and also its limit.
- **The session path has fixed overhead that a small program does not amortize.** Its first
  compile of the same source took **245.60 ms against a fresh 151.80 ms** (1.6×), and at one
  heavy cell an edit cost 87 ms against a fresh 57 ms. Unattributed in this reading — the
  diff, the freeze, or materializing static reads — and it is the thing to profile next,
  because a data-science edit loop pays it on every edit.

### 9.2 The pipeline application, and the two things it found

`crates/lichen-language/examples/pipeline_edits.rs` is the same question on the shape the two
domains actually have: a generator pipeline of 6 stages, each `@cache`d stage reading the one
before it, each doing a branching recurrence, with a tail that reads all of them. An agent's
five edits are applied in sequence to one session. **Every row also compares the session's
root value with the same source's fresh compile** — `Module::node_value` reads the raw answer,
so this is the differential oracle and not a proxy for it.

**Release build** (`cargo run --release`), which is the headline because it had to survive one:

| edit | fresh ms | session ms | speedup | reused | frozen | dropped | value |
|---|---|---|---|---|---|---|---|
| upstream `s0` | 65.42 | 197.17 | **0.3×** | 0 | 6 | 6 | same |
| middle `s3` | 89.92 | 50.15 | 1.8× | 3 | 3 | 3 | same |
| leaf `s5` | 226.20 | 46.54 | 4.9× | 5 | 1 | 1 | same |
| append a stage | 208.42 | **2.61** | 80× | 6 | 1 | 0 | same |
| whitespace only | 195.21 | **0.14** | 1374× | 0 | 0 | 0 | same |
| **five edits** | **785.17** | **296.61** | **2.6×** | | | | **5/5** |

The debug build read fresh 404.86 / 556.44 / 917.40 / 957.78 / 1318.07 and session
1398.48 / 322.16 / 255.88 / 9.27 / 0.55, totals 2.1×. **The ratios agree between the two
builds** (0.3×, 1.7–1.8×, 3.6–4.9×, 80–103×, 1374–2384×), so the shape of the result is not a
debug artefact — only the constants are.

**Value agreement is 5 of 5** in both builds, which is the mechanism's soundness reading on a
real edit sequence: every session answer equalled a fresh compile's, including the edits that
reused every cell.

Two findings, and the first is a caveat the table forces:

- **The win is proportional to the fraction of cells reused, and the all-dirty case is a
  loss.** Editing the upstream stage dirties all six, nothing is reused, and the session is
  **3× slower than a fresh compile** in both builds (197.17 against 65.42 in release). Two
  costs produce that: the session's own bookkeeping, and **the per-cell freeze**, which for a
  cell holding a heavy apply walks that apply's *clone graph* — thousands of nodes for ~900
  applications — rather than its value. So a heavy cell is expensive to *freeze* as well as to
  compute: **~22 ms per cell in release**, against a 65 ms fresh compile of the whole
  program. The break-even is a cell reused more often than it is recomputed. That is the
  retention policy's real design constraint, and it is new to this note.
- **The fresh arm is not stationary across runs.** One unchanged source compiled five times
  reads flat (277.96 / 277.31 / 285.91 / 291.90 / 287.24 in debug), so the process does not
  drift on its own; but the fresh arm varies with source size superlinearly, and two debug
  runs of the whole probe disagreed by up to 76 % on one row's fresh time (1318.07 against
  2322.66 for the same structure). So read the **row ratios** as the reading, the aggregate
  as a range (2.1–2.7× debug, 2.6× release), and the whole-build-reuse row as the robust one
  because it skips the compile entirely.

### 9.3 The freeze's closure, measured, and why it cannot simply be cut

§10 says a heavy cell is expensive to freeze. This is what the closure actually contains, and
the experiment that tried to shrink it — it **failed**, and the failure is the result.

A temporary census inside `StaticModule::freeze_closure` (the `CELL_FREEZE_CENSUS` print, gone)
read, for the 6-stage pipeline's six cells:

| | closure nodes | functions | undecided | with an operation |
|---|---|---|---|---|
| `s0` … `s5` | **14 030 – 43 148** | **1** | 1 648 – 5 057 | 4 667 – 14 373 |

So the function-template edge is *not* the cost (one function), and nearly half the closure
is operation-bearing nodes — the apply clone graph, reached through the operand edge and
through the classes that undecided nodes pull whole.

**The operand edge is a storage requirement, not a semantic one.** `freeze_set` writes
`operation.operand` as a reference into the artifact, so every node it stores that has an
operation must have its operand in the artifact — which is why `closure` follows the operand
unconditionally, including from nodes that are decided and will never re-run.

**The experiment.** Following an operand only from a *residual* node, and storing no operation
for a decided one, collapsed a cell's closure from **14 030 nodes to 6** and moved the whole
pipeline from **2.6× to 6.6×** (the all-dirty edit from "3× slower than fresh" to 1.17×), with
the differential oracle still at 5 of 5. **It is unsound.** `lichen-lowlevel`'s
`table::a_function_key_survives_a_freeze_and_a_reload` fails, and the reason is in the key
hash's own contract: it *"unfolds a node's operation edge in preference to its memoized value,
so the answer does not depend on how far the definition pass has run"* (`table.rs:74-75`). A
decided node's operation is part of its **content identity**, so dropping it changes what the
artifact hashes to and a key taken before a freeze stops matching after a reload. The two
obvious alternatives are closed for the same reason: keeping the operand chain is what the
storage requirement demands, and hashing a decided node by its value instead would reintroduce
exactly the definition-pass dependence that sentence exists to exclude.

**What is left is a contract question, not a performance one**: the cheap freeze is available
only to a cell that gives up being usable as a table key. That is the granularity knob, and
the superior ruled it in as two modes with `Portable` the default.

### 9.4 The knob, built and measured

`FreezeMode { Portable, ReadOnly }` on the freeze, opted into per cell by **binding name**
through `CellStore::set_read_only` / `BufferSession::set_read_only`; `Portable` is the default
so nothing changed for an existing caller. `ReadOnly` follows an operand only from a residual
node, stores no operation for a decided one, and marks its artifact, and the key hash
**refuses** a `ReadOnly` static node it meets in template mode (`KeyState::Undecided`, so a
table reports its existing undecided-key error) rather than hashing a number that is not the
node's content.

The same five-edit sequence, release, both modes, both on this branch:

| edit | Portable | ReadOnly | reused | frozen |
|---|---|---|---|---|
| upstream `s0` (all dirty) | **0.3×** | **1.1×** | 0 | 6 |
| middle `s3` | 2.5× | 5.3× | 3 | 3 |
| leaf `s5` | 4.5× | 8.9× | 5 | 1 |
| append a stage | 142× | 111× | 6 | 1 |
| whitespace only | 1110× | 1671× | 0 | 0 |
| **five edits** | **2.9×** | **7.1×** | | |

**Value agreement is 5 of 5 in both modes**, so the small artifact answers exactly what the
full one does. The all-dirty edit stops being a loss (0.3× → 1.1×), which is the whole point:
the freeze no longer costs more than the recomputation it preserves.

### 9.5 A compute value retains as its bytes, and nothing else

The pipeline's heavy cells are scalars from host-side recursion, so §9.3's 14 030-node closure
is the *scalar* case. The two domains' heavy data is the compute layer's, where a value is a
packed payload — `ComputeValue::Buffer(AnyHandle<[u8]>, ScalarClass)`. Probed without opening a
device (`crates/lichen-compute/examples/cache_buffer.rs`, temporary), by freezing a node that
holds one and reading it back through the static ref:

| payload | freeze ms (Portable) | artifact nodes | arena bytes | read back |
|---|---|---|---|---|
| 32 KB (4 096 Int) | 0.09 | **1** | 32 776 | 4 096 elements, equal |
| 512 KB (65 536) | 0.24 | **1** | 524 296 | 65 536, equal |
| 2 MB (262 144) | 0.42 | **1** | 2 097 160 | 262 144, equal |
| 8 MB (1 048 576) | 1.61 | **1** | 8 388 616 | 1 048 576, equal |

Three readings:

- **A compute value freezes as one node plus its bytes** — `arena = payload + 8` at every size.
  There is no clone graph to hold, because a buffer node has no operation and pulls no class.
  So §9.3's 14 030-node closure is a property of the *operand chain*, not of heavy data as
  such: a heavy value that is a leaf costs its bytes and nothing more.
- **The freeze is one `memcpy` of the value**, ~5 GB/s here, so 8 MB costs 1.6 ms. For the
  domains this is the honest price of in-process retention: one copy of the value, paid once
  per recomputation, against a dispatch.
- **Both modes give the same artifact** for this value (one node), so a buffer cell needs no
  mode choice; the knob matters only where an operand chain exists.

**The caveat, stated because the probe is a lower bound**: the module built here has no type
side. A real `@cache out = …` binding carries a `[value, type]` expr, and the type graph is
where the classes are; so a language-level buffer cell's closure is larger than one node. The
claim measured here is about the value, not about the whole cell.

### 9.6 T2's persistence half, lifted and measured

`ComputeValue::write_value` refused every arm by name, `Buffer` included. It no longer does:
a `Buffer` is host bytes in the arena, so it files like an array — tag `9` of that leaf, the
element class byte (which the payload cannot say), and the relocated handle through the
lowlevel's `write_relocated_handle`. `read_value` gained the matching arm; the other five arms
keep their refusals, because their ids name process-local registries.
`ARTIFACT_FORMAT_VERSION` moved `10 → 11` with its history entry, as
[artifact-cache](artifact-cache.md) requires.

The probe now round-trips the artifact through `serialize_artifact_with` /
`deserialize_artifact_with` and compares element for element:

| payload | arena bytes | artifact bytes | framing | reloaded |
|---|---|---|---|---|
| 512 B (64 Int) | 520 | 699 | 179 | 64 elements, equal |
| 32 KB (4 096) | 32 776 | 32 955 | 179 | 4 096, equal |
| 2 MB (262 144) | 2 097 160 | 2 097 339 | 179 | 262 144, equal |
| 8 MB (1 048 576) | 8 388 616 | 8 388 795 | 179 | 1 048 576, equal |

So **framing is a constant 179 bytes and the rest is the value**: a persisting heavy buffer
costs its own size plus a header, and reloads byte-identical. That makes T2 closed on both
halves — in-process retention and persistence — and it is the first value tier where a heavy
compute result survives a process.

### 9.7 A compute program's buffer cells, under an edit

The readings above are about a value in a module. The target shape is a *language program* whose
cached bindings hold buffers, so `crates/lichen-compute/examples/cache_pipeline.rs` (temporary)
drives one through `BufferSession` over the composed vocabulary: two kernels, `adder` and
`doubler`, with `@cache data = raw.z` and `@cache result = raw2.w.native` — the annotation that
`plrun` needs sits on the unmarked binding `raw`, because a marked binding whose value spells
an annotation is not eligible (§3.3).

| edit | fresh ms | session ms | speedup | build | reused | frozen | value |
|---|---|---|---|---|---|---|---|
| the kernel's constant (`i + 3` → `i + 4`) | 5.07 | 5.05 | 1.0× | new | 0 | 2 | [8, 10, 12, 14], same |
| whitespace only | 4.74 | 5.07 | 0.9× | new | 0 | 2 | same |
| the same source again | 4.63 | **0.38** | **12.1×** | reuse | 0 | 0 | same |

Three readings:

- **Buffer cells freeze and are reused on a real program, and the answers agree.** Both rows'
  session values equal the fresh arm's element for element, which is the differential oracle at
  language level rather than at module level.
- **The all-dirty edit costs parity, not the scalar pipeline's 0.3×.** Every cell is dropped
  and re-frozen, and the session is still 1.0× — because a buffer cell freezes as one node
  (§9.5), where §9.3's scalar cell froze a 14 030-node operand chain. So the freeze cost that
  made `ReadOnly` necessary for host scalars is simply not there for compute values.
- **The whitespace row is a gate defect, and it is now named.** A whitespace-only edit inside a
  *preprocessed view* recompiled while a same-source recompile reused (`12.1×`). The content key
  is not at fault: `key_eq` was true in every row, and two *full* parses of the two sources give
  equal keys. The coarse gate is `view_same && cache.key == key && cache.build.is_some()`, and
  `view_same` requires `prev.base == self.base && (base == 0 || prev.line_starts == self.line_starts)`.
  A view's line starts are **absolute byte offsets**, so any edit that shifts an offset changes
  them — which every real edit does. Measured directly: `base_same true, starts_same false,
  key_eq true`. The `base == 0` arm short-circuits the check, which is exactly why `set_source`
  (the bare-language probe) reuses and `set_view` does not. **Every host that compiles a program
  with imports takes the view path**, and both target domains do. Fixed in §9.8.

### 9.8 The gate fix, and what it is worth

Decided by the superior: split the predicate. `view_same` becomes `base_same` — only the
coordinate space decides whether the snapshot is usable — `previous_starts` is read from the
snapshot whenever `base_same`, and `Cache::moved` keeps its role as the span-level refusal (a
check diagnostic that cannot be mapped still forces a recompile).

| edit | before | after |
|---|---|---|
| whitespace on the first build | 0.9× `new` | **26.3× `reuse`** |
| whitespace after another edit | 0.9× `new` | **25.5× `reuse`** |
| the kernel's constant (all dirty) | 1.0× `new` | 1.0× `new` |
| the same source again | 12.1× `reuse` | 13.3× `reuse` |

So the whole-build reuse on the view path goes from **unreachable to 26× on a one-byte edit**,
and the value still agrees element for element in every row. The tests are unchanged by it:
`lichen-language` 261/1 and `lichen-language-server` 31/1, each failure the one `dev` already
has (`gc_block::a_gc_block_releases_its_scope`, and
`analysis::tests::a_prelude_name_use_jumps_into_the_builtins_own_file`).


### 9.9 The key beside the judge: an oracle, not a replacement

`lichen_language::key::binding_keys` builds the `K_in` of §2 for every marked binding, and
`dirty_marked_paths` is now public as the oracle it must be compared against. At the time of
this section the key only observed; **as of §9.10 it decides**, with propagation as the
fallback.

`crates/lichen-language/examples/key_oracle.rs` (temporary) applies seven edits in sequence and
compares the **path sets**, never counts:

| edit | propagation | key | agree |
|---|---|---|---|
| change `c`'s constant | 3 dirty | 3 changed | yes |
| the tail only | 0 | 0 | yes |
| append a marked binding | 1 | 1 | yes |
| insert an unmarked binding at the top | 0 | 0 | yes |
| whitespace only | **5** | **0** | **no — the key is more precise** |
| change `a`'s constant | 5 | 5 | yes |
| insert a duplicate `a` | **0** | **6** | **no — the key is less precise** |

Both disagreements are the two documented ways and neither is unsound:

- **Whitespace**: propagation seeds from the re-parsed statement, so a byte-only edit dirties
  that statement and its readers; the key sees no content change. This is §4.4's over-dirtying,
  not a defect — and it is the one place the key is *better* than the shipped judge.
- **A duplicate name**: `@cache a`'s path stops being `Name("a")` and becomes an `Index`, which
  is the documented "both steps become indices" row of [incremental-update](incremental-update.md)
  §2.2. Every later key folds a read by path, so the whole chain moves. Safe over-dirtying:
  path churn drops cells propagation would have kept.

**No edit put the key clean where a value changed**, which is the direction that would make it
unfit as a judge. The sample has since been widened (§9.10), and the key now decides.

### 9.10 The key as the judge, with propagation as the fallback

Widened to twelve edits over three shapes (a chain, a diamond, and a cell whose value reads a
nested block binding), the oracle reads **8 of 12 agree**. Every disagreement is one of the two
classes §9.9 names, and the new shape adds two readings:

- **The key is more precise in two independent shapes.** On a whitespace edit of `b` in the
  diamond, propagation dirties `b` *and* `d`, because `d` reads `b`; the key sees that neither
  content changed. So the key does not merely skip the byte-only statement — it stops the
  over-dirtying from travelling down the graph, which propagation cannot do by construction.
- **The key is less precise exactly where it refuses.** A cell reading a nested binder gets
  `None`, which is read as changed; and a duplicate name moves every path in the chain. Both are
  the documented conservative directions.

**Wired, per the ruling.** `BufferSession::compile` now takes the key's verdict as the judge and
consults propagation **only where the key is `None`** — a nested binder or a cycle. The
demonstration is a single `set_source` carrying a whitespace edit inside a cell *and* a real
change to an unmarked binding: propagation reports 2 dirty, the key reports 0, and the session
drops **0** and reuses **2**. That is a case the byte-window seed cannot see through, and it is
the first measured edit where the key's verdict reaches the cells.

The pipeline and the suite are unchanged by the wiring: `pipeline_edits` still reads 2.8× total
with 5 of 5 values, and `lichen-language` is 262 passed / 0 failed. The oracle's `dirty` column
uses the window its own edit implies, while the session computes its window from the splice —
which is why one boundary-append row drops a cell the oracle's narrow window calls clean
(§4.2's widened-window rule, not the key).

### 9.11 The pipeline under the key judge, and the cycle it exposed

The tuned pipeline (§9.2's shape) gained an **unmarked** `bias` the tail reads, so a single
`set_source` can carry a whitespace edit *inside* a cell and a real change the build key sees
while no cell's content moves. Release build:

| edit | fresh ms | session ms | speedup | reused | frozen | dropped |
|---|---|---|---|---|---|---|
| edit upstream `s0` | 63.88 | 193.00 | **0.3×** | 0 | 6 | 6 |
| edit middle `s3` | 93.92 | 45.16 | 2.1× | 3 | 3 | 3 |
| edit leaf `s5` | 194.06 | 36.06 | 5.4× | 5 | 1 | 1 |
| append a stage | 218.27 | **2.21** | 99× | 6 | 1 | 0 |
| **combined: cell + bias** | 209.23 | **0.47** | **444×** | **7** | 0 | **0** |
| whitespace only | 207.60 | **0.23** | 887× | 0 | 0 | 0 |
| **six edits** | **986.96** | **277.15** | **3.6×** | | | |

Value agreement is **6 of 6**. The combined row is the key's verdict reaching a realistic
pipeline: propagation's byte window spans from the edited cell to `bias`, so it would drop all
seven, and the key drops none — 0.47 ms against a 209 ms fresh compile.

**The first run of that row disagreed in the other direction, and it was the finding.** It read
`reused 0, frozen 7, dropped 7` at 0.5×, because every stage reads the recursive `step`, and the
key walk treated a read of the enclosing statement as a cycle — so every stage's key was `None`
and the fallback dirtied everything. A key that refuses recursion is inert for every heavy
program, which is exactly the population it exists for. The fix is §2's rule: a **direct**
self-reference folds to a constant; a longer cycle still refuses. The oracle is unchanged by it
(8 of 13 agree, the same two classes), and `lichen-language` stays 262 passed / 0 failed.



### 9.12 A cell reloads into an empty registry and answers the same

The persistence-critical property, measured before any file layout exists
(`crates/lichen-language/examples/cell_persist.rs`, temporary): a **cell's** artifact — the
`[value, type]` closure, not a bare buffer — serializes with the artifact codec, reloads into a
registry that was **empty**, is restored into a `CellStore`, and a compile that reads it back
answers the value the freezing compile answered.

For `@cache a = 1 + 2` / `@cache b = a + 3` / `a + b`: `a`'s artifact is **841 bytes**, `b`'s is
**1 073**, and the restored compile's root is `USize(9)` — equal to the original's.

Two things the experiment established, and one it refuted:

- **A loaded cell's key must be reserved in the allocator.** A cell key is
  `CELL_KEY_BIT | counter`, so a fresh registry's first allocation is the same key a loaded
  artifact already holds — and `freeze_closure_mapped` asserts the key is free, so the restored
  compile panicked on its first freeze. `Registry::reserve_cell_key` is the answer: filing a
  loaded artifact moves the counter past it, and it refuses a key outside the cell space. This
  is a requirement the design did not have until it was built.
- **A cell artifact is self-contained.** Its refs are absolute from birth, so the empty
  registry needed nothing but the cell itself — no dependency map, no retargeting.
- **Comparing the expr values directly is meaningless**, and the first version of the probe did
  it: an expr is an array, and array equality is *allocation identity*, so two artifacts can
  never compare equal that way. The check has to be the compile's answer, which is what the
  probe now measures. Worth recording because it is the obvious wrong test.

What persistence still needs: a save sink inside `freeze_cells` (the expr node is only reachable
there), the on-disk layout, and the judge arm that compares a **stored** key against the
computed one on the first compile of a process — §13.

### 9.13 Cells across processes: the first reuse that survives a restart

The vertical is built. A session with a cache directory stores its retained cells and loads
what an earlier session left; a cell is reused when the `K_in` it was stored with equals the one
this compile computes. `crates/lichen-language/examples/cell_reload.rs` (temporary) drives four
sessions, each with a **fresh, empty registry**:

| session | source | frozen | reused | dropped | root |
|---|---|---|---|---|---|
| first (cold) | original | 2 | 0 | 0 | 9 |
| **second (fresh registry)** | original | **0** | **2** | 0 | **9** |
| third (fresh registry) | edited constant | 2 | 0 | **2** | 13 |
| fourth (fresh registry) | edited constant | **0** | **2** | 0 | **13** |

So the second session recompiled nothing and answered the same; an edit moved both keys, so the
third recomputed and re-stored; and the fourth reused what the third stored. **This is the first
time a retained value has survived a process boundary.**

The design decisions, each of which removes work the design was expected to need:

- **The bytes are stored as they are, and a load that collides is skipped.** The bytes embed the
  artifact's `ModuleKey`, which is process-local, so two processes can hand out the same key.
  Rather than derive a deterministic key — which then collides on *recompile* and needs the old
  artifact evicted before the new one is filed — a load asks whether the key is free and skips
  the cell if it is not: a collision is a missed cell, never a shadowed one. `reserve_cell_key`
  keeps a filed key from being handed out again.
- **The `K_in` is stored beside the artifact, not derived from it.** The artifact's own hash slot
  is a placeholder (§3), so the container carries the key words the cell was frozen under; the
  judge compares them on a process's first compile, where there is no previous program to compare
  against.
- **The container is the artifact plus framing.** `cellcache` writes `path | module key | K_in |
  artifact bytes` and nothing else; the artifact codec remains the format, so corruption is
  caught by its body digest and a malformed file decodes to an empty cache.

The limits, stated rather than discovered:

- **Nothing evicts.** A source's file is rewritten whole on every compile, so stale cells leave
  the file, but a source that is never opened again keeps its file forever. The package store's
  reclamation is the template; none of it is here.
- **Every compile rewrites the file**, including the cells it reused, so the save cost is one
  serialize per retained cell per compile. That is why it is opt-in (`with_cell_cache`) rather
  than always on.
- **A saved cell is only as good as its key.** The `K_in` covers the binding's own structure and
  its reads; a change outside that cone (an import's content, a plugin set) is not in it. For a
  cell that read an import, the session's own import invalidation is the guard, and the plugin
  set is not guarded at all — the cache directory would have to be scoped per plugin set, as the
  artifact store already is.

### 9.14 A restart on the pipeline, and what the write costs

The tuned pipeline (§9.11) restarted over a cache an earlier session stored, release build:

| arm | ms | cells |
|---|---|---|
| a fresh compile | 215.84 | — |
| cold: empty cache, compile and write | 521.28 | frozen 7 |
| **warm: load, reuse, write** | **96.42** | reused 7, frozen 0 |
| load only, no write | 85.57 | reused 7 |

So **a restart over its own cache is 2.24× cheaper than recompiling**, and the value is the
same. A second run read warm 123.09 ms against fresh 229.59, so the ratio is **1.9–2.2×**
across runs — §9.2's variance, not a different result.

**The first version of this row read 180.55 ms and that was the finding.** It rewrote *every*
retained cell, so the write of seven 14 030-to-43 148-node closures cost ≈90 ms — as much as
the reuse saved, and the restart measured 1.15×. Two changes fixed it: the container became
**one file per cell** (so a rewritten cell replaces only its own file), and `save_cells` now
takes the paths `compile_resolved_with_cells` actually froze instead of every retained path. On
the warm arm nothing is frozen, so nothing is serialized, and the write drops to ~11 ms.

The price is on the other side. The cold arm pays **≈305 ms to persist seven heavy closures** —
a cell's freeze is its operand chain (§9.3), and writing it is writing that chain. So the cache
pays for itself when a source is opened again, and a program compiled once and abandoned pays
for nothing but the file.

### 9.15 Two defects the vertical had, and their fixes

Both were in the persistence path §9.13 and §9.14 built, and both are found by asking what a
*stored* cell can be wrong about.

- **A cell that read an import must not persist.** The key's `EXTERNAL` token stands in for
  "the prelude, or an import's binding", so two runs that read different bytes through the same
  import compute the *same* key. In-process that is safe — the resolver re-derives the read and
  the session's import invalidation is the guard — but a persisted cell would be reused by a
  later process against a source whose import has changed. The fix refuses to save such a cell:
  a self-contained artifact's `refs` is exactly its own key, so a `refs` with any *other* key
  means the cell read a dependency. Measured by the first version of the guard, which tested
  `refs.is_empty()` and skipped **every** cell — that is how "`refs` includes self" was learned;
  the cache-file count fell to zero and the reload rows stopped reusing. The guard now excludes
  self, and the four-session probe writes two files again.
- **A renamed cell's file was never reclaimed.** Per-cell files are named by the path, so a
  rename leaves the old file behind forever. `cellcache::prune` removes every `*.bin` in the
  source's own directory that is not the file of a retained path, and `save_cells` calls it with
  `cells.paths()` after the write. Measured: after four sessions the cache holds **2** files for
  two cells, and a rename compiles two fresh cells and still holds **2**, not 3.

The import guard's negative case is **not exercised end to end**: constructing an import in a
probe needs a package store, so what is verified is the predicate's two halves — a
self-contained cell's `refs` is exactly its own key (`files = 2` after the fix), and any other
key is what the guard tests.

### 9.16 The cache directory is scoped by the vocabulary

A record's value encoding is the **plugin set's**, and the artifact codec distinguishes leaves by
type name — so two plugin sets that share a leaf name write bytes neither can safely read. §9.13
recorded this as an open hazard with the fix "the directory must be scoped per plugin set, as the
artifact store is"; this is that fix.

The scoping is the program type: `cellcache`'s directory name folds
`std::any::type_name::<P>()` with the source id, so two program types never see each other's
records. Measured by pointing **both** the bare-language pipeline and the compute pipeline at one
cache root — the root then holds two directories, `cells-5498c61155aeeede` and
`cells-81140dc1e61a4251` — while each probe's own rows are exactly what they were. Both probes
now use their own root so a run starts cold.

The honest limit: `type_name` is not a stable ABI. Across builds it moves with the code's
structure, which costs a **miss** — safe — and never a read of another vocabulary's bytes. A
persisted cache that should survive a rebuild would need a deliberate scope key (the artifact
store's `compiler_slot_key` is that shape) instead of a type name; that is not built.

### 9.17 The world generator, and the defect a real application found

The validation §9.7 and §9.11 ran was target-shaped: a pipeline of arithmetic stages whose cells
only ever produced a value for the root to read. `examples/worldgen.lichen` is the application the
framework is for — a seeded terrain generator whose twelve `@cache` bindings are noise octaves,
blends, two smoothing passes, a biome classification and a tile atlas, over a `128 × 128` grid —
and it found a defect that the pipeline could not expose.

| edit | fresh ms | session ms | ratio | reused |
|---|---|---|---|---|
| same source again | 193.67 | 1.88 | 103× | 0 (build reuse) |
| whitespace only | 185.38 | 0.81 | 229× | 0 (build reuse) |
| biome sea 0.35 → 0.40 | 181.70 | 53.08 | 3.4× | 10 |
| octave scale 32 → 40 | 182.58 | 125.30 | 1.5× | 5 |
| mix weight 0.25 → 0.35 | 191.12 | 95.31 | 2.0× | 7 |
| moisture seed +3 → +5 | 176.52 | 78.41 | 2.3× | 8 |
| append a smoothing pass | 206.22 | 65.31 | 3.2× | 10 |
| seed 7 → 11, everything dirty | 201.13 | 219.73 | 0.9× | 0 |
| **eight edits** | **1518.32** | **639.83** | **2.4×** | — |

The same application at `512 × 512` — 262 144 cells, 1 MB per buffer, a 3.1 s fresh compile —
repeats the shape with more room between the arms: 1548× and 3313× on the two edits where no cell
moved, 4.6× / 1.6× / 2.3× / 2.6× / 3.3× on the five real ones, 1.0× on the all-dirty one, and
**2.7×** over the eight (25 425.63 ms against 9 443.68). All eight answers equalled a fresh
compile's at both sizes, element for element, over the whole collected map.

**What broke, and why it was silent.** A launch whose input was a *frozen* buffer answered
`raw none`: the session reported ten cells reused, recorded no diagnostic, and left the whole map
undecided. Two refusals sat in the operators' read path, and both outlived their reason:

- `buffer_items` answered `None` for `AnyHandle::Static`, under a comment that the codec "refuses
  to serialize a frozen one" — which stopped being true when §9.6's buffer encoding landed;
- `buf_payload` and `value_at_path` walked a value with `node.dynamic()`, so a walk that reached an
  item *inside* a cell's artifact stopped there. A reused value is an array whose items are
  static refs, so `items_of` now reads either side.

Before the fix, **0 of 8** rows agreed with a fresh compile while the session was still fast; after
it, **8 of 8**. That inversion is what the differential oracle is for: the timings alone called the
broken arm a win.

**The surface needed no new operator.** The kernels use `range`, `row`, `col`, `read`, `write` and
`if`; the biome classification's nested `if` compiles to `Select`, the same instruction §9.7's
boundary masking used. What was missing was not an operator but the operators' ability to consume a
cell at all — and that is the join the two pipelines never made, because their cells only ever fed
the root.

**A restart stored nothing, and the reason was three layers deep.** The first reading was
`files 0` with an *empty* key on every cell, and each layer had to be peeled with a measurement:

- **Every cell's key was `None`.** `dirty.rs`'s walk records a statement's reads *including* the
  locals its own lambda bodies declare, so a cell that reads any helper holding a lambda folded
  an "external" read and refused. Every kernel of a compute application is such a helper. The fix
  is §2's free-reads rule, and the probe's temporary `set_key` trace showed thirteen real keys
  where there had been thirteen `None`s.
- **The import was invisible to the key.** With keys in hand, the import still folded the
  constant `EXTERNAL`, so two runs that read different `compute.lichen` bytes computed one key.
  The fix is §2's import-digest rule; the key material now carries the package's artifact hash.
- **The artifact codec refuses a compute operation.** The real blocker, and it had been hidden
  behind the two above: `Portable` keeps every operation (that is what makes its content identity
  equal to the source's), a launch's node holds a compute operation, and the codec's answer is
  *"this package is not cached: it holds a compute operation, and a compute operation is a
  runtime form with no on-disk representation"*. A `ReadOnly` freeze drops a decided node's
  operation, so a `ReadOnly` cell is storable — and `save_cells` skipped `ReadOnly` cells because
  the wire form had no mode. That is now in the record (§9.18), so the mode survives a reload and
  a launch's cell can persist.

With all three, the restart reuses the map and the numbers are the point of the whole exercise:
**warm 85.30 ms against a fresh 195.34 ms (2.3×), twelve cells reused, one recomputed, values
identical.** Twelve of the application's thirteen cells write a file; `octave_a` refuses because
its closure still holds the kernel-creation operation — an undecided compute node — so it is the
one cell a warm start recomputes. (§9.19 removed the codec refusal itself, so a `Portable` cell
persists and all thirteen survive; the numbers here are `ReadOnly`'s, as they were.)

**The reading that was not chased, and what it was.** A reused cell printed `(raw Buffer, raw
Int)` where the fresh build printed `(raw Buffer, Int)`. The **type** was never lost: it prints
correctly in both arms. The reused binding's `[value, type]` expr is the artifact's own, so the
type is a **static** node, and `ValuePrinter` had no static path — `element_any` dumped every
frozen cell through `raw_any`, which is why the value's cells came out raw. `ValuePrinter::value_any`
now takes an `AnyNodeId` type exactly as the type printer's `static_elements` already did, and
`slot1_is_self` compares a static self-ref; the same program prints `((1, 2), (2, 2))` and
`tuple{struct{.x Int, .y Int}, struct{.x Int, .y Int}}` in both arms. The raw mark stays the
fallback for a value the type chain cannot explain — `raw-rendering-mark.md`'s ruling is that such
a mark is a defect, and a frozen cell no longer reaches it.

### 9.18 What the cell record carries, and the restart it bought

The record was a path, the artifact's module key, the `K_in` words and the artifact bytes. It
carries two more things now, both of which the artifact's own wire form cannot say:

- **`refs` — every referenced module's key and artifact hash.** An artifact's values are handles
  into the module that filed them, written as `(key, offset, len)`; a loader therefore needs those
  modules registered under the *same* keys. The record names each one with its hash, and a record
  loads only when every key is registered with the hash it names. A dependency that changed is a
  miss, never a stale value.
- **`read_only` — the freeze mode.** Without it a reloaded `ReadOnly` artifact would answer
  `Portable`, which is the refusal that is its whole guarantee. The record says the mode, and
  `load_cells` writes it onto the deserialized module.
- The container's `FORMAT` is 3. An older container is a miss, which is the container's whole
  compatibility story.

The restart, with the application's cells opted into `ReadOnly` (its values are buffers, never
table keys, so the small freeze is the honest mode):

| arm | 128 × 128 ms | 512 × 512 ms | cells |
|---|---|---|---|
| a fresh compile | 267.42 | 3894.25 | — |
| cold: empty cache, compile and write | 260.89 | 4075.05 | frozen 13, files 12 |
| **warm: load and reuse** | **81.71** | **831.92** | **reused 12, frozen 1** |

So a restart over its own cache is **2.5–3.3× cheaper at `128 × 128`** — the warm arm held at
78–82 ms while the fresh one ranged 195–267 ms across runs, §9.2's variance again — **and 4.7× at
`512 × 512`**, the values equal a fresh compile's at both sizes, and all thirteen cells survive
(§9.19 removed the one that did not).

### 9.19 A `Portable` cell persists, because a kernel id is its content

The codec refused compute operations and kernel values, and that refusal was the only reason a
default-mode cell could not be stored. Both now have an on-disk form, and the form is the identity
that already existed:

- **A kernel value is its digest.** `KernelId` is the entry's content digest — the fragment's
  hash folded with its claim — so the id names the same kernel in every process (`D15`; it was a
  counter until `kernel-disk-cache.md`'s slice measured the difference, and `intern_kernel` now
  returns the digest). `Kernel(id)` is the value tag
  `10` and `ParKernel(id, backend)` `11`, the parallel one adding the backend tag. A cell whose
  value is a kernel therefore persists and resolves like any other: measured, a `@cache`d
  `compute.parallel` result writes its one file and the program still answers.
- **An operator is a tag.** All fifteen `ComputeOperator` variants are tagged `0..=14`, never
  reused, so a `Portable` cell's operations — the `Parallel` that made its kernel and the
  `ParLaunch` that filled its buffer — are written like any other node.
- What still refuses is what names a registry entry rather than content: `Graph` (a recording's
  slot), `DeviceBuffer` (device residency) and a recording's placeholders. A cell whose closure
  holds one of those stays uncached, by feature rather than by bug.
- **The package boundary is where this must not apply.** A package artifact *replaces* its
  source, so a handle in it would name a registry entry the reader's process never creates; the
  package writer therefore asks `ArtifactCodec::runtime_only` (does the module hold any handle?)
  and leaves the package uncached, which is the rule `runtime_only_package`'s test states. A cell
  artifact is loaded *beside* its source, where the same id resolves.

Measured, `128 × 128`, the two modes side by side (§9.18's own numbers were `ReadOnly`):

| arm | files | warm ms | fresh ms | restart |
|---|---|---|---|---|
| `ReadOnly` | 13 | 70.42 | 321.65 | 4.6× |
| **`Portable`** | **13** | **139.12** | **403.85** | **2.9×** |

So the default mode has the cross-process half too, and its warm path is *slower* than
`ReadOnly`'s: the artifact carries the operations, so the load is heavier. `ReadOnly` stays the
right choice for an application that can make it; `Portable` is no longer the mode that cannot
persist at all.

## 10. Costs and failure modes

- **A heavy cell is expensive to freeze, and the cost is its operand chain and classes, not
  its value and not its template.** Measured in §9.2 and §9.3: freezing six cells whose
  values came from ~900-application applies cost **≈22 ms per cell in release** (≈165 ms in
  debug), against a 65 ms fresh compile of the whole program, and the artifact is 14 030 to
  43 148 nodes of apply clone graph held open by the operand edge. So a cell's break-even is
  *reused more often than recomputed*, and a policy that freezes a cell on every dirty edit
  can cost more than it saves. This is the retention policy's real constraint and it was not
  visible before a heavy cell existed to freeze.

- **`KEY_FORMAT_VERSION` stops being free to bump.** `incremental-update.md` §8 currently
  says a bump "invalidates every cached key; today that is in-memory only … so a bump is
  free — check that before the keys are ever persisted". **This is that moment.** Once a
  cell key is persisted, a bump retires every cache and becomes the same kind of contract as
  `ARTIFACT_FORMAT_VERSION`, which has a written version history for exactly this reason.
  The version history must be started when the first persisted key lands, not after.
- **A key that omits a read is unsound, silently.** This is the obligation §4 narrowed, and
  it is the mechanism's whole risk. The edge set is `dirty.rs`'s exhaustive `walk` over
  `Expr`, which is written as a full match on purpose so a new expression form is a compile
  error. **The key must be derived from that same walk, not from a second traversal**,
  or the two can disagree about what a binding reads.
- **The duplicate read graph.** `Resolver` already builds `binding_edges`
  (`resolve.rs:100`), uses it only for cycle detection, and never exposes it; `dirty.rs`
  re-derives an equivalent graph by AST walk every compile. The key needs the same edges, so
  this is now a third consumer of one relation. It should be computed once and shared, but
  note the resolver's is `BinderId`-keyed and reports *unresolved* reads differently from
  `dirty.rs`'s statement-granular one — they are not interchangeable as they stand, so
  unifying them is its own step, not a side effect.
- **`~/.lichen` is untrusted input** (`D1`). A key match decides *whether to use* a stored
  artifact; it does not decide whether the bytes are *safe*. The `body_digest` and the total
  field validation stay, and a key is not a signature.
- **A shared entry has one owner.** Two bindings with one key share one artifact; the byte
  budget must count it once, and the event report must name the cell that is reading it.
- **The window is an accelerator, so it may be wrong without being unsound** — but only once
  the key path is the judge. Until then it is still load-bearing, which is why propagation
  is kept rather than deleted (§1).

## 11. What would falsify it

- **The key fold dominates a build.** Then §5's "resolve is already whole-program" stops
  being a reason and the answer is to make resolve incremental.
- **A key omits a read and the differential oracle does not catch it.** Then the edge set has
  a hole the exhaustive walk does not cover, and the oracle must be strengthened before
  anything else.
- **`K_out`'s hash costs more than the recomputation it saves.** Then backdating should be
  opt-in per cell (it is already proposed as such for T3) rather than automatic, and the
  tier table becomes a policy table.
- **Keys deduplicate in a way that confuses the report.** Then sharing must be keyed to the
  reporting cell rather than to the artifact, which is a report-shape change, not a cache
  change.
- **A typical agent edit dirties most cells.** §9.2 measures the all-dirty case at **0.3×** —
  the session three times slower than a fresh compile — so if an agent's real edits usually
  reach the upstream-most stage, retention saves nothing and the freeze *costs*. The
  measurement that would settle it is the distribution of dirty-fraction over a real edit
  session, which nobody has taken.

## 12. Decisions taken

- **Keys are allowed; the key is the judge and propagation is the accelerator.** Where they
  disagree the key wins, and that is a bug in propagation.
- **Every binding gets a key; `@cache` decides retention, not existence.**
- **`K_in` folds direct reads' `K_out`, not a transitive rehash**, and is over the resolved
  structure with spans dropped.
- **A binding's `@cache` mark is not in its `K_in`** (unlike the build content key).
- **Backdating is obtained from `K_out` folding, not from a recompute heap**, which retires
  `incremental-update.md` §4.4's "not a local addition" conclusion.
- **The two-program propagation argument is not needed by the key route** (§4); the window
  stays as an accelerator.
- **§2.4's rejection of content identity is not reopened** — it was about retention
  identity, and this is value-cache identity (§6).
- **T3 is untouched by the ruling**, and its digest and its persistence share one cost: the
  device→host readback (§7).
- **Propagation is kept, not deleted**, until the key path is measured (§1, §5).
- **The freeze granularity is `Portable` by default, `ReadOnly` per cell.** Decided by the
  superior after §9.3's falsification; built in §9.4, with the key hash refusing a
  non-portable value rather than mis-hashing it.
- **T2 is closed on both halves** (§9.5, §9.6): a collected buffer retains as one node plus its
  bytes, and the codec now files it (`ARTIFACT_FORMAT_VERSION` 11).
- **T3 materializes on freeze.** Decided by the superior: a device-resident cell reads back to
  the host when it is frozen, so it persists as T2's `Buffer` rather than needing a device
  identity in the bytes. The price is one device→host read per freeze — the same read §7 says
  a `K_out` digest of such a value would already cost — and the restored value is a host
  buffer, so a consumer that wants it resident again must re-upload. **Not built.**
- **The reuse gate is the coordinate space, not the line table.** Decided by the superior after
  §9.7 named the defect; built and measured in §9.8, which takes a whitespace edit on the view
  path from unreachable to 26×.
- **The key decides, and propagation is the fallback where the key cannot be written.** Landed
  in §9.10 after §9.9's oracle and §9.10's widened sample showed the two classes of disagreement
  are both conservative. A `None` key is read as changed.



## 13. Open

- **T3 — a device-resident value — is decided and not built.** The ruling is to materialize on
  freeze: the cell reads back to the host and persists as T2's `Buffer`, priced at one
  device→host read per freeze. What blocks it is a **fixture**: no language-level program yields
  a `DeviceBuffer` (`plrun` collects to host; only a graph run keeps a value resident), and a
  hand-built `ResidentId` would meet a release obligation naming an id that no backend issued.
  The safety basis is verified even so: the indexed read, the whole read and the run-input path
  each accept a `Buffer` beside a `DeviceBuffer`, so materialization changes the representation
  and not what a consumer can do.
- **The scope key for the cache directory is a type name.** §9.16 scopes by
  `std::any::type_name::<P>()`, which is not a stable ABI: a rebuild moves it and costs a miss.
  A cache meant to survive a rebuild needs a deliberate key — the artifact store's
  `compiler_slot_key` is the shape — and that is not built.
- **A `ReadOnly` cell persists**, and its mode travels in the cell record rather than in the
  artifact's wire form; §9.18 is the change and its measurement.
- **A cell that reads an import persists too**, keyed by the imported package's artifact hash and
  guarded on load by the record's dependency hashes (§9.18). What is left is the unmeasured half:
  a *rebuild* moves the package's artifact hash and the record's keys alike, so a cache that
  should survive a compiler rebuild is §9.16's deliberate scope key away, not this one.
- **The graph-node axis** — the descriptor `incremental-update.md` §5 promises. Under the key
  ruling it is an accelerator and a reason source, not a soundness requirement, so it is a
  smaller job than that note assumes.
- **What replaces the removed work bound** (§8.2). The count conflated "too much work" with
  "does not terminate"; removing it left a non-nesting divergence unguarded, so an endless
  `@loop` hangs the compiler and the language server's worker. The shape to choose is a
  wall-clock deadline recorded as its own refusal, with a limit the editor path can set lower
  than a batch host's.
- **The session's fixed per-edit overhead is unattributed** (§9): 1.6× on a first compile, and
  an edit costing 87 ms against a fresh 57 ms at one heavy cell.
- **The deep pass is still not separated from the check** (`incremental-evaluation.md` Q9):
  §9.1 splits the frontend from the check, not the check from its evaluation.
- **A cell whose closure holds a graph or a device buffer still cannot persist.** Both name a
  registry entry rather than content — a recording's slot, device residency — so the codec
  refuses them by name (§9.19). Encoding a graph's structure and re-registering it on load is the
  step that would close it; §9.19's kernel form does not reach it.
- **A reused cell's type is restored, and its value prints by its form.** The `raw` reading
  was the *value* printer lacking a static path, not a lost type (§9.17); both halves now read
  as a fresh compile does.
- **No operator was missing for a real application, and the one §7 predicts is still absent.** The
  world generator needed only `range`/`row`/`col`/`read`/`write`/`if`; the PCG domain's next
  workload, a scatter-accumulate, is still silently last-writer-wins on both backends
  ([gpu-algorithms-ladder](gpu-algorithms-ladder.md#5-a-scatter-accumulate-is-silently-wrong)),
  and the design on file is to refuse a contended write rather than to add atomics
  ([shared-memory-and-barriers](shared-memory-and-barriers.md)). Nothing measured since changed
  that preference.

## 14. Probes

Every reading in §9 comes from one of these. They print and assert nothing, and each one's
module doc says what it is for; the note is the durable record, so a probe is deleted once the
question it serves stops moving.

| probe | question |
|---|---|
| `lichen-language/examples/cache_timing` | the session against a fresh compile, per edit kind |
| `lichen-language/examples/pipeline_edits` | the tuned pipeline's edit loop and its restart (§9.2, §9.11, §9.14) |
| `lichen-language/examples/view_reuse` | whether a whitespace edit reuses through `set_view` (§9.7) |
| `lichen-language/examples/key_oracle` | the key against propagation, by path set (§9.9, §9.10) |
| `lichen-language/examples/cell_persist` | a cell's artifact into an empty registry (§9.12) |
| `lichen-language/examples/cell_reload` | four sessions over one cache directory (§9.13, §9.15) |
| `lichen-compute/examples/cache_buffer` | a compute value's retention cost and round trip (§9.5, §9.6) |
| `lichen-compute/examples/cache_pipeline` | buffer cells on a real compute program (§9.7, §9.16) |
| `lichen-compute/examples/worldgen_edits` | the world generator's edit loop and restart, whole map against whole map (§9.17) |
| `lichen-compute/examples/buffer_reuse` | one frozen buffer feeding one later launch, alone (§9.17) |
