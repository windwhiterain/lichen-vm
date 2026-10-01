# Incremental update: identity by path, retention by `cache`

> Status: **landed through step 3, unwired.** Path identity, the `cache` syntax, the
> per-cell closure freeze, the general release obligations, the cell store, the dirty
> input and eviction all exist and are measured (§7.1); the mechanism has **no
> production caller** yet — the session's reuse gate is still a whole-`Build` content
> key. **§12 is the handoff**: what exists, in what order to continue, the landmines,
> and how to verify. Read that first if you are picking this up cold.
>
> This is the *cross-build* half of the incrementality question. It supersedes the
> withdrawn cross-build halves of [incremental-evaluation](incremental-evaluation.md),
> which keeps the *within-build* settled cut, the deep-pass measurement and the
> mutation inventory; where the two meet, this note links there instead of restating.
>
> **The requirement is external mutation, under two constraints (both the
> superior's):** an agent edits the generator program *and* the graph between runs,
> so the consequences of an edit must be re-derived incrementally; and **no keys**
> may decide reuse — no content hash, no interning, no identity derived from a
> value. Invalidation must be **dirty propagation**.
>
> **Target consumer: an agentic PCG graph.** The graph is long-lived, its expensive
> outputs are generated content, and the agent's edit loop is what has to get cheap.
>
> Points at: `crates/lichen-language-parser/src/path.rs`,
> `crates/lichen-language/src/{cells,compile,lib,session,run}.rs`,
> `crates/lichen-lowlevel/src/{lib,registry}.rs` +
> `static_module/{freeze,apply}.rs`, `crates/lichen-compute/src/compute.rs`,
> `crates/lichen-language/src/program.rs` (the composition macro),
> and the notes [incremental-parse-compile](incremental-parse-compile.md),
> [artifact-cache](artifact-cache.md), [static-modules](static-modules.md),
> [attributes](attributes.md), `compute-graph-jit`.

## 1. The principle: identity is allocated, not derived

Every earlier attempt to make this layer incremental looked for an identity the
*compiler* could compute: a content key, a hash, an interned id. All of them are
keys, and all of them were rejected. The reason is structural, not stylistic:

- `NodeId` is a versioned slot key allocated by the checker's walk — editing an IR
  prefix shifts every later allocation (`incremental-evaluation.md` §2).
- `BlockId` is one per lambda (`checker/lambda.rs:31`), not a unit boundary.
- `ExprId` is a counter over the IR; it shifts under an insertion.
- A content key is the thing the requirement forbids.

So the identity has to be **allocated by something that outlives the edit**, and
there are exactly two such allocators: the *structure* (a node's position in the
tree) and the *user* (a mark the user wrote). This note uses both, for different
jobs:

- **identity** comes from the structure — a node's **occurrence path** from the
  program root (§2);
- **retention** comes from the user — the **`cache`** mark, which selects *which*
  paths are kept across a build (§5).

The two are orthogonal: every node has a path, so dirty propagation is universal;
only marked paths hold memory, so retention is bounded by what the user asked for.

### 1.1 What this is *not*: the JIT's graph IR

`crates/lichen-graph-ir` is the **compute** layer's graph: the shape of a lichen
program that has been *evaluated once and whose evaluation was recorded* — kernel
and native nodes, and the submission policy that decides when the host observes
completion. It is **JIT-specific**, and it is not this design's subject.

A `cache` cell is a **language-level retention point**: a marked position in the
source, identified by its occurrence path, whose value survives a build. The two
structures share no identity and no node set — a graph node is something that
*ran*, a cell is something the user *marked*. Where they do meet is residency, and
only residency: a cached cell's value may hold a device buffer the compute layer
produced, which is §8's problem (release on eviction), not an identity one.

## 2. Identity: the occurrence path

### 2.1 The definition

A node's identity is the sequence of steps that reaches it from the program root —
an occurrence path, in the sense of Dewey-order/path-based node identification
([Dagstuhl survey](https://drops.dagstuhl.de/storage/16dagstuhl-seminar-proceedings/dsp-vol05061/DagSemProc.05061.6/DagSemProc.05061.6.pdf)),
and the same shape as rust-analyzer's `SyntaxNodePtr`, whose own module doc gives
the argument this note rests on: syntax trees are **transient objects**, so holding
a node is ill-advised; instead store the node's **location**, a small value that is
**resolved back into the tree when needed**
([`syntax::ptr`](https://rust-lang.github.io/rust-analyzer/syntax/ptr/index.html)).

That is exactly this design: the `Module` stays transient (a rebuild still starts
from `Module::new()`, `checker.rs:518`), and the retained state is keyed by a path,
which is small, storable and resolvable.

### 2.2 Steps: a name where the syntax has one, an index only where it does not

A step is a **name** wherever the position sits in a *list the syntax can name* — a
binding statement, a named struct field, a named instantiation argument — and a
**child index** everywhere else: an element of a tuple, array or table literal, an
argument of a `$op(…)` form, or a fixed-arity role. Fixed-arity positions use
**reserved role slots**, never compacted ones: a lambda's `parameter_type` is always
`Index(0)` and its `return` always `Index(2)`, whether or not the optional
annotations are present, so adding an annotation does not renumber a stored path.

The vocabulary is the contract, and it lives in exactly one place —
`crates/lichen-language-parser/src/path.rs`, whose `children` is the only
enumeration of a node's positions. Changing it renumbers every stored path, so it is
a compatibility contract exactly as the attribute order is.

Names are preferred because of the edit that dominates an agent's loop: **an
insertion before an existing statement**. Under a pure child-index path, that
insertion shifts the path of everything after it in the block, so every retained
cell in that block is lost. Under a name step, nothing after it moves. The residual
invalidations are honest and rare:

| edit | effect on a path | retention |
|---|---|---|
| append a statement | none for the statements before it | kept |
| insert *before* `terrain` | `terrain`'s own step is a name | kept |
| change `terrain`'s body | `terrain`'s path unchanged; its value may change | re-derived, then cut by §4.3 |
| rename `terrain` | the step changes | **lost** — a different name is a different cell |
| move a statement into a block | steps change | **lost** |
| edit an anonymous literal's children | index steps after it shift | lost below that point |

Every "lost" row is *safe* (it over-invalidates; it never serves a stale value), and
§5.2's event surface is where each one is meant to become a diagnostic.

### 2.3 Resolution is dynamic — a path is a locator, not a registry entry

**Nothing is registered when a node is built.** No `Path → NodeId` table exists, and
neither the front end's compilation nor the checker's allocation path is touched. A
path is resolved **on demand** by walking the tree one step at a time — exactly as
`SyntaxNodePtr` is resolved into a node when someone needs one.

The tree is the **AST**, not the highlevel IR: the IR is a *graph* (a binding's node
*is* its value's node, so it has several positions) and it carries no binding names,
while the AST is the tree the resolver already walks by name. A path therefore names
a **position**: a node reached by several positions has several paths, and a
retained cell is a position, not a node.

Three consequences, all wanted:

- there is no bookkeeping to keep in step with allocation, so there is no second
  list that can drift (the failure mode [attributes](attributes.md) had to delete);
- a path that **does not resolve** in the current build is a lost cell — a
  reportable event (§5.2), never a silent reuse;
- the resolution cost is paid only for marked paths, on demand, and it is
  `O(path length)` — not per node.

### 2.4 Why not the alternatives

- **Content keys / hashing** — forbidden by the requirement, and the reason is not
  merely a rule: a hash answers "is this the same?" only by comparing content, so it
  cannot distinguish "the same node, moved" from "a different node" without also
  hashing the context, which is the path again.
- **Interning** — a key with a nicer interface.
- **`NodeId` / `ExprId`** — allocated by a walk that shifts; §1.
- **A user-written name only** (an earlier draft of this note) — it covers named
  bindings and leaves anonymous intermediates with no identity at all, which breaks
  the instance case (§3). A path covers every node.

## 3. Instances: a node inside a lambda

A binding inside a lambda body is not a per-apply accident: its path runs through
the *template*, and the apply materializes an **instance** of that template
(`node_apply`'s clone walk, `function.rs:318-338`; one block per lambda,
`checker/lambda.rs:31`). The instance's identity is:

> **(the template's path, the paths of the arguments/captures it was instantiated with)**

— all paths, so a `cache` inside a lambda is well-defined and needs no extra
mechanism. This is the precise form of the superior's observation that an instance
"eventually reaches a top-level scope with no free variables": the instance is
grounded in *paths*, not in values, so grounding never requires a key. An anonymous
intermediate passed as an argument is not a hole either — it has a path of its own.

**Not built.** Nothing instantiates a cell per instance today: `cache` is honored for
a binding the compiler walks once (a program or block statement, a record field), and
a binding inside a lambda body is lowered per apply like any other. §8's instance
explosion is the item that has to be settled before that changes.

## 4. Invalidation: path-valued dependencies, verified on demand

> **Not built.** This is the design for the *finer* second step. What exists today is
> coarser and sound without any of it: the caller declares which sources an edit
> dirtied (§7.1), so a cell is either consulted or dropped, and nothing is recorded.

### 4.1 What a retained cell would record

A marked cell would store, beside its value:

- **its own path** — its key in the store;
- **the paths it read** — recorded when its value was computed;
- **a version stamp** — when it was last verified.

Dependencies are stored as **paths**, not as node ids and not as a reverse-edge
index: the same "location, not pointer" move as §2.3, applied to the dependency
relation. There is no `dependents` graph to maintain, and nothing to hook into
allocation.

### 4.2 Verification (pull), not propagation (push)

To re-derive after an edit, a demanded cell is **verified before it is recomputed**:
for each recorded dependency path, resolve it (§2.3) and ask when it last changed.
If none changed since the cell's own stamp, the cell is clean and is **not
recomputed at all** — the red-green step of Salsa's algorithm, with paths in place
of query keys ([Salsa, the "red-green" algorithm](https://salsa-rs.github.io/salsa/reference/algorithm.html)).
The topological recomputation order, where recomputation is needed, is the
recompute-heap discipline of [Incremental](https://ocaml.janestreet.com/ocaml-core/v0.12/doc/incremental/Incremental__/Recompute_heap/)
(a node only ever consumes an already-verified dependency).

A dependency path that no longer resolves counts as **changed** (conservative) and
is reported.

### 4.3 Cutting the cone: compare values, never keys

After a recomputation, the new value is compared with the retained one. Equal means
**backdate**: the cell keeps its old stamp, so no consumer is dirtied. This is the
role `content_key` plays today, replaced by "recompute one layer and compare" — the
cost the no-keys constraint buys. The comparison needs no authoritative digest: a
false "different" only costs work, never correctness. (This retires the old note's
"authoritative value digest" question: a digest is only needed when a *key hit*
must be trusted to serve an answer, and nothing here is served from a key.)

### 4.4 Cycles

A marked cell that participates in a value cycle is verified and recomputed as one
**SCC-atomic** unit; the in-progress assumption the within-build note names (`P1-31`)
is the same assumption, at the same place.

## 5. Retention: the `cache` mark

### 5.1 Syntax (landed)

```lichen
cache terrain = noise(seed) |> erode(200)
cache scatter = poisson(terrain, density)
```

`cache` is a **keyword** and a **binding modifier**, in the same position as `pub`
and `let` (`pub` is `BlockStmt`-level, `let` is `Binding.restrictive`). It is
**allowed in any scope**, because §3 gives a node inside a lambda a well-defined
identity.

It is deliberately **not an attribute**: an attribute's slot is a *runtime* value —
the annotation's own `[value, type]` term pair ([attributes](attributes.md)) — while
this mark is static and identity-selecting. A `cache` mark must be visible at
lowering, before any node exists, which is what a keyword is and what a slot is not.

Durability and the equality policy for backdating (§4.3) are deliberately **not**
in this syntax: the mark carries identity work only, and those two are policy that
can arrive later on the host side without renumbering anything.

The mark is **not** an error anywhere: it is accepted in every scope and combined
freely with `let`, and no diagnostic is emitted for it. A record block's field
carries it too (`RecordField.cached`), since a record block's fields *are* its
statements — without that the mark vanished in the statement → field conversion,
which is how it was found.

### 5.2 Semantics and the event surface

- On first computation, the value is frozen into the store under the cell's path and
  later read **in place** (the `StaticModule` read path — no payload copy). **Landed.**
- On a later build with the same path, the cell is reused or (when its source was
  invalidated) recomputed. **Landed**, at source granularity.
- Every one of the events — `created`, `reused`, `recomputed (because …)`, `lost
  (path changed / unresolved)` — is meant to be **reportable**: the PCG-side analogue
  of a cook graph's "why did this re-cook", and the differential harness's oracle.
  **Not built**: `CellStore` records and drops silently today; the event surface is a
  caller-facing API that does not exist yet.

## 6. The two edit surfaces

Decided by the superior:

- **Graph edits use a descriptor.** Here "graph" is the *program's own* graph data —
  the structure an agent builds and edits (a PCG graph), **not** the JIT's
  `lichen-graph-ir` (§1.1). It is edited *as data* — add / remove / rewire /
  re-parameterize a node — so the edit names the node paths it touched and the
  invalidation is exact. No diffing is involved.
- **Source edits use file granularity.** A source edit dirties the file; the file's
  marked cells are re-derived, and §4.3's backdating is what keeps the rest of the
  program, and the graph, from re-running. Sub-file identity is *not* needed for
  this, which is why no content key is needed either.

## 7. What changes where

| step | file / item | state |
|---|---|---|
| 1 | `lichen-language-parser/src/path.rs` | **landed**: `Step`/`Path`, the position vocabulary (`children`), dynamic `resolve`, the descent (`for_each`) |
| 1 | the front end / checker | **not built**: recording a marked cell's read paths while it is computed (§4) |
| 2 | `lichen-language-lex` / `-parser` / `ast.rs` | **landed**: the `cache` keyword, `Binding.cached`, `RecordField.cached` |
| 2 | `language-spec.md` §2 + `tree-sitter-lichen` (`grammar.js`, `highlights.scm`) | **landed**: the statement form, the keyword list, the highlighting |
| 3 | `lichen-lowlevel/src/static_module/freeze.rs` | **landed**: `freeze_closure` (the closure of a root set) and `freeze_set` (the shared phases); `from_module_mapped` is now "the closure of every node" |
| 3 | `lichen-lowlevel/src/lib.rs` (`Release`, `ValueExt::release_obligations`, `StaticModule::releases` + `Drop`) | **landed**: the general ownership transfer — an artifact owns its out-of-arena resources and releases them when it is dropped |
| 3 | `lichen-compute/src/compute.rs`, `lichen-language/src/program.rs` | **landed**: the compute leaf's obligation (a `DeviceBuffer` value) and the composition macro's forwards (`traced`, `release_obligations`) |
| 3 | `lichen-language/src/{cells,compile,lib}.rs` | **landed**: `CellStore`, the lowering hook (a clean cell lowers to `ExprKind::Static`), the per-cell freeze after the build, `compile_with_cells` |
| 3 | the dirty input and eviction | **landed**: `invalidate(source)` drops a source's cells and hands back the artifacts; `Registry::evict` frees one — the caller's call, because a static ref is a raw handle into the artifact's arena |
| 3 | a consumer (`session.rs:243`'s `content_key` gate, the LSP, the package store) | **not built**: nothing production calls `compile_with_cells`, so no edit path uses cells yet; `content_key`/`artifact_hash` are to be **demoted** to transport and diagnostics rather than deleted |
| 4 | the program's own graph data (the PCG graph a program builds) | **not built**: node paths and the edit descriptor. **Not** `lichen-graph-ir`: that crate is the JIT's recorded evaluation graph, a different structure with a different identity (§1.1) |

Step 4 is a *consumer*, not a mechanism: the agent edits the graph as data, and the
cells it marks are what make an edit cheap. Nothing in `lichen-graph-ir` changes for
it — the two graphs are different structures (§1.1).

### 7.1 The retention landing's shape (step 3) — what exists

The mechanism is the **existing** import path, which is what made this step small:

1. A cell's value is a `[value, type]` pair, and freezing turns a solved node set
   into a static artifact *and* hands back the `NodeId → LocalNodeId` map — so a
   cell's frozen reference is
   `StaticNodeId { module: key, index: node_map[&pair] }` (`lib.rs:901`).
2. A later build **reuses** a clean cell by lowering it to
   `ExprKind::Static { export }` instead of compiling its body — the node the front
   end already emits for an import (`compile.rs:227`), materialized by the checker's
   own arm (`checker.rs:1399`). The body is not lowered, not checked and not
   evaluated: the reuse skips the work rather than caching its result.
3. So the store is a map `Path → StaticNodeId` and nothing else is new: no second
   read path, no new node kind.

**The freeze is per cell** (the superior's decision), not whole-module: each cell gets
its own artifact and its own key, and only a *recomputed* cell is frozen, so an edit
pays for the cells it dirtied and nothing else. That needed one new entry point — a
**closure** freeze — because `from_module_mapped` maps *every* node of the module.
The three phases already work on a set of nodes; what changed is that the set is the
closure. The closure must be **closed under four edge kinds**, or the existing
`node_map[&x]` lookups panic — they assume totality, which is the built-in check:

- a value's items/entries (arrays, tables, ext handles);
- `operation.operand` — a residual node must be able to re-run later, so unlike the
  GC's walk, which deliberately does not follow a cached value's operand, this one
  must;
- the **whole equality class** (`parent`/`next`/`tail` all go through `node_map`, and
  half a class is a broken class);
- the **whole function template** (`StaticFunction.nodes` is the template's member
  list, and a missing member is a broken template);

plus whatever the value reports through `ValueExt::traced` — the contract the GC
relies on, and the only way to see a node an opaque ext payload holds, because phase 3
rewrites a handle and never the bytes behind it. **A value that fails to answer is not
caught**, here exactly as in the GC.

**Per-cell freeze is an ownership transfer, and it is general** (the superior's
correction):

- a **handle-type** value (an array or table whose items live in the module's block
  arena) is **copied** into the artifact's arena, as phase 2 already does; the module
  keeps its own copy, so nothing dangles;
- **everything else a value owns outside the arena** is transferred by *obligation*,
  and the mechanism is deliberately not a device-buffer special case:

  ```rust
  pub trait Release { fn release(self: Box<Self>); }          // lichen-lowlevel
  fn release_obligations(&self, out: &mut Vec<Box<dyn Release>>) // ValueExt, per leaf
  ```

  `StaticModule` owns the list (`releases`) and its `Drop` runs it, so **eviction is
  the early release path**; an artifact loaded from the device's store owns none
  (`persist/container.rs`), because a resource handle is process-local and cannot be
  in the bytes. The hook is **per leaf** on `ValueExt` — composed by the manifest
  macro exactly as `is_handle`/`handle`/`set_handle`/`alignment` are
  (`program.rs:312-357`) — because the issuer is reachable *process-wide*
  (`lichen_kernel_ir::parallel_backend()`, installed by `install_parallel_backend`),
  so a leaf needs no program parameter to build its obligation. `ComputeValue`
  implements it for `DeviceBuffer` (`compute.rs`): the obligation holds the `Arc` the
  lookup hands back — the same "the installed backend is the issuer" assumption
  `collect`/`read` already make — and a backend that is not installed has nothing to
  release.

  This composes with the existing rule rather than fighting it: a value dropped by
  `drop_block` **still does not release** (the deliberate no-per-value-release
  decision), the context still owns the memory until it drops, and the artifact's
  obligations are the **early** release path. Because the module's own copy never
  releases, taking the obligation cannot double-release; adding per-value release
  later is what would have to revisit this, not the other way round.

**The store, the hook and the edit side** (all landed in `lichen-language`):

- `CellStore` (`cells.rs`) keys a cell by its **occurrence path** and holds only the
  frozen reference — the artifact lives in the caller's registry, exactly as an
  import's does. `compile_with_cells(source_id, source, cells, registry)` is the entry
  point.
- **One `path::for_each` pass** over the resolved AST collects the marked bindings'
  paths before lowering (each binding carries its own `BinderId`), so the lowering
  walk never carries a path. A marked binding whose cell is clean lowers to
  `ExprKind::Static`; a block-wide binding's placeholder *is* that static read, made
  in place rather than transplanted from a second node, which would leave that node
  behind in the IR.
- A marked binding that *was* compiled is frozen **per cell** once the build is solved
  (`Registry::freeze_closure_mapped`, the new entry beside `freeze_mapped`) and
  recorded under its path and its source. The registry's `hash` slot is a documented
  **placeholder** on this path — a cell's reuse is decided by its path, never by
  content (§4.3) — which is the same demotion §7's table records for `content_key`.
- The **dirty input** is the caller's, and it is the granularity chosen for source
  edits: `CellStore::invalidate(source_id)` drops that source's cells and hands back
  the artifacts they named, so the caller can evict once it is safe. The caller also
  drops the cells of every file that **imports** the changed one (the reverse import
  closure) — that graph is the package store's, not the cell store's. A cell is reused
  iff its own source was not invalidated. That is sound **without recording a single
  read**, which is why it comes before §4.

**Two limitations, recorded rather than hidden:**

- a marked binding whose value spells an **annotation** is not eligible (a static read
  materializes a two-wide pair, and an annotation makes the source's pair wider, so it
  would be read back at the wrong arity) — it is compiled and re-frozen as an ordinary
  binding;
- a **`Parameterized`** cell is not retained (it has no answer to keep): the cell is
  left out and the next build compiles the binding again.

**Measurements · all temporary probes, since removed.**

- **Closure freeze** (lowlevel's own test harness): on a four-node module — an array of
  two constants plus one node no root reaches — the array root's closure filed **3
  nodes of 4**, the unreachable node was not pulled in, and both items read back (10,
  20) through the artifact's own arena; adding that node as a second root grew the
  closure to 4.
- **Ownership**: a value that reports a node through `traced` and owns one obligation
  produced a closure of **2 nodes** (the value and the node it reports — that edge
  doing its job) and an artifact carrying **1 obligation**, which ran **exactly once**
  on drop.
- **Path identity** (`--example path_probe`): over seven shapes, **13 binding/field
  positions, 0 mismatches**; a binding inside a lambda body is reached as `f/2/y`; and
  `b`'s path is `b` both before and after a statement is inserted ahead of it — the
  whole reason a step is a name. A path that no longer names anything answers `None`.
- **The cell path** (`--example cell_probe`): a two-cell program
  (`cache terrain = 5`, `cache layer = terrain + 1`) — first build **0** static nodes,
  **2** cells; second build **2** static nodes (one per clean cell, **no new artifact
  filed**), the frozen pair read back as `USize(5)`; then `invalidate("a.lichen")`
  handed back **2** artifact keys, the next build lowered **0** static nodes again, and
  each key evicted `true` once and `false` again.

## 8. Costs and failure modes

- **Path churn is the whole risk.** If an agent's edits keep moving nodes, paths keep
  changing and retention never hits. That is measurable, and §9 says what it would
  falsify.
- **A read that is not recorded is a silent stale reuse** — the same class of
  obligation as `ValueExt::traced` ("nothing checks the answer, because nothing can").
  Today's landing does not record reads at all, so it is not exposed to this; §4 is
  where it becomes the obligation. The read sites are enumerable choke points, already
  inventoried in `incremental-evaluation.md` §3 (`class_value`, the operand read,
  payload items, table entries, a `StaticNode` reference), and the differential harness
  is the oracle rather than an argument.
- **Instance explosion.** A `cache` inside a lambda instantiated per recursion step
  would create one cell per instance (§3). The retention policy must be able to evict,
  and a cell inside a cyclic instance set should be refused (or bounded) rather than
  silently multiplied.
- **Store lifetime.** Cells hold device buffers (`ResidentId`), and a value dropped by
  `drop_block` does not release (`compute-graph-jit`'s landmine list). §7.1's
  obligation list is what makes an eviction release early instead of leaking VRAM for
  the life of the context — and **eviction is the caller's call**, because a static ref
  is a raw handle into the artifact's arena (`Registry::evict`'s precondition).
- **The registry grows between evictions.** Every recomputed cell files a new artifact
  under a new key; nothing evicts automatically, so a long agent session leaks device
  memory until the caller evicts. That is the one place where this design is *not* yet
  safe to leave running for hours.

## 9. What would falsify it

- **Paths churn more than they persist**: if ordinary agent edits resolve to "lost"
  often enough that the retained cone is small, the mechanism buys little and the
  honest conclusion is that the graph's cost is not in the marked cells.
- **The marked granularity is wrong**: if the expensive work is *inside* one marked
  cell rather than *across* cells, freezing the cell's boundary saves nothing, and the
  retention unit needs to be finer (or the work needs its own incremental operator).
- **A missed read shows up as a value divergence** in the harness (once §4 records
  reads). Then the read sites must be widened before anything else.

## 10. Decisions taken, and alternatives rejected

- **No keys** (the superior) — so identity is allocated (§1) and change detection is
  "recompute and compare" (§4.3) rather than "hash and look up".
- **Identity = occurrence path, name-preferred** (the superior) — chosen over a
  user-written name only (no identity for anonymous nodes, §2.4) and over pure child
  indices (an insertion invalidates the whole suffix, §2.2).
- **Resolution is dynamic** (the superior): no `Path → NodeId` registration; a path is
  resolved on demand (§2.3).
- **The tree is the AST, not the IR** (decided while landing step 1): the IR is a graph
  with no binding names, and the AST is the tree the resolver already walks by name. A
  path therefore names a position, which is what a cell is (§2.3). Landing it there
  also kept the front end's compilation and the checker's allocation path untouched,
  which a name-carrying IR would not have.
- **`cache` is a keyword, not an attribute** (the superior): a slot is a runtime value;
  the mark is static and identity-selecting (§5.1).
- **`cache` is allowed in any scope** (the superior): an instance reaches a
  free-variable-free scope in which a name/path is again stable (§3).
- **Per-cell freeze, not whole-module** (the superior): only a recomputed cell is
  frozen, so an edit pays for what it dirtied. It forced one new entry point (the
  closure freeze) rather than a smaller `from_module`.
- **The container stays a dense `Vec`, not a hash table** (decided while scoping the
  closure freeze): a hash table would buy the one mechanical renumbering pass and cost
  the dense `LocalNodeId` that `StaticNodeId`, the artifact codec, the
  content-addressed `artifact_hash` and `regroup_clones`' ordering all rest on, plus a
  hash lookup on every static read.  The renumbering was never the hard part — the
  closure is.
- **The ownership hook is per leaf on `ValueExt`, not on `Program`** (the superior's
  "most general method", and a correction of this note's first landing): the issuer is
  reachable process-wide, so a leaf needs no program parameter, and the composition
  macro chains leaves exactly as it does for the payload methods.  A first landing put
  it on `Program` on the argument that only the program could reach an issuer; that
  argument was wrong and the hook was moved rather than duplicated.
- **Small step first**: retained cells in a store, `Module::new()` per build unchanged.
  A live module with universal node-level tracking is the next segment of the same
  route, not a second design.
- Rejected: content-keyed reuse (the requirement), and the fine-grained `StaticModule`
  units of the earlier draft (their identity was a content key; the representation
  survives as §5.2's freeze-and-read-in-place, with the path as key).

## 11. Open questions

- **A path across an import boundary.** The step vocabulary is settled and landed
  (§2.2); what is not is how a package's file identity prefixes a path — the registry's
  file identity is path-derived (`is_lichen_file_id` / `file_id_hash(file_id: &str)`,
  `lichen-registry/src/device.rs:57,85`), which is a name, not a content hash, and must
  stay that way.
- **Who evicts, and when.** `Registry::evict` has a hard precondition (no live ref) and
  refuses to guess; a policy — evict on the next build, on a memory budget, on an
  explicit call — is not chosen yet. This is what stands between the design and a
  session that runs for hours (§8).
- **Where the reverse import closure is computed.** The cell store drops what it is
  told; the graph of who imports whom is the package store's.
- **The graph-side descriptor's shape**, and whether a graph node's path is expressed
  in the same step vocabulary as a source node's.
- **The event surface** (§5.2): `created`/`reused`/`recomputed`/`lost` as an API a
  caller (and the differential harness) can read.
- **Per-revision diagnostics and budget semantics** — inherited from
  `incremental-evaluation.md` §7's pending list, since recomputation is observable
  through the budgets.

## 12. Handoff: what exists, what is next, and the landmines

### 12.1 The commit trail (all on `dev`)

| commit | what |
|---|---|
| `5f78456` | this note: identity by occurrence path, retention by `cache` |
| `df32828` | the `cache` keyword: lexer, parser, `Binding.cached`, spec, tree-sitter |
| `063660c` | `RecordField.cached` — the mark survives a record block |
| `4e0d930` | `path.rs`: `Step`/`Path`, `children`, `resolve`, `for_each` |
| `6e9f7f3` | the general cache is not the JIT's graph IR; the retention plan |
| `a59d531` | per-cell freeze, and the ownership transfer it forces |
| `ccee4d0` | the ownership hook belongs on `ValueExt`; the macro does not forward `traced` |
| `8d28ed3` | `freeze_closure` / `freeze_set` (behavior-preserving refactor) |
| `f692d38` | `Release`, `StaticModule::releases` + `Drop`, the freeze fills them |
| `18c48d8` | the hook moved to `ValueExt`; the macro forwards `traced` and `release_obligations`; `ComputeValue` implements it |
| `5623f45` | `CellStore`, the lowering hook, the per-cell freeze, `compile_with_cells` |
| `be43c78` | the edit names the source; `invalidate` drops its cells |
| `b82b904` | `Registry::evict`; `invalidate` hands back the artifacts |

### 12.2 The entry points

- `lichen_language::compile_with_cells(source_id, source, cells, registry) -> Report<LangProgram>`
  — the whole cell path. `compile`, `compile_with_imports*`, `frontend*` and
  `build_report` all delegate with no cells, so nothing else changed.
- `lichen_language::cells::CellStore` — `reference`, `record`, `invalidate` (returns
  the artifact keys), `allocate_key`, `len`.
- `lichen_lowlevel::Registry` — `freeze_closure_mapped(module, key, roots, hash)`,
  `evict(key)`.
- `lichen_lowlevel::StaticModule` — `freeze_closure(module, key, roots)`, `releases`,
  `Drop`.
- `lichen_lowlevel::ValueExt` — `traced`, `release_obligations`;
  `lichen_lowlevel::Release`.
- `lichen_language_parser::path` — `Step`, `Path`, `children`, `root_children`,
  `resolve`, `for_each`, `Node`.

### 12.3 What to do next, in order

1. **Wire a consumer** (the largest piece, and the reason the feature is inert in
   production). `BufferSession`/`session.rs` is the natural place: keep a `CellStore`
   beside the session, call `compile_with_cells` with the buffer's identity as
   `source_id`, and invalidate that source when the edit names it. The session's
   current reuse gate — the whole-`Build` `content_key` (`session.rs:243`) — is what
   this replaces; `content_key`/`artifact_hash` stay for transport and diagnostics.
   Note `P2-1`: `BufferSession` has no production consumer today, so check who
   actually runs it before investing (the LSP compiles through `analysis.rs`, which
   passes no cells yet).
2. **Decide who evicts and when** (§11), then do it where the session replaces its
   previous build: at that moment the old build's static refs are gone, which is
   exactly `Registry::evict`'s precondition.
3. **The reverse import closure**: drop the cells of every file that imports the
   changed one, transitively. The graph is the package store's
   (`ResolvedImport`, `package.rs`).
4. **§4** (read recording + backdating) — the finer cut, and the only step that needs
   the read sites widened.
5. **Step 4** (the PCG graph's own node paths and edit descriptor) — a consumer, not a
   change to `lichen-graph-ir`.

### 12.4 Landmines, each of which is a silent wrong answer or a leak

- **Evicting a live artifact dangles.** A static ref is a raw handle into the
  artifact's arena: `Module::static_module` panics on an unregistered key, and a
  payload already read through it dangles. Evict only when every module that could
  still hold a ref is gone.
- **An annotation on a marked binding is skipped, not mis-read.** A static read
  materializes a two-wide pair; an annotation makes the source's pair wider. The
  eligibility check is in `cached_bindings` (`compile.rs`).
- **A `Parameterized` cell is silently not retained** — by design, but it means a
  marked binding that never solves is recompiled every build.
- **The closure's four edges plus `traced`.** Drop any of them and the freeze panics
  (the good case) or the artifact references a node it does not contain (the bad one —
  phase 3 rewrites handles, never the bytes behind them).
- **`traced` is only as good as its implementors.** A production value that holds
  nodes must implement it, and the composition macro must forward it (it now does —
  this was the landmine the graph work would have hit).
- **The registry grows between evictions** (§8): a session that never evicts leaks.

### 12.5 How to verify

```
cargo check --workspace --all-targets
cargo test -p lichen-lowlevel -p lichen-highlevel -p lichen-language -p lichen-language-server -p lichen-compute
```

The influenced set is those five crates. The temporary probes are gone; to re-take a
reading, write one as an `examples/` binary and delete it after — the numbers to
expect are in §7.1 (0/2/2 static nodes and `USize(5)`; 13 positions, 0 mismatches;
3-of-4 nodes; 1 obligation released once).
