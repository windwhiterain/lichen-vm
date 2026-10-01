# Incremental update: identity by path, retention by `cache`

> Status: **the mechanism is complete and measured (§7.1, §7.2); the consumer is
> not.** Path identity, the `cache` syntax, the per-cell closure freeze, the general
> release obligations, the cell store and **dirty propagation** all exist, and
> `BufferSession` now lowers through the store — an edit reuses every marked binding
> it did not reach. What is still missing is a **production caller**: `BufferSession`
> itself has none (`P2-1`), and the LSP's analysis path passes no cells. Eviction is
> deliberately unwired (the session hands the unreachable keys on). **§12 is the
> handoff**: what exists, in what order to continue, the landmines, and how to
> verify. Read that first if you are picking this up cold.
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
> `crates/lichen-language/src/{cells,compile,dirty,lib,session,run}.rs`,
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

A name is only a position when it is **unique in its own list**: two entries named
alike (a shadowing top-level `a = 1; a = 2`, a duplicated `.a` field) would otherwise
share one path, and a path is an identity — a cell keyed by it could be read back at
the wrong entry. So a **repeated** name falls back to its index, exactly as an unnamed
entry does, and `resolve` matches by step as it always did. This is the only case where
a name is not a name; it is measured (`path_probe`, §7.1).

Names are preferred because of the edit that dominates an agent's loop: **an
insertion before an existing statement**. Under a pure child-index path, that
insertion shifts the path of everything after it in the block, so every retained
cell in that block is lost. Under a name step, nothing after it moves. The residual
invalidations are honest and rare:

| edit | effect on a path | retention |
|---|---|---|
| append a statement | none for the statements before it | kept |
| insert *before* `terrain` | `terrain`'s own step is a name | kept |
| change `terrain`'s body | `terrain`'s path unchanged; its value may change | re-derived, then cut by §4 |
| rename `terrain` | the step changes | **lost** — a different name is a different cell |
| move a statement into a block | steps change | **lost** |
| edit an anonymous literal's children | index steps after it shift | lost below that point |
| add a second binding named `terrain` | both steps become indices | **lost** — neither is a name any more |

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

