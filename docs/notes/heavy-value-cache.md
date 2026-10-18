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
"a key that omits a read is unsound", which is a narrower and checkable obligation — §9.

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
so byte accounting and the "why did this recompute" report must attribute it once. §8.

## 7. What keys do not solve: the three value tiers

Both target domains' heavy data is the data that is hardest to persist. The tiering is by
value shape, and the key ruling moves two of the three:

| tier | value shape | in-process reuse | backdating | persistence | why |
|---|---|---|---|---|---|
| **T1** | a solved host value (scalar, string, host array, table, struct) | yes | yes | **yes** | `LowValue` codec tags `0,1,5,6,8`; the arena is already flat bytes |
| **T2** | a packed host buffer `Buffer(AnyHandle<[u8]>, ScalarClass)` | yes | yes | **yes, after lifting a refusal** | the bytes are in the block arena, but the codec refuses the value *by name*; the element class rides on the value, so it must travel with it |
| **T3** | device-resident `DeviceBuffer` / `Graph` / `Kernel` / `ParKernel` | **yes** | **at a cost** | **no** | see below |

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

## 8. Costs and failure modes

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

## 9. What would falsify it

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

## 10. Decisions taken

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

## 11. Open

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
- **The measurement** (`incremental-evaluation.md` Q9 is still owed): no number exists for a
  build's wall-clock split, so "the key fold is the same order as resolve" is an argument
  from the code and not a reading.
