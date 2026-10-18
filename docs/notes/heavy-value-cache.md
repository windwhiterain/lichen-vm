# Heavy values across edits: keys as the judge, propagation as the accelerator

> Status: **proposed for the key half; nothing here is implemented.** This note records a
> decision that replaces one half of [incremental-update](incremental-update.md): that note's
> **"no keys"** ruling is **withdrawn**, and a content key may now decide reuse. What the
> decision does *not* touch is stated in §5, and what it does not *solve* is in §7.
>
> This is the plan for two application domains — data-science dependency graphs and
> procedural-content dependency graphs — where a long-lived graph is edited by an agent and
> some nodes carry heavy computed data. The mechanism that already exists is
> [incremental-update](incremental-update.md)'s: occurrence-path identity, the `cache` mark,
> per-cell freeze into a `StaticModule` closure, a `CellStore`, and dirty propagation over the
> resolver's read graph. **This note does not restate any of that**; it records only what the
> key ruling changes.
>
> Points at: `crates/lichen-language/src/{cells,dirty,session,compile}.rs`,
> `crates/lichen-language/src/lib.rs` (`freeze_cells`), `crates/lichen-language/src/resolve/content_key.rs`,
> `crates/lichen-lowlevel/src/registry.rs` (`freeze_closure_mapped`), `crates/lichen-registry/src/device.rs`
> (`artifact_hash`, `verify`, `publish`), `crates/lichen-compute/src/compute.rs` (the value codec),
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

Every binding gets a key. `cache` decides which keys are *retained*, not which exist. That
is what lets a marked binding's key be compared against an unmarked dependency's without
the dependency having to be marked too.

```
K_in(b) = H( KEY_FORMAT_VERSION
           , vocabulary_key                 // the plugin set: value/operator leaves
           , source_identity                // the file id, for imports and cross-file reads
           , resolved_structure(b)           // b's own AST, names by BinderId, spans dropped
           , [ K_out(d) for d in reads(b) ]  // direct reads, in the resolver's order
           )
K_out(b) = H( the value b evaluated to )
```

Five properties are contractual, and each one is a decision rather than an accident:

- **`K_in` folds the *direct* reads, and each read contributes its `K_out`.** A transitive
  fold would rehash a shared subexpression once per path to it; this shape costs one hash
  per binding per build, which is the same order as the resolve walk that already runs
  (see §5).
- **A binding's own `cache` mark is NOT in `K_in`.** Marking or unmarking a binding does not
  change the computation, so it must not move the key. This is deliberately *unlike* the
  build content key, where `cache` **is** included (`KEY_FORMAT_VERSION` 2) because the
  lowering consumes it. Two keys, two questions; conflating them would make toggling a mark
  invalidate values it did not change.
- **`K_in` is over the resolved structure, not the text.** Names are `BinderId`s, never
  spellings, and spans are dropped — the same rules `resolve/content_key.rs` already applies
  to the build content key. Two consequences, both wanted: editing whitespace or moving a
  binding without changing resolution does not move any key, and a resolution that changes
  under unchanged text (a shadowing insertion) *does* move it.
- **`vocabulary_key` participates** because a `jit`/`parallel` leaf's meaning is the
  plugin's, and the artifact store already scopes per plugin set for exactly that reason
  ([artifact-cache](artifact-cache.md), "the artifact store is scoped per plugin set").
- **`K_out` is computed after evaluation, from the value.** It is what makes backdating
  free; §4.

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
remains correct for that job: a `cache` cell is still identified by its occurrence path, and
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

One process, wall clock, on `feature/heavy-value-cache`. §9.1 is a **debug** build, so its
absolute milliseconds are inflated and its ratios are the reading. §9.2 has **both** builds,
because its headline is the one that had to survive a release compile.
Probes: `crates/lichen-language/examples/{cache_timing,pipeline_edits}.rs` (temporary; delete
when this closes).

**Shape**: 8 cheap `cache`d bindings in a chain plus 4 heavy ones calling a naive `fib 10`,
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
domains actually have: a generator pipeline of 6 stages, each `cache`d stage reading the one
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
side. A real `cache out = …` binding carries a `[value, type]` pair, and the type graph is
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
`doubler`, with `cache data = raw.z` and `cache result = raw2.w.native` — the annotation that
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
- **Every binding gets a key; `cache` decides retention, not existence.**
- **`K_in` folds direct reads' `K_out`, not a transitive rehash**, and is over the resolved
  structure with spans dropped.
- **A binding's `cache` mark is not in its `K_in`** (unlike the build content key).
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


## 13. Open

- **The exact persistence container for a cell.** A cell artifact is a `StaticModule` closure
  with the closure's own ref set; the package store's six acceptance criteria
  (round-trip across stores, recovery from a crash between alloc and publish, corruption
  rejected by the body digest, only the changed chain recompiled, reclamation) were written
  for the package container and are the template, but a cell's container is not a package's.
- **Whether the key is computed for unmarked bindings at all in the first build**, or only
  lazily when a marked binding reads one. The former is simpler and costs one hash per
  binding; the latter is cheaper and needs a demand order.
- **How a graph node's key folds its edges** — the descriptor that
  `incremental-update.md` §5 promises becomes an accelerator and a reason source under this
  ruling, not a soundness requirement, which is a smaller job than that note assumes.
- **What replaces the removed work bound** (§8.2). The count conflated "too much work" with
  "does not terminate"; removing it unblocked heavy helper-side generation and left a
  non-nesting divergence unguarded, so an endless `@loop` hangs the compiler and the language
  server's worker. The shape to choose is a *wall-clock deadline* recorded as its own refusal,
  which bounds the compiler without refusing honest work. Its limit is a policy (seconds, not
  counts), and it must be one the editor path can set lower than a batch host's.
- **The session's fixed per-edit overhead is unattributed** (§9): 1.6× on the first compile,
  and an edit costing 87 ms against a fresh 57 ms at one heavy cell. Profile before designing
  around it.
- **The freeze's granularity is built (§9.4), and what remains is its enforcement.** The mode
  is per cell, by binding name, defaulting to `Portable`; the key hash refuses a `ReadOnly`
  node it meets in template mode instead of mis-hashing it. What is *not* settled: the refusal
  covers the template unfolding (where the operation was the content), not every way a value
  can be compared — `key_eq` compares values, which the two modes agree on, but a future key
  path that read a stored operation would have to repeat the check. And a **persisted**
  `ReadOnly` artifact would need the mode in the wire format; today the loader sets
  `read_only: false`, which is right only because `ReadOnly` cells are not persisted yet.
- **The measurement** (`incremental-evaluation.md` Q9 is still owed): §9 has the
  frontend-versus-check split now, but not the deep pass separated from the check, and not
  the session's overhead broken out.