The boundary is measured, and it is the *lambda* that is not built, not the nesting: a
mark in a **block** is solved (it has no free variable) and is retained at its nested
path, exactly like a top-level one — `cache a = { cache i = 1 + 2; i + 4 }` retains two
cells and reads both back on an edit outside them (§7.2). A mark in a **lambda** body is
a template, so its pair is not solved and nothing is retained (§7.2's landmine list).

## 4. Invalidation: dirty propagation over the resolver's own graph

> **Landed** (`crates/lichen-language/src/dirty.rs`, §7.2). This section is the design
> as built; §4.3 records what the earlier draft of this note proposed and why it was
> dropped.

An edit does not invalidate a *file*, it invalidates **positions**. The propagation is
push, from the edit outward along read edges, and it needs no recorded read set — the
AST already *is* the read graph, exactly: the resolver wrote a `BinderId` into every
`Name` use, so "what does this binding read" is a walk, not a log.

### 4.1 The graph's node is the top-level statement

A statement **declares** the binders its subtree binds and **reads** the binders its
`Name` uses resolve to. A nested binder (a lambda parameter, a block local) is visible
only inside its own statement, so it can never carry an edge *between* two statements —
which is why the statement is the right node, and why a cell anywhere inside a statement
(a mark in a nested block included) is dirtied exactly when its statement is.

The **seed** is the window the splice already computed: the statements the edit
re-parsed. Nothing else has to be detected — the incremental parser knows the dirty
region because it had to re-parse it.

The **fixpoint**: a statement is dirty if it was re-parsed, or if it reads a binder a
dirty statement declares. That is the whole rule.

### 4.2 Both programs are propagated over

An edit can move a resolution, and the two programs disagree about what a name reads, so
one propagation is not enough:

- the **previous** program catches a read that **disappeared** — a name the edit deleted
  or moved out of scope. The current program no longer records that read, so only the old
  graph has the edge.
- the **current** program catches a read that **appeared** — a name the edit brought into
  scope, so the binding now reads something it did not. Only the new graph has the edge.

Either alone is unsound, and neither is a superset of the other (measured: §7.2's
"a read disappears" and "propagate a → b" cases). A full re-parse has no window, and
then the whole program is the dirty region — the honest answer.

### 4.3 What the earlier draft proposed, and why it was dropped

The draft recorded, per cell, **the paths it read** plus a version stamp, and *verified*
a demanded cell (pull) before recomputing it: resolve each recorded path, ask when it
last changed, reuse the cell if nothing did. That is Salsa's red-green step with paths
in place of query keys, and it was called "verification (pull), not propagation (push)".

It was dropped for three reasons:

- **it contradicts the requirement** — the superior asked for a dirty propagation
  mechanism, and pull verification is the other thing;
- **recording is redundant**: the resolver's `BinderId` annotations *are* the read set,
  complete and exact, and they are re-derived from the AST every compile. A recorded copy
  is a second source of truth that can go stale, for nothing;
- **a stamp needs a clock**, and with no keys there is no cheap "when did this last
  change" — the answer would be the recomputation the stamp was meant to avoid.

The dependency relation the draft stored as paths is still there, and still path-shaped:
a cell's identity is a path, and what dirties it is the *statement* it sits in.

### 4.4 Backdating — not built

After a recomputation the new value could be compared with the retained one, and equal
means the cell keeps its identity *and* its consumers stay clean. That is the one part of
the draft worth keeping, and it is a refinement, not a correction: propagation dirties
more than necessary and never less, so its absence costs work, never correctness. It
would pay where an edit changes a marked binding's *bytes* without changing its *value*
(`cache a = 1 + 2` → `1 + 1 + 2`), which the byte-position seed cannot see through.

### 4.5 Cycles

A marked cell that participates in a value cycle is dirty, recomputed and re-frozen as
one **SCC-atomic** unit; the in-progress assumption the within-build note names (`P1-31`)
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

Durability and the equality policy for backdating (§4.4) are deliberately **not**
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
- On a later build with the same path, the cell is reused or (when dirty propagation
  reached it) recomputed. **Landed**, at path granularity (§4).
- Every one of the events — `created`, `reused`, `recomputed (because …)`, `lost
  (path changed / unresolved)` — is meant to be **reportable**: the PCG-side analogue
  of a cook graph's "why did this re-cook", and the differential harness's oracle.
  **Partly landed**: `SessionReport::cells` (`CellEvents { reused, frozen, dropped }`)
  reports the counts, which is what §7.2's readings are taken from. What is still
  missing is the *reason* per event (which edit reached this cell, which read carried
  the dirt) — the counts say "two dropped", not "these two, because `b` reads `a`".

## 6. The two edit surfaces

Decided by the superior:

- **Graph edits use a descriptor.** Here "graph" is the *program's own* graph data —
  the structure an agent builds and edits (a PCG graph), **not** the JIT's
  `lichen-graph-ir` (§1.1). It is edited *as data* — add / remove / rewire /
  re-parameterize a node — so the edit names the node paths it touched and the
  invalidation is exact. No diffing is involved.
- **Source edits use file granularity.** A source edit dirties the file; the file's
  marked cells are re-derived, and §4's propagation is what keeps the rest of the
  program, and the graph, from re-running. Sub-file identity is *not* needed for
  this, which is why no content key is needed either. (Landed finer than this, and the
  decision is unchanged: the *caller* still names a file, and what the session does
  inside it — propagate from the re-parsed window — is strictly more precise than
  dropping the file's cells. `CellStore::invalidate_source` remains the coarse cut for
  a file the session does not hold, §7.1.)

## 7. What changes where

| step | file / item | state |
|---|---|---|
| 1 | `lichen-language-parser/src/path.rs` | **landed**: `Step`/`Path`, the position vocabulary (`children`), dynamic `resolve`, the descent (`for_each`), and a repeated name falling back to its index so a path is a unique identity |
| 1 | `lichen-language/src/dirty.rs` | **landed**: dirty propagation over the resolver's `BinderId` graph, both index spaces (§4) |
| 2 | `lichen-language-lex` / `-parser` / `ast.rs` | **landed**: the `cache` keyword, `Binding.cached`, `RecordField.cached` |
| 2 | `language-spec.md` §2 + `tree-sitter-lichen` (`grammar.js`, `highlights.scm`) | **landed**: the statement form, the keyword list, the highlighting |
| 3 | `lichen-lowlevel/src/static_module/freeze.rs` | **landed**: `freeze_closure` (the closure of a root set) and `freeze_set` (the shared phases); `from_module_mapped` is now "the closure of every node" |
| 3 | `lichen-lowlevel/src/lib.rs` (`Release`, `ValueExt::release_obligations`, `StaticModule::releases` + `Drop`) | **landed**: the general ownership transfer — an artifact owns its out-of-arena resources and releases them when it is dropped |
| 3 | `lichen-compute/src/compute.rs`, `lichen-language/src/program.rs` | **landed**: the compute leaf's obligation (a `DeviceBuffer` value) and the composition macro's forwards (`traced`, `release_obligations`) |
| 3 | `lichen-language/src/{cells,compile,lib}.rs` | **landed**: `CellStore`, the lowering hook (a clean cell lowers to `ExprKind::Static`), the per-cell freeze after the build, `compile_with_cells` |
| 3 | `lichen-language/src/session.rs` | **landed**: the session holds the store and the registry, reconciles them per rebuild, lowers through the cells, and reports what it did (`CellEvents`) |
| 3 | eviction | **deliberately unwired**: `CellStore::invalidate_*` hands back the artifacts that became unreachable and the session accumulates them (`take_unreachable`); `Registry::evict` frees one. Who evicts and when is §11's question, and the reason is §12.4's first landmine |
| 3 | a **production** consumer (the LSP, the package store) | **not built**: `BufferSession` has no production caller (`P2-1`), so the retention is exercised only by probes; `content_key`/`artifact_hash` stay for transport and diagnostics |
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
  **placeholder** on this path — a cell's reuse is decided by its path and by
  propagation, never by content (§1, §4) — which is the same demotion §7's table
  records for `content_key`.
- The **dirty input** has two cuts. The coarse one is the caller's, and it is the
  granularity chosen for *another* file: `CellStore::invalidate_source(source_id)` drops
  that source's cells and hands back the artifacts they named. The fine one is the
  session's, for an edit to the file it holds: `CellStore::invalidate_paths` drops the
  positions dirty propagation reached (§4) and `CellStore::retain_marked` drops the
  positions the program no longer marks (a removed `cache`, a rename). A cell is reused
  iff neither reached it. Both are sound **without recording a single read** — the AST
  is the read graph — which is why they come before anything that records.
- **The cell's mark is part of the content key** (`KEY_FORMAT_VERSION` 2). The key is
  documented as "the structure the lowering consumes", and the lowering *does* consume
  `cached`: it is what makes a binding the one a clean cell may replace. A key that
  ignored it would describe a lowering that is not the one that runs — and adding
  `cache` to a binding would silently do nothing until the next structural edit.

**Two limitations, recorded rather than hidden:**

- a marked binding whose value spells an **annotation** is not eligible (a static read
  materializes a two-wide pair, and an annotation makes the source's pair wider, so it
  would be read back at the wrong arity) — it is compiled and re-frozen as an ordinary
  binding;
- a **`Parameterized`** cell is not retained (it has no answer to keep): the cell is
  left out and the next build compiles the binding again. A **failed build** retains
  nothing at all, for a sharper reason — see §7.2.

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
  Re-taken with the repeated-name rule: five shapes (a shadowing top-level pair, a
  duplicate block pair, a duplicate struct-instantiation field pair, a nested lambda
  block) — **every position unique and every path round-tripping, 0 mismatches**; the
  shadowing pair reads `0`, `1`, and a duplicated `.a` field reads `x/1`, `x/2`.
- **The cell path** (`--example cell_probe`): a two-cell program
  (`cache terrain = 5`, `cache layer = terrain + 1`) — first build **0** static nodes,
  **2** cells; second build **2** static nodes (one per clean cell, **no new artifact
  filed**), the frozen pair read back as `USize(5)`; then `invalidate_source("a.lichen")`
  handed back **2** artifact keys, the next build lowered **0** static nodes again, and
  each key evicted `true` once and `false` again. (The reading predates the rename of
  `invalidate` to `invalidate_source`; it is the same call.)

### 7.2 The session landing (step 1 + the fine cut) — what exists

`BufferSession` is the first consumer. It holds the store, the registry the artifacts
live in, and the caller's `source_id`; `compile` lowers through
`compile_resolved_with_cells` and reconciles the store against the program about to be
lowered, so the order is: reconcile (drop dirty and unmarked) → lower (a clean cell
becomes a static read) → check → freeze what was compiled. Its `SessionReport` carries
`CellEvents { reused, frozen, dropped }` — without it the mechanism is silent, and a
caller cannot tell a rebuild that reused nine cells from one that reused none.

Two properties of the shape are worth naming, because both were decisions:

- the **coarse gate runs first**: if the resolved content key is unchanged the whole
  `Build` is reused and the store is not touched at all — no cell is consulted when no
  lowering happens, and the build being reused *is* the one that was correct for this
  content. The cells are the finer cut *under* that gate, for the edits it rejects.
- the window dirty propagation is seeded from is the **splice's own**, in both index
  spaces (§4.2). It cost one field (`SpliceOut::old_hi`) and no extra detection.

**Measurements · a temporary probe (`--example session_probe`), since removed.**
`base = cache a = 1 + 2 / cache b = 3 + 4 / c = a + b / c` unless noted; every row is the
*second* compile, after replacing the whole source. `reused`/`frozen`/`dropped` are the
`CellEvents`; the value is the session's own root value, read through the build.

| edit | build reused | reused | frozen | dropped | value |
|---|---|---|---|---|---|
| a leaf inside `b` (`3 + 4` → `30 + 4`) | no | 1 | 1 | 1 | `USize(37)` |
| a leaf inside `a` (`1 + 2` → `10 + 2`) | no | 1 | 1 | 1 | `USize(19)` |
| the unmarked `c` (`a + b` → `a + b + 1`) | no | 2 | 0 | 0 | `USize(11)` |
| `a`'s `cache` mark removed | no | 1 | 0 | 1 | `USize(10)` (1 cell) |
| a rename of `a` (and of `b`'s read) | **yes** | 0 | 0 | 0 | the build was reused |

The propagation cases, on `chain = cache a = 1 + 2 / cache b = a + 4 / b` (the third row
is a *further* edit on the second row's program, which is the point of it):

| edit | reused | frozen | dropped | value | what it proves |
|---|---|---|---|---|---|
| `a`'s leaf (`1 + 2` → `10 + 2`) | 0 | 2 | 2 | `USize(16)` | `b` reads `a`, so `a`'s dirtiness reaches it — a stale `b` would answer 7 |
| `a` renamed to `z`, `b` still reads `a` | 0 | 2 | 2 | `Parameterized`, `ok=false` | the read *disappeared*: only the **old** graph has that edge |
| then `b` → `b + 1` | 2 | 0 | 0 | `ok=false` | the resolve error is still reported with both cells reused — a retained body never swallows a frontend diagnostic |

And the boundaries:

- a **nested mark** (`cache a = { cache i = 1 + 2; i + 4 }`, `c = a + 1`): 2 cells
  (the top-level `a` and the nested `i`), an edit to `c` reuses **both** (2/0/0), value
  `USize(9)` — so nesting is not the limit, the lambda is (§3).
- a **failed check** (`cache b = a + "x"`): the first build is `ok=false` and retains
  **0** cells; the second (an edit to `c`) still fails and still retains 0. Without the
  guard the cell would have been frozen from the failed build and read back, and the
  program would have started reporting `ok` — the failure would be reported once and
  then silently disappear.
- **pruning**: after a rename (which the key gate reuses wholesale) a second edit forces
  a rebuild, and the store is reconciled against the *new* program's marks — `dropped`
  is 2, not 1, so the path the rename left behind does not accumulate.

## 8. Costs and failure modes

- **Path churn is the whole risk.** If an agent's edits keep moving nodes, paths keep
  changing and retention never hits. That is measurable, and §9 says what it would
  falsify.
- **A missed edge is a silent stale reuse.** The read set is no longer the risk it was
  in the draft — the resolver's `BinderId` annotations are complete and exact, and they
  are re-derived every compile — so the obligation moved to the *walk* that collects
  them: `dirty.rs`'s `walk` is exhaustive over `Expr` on purpose, so a new expression
  form is a compile error rather than a silent missing edge. The other edge is the
  graph's node: a binder that could be read from a *different* statement would need a
  finer node than the statement. Today it cannot (a nested binder is visible only inside
  its own statement), and that is the invariant to re-check if scoping ever changes.
  §4.5's cycles and §9's differential harness are the oracle, not an argument.
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
  memory until the caller evicts. The session accumulates the keys of every cell it
  drops and hands them on (`take_unreachable`), so nothing is *lost* — but a caller that
  never takes them and never evicts grows the registry without bound. That is the one
  place where this design is *not* yet safe to leave running for hours.

## 9. What would falsify it

- **Paths churn more than they persist**: if ordinary agent edits resolve to "lost"
  often enough that the retained cone is small, the mechanism buys little and the
  honest conclusion is that the graph's cost is not in the marked cells.
- **The marked granularity is wrong**: if the expensive work is *inside* one marked
  cell rather than *across* cells, freezing the cell's boundary saves nothing, and the
  retention unit needs to be finer (or the work needs its own incremental operator).
- **A missed edge shows up as a value divergence** in the differential harness: the
  session's value after an edit against a fresh compile of the same source. That is the
  cheap oracle for the whole mechanism, and it is what §7.2's probe does by hand.

## 10. Decisions taken, and alternatives rejected

- **No keys** (the superior) — so identity is allocated (§1) and change detection is
  dirty propagation over the resolver's own read graph (§4) rather than "hash and look
  up".
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
- **Propagation, not recorded reads** (landed, and a correction of this note's draft):
  the read set is the resolver's `BinderId` annotations, walked on demand, so nothing is
  recorded and nothing can go stale. §4.3 has the rejected pull-verification design and
  the three reasons.
- **Both programs are propagated over** (landed): the previous one catches a read that
  disappeared, the current one a read that appeared. Each alone is unsound, and neither
  is a superset of the other — the one place where "just dirty what the edit touched"
  is not enough (§4.2).
- **A failed build freezes no cell** (landed): a cell is read back by skipping its body,
  so one frozen from a failed *check* would carry that failure's silence — the error
  reported once, then gone. Conservative on purpose: one broken binding costs the whole
  program its reuse rather than risking a vanished diagnostic. The finer rule
  (per-binding cleanliness, which needs the checker's error attribution) is available
  and not taken.
- **The `cache` mark is in the content key** (landed, `KEY_FORMAT_VERSION` 2): the key
  describes what the lowering consumes, and the lowering consumes the mark.
- **A repeated name falls back to its index** (landed): a path is an identity, so two
  entries in one list must never share a step. Name-preferred survives; only the
  ambiguity case changes (§2.2).
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
  explicit call — is not chosen yet. The session now *hands the keys on*
  (`take_unreachable`), so the question is only the timing, and it is the caller's
  because only the caller knows when the last `SessionReport` clone died. This is what
  stands between the design and a session that runs for hours (§8).
- **Where the reverse import closure is computed.** The cell store drops what it is
  told; the graph of who imports whom is the package store's. Dirty propagation today
  covers one file's own statements — an *imported* file's edit needs the coarse cut
  (§7.1) plus that closure.
- **The graph-side descriptor's shape**, and whether a graph node's path is expressed
  in the same step vocabulary as a source node's.
- **Backdating** (§4.4): comparing a recomputed value with the retained one so a
  byte-level change that does not change the value stops dirtying consumers.
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
| `be43c78` | the edit names the source; the coarse cut drops its cells |
| `b82b904` | `Registry::evict`; the cut hands back the artifacts |
| `66043e9` | the note becomes a handoff; the code stops saying `cache` is inert |
| `389e727` | a repeated name falls back to its index, so a path is an identity |
| `b42dac2` | the session retains cells and dirties them by propagation (`dirty.rs`, the `cache` key, the failed-build guard) |

### 12.2 The entry points

- `lichen_language::session::BufferSession` — the consumer. `with_source_id(source,
  source_id)`, `compile() -> SessionReport` (carrying `CellEvents`), `retained_cells()`,
  `take_unreachable()`. `new(source)` is the unnamed-buffer shorthand.
- `lichen_language::compile_with_cells(source_id, source, cells, registry) -> Report<LangProgram>`
  — the whole cell path, without a session. `compile`, `compile_with_imports*`,
  `frontend*` and `build_report` all delegate with no cells, so nothing else changed.
- `lichen_language::cells::CellStore` — `reference`, `record`, `invalidate_source`,
  `invalidate_paths`, `retain_marked`, `allocate_key`, `len`.
- `lichen_language::dirty` (crate-private) — `dirty_marked_paths(previous, current,
  previous_window, current_window)`; the statement graph, the fixpoint and the
  exhaustive `walk` are inside.
- `lichen_lowlevel::Registry` — `freeze_closure_mapped(module, key, roots, hash)`,
  `evict(key)`.
- `lichen_lowlevel::StaticModule` — `freeze_closure(module, key, roots)`, `releases`,
  `Drop`.
- `lichen_lowlevel::ValueExt` — `traced`, `release_obligations`;
  `lichen_lowlevel::Release`.
- `lichen_language_parser::path` — `Step`, `Path`, `children`, `root_children`,
  `resolve`, `for_each`, `Node`.

### 12.3 What to do next, in order

1. **Run the session somewhere real** (`P2-1`). The session is cell-aware now, so this
   is no longer "wire the mechanism" but "give it a caller": `BufferSession` has none,
   and the LSP's analysis path (`analysis.rs:323`) compiles one-shot through
   `frontend_at`, passing no cells. Whoever takes this must decide *what a source edit
   is* in that caller — the session's dirty propagation needs the edit as a *source
   replacement* (it diffs `LastState::source`), so an incremental caller feeds it whole
   buffers, not ranges.
2. **Decide the eviction timing** (§11) and call `Registry::evict` on what
   `take_unreachable` hands back — but read §12.4's first two landmines first: "the
   reports are gone" is necessary and *not* sufficient (a surviving cell's artifact may
   reference a dropped one).
3. **The reverse import closure**: drop the cells of every file that imports the changed
   one, transitively (`CellStore::invalidate_source` plus the package store's
   `ResolvedImport` graph, `package.rs`).
4. **Backdating** (§4.4) — the refinement that stops a value-preserving byte edit from
   dirtying consumers.
5. **Step 4** (the PCG graph's own node paths and edit descriptor) — a consumer, not a
   change to `lichen-graph-ir`.

### 12.4 Landmines, each of which is a silent wrong answer or a leak

- **Evicting a live artifact dangles.** A static ref is a raw handle into the
  artifact's arena: `Module::static_module` panics on an unregistered key, and a
  payload already read through it dangles. Evict only when every module that could
  still hold a ref is gone.
- **"The reports are gone" is not enough to evict.** A surviving cell's artifact can
  reference a *dropped* one — a marked binding that reads another marked binding freezes
  a static ref into it, filed verbatim (`freeze.rs`'s closure deliberately does not pull
  a static payload in). So eviction also needs "no live registered artifact references
  this key", and there is no such check yet: `referenced_keys` takes a `Module`, not a
  `StaticModule`, so it cannot be run over the registry as it stands.
- **A cell is read back by *skipping the body*.** Its lowering, its check and its
  evaluation all do not happen, so a cell frozen from a build that failed *carries that
  failure's silence*: the error is reported once and then disappears. The guard is
  `freeze_cells`' `!build.ok` early return, and it is load-bearing — measured (§7.2's
  "a failed check": 0 cells retained, `ok=false` on both builds). Frontend diagnostics
  are *not* at risk (lex/parse/resolve re-derive from the AST every compile, measured),
  which is why the guard tests `build.ok` rather than the report.
- **Dropping either propagation is unsound, not merely imprecise.** The old program's
  graph is the only one that has a read the edit *deleted*; the new one's is the only one
  that has a read the edit *created*. Both cases are measured (§7.2).
- **The window must be reported in both index spaces.** Dirty propagation is seeded by
  the statements the splice re-parsed, and the old program's window is not the new
  program's whenever the edit added or removed statements — hence `SpliceOut::old_hi`. A
  window past the end of a program is silently empty, so an off-by-one there is a silent
  missed dirty.
- **A full re-parse dirties everything.** That is correct and expensive: the fallback
  path (`splice_program` → `None`) drops every cell. It is also what makes the fallback
  *safe* — do not "optimize" it into a narrower window without the two-space argument.
- **The `dirty.rs` walk must stay exhaustive over `Expr`.** It is written as a full match
  on purpose: a new expression form that falls into a catch-all would contribute no
  edges, and a missed edge is a stale cell.
- **`retain_marked` is what keeps the store honest.** A cell outliving its mark would be
  read for a binding that no longer asks to be retained, and would never be compiled
  again. It is also what prunes the paths a rename leaves behind.
- **The content key carries `cache`** (`KEY_FORMAT_VERSION` 2). Bumping the constant
  invalidates every key; today that is in-memory only (the session and the tests), so a
  bump is free — check that before the keys are ever persisted.
- **An annotation on a marked binding is skipped, not mis-read.** A static read
  materializes a two-wide pair; an annotation makes the source's pair wider. The
  eligibility check is in `cached_bindings` (`compile.rs`).
- **A `Parameterized` cell is silently not retained** — by design, but it means a
  marked binding that never solves is recompiled every build. This is the whole of a
  `cache` inside a **lambda**: the mark is honored for a binding the compiler walks once
  (a program or block statement, a record field), and a lambda body's mark is a template
  whose pair is unsolved (§3).
- **The closure's four edges plus `traced`.** Drop any of them and the freeze panics
  (the good case) or the artifact references a node it does not contain (the bad one —
  phase 3 rewrites handles, never the bytes behind them).
- **`traced` is only as good as its implementors.** A production value that holds
  nodes must implement it, and the composition macro must forward it (it now does —
  this was the landmine the graph work would have hit).
- **The registry grows between evictions** (§8): a session that never evicts leaks, and
  one that never calls `take_unreachable` leaks the list too.

### 12.5 How to verify

```
cargo check --workspace --all-targets
cargo test -p lichen-lowlevel -p lichen-highlevel -p lichen-language -p lichen-language-server -p lichen-compute
```

The influenced set is those five crates. The temporary probes are gone; to re-take a
reading, write one as an `examples/` binary and delete it after. The numbers to expect:
§7.1 (0/2/2 static nodes and `USize(5)`; 13 positions, 0 mismatches; every path unique;
3-of-4 nodes; 1 obligation released once) and §7.2 (the `CellEvents` per edit, the two
propagation cases, `USize(37)`/`USize(19)`/`USize(16)`, and 0 cells from a failed check).

A session probe reads the value by **consuming** the session: a `Build`'s module is
evaluated through a `&mut`, and the report's `Arc` is only unique once the session's own
clone is gone. On the key-reused path it is still shared, and the probe prints
`<still shared>` — that is the probe's limit, not a bug.
