# Incremental update: identity by path, retention by `cache`

> Status: current — the mechanism (path identity, the `cache` mark, per-cell freeze,
> dirty propagation, eviction, the cell store) is complete, and the language server
> drives one session per open document, so an edit reuses every marked binding it did
> not reach. Still open: the keystroke path through the server, the reverse import
> closure, the eviction timing, backdating, and the graph-side edit descriptor — all
> listed in [Open items](#open-items).
>
> This is the *cross-build* half of the incrementality question. Its companion
> [incremental-evaluation](incremental-evaluation.md) keeps the *within-build* settled
> cut, the deep-pass measurement and the mutation inventory; where the two meet, this
> note links there instead of restating.
>
> Points at: `crates/lichen-language-parser/src/path.rs`,
> `crates/lichen-language/src/{cells,compile,dirty,lib,session,spans}.rs`,
> `crates/lichen-language-lex/src/lib.rs` (`lex_resume`),
> `crates/lichen-lowlevel/src/{lib,registry}.rs` + `static_module/{freeze,apply}.rs`,
> `crates/lichen-compute/src/compute.rs`, `crates/lichen-language/src/program.rs` (the
> composition macro), `crates/lichen-language-server/src/{server,analysis}.rs` (the
> caller), and the notes [incremental-parse-compile](incremental-parse-compile.md),
> [artifact-cache](artifact-cache.md), [static-modules](static-modules.md),
> [attributes](attributes.md) and [liche-lsp-home](liche-lsp-home.md).

## What it is and why

A lichen program is compiled from source; the expensive part of an interactive tool —
an editor, or an agent's edit loop — is that a build today starts from
`Module::new()` and re-derives everything. Incremental update makes a *rebuild* cheap:
the caller re-supplies the source and everything the edit did not reach is reused.

Two things are being made cheap at once, and they need different identities:

- **identity** — which unit of the old build a unit of the new build is. Here it is a
  node's **occurrence path** from the program root, so an edit that inserts a statement
  before a retained one does not renumber it.
- **retention** — which units are worth keeping at all. That is the user's choice,
  spelled with the `cache` mark, so memory is bounded by what the program asked for.

The **requirement is external mutation, under two constraints**: an agent edits the
generator program *and* its graph data between runs, so the consequences of an edit must
be re-derived incrementally; and **no keys** may decide reuse — no content hash, no
interning, no identity derived from a value. Invalidation is therefore **dirty
propagation** over the program's own read graph.

The target consumer is an agentic PCG graph: the graph is long-lived, its expensive
outputs are generated content, and the agent's edit loop is what has to get cheap. The
one consumer designed so far is [whiting-scene-document](whiting-scene-document.md),
where a `cache`d generator is the thing a scene document keeps not re-deriving.

### What this is *not*: the JIT's graph IR

`crates/lichen-graph-ir` is the **compute** layer's graph: the shape of a lichen program
that has been *evaluated once and whose evaluation was recorded* — kernel and native
nodes, and the submission policy that decides when the host observes completion. It is
JIT-specific and shares no identity and no node set with this design.

A `cache` cell is a **language-level retention point**: a marked position in the source,
identified by its occurrence path, whose value survives a build. A graph node is
something that *ran*; a cell is something the user *marked*. Where they meet is
residency only: a cached cell's value may hold a device buffer the compute layer
produced, which is a release-on-eviction problem, not an identity one.

## 1. The principle: identity is allocated, not derived

Every earlier attempt to make this layer incremental looked for an identity the
*compiler* could compute: a content key, a hash, an interned id. All of them are keys,
and all of them are rejected. The reason is structural, not stylistic:

- `NodeId` is a versioned slot key allocated by the checker's walk — editing an IR
  prefix shifts every later allocation (`incremental-evaluation.md` §2).
- `BlockId` is one per lambda (`checker/lambda.rs`), not a unit boundary.
- `ExprId` is a counter over the IR; it shifts under an insertion.
- A content key is the thing the requirement forbids.

So the identity has to be **allocated by something that outlives the edit**, and there
are exactly two such allocators: the *structure* (a node's position in the tree) and the
*user* (a mark the user wrote). This design uses both, for different jobs:

- **identity** comes from the structure — a node's **occurrence path** from the program
  root (§2);
- **retention** comes from the user — the **`cache`** mark, which selects *which* paths
  are kept across a build (§3).

The two are orthogonal: every node has a path, so dirty propagation is universal; only
marked paths hold memory, so retention is bounded by what the user asked for.

## 2. Identity: the occurrence path

### 2.1 The definition

A node's identity is the sequence of steps that reaches it from the program root — an
occurrence path, in the sense of Dewey-order/path-based node identification
([Dagstuhl survey](https://drops.dagstuhl.de/storage/16dagstuhl-seminar-proceedings/dsp-vol05061/DagSemProc.05061.6/DagSemProc.05061.6.pdf)),
and the same shape as rust-analyzer's `SyntaxNodePtr`, whose own module doc gives the
argument this design rests on: syntax trees are **transient objects**, so holding a node
is ill-advised; instead store the node's **location**, a small value that is **resolved
back into the tree when needed**
([`syntax::ptr`](https://rust-lang.github.io/rust-analyzer/syntax/ptr/index.html)).

The `Module` stays transient (a rebuild still starts from `Module::new()`), and the
retained state is keyed by a path, which is small, storable and resolvable.

### 2.2 Steps: a name where the syntax has one, an index only where it does not

A step is a **name** wherever the position sits in a *list the syntax can name* — a
binding statement, a named struct field, a named instantiation argument — and a **child
index** everywhere else: an element of a tuple, array or table literal, an argument of a
`$op(…)` form, or a fixed-arity role. Fixed-arity positions use **reserved role slots**,
never compacted ones: a lambda's `parameter_type` is always `Index(0)` and its `return`
always `Index(2)`, whether or not the optional annotations are present, so adding an
annotation does not renumber a stored path.

The vocabulary is the contract, and it lives in exactly one place —
`crates/lichen-language-parser/src/path.rs`, whose `children` is the only enumeration of
a node's positions. Changing it renumbers every stored path, so it is a compatibility
contract exactly as the attribute order is.

A name is only a position when it is **unique in its own list**: two entries named alike
(a shadowing top-level `a = 1; a = 2`, a duplicated `.a` field) would otherwise share one
path, and a path is an identity — a cell keyed by it could be read back at the wrong
entry. So a **repeated** name falls back to its index, exactly as an unnamed entry does,
and `resolve` matches by step as it always did. This is the only case where a name is not
a name.

Names are preferred because of the edit that dominates an agent's loop: **an insertion
before an existing statement**. Under a pure child-index path, that insertion shifts the
path of everything after it in the block, so every retained cell in that block is lost.
Under a name step, nothing after it moves. The residual invalidations are honest and
rare:

| edit | effect on a path | retention |
|---|---|---|
| append a statement | none for the statements before it | kept |
| insert *before* `terrain` | `terrain`'s own step is a name | kept |
| change `terrain`'s body | `terrain`'s path unchanged; its value may change | re-derived, then cut by §4 |
| rename `terrain` | the step changes | **lost** — a different name is a different cell |
| move a statement into a block | steps change | **lost** |
| edit an anonymous literal's children | index steps after it shift | lost below that point |
| add a second binding named `terrain` | both steps become indices | **lost** — neither is a name any more |

Every "lost" row is *safe* (it over-invalidates; it never serves a stale value), and the
event surface of §3.2 is where each one is meant to become a diagnostic.

### 2.3 Resolution is dynamic — a path is a locator, not a registry entry

**Nothing is registered when a node is built.** No `Path → NodeId` table exists, and
neither the front end's compilation nor the checker's allocation path is touched. A path
is resolved **on demand** by walking the tree one step at a time — exactly as
`SyntaxNodePtr` is resolved into a node when someone needs one.

The tree is the **AST**, not the highlevel IR: the IR is a *graph* (a binding's node *is*
its value's node, so it has several positions) and it carries no binding names, while the
AST is the tree the resolver already walks by name. A path therefore names a **position**:
a node reached by several positions has several paths, and a retained cell is a position,
not a node.

Three consequences, all wanted:

- there is no bookkeeping to keep in step with allocation, so there is no second list
  that can drift (the failure mode [attributes](attributes.md) had to delete);
- a path that **does not resolve** in the current build is a lost cell — a reportable
  event (§3.2), never a silent reuse;
- the resolution cost is paid only for marked paths, on demand, and it is
  `O(path length)` — not per node.

### 2.4 Why not the alternatives

- **Content keys / hashing** — forbidden by the requirement, and the reason is not merely
  a rule: a hash answers "is this the same?" only by comparing content, so it cannot
  distinguish "the same node, moved" from "a different node" without also hashing the
  context, which is the path again.
- **Interning** — a key with a nicer interface.
- **`NodeId` / `ExprId`** — allocated by a walk that shifts (§1).
- **A user-written name only** — it covers named bindings and leaves anonymous
  intermediates with no identity at all, which breaks the instance case (§2.5). A path
  covers every node.

### 2.5 Instances: a node inside a lambda

A binding inside a lambda body is not a per-apply accident: its path runs through the
*template*, and the apply materializes an **instance** of that template (`node_apply`'s
clone walk; one block per lambda, `checker/lambda.rs`). The instance's identity is

> **(the template's path, the paths of the arguments/captures it was instantiated with)**

— all paths, so a `cache` inside a lambda is well-defined and needs no extra mechanism.
This is the precise form of the observation that an instance "eventually reaches a
top-level scope with no free variables": the instance is grounded in *paths*, not in
values, so grounding never requires a key. An anonymous intermediate passed as an
argument is not a hole either — it has a path of its own.

**Not built.** Nothing instantiates a cell per instance today: `cache` is honored for a
binding the compiler walks once (a program or block statement, a record field), and a
binding inside a lambda body is lowered per apply like any other. The instance explosion
of §8 is the item that has to be settled before that changes.

The boundary is the *lambda*, not nesting: a mark in a **block** is solved (it has no
free variable) and is retained at its nested path, exactly like a top-level one —
`cache a = { cache i = 1 + 2; i + 4 }` retains two cells and reads both back on an edit
outside them. A mark in a **lambda** body is a template, so its pair is not solved and
nothing is retained.

## 3. Retention: the `cache` mark

### 3.1 Syntax

```lichen
cache terrain = noise(seed) |> erode(200)
cache scatter = poisson(terrain, density)
```

`cache` is a **keyword** and a **binding modifier**, in the same position as `pub` and
`let` (`pub` is `BlockStmt`-level, `let` is `Binding.restrictive`). It is **allowed in
any scope**, because §2.5 gives a node inside a lambda a well-defined identity.

It is deliberately **not an attribute**: an attribute's slot is a *runtime* value — the
annotation's own `[value, type]` term pair ([attributes](attributes.md)) — while this
mark is static and identity-selecting. A `cache` mark must be visible at lowering, before
any node exists, which is what a keyword is and what a slot is not.

Durability and the equality policy for backdating are deliberately **not** in this
syntax: the mark carries identity work only, and those two are policy that can arrive
later on the host side without renumbering anything.

The mark is **not** an error anywhere: it is accepted in every scope and combined freely
with `let`, and no diagnostic is emitted for it. A record block's field carries it too
(`RecordField.cached`), since a record block's fields *are* its statements — without that
the mark would vanish in the statement → field conversion.

### 3.2 Semantics and the event surface

- On first computation, the value is frozen into the store under the cell's path and
  later read **in place** (the `StaticModule` read path — no payload copy).
- On a later build with the same path, the cell is reused or (when dirty propagation
  reached it) recomputed, at path granularity (§4).
- Every one of the events — `created`, `reused`, `recomputed (because …)`, `lost (path
  changed / unresolved)` — is meant to be **reportable**: the PCG-side analogue of a cook
  graph's "why did this re-cook", and the differential oracle's subject.
  `SessionReport::cells` (`CellEvents { reused, frozen, dropped }`) reports the counts.
  What is still missing is the *reason* per event (which edit reached this cell, which
  read carried the dirt) — the counts say "two dropped", not "these two, because `b`
  reads `a`".

### 3.3 What a cell holds, and what it costs

A cell's value is a `[value, type]` pair; freezing turns a solved node set into a static
artifact *and* hands back the `NodeId → LocalNodeId` map, so a cell's frozen reference is
a `StaticNodeId` into that artifact. A later build **reuses** a clean cell by lowering it
to `ExprKind::Static { export }` instead of compiling its body — the node the front end
already emits for an import, materialized by the checker's own arm. The body is not
lowered, not checked and not evaluated: the reuse skips the work rather than caching its
result. The store is therefore a map `Path → StaticNodeId` and nothing else is new — no
second read path, no new node kind.

**The freeze is per cell**, not whole-module: each cell gets its own artifact and its own
key, and only a *recomputed* cell is frozen, so an edit pays for the cells it dirtied and
nothing else. That needs a **closure** freeze entry point, because the whole-module
freeze maps *every* node of the module; the three phases already work on a set of nodes,
and what differs is that the set is the closure.

**The closure must be closed under four edge kinds**, or the existing node-map lookups
panic — they assume totality, which is the built-in check:

- a value's items/entries (arrays, tables, ext handles);
- `operation.operand` — a residual node must be able to re-run later, so unlike the GC's
  walk, which deliberately does not follow a cached value's operand, this one must;
- the equality class of a node **whose own value is still undecided** — the class is what
  holds that node's answer, so it is taken whole. A node that already carries its own
  solved value takes nothing from its class, and the class the artifact does not hold
  whole is spliced to the members it holds, exactly as the GC splices a class that lost
  members;
- the **whole function template** (`StaticFunction.nodes` is the template's member list,
  and a missing member is a broken template);

plus whatever the value reports through `ValueExt::traced` — the contract the GC relies
on, and the only way to see a node an opaque ext payload holds, because phase 3 rewrites
a handle and never the bytes behind it. **A value that fails to answer is not caught**,
here exactly as in the GC.

**Per-cell freeze is an ownership transfer, and it is general:**

- a **handle-type** value (an array or table whose items live in the module's block arena)
  is **copied** into the artifact's arena, as phase 2 already does; the module keeps its
  own copy, so nothing dangles;
- **everything else a value owns outside the arena** is transferred by *obligation*, and
  the mechanism is deliberately not a device-buffer special case:

  ```rust
  pub trait Release { fn release(self: Box<Self>); }              // lichen-lowlevel
  fn release_obligations(&self, out: &mut Vec<Box<dyn Release>>)  // ValueExt, per leaf
  ```

  `StaticModule` owns the list (`releases`) and its `Drop` runs it, so **eviction is the
  early release path**; an artifact loaded from the device's store owns none, because a
  resource handle is process-local and cannot be in the bytes. The hook is **per leaf** on
  `ValueExt` — composed by the manifest macro exactly as
  `is_handle`/`handle`/`set_handle`/`alignment` are — because the issuer is reachable
  *process-wide* (`lichen_kernel_ir::parallel_backend()`, installed by
  `install_parallel_backend`), so a leaf needs no program parameter to build its
  obligation. `ComputeValue` implements it for `DeviceBuffer`: the obligation holds the
  `Arc` the lookup hands back — the same "the installed backend is the issuer" assumption
  `collect`/`read` already make — and a backend that is not installed has nothing to
  release.

  This composes with the existing rule rather than fighting it: a value dropped by
  `drop_block` **still does not release** (the deliberate no-per-value-release decision),
  the context still owns the memory until it drops, and the artifact's obligations are the
  **early** release path. Because the module's own copy never releases, taking the
  obligation cannot double-release; adding per-value release later is what would have to
  revisit this, not the other way round.

**The store, the hook and the edit side.** `CellStore` (`cells.rs`) keys a cell by its
**occurrence path** and holds only the frozen reference — the artifact lives in the
caller's registry, exactly as an import's does. `compile_with_cells(source_id, source,
cells, registry)` is the entry point. **One `path::for_each` pass** over the resolved AST
collects the marked bindings' paths before lowering (each binding carries its own
`BinderId`), so the lowering walk never carries a path. A marked binding whose cell is
clean lowers to `ExprKind::Static`; a block-wide binding's placeholder *is* that static
read, made in place rather than transplanted from a second node, which would leave that
node behind in the IR. A marked binding that *was* compiled is frozen **per cell** once
the build is solved and recorded under its path and its source. The registry's `hash` slot
is a documented **placeholder** on this path — a cell's reuse is decided by its path and
by propagation, never by content.

The **dirty input** has two cuts. The coarse one is the caller's, and it is the
granularity chosen for *another* file: `CellStore::invalidate_source(source_id)` drops
that source's cells and hands back the artifacts they named. The fine one is the
session's, for an edit to the file it holds: `CellStore::invalidate_paths` drops the
positions dirty propagation reached (§4) and `CellStore::retain_marked` drops the
positions the program no longer marks (a removed `cache`, a rename). A cell is reused iff
neither reached it. Both are sound **without recording a single read** — the AST is the
read graph.

**Two limitations, recorded rather than hidden:**

- a marked binding whose value spells an **annotation** is not eligible (a static read
  materializes a two-wide pair, and an annotation makes the source's pair wider, so it
  would be read back at the wrong arity) — it is compiled and re-frozen as an ordinary
  binding;
- a **`Parameterized`** cell is not retained (it has no answer to keep): the cell is left
  out and the next build compiles the binding again. A **failed build** retains nothing at
  all (§6).

## 4. Invalidation: dirty propagation over the resolver's own graph

An edit does not invalidate a *file*, it invalidates **positions**. The propagation is
push, from the edit outward along read edges, and it needs no recorded read set — the AST
already *is* the read graph, exactly: the resolver wrote a `BinderId` into every `Name`
use, so "what does this binding read" is a walk, not a log.

### 4.1 The graph's node is the top-level statement

A statement **declares** the binders its subtree binds and **reads** the binders its
`Name` uses resolve to. A nested binder (a lambda parameter, a block local) is visible
only inside its own statement, so it can never carry an edge *between* two statements —
which is why the statement is the right node, and why a cell anywhere inside a statement
(a mark in a nested block included) is dirtied exactly when its statement is.

The **seed** is the window the splice already computed: the statements the edit re-parsed.
Nothing else has to be detected — the incremental parser knows the dirty region because it
had to re-parse it.

The **fixpoint**: a statement is dirty if it was re-parsed, or if it reads a binder a
dirty statement declares. That is the whole rule.

`dirty.rs` implements it: `dirty_marked_paths(previous, current, previous_window,
current_window)` holds the statement graph, the fixpoint, and an exhaustive `walk` over
`Expr`.

### 4.2 Both programs are propagated over

An edit can move a resolution, and the two programs disagree about what a name reads, so
one propagation is not enough:

- the **previous** program catches a read that **disappeared** — a name the edit deleted
  or moved out of scope. The current program no longer records that read, so only the old
  graph has the edge.
- the **current** program catches a read that **appeared** — a name the edit brought into
  scope, so the binding now reads something it did not. Only the new graph has the edge.

Either alone is unsound, and neither is a superset of the other. A full re-parse has no
window, and then the whole program is the dirty region — the honest answer.

### 4.3 Cycles

A marked cell that participates in a value cycle is dirty, recomputed and re-frozen as one
**SCC-atomic** unit; the in-progress assumption the within-build note names is the same
assumption, at the same place.

### 4.4 Backdating — not built

After a recomputation the new value could be compared with the retained one, and equal
means the cell keeps its identity *and* its consumers stay clean. That is a refinement,
not a correction: propagation dirties more than necessary and never less, so its absence
costs work, never correctness. It would pay where an edit changes a marked binding's
*bytes* without changing its *value* (`cache a = 1 + 2` → `1 + 1 + 2`), which the
byte-position seed cannot see through.

**It is not a local addition to `dirty.rs`.** Today the dirty set is computed *before* the
lowering and every dirty cell is dropped; backdating needs the opposite order — recompute
in dependency order, compare each result with the retained one, and only then decide
whether a *consumer* is dirty. That is a recompute heap, not a filter on the propagation.
A cheaper half exists (compare a recomputed cell's value with the retained artifact's and
keep the old artifact, saving the freeze and the key churn) but it saves the least
expensive part of the work.

### 4.5 Why not recorded reads (pull verification)

An earlier draft recorded, per cell, **the paths it read** plus a version stamp, and
*verified* a demanded cell (pull) before recomputing it: resolve each recorded path, ask
when it last changed, reuse the cell if nothing did. That is Salsa's red-green step with
paths in place of query keys, and it was called "verification (pull), not propagation
(push)". It is rejected for three reasons:

- **it contradicts the requirement** — the requirement asks for a dirty propagation
  mechanism, and pull verification is the other thing;
- **recording is redundant**: the resolver's `BinderId` annotations *are* the read set,
  complete and exact, and they are re-derived from the AST every compile. A recorded copy
  is a second source of truth that can go stale, for nothing;
- **a stamp needs a clock**, and with no keys there is no cheap "when did this last
  change" — the answer would be the recomputation the stamp was meant to avoid.

The dependency relation the draft stored as paths is still there, and still path-shaped: a
cell's identity is a path, and what dirties it is the *statement* it sits in.

## 5. The two edit surfaces

- **Graph edits use a descriptor.** Here "graph" is the *program's own* graph data — the
  structure an agent builds and edits (a PCG graph), **not** the JIT's `lichen-graph-ir`.
  It is edited *as data* — add / remove / rewire / re-parameterize a node — so the edit
  names the node paths it touched and the invalidation is exact. No diffing is involved.
- **Source edits use file granularity.** A source edit dirties the file; the file's marked
  cells are re-derived, and §4's propagation is what keeps the rest of the program, and
  the graph, from re-running. Sub-file identity is *not* needed for this, which is why no
  content key is needed either. The session lands finer than this: the *caller* still
  names a file, and what the session does inside it — propagate from the re-parsed window
  — is strictly more precise than dropping the file's cells.
  `CellStore::invalidate_source` remains the coarse cut for a file the session does not
  hold.

## 6. Using it

### 6.1 The session

`lichen_language::session::BufferSession` is the consumer. It holds the cell store, the
registry the artifacts live in, and the caller's `source_id`. `compile` lowers through the
cell path and reconciles the store against the program about to be lowered, so the order
is: reconcile (drop dirty and unmarked) → lower (a clean cell becomes a static read) →
check → freeze what was compiled. Its `SessionReport` carries `CellEvents { reused,
frozen, dropped }` — without it the mechanism is silent, and a caller cannot tell a
rebuild that reused nine cells from one that reused none. What it dropped it owes the
registry, and `evict_unreachable` is where it pays (§6.3).

Two properties of the shape are worth naming, because both are decisions:

- the **coarse gate runs first**: if the resolved content key is unchanged the whole
  `Build` is reused and the store is not touched at all — no cell is consulted when no
  lowering happens, and the build being reused *is* the one that was correct for this
  content. The cells are the finer cut *under* that gate, for the edits it rejects.
- the window dirty propagation is seeded from is the **splice's own**, in both index
  spaces (§4.2). It costs one field (`SpliceOut::old_hi`) and no extra detection.

The cell's mark is part of the content key (`KEY_FORMAT_VERSION` 2). The key describes
"the structure the lowering consumes", and the lowering *does* consume `cached`: it is
what makes a binding the one a clean cell may replace. A key that ignored it would
describe a lowering that is not the one that runs — and adding `cache` to a binding would
silently do nothing until the next structural edit.

A **failed build freezes no cell**: a cell is read back by skipping its body, so one
frozen from a failed *check* would carry that failure's silence — the error reported once,
then gone. Conservative on purpose: one broken binding costs the whole program its reuse
rather than risking a vanished diagnostic. Frontend diagnostics are not at risk (lex,
parse and resolve re-derive from the AST every compile), which is why the guard tests the
build's `ok` rather than the report.

### 6.2 The two ways a caller supplies text

- `set_source` is the whole-file entry point: the caller re-reads the file, hands the
  buffer over, and the session derives the edit itself by diffing the token streams.
- `set_view(code, base, line_starts, imports)` is the preprocessed view. A caller that
  owns the preprocessor — the language server does, because it owns the store and the
  `---…---` block's directive spans — hands the session the *code after the block* rather
  than a whole file. Two consequences: `lex_resume` takes the base, because it was slicing
  the code it was handed at a byte offset read out of an **absolute** token range (the
  same thing only while the base is 0), and a view whose mapping moved drops the
  incremental snapshot, because every token range and span in it is in the old
  coordinates. The cells are untouched: a cell's identity is a path, not a position.

### 6.3 Eviction: who pays, and what cannot be paid

`CellStore::invalidate_paths`/`retain_marked` hand back the artifacts they just made
unreachable, and the session accumulates them (`pending_evictions`).
`BufferSession::evict_unreachable` pays the debt: it calls `Registry::evict` on each, in
**repeated passes until a pass frees nothing** — because a key that is still referenced
may be free once the artifact referencing it is gone, and that artifact may be in the same
list, later in it. What a pass cannot free is left pending, honestly.

`Registry::evict` answers `Eviction` rather than a `bool`, because "not registered" and
"still referenced" are different facts to a caller (the second means *retry later*), and
it checks the half of its precondition a registry can check: an artifact that a live
registered artifact still references is **refused**. A static ref is a raw handle into the
artifact's arena, so freeing it would leave the referencing artifact dangling. The other
half stays the caller's and is documented at the call site: a static ref also lives in
every `SessionReport` the caller still holds, and the session never sees the last `Arc`
clone die. **A caller that never calls `evict_unreachable` leaks; a caller that calls it
while still holding a report dangles.**

When does one artifact reference another? Only when a cell's value *is* a static payload
shared from another cell's arena — an array, a table, a function, an ext handle. A scalar
is just a value: `cache b = a + 4` copies nothing and holds no ref, so the whole chain is
free to go. The sharing needs two builds: cells are frozen *after* a build, so within one
build a later binding never reads an earlier one's cell — it compiles the body and gets
its own copy. A *later* build, with `b` recompiled while `a`'s cell is clean, is what
makes `b`'s artifact read `a`'s in place.

### 6.4 The language server as the caller

`BufferSession<P>` is `!Send` — it holds the checker's `Build<P>`, whose arena handles are
raw pointers, exactly as `Doc` does — and unlike `Doc` it must *outlive* the request that
used it. `tokio::task::spawn_blocking` may run its closure on any pool thread, so a
session parked in a `Mutex` would be touched from whichever thread took it. The boundary
is therefore a **dedicated thread** (`server::Worker`): the package store, one shared
registry, and one session per open document live on it, a job goes in (a `Url`, the text,
a reply channel) and a `Send` index comes back. One thread costs nothing: the transport
already serializes requests (`concurrency_level(1)`), and a session is a single-threaded
object by construction — its value *is* the state the last compile left behind.

**One registry, for cells and imports and every open document.** A cell's artifact is a
*frozen closure*, and a closure whose value read an import names that import's module key;
`Registry::freeze_closure_mapped` asserts every key the module references is registered
**in the registry the artifact is filed in**, so a cell and the imports it read must share
one registry. That has three consequences:

- the **cell key counter lives in the registry** (`Registry::allocate_cell_key`), in a key
  space of its own (the top bit). A counter per store and a counter per session would each
  collide, and the registry is what a key has to be unique *in*.
- a **recompiled package replaces its slot** (`Registry::freeze_mapped_replacing`). The
  store builds a fresh `PackageStore` per run (so its `packages` map is exactly that
  document's import closure, which is what the analysis records as dependencies), but the
  *registry* is long-lived — so the previous run's artifact for that file is still
  resident, and `freeze_mapped`'s "not already registered" assertion is exactly what a
  recompile violates. The other half of the contract is the caller's: every live artifact
  that referenced the replaced one is gone, or is being replaced in the same operation (a
  recompile walks the import closure and rebuilds every dependent whose identity moved,
  because a dependent's identity folds its dependencies'). Dropping the old artifact runs
  its release obligations, and those own host resources rather than reading other arenas,
  so the replacement order is the caller's business only for the reads it plans to do.
- **an import edit is the coarse cut.** A cell is a *value* computed from the bytes of the
  files that were loaded then, and the document's own content key — which is over its
  resolved structure — says nothing about them. So the worker keeps a per-document record
  of the files the last analysis read, and drops the session when it cannot prove them
  unchanged (`None` from a file that cannot be read back counts as unprovable).

**The report carries what the editor reads.** `SessionReport` carries the token stream,
the resolved AST and the build's `ExprId → span` index, because a caller that had to
re-lex or re-parse to get them would not be incremental at all. The span index is retained
across a reuse, and moved through the edit: a rendered diagnostic is a `(line, col)` pair
and the content key is span-free, so an edit that moves text without changing the resolved
structure would otherwise leave the cached spans describing the old file. A position the
edit replaced outright — a check diagnostic pointing into the text it rewrote — has no
honest mapping, so the session re-lowers and re-checks instead of pointing it at the
replacement. `spans.rs` (`shift_expr`/`shift_stmt`/`shift_field`, and `moved_offset` for a
*retained* position) is the mechanism. The walk is exhaustive over `Expr` on purpose, so a
new expression form is a compile error rather than a span that quietly stops moving.

**Telemetry, because silence is the failure mode.** The server pushes what each compile
did as a `lichen/analysis` notification. Nothing in the tree asserts it — the project
writes no tests without a reason to — so it is the surface a client (or a probe) reads to
see whether the server is really driving the session, rather than a claim the code makes
about itself.

**A compile that panics does not take the worker with it.** A panic inside a job is
caught, the session that did it is dropped (its state is not trusted again), and the
request is answered by the one-shot path — the degradation this mechanism exists to avoid,
not a failure. Without it the first bad input would kill the thread and every later
request with it, which is worse than the `spawn_blocking` behaviour it replaced (there,
the closure's panic cost one request).

## 7. Costs and failure modes

- **Path churn is the whole risk.** If an agent's edits keep moving nodes, paths keep
  changing and retention never hits. §9 says what that would falsify.
- **A missed edge is a silent stale reuse.** The read set is not the risk it was in the
  draft — the resolver's `BinderId` annotations are complete and exact, and they are
  re-derived every compile — so the obligation moved to the *walk* that collects them:
  `dirty.rs`'s `walk` is exhaustive over `Expr` on purpose, so a new expression form is a
  compile error rather than a silent missing edge. The other edge is the graph's node: a
  binder that could be read from a *different* statement would need a finer node than the
  statement. Today it cannot (a nested binder is visible only inside its own statement),
  and that is the invariant to re-check if scoping ever changes. §4.3's cycles and §9's
  differential harness are the oracle, not an argument.
- **Instance explosion.** A `cache` inside a lambda instantiated per recursion step would
  create one cell per instance (§2.5). The retention policy must be able to evict, and a
  cell inside a cyclic instance set should be refused (or bounded) rather than silently
  multiplied.
- **Store lifetime.** Cells hold device buffers (`ResidentId`), and a value dropped by
  `drop_block` does not release. §3.3's obligation list is what makes an eviction release
  early instead of leaking VRAM for the life of the context — and **eviction is the
  caller's call**, because a static ref is a raw handle into the artifact's arena.
- **The registry grows between evictions.** Every recomputed cell files a new artifact
  under a new key, so a long agent session leaks device memory until something evicts.
  The session *can* pay it back (`evict_unreachable`, §6.3), in repeated passes and with a
  refusal for anything a live artifact still references — but **it does not do so on its
  own**, because the other half of the precondition is the caller's (§6.3). That is the
  remaining sharp edge.

## 8. The mechanism's sharp edges

Each of these is a silent wrong answer or a leak, and each is load-bearing:

- **A session is `!Send`, and that is not a detail.** It holds `Build<P>` (raw arena
  handles), so it can live in neither the async server's state nor a `spawn_blocking`
  closure — the thread is the boundary (§6.4). A `Mutex<HashMap<Url, BufferSession>>`
  would compile only if the session were `Send`, which it is not; forcing it (`unsafe impl
  Send`) means a checker reading another thread's arena.
- **A cell and its imports must share one registry.** `freeze_closure_mapped` asserts
  every key the **closure** references is registered where the artifact is filed, so a
  session with imports and a *private* registry panics on the first freeze (a `cache`d
  binding whose value read the import). `BufferSession::with_registry` is the entry point,
  and the two key allocators must not meet: the device's is a dense counter, the cell's is
  the top bit of the key.
- **The dependency check runs before the freeze, and that order is load-bearing.** The
  check is handed the closure's keys (`referenced_keys_of`, the closure-scoped form of the
  module-wide `referenced_keys`) *before* `freeze_set` runs, because a freeze takes the
  artifact's **release obligations** off the values it freezes: a refusal that came after
  it would have to drop an artifact whose obligations it had already taken, releasing (say)
  a device buffer the live module still names. So the check cannot be moved to "after the
  build, on the artifact's own refs", however tempting that is — the sets agree, but the
  ownership does not.
- **A recompiled package must replace its slot, not assert it empty** (§6.4).
- **A retained cell is a value, not a program.** Nothing in the document's text records
  which files its cells were computed from, so an edited import must invalidate the session
  by a record the *caller* keeps (the worker does), not by the content key.
- **A reuse must move its retained positions** (§6.4).
- **Evicting a live artifact dangles.** A static ref is a raw handle into the artifact's
  arena: `Module::static_module` panics on an unregistered key, and a payload already read
  through it dangles. `Registry::evict` refuses the half it can see (a live registered
  artifact's `refs`, §6.3); the other half — a `StaticNodeId` in a `Module` the caller
  still holds — is the caller's, and calling `evict_unreachable` while still holding a
  `SessionReport` is how it is violated. The server's rule is that a report never leaves
  the worker thread: only the `Send` `DocIndex` crosses, and the index holds no static ref.
- **A shared payload is what makes one artifact reference another**, and a scalar is not
  (§6.3).
- **Two artifacts that reference each other are never freed.** `evict` refuses both, and
  `evict_unreachable`'s passes stop making progress, so the pair stays pending forever.
  The honest answer for a cycle, but it is a leak.
- **A cell is read back by *skipping the body*.** Its lowering, its check and its
  evaluation all do not happen, so a cell frozen from a build that failed *carries that
  failure's silence*. The guard is `freeze_cells`' `!build.ok` early return, and it is
  load-bearing (§6.1).
- **Dropping either propagation is unsound, not merely imprecise** (§4.2).
- **The window must be reported in both index spaces.** Dirty propagation is seeded by the
  statements the splice re-parsed, and the old program's window is not the new program's
  whenever the edit added or removed statements — hence `SpliceOut::old_hi`. A window past
  the end of a program is silently empty, so an off-by-one there is a silent missed dirty.
- **A full re-parse dirties everything.** That is correct and expensive: the fallback path
  (`splice_program` → `None`) drops every cell. It is also what makes the fallback *safe* —
  do not "optimize" it into a narrower window without the two-space argument.
- **The window's new end must be projected from the *suffix's* first byte**, and the
  suffix's shift measured from the suffix's **own** first token in each stream. An edit
  that deletes whole statements also deletes the separator between the window and the
  suffix, so both the window's last token and its end token end up *inside* the replaced
  region: project either and the window cuts through the statement that follows — a
  truncated binding is re-parsed and the suffix is spliced after it, which duplicates
  statements and can push a `stmt_ranges` entry one past the token stream (the next splice
  then panics). The corrupted program still *evaluated* correctly, so only the diagnostic
  count and the later panic give it away — which is why the differential oracle compares
  diagnostics too.
- **An empty window is a splice, not a parse.** `ns == ne` is legal (prefix meets suffix)
  and must not be handed to `parse_statement_region_traced`: the region parser requires at
  least one statement, so an empty region reports "found the end of the program" — a
  spurious diagnostic on an edit that deleted a statement.
- **The `dirty.rs` walk must stay exhaustive over `Expr`.** It is written as a full match
  on purpose: a new expression form that fell into a catch-all would contribute no edges,
  and a missed edge is a stale cell.
- **`retain_marked` is what keeps the store honest** (§3.3).
- **The content key carries `cache`** (`KEY_FORMAT_VERSION` 2). Bumping the constant
  invalidates every key; today that is in-memory only (the session and the tests), so a
  bump is free — check that before the keys are ever persisted.
- **An annotation on a marked binding is skipped, not mis-read** (§3.3).
- **A `Parameterized` cell is silently not retained** — by design, but it means a marked
  binding that never solves is recompiled every build (§2.5).
- **The closure's four edges plus `traced`** (§3.3). Drop any of them and the freeze
  panics (the good case) or the artifact references a node it does not contain (the bad
  one — phase 3 rewrites handles, never the bytes behind them).
- **`traced` is only as good as its implementors.** A production value that holds nodes
  must implement it, and the composition macro must forward it.
- **A frozen class is only ever read by `static_find`, and only about nodes the artifact
  holds.** That is what lets the closure take a class whole *only* from a node whose own
  value is undecided, and lets `freeze_set` **splice** a class the artifact does not hold
  whole down to the members it holds (`disjoint::rebuild`'s splice, as the GC's
  `flatten_class` does). Two consequences to keep in view if this edge changes again: the
  splice must keep the **partition** the source's classes induce on the artifact's nodes
  (a member the artifact drops has no clone to be grouped with, but two it holds that
  shared a class must still share one), and a member the artifact holds whose own slot is
  undecided must not lose the value its class carries — which is why the class is taken
  whole from such a node. A shared type makes one class the program: a scalar cell's class
  can hold hundreds of members. Anything that changes the edge must re-check `apply.rs`'s
  `regroup_clones`/`static_function_captures` and the equality suite's class tests.

## 9. What would falsify it

- **Paths churn more than they persist**: if ordinary agent edits resolve to "lost" often
  enough that the retained cone is small, the mechanism buys little and the honest
  conclusion is that the graph's cost is not in the marked cells.
- **The marked granularity is wrong**: if the expensive work is *inside* one marked cell
  rather than *across* cells, freezing the cell's boundary saves nothing, and the retention
  unit needs to be finer (or the work needs its own incremental operator).
- **A missed edge shows up as a value divergence in the differential harness**: the
  session's value after an edit against a fresh compile of the same source, comparing
  diagnostics as well as values. That is the cheap oracle for the whole mechanism, and it
  is what a session probe does by hand.

## 10. Decisions taken

- **No keys** — so identity is allocated (§1) and change detection is dirty propagation
  over the resolver's own read graph (§4) rather than "hash and look up".
- **Identity = occurrence path, name-preferred** — chosen over a user-written name only
  (no identity for anonymous nodes, §2.4) and over pure child indices (an insertion
  invalidates the whole suffix, §2.2).
- **Resolution is dynamic**: no `Path → NodeId` registration; a path is resolved on demand
  (§2.3).
- **The tree is the AST, not the IR.** The IR is a graph with no binding names, and the AST
  is the tree the resolver already walks by name. A path therefore names a position, which
  is what a cell is. Landing it there also keeps the front end's compilation and the
  checker's allocation path untouched, which a name-carrying IR would not.
- **`cache` is a keyword, not an attribute**: a slot is a runtime value; the mark is
  static and identity-selecting (§3.1).
- **`cache` is allowed in any scope**: an instance reaches a free-variable-free scope in
  which a name/path is again stable (§2.5).
- **Per-cell freeze, not whole-module**: only a recomputed cell is frozen, so an edit pays
  for what it dirtied. It forces one new entry point (the closure freeze) rather than a
  smaller whole-module freeze.
- **The container stays a dense `Vec`, not a hash table**: a hash table would buy the one
  mechanical renumbering pass and cost the dense `LocalNodeId` that `StaticNodeId`, the
  artifact codec, the content-addressed `artifact_hash` and `regroup_clones`' ordering all
  rest on, plus a hash lookup on every static read. The renumbering was never the hard
  part — the closure is.
- **The ownership hook is per leaf on `ValueExt`, not on `Program`**: the issuer is
  reachable process-wide, so a leaf needs no program parameter, and the composition macro
  chains leaves exactly as it does for the payload methods.
- **Propagation, not recorded reads**: the read set is the resolver's `BinderId`
  annotations, walked on demand, so nothing is recorded and nothing can go stale (§4.5).
- **Both programs are propagated over**: the previous one catches a read that disappeared,
  the current one a read that appeared. Each alone is unsound, and neither is a superset of
  the other — the one place where "just dirty what the edit touched" is not enough (§4.2).
- **A failed build freezes no cell** (§6.1).
- **The `cache` mark is in the content key** (`KEY_FORMAT_VERSION` 2, §6.1).
- **A repeated name falls back to its index**: a path is an identity, so two entries in
  one list must never share a step. Name-preferred survives; only the ambiguity case
  changes (§2.2).
- Rejected: content-keyed reuse (the requirement), the fine-grained `StaticModule` units
  of the earlier draft (their identity was a content key; the representation survives as
  the freeze-and-read-in-place of §3.2, with the path as key), and pull verification
  (§4.5).
- **Small step first**: retained cells in a store, `Module::new()` per build unchanged. A
  live module with universal node-level tracking is the next segment of the same route,
  not a second design.

## 11. Open items

- **The keystroke path.** The server drives a session per open document, but the edit
  path it consumes is still the whole-view one; the fine, keystroke-granularity wiring is
  open (`P1-17` (b)).
- **The eviction timing.** *Answered for the mechanism and for one production caller,
  open for the general policy.* The session evicts what it dropped when the caller says so
  (`evict_unreachable`), and the registry refuses what it must; the server calls it at a
  document close and at a session drop, because that is where it can promise no report is
  still held. What is not decided is whether a *long-lived* document should pay earlier —
  after a compile that dropped cells, on a memory budget, or on an explicit call. The
  caller owns it because only the caller knows when the last `SessionReport` clone died,
  which is the half of the precondition the registry cannot check (§6.3).
- **The reverse import closure.** The cell store drops what it is told; the graph of who
  imports whom is the package store's. Dirty propagation covers one file's own statements —
  an *imported* file's edit needs the coarse cut plus that closure. The server takes the
  coarse form (a per-document record of the files its last analysis read; the session is
  dropped when they cannot be proven unchanged, §6.4), which is sound but throws away
  cells an import edit did not affect. This is also where the eviction refusal stops being
  unreachable-by-accident and starts being the thing that keeps a cross-file reference
  from dangling.
- **A path across an import boundary.** The step vocabulary is settled (§2.2); what is not
  is how a package's file identity prefixes a path — the registry's file identity is
  path-derived (`is_lichen_file_id` / `file_id_hash(file_id: &str)`,
  `lichen-registry/src/device.rs`), which is a name, not a content hash, and must stay
  that way.
- **The graph-side descriptor's shape**, and whether a graph node's path is expressed in
  the same step vocabulary as a source node's (§5).
- **Backdating** (§4.4).
- **Per-revision diagnostics and budget semantics** — inherited from
  `incremental-evaluation.md` §7's pending list, since recomputation is observable through
  the budgets.

## 12. Entry points

- `lichen_language::session::BufferSession` — `with_source_id(source, source_id)`,
  `with_registry(source, source_id, registry)` (a session whose cells share the registry
  its imports live in — what a caller with imports must use), `set_view(code, base,
  line_starts, imports)`, `set_source`, `compile() -> SessionReport`,
  `retained_cells()`, `pending_evictions()`, `evict_unreachable()`. `new(source)` is the
  unnamed-buffer shorthand.
- `lichen_language::compile_with_cells(source_id, source, cells, registry) ->
  Report<LangProgram>` — the whole cell path, without a session. `compile`,
  `compile_with_imports*`, `frontend*` and `build_report` all delegate with no cells, so
  nothing else changed.
- `lichen_language::cells::CellStore` — `reference`, `record`, `invalidate_source`,
  `invalidate_paths`, `retain_marked`, `len`. It does not allocate keys: the registry does
  (`Registry::allocate_cell_key`).
- `lichen_language::dirty` (crate-private) — `dirty_marked_paths(previous, current,
  previous_window, current_window)`; the statement graph, the fixpoint and the exhaustive
  `walk` are inside.
- `lichen_language::spans` (crate-private) — `shift_expr`/`shift_stmt`/`shift_field`: the
  span shift a spliced-in *clone* needs (`offset_of_span` → `+ delta` → `line_col`), and
  `moved_offset`: the same move for a *retained* position, with `None` for one the edit
  replaced.
- `lichen_lowlevel::Registry` — `freeze_closure_mapped(module, key, roots, hash)`,
  `freeze_mapped_replacing(module, key, hash)`, `allocate_cell_key()`, `evict(key) ->
  Eviction`; `lichen_lowlevel::Eviction`.
- `lichen_lowlevel::Package` — `refs`, the keys the artifact references (read off the
  frozen values, so it is the closure's set); `StaticModule::referenced_keys` is the
  reader.
- `lichen_lowlevel::StaticModule` (crate-private) — `freeze_closure(module, key, roots,
  check)`: `check` is handed the closure's dependency keys (`referenced_keys_of`) *before*
  anything is frozen, which is the registry's precondition and must stay in that order
  (§8); `freeze_set` is the shared phase. `releases`, `Drop`.
- `lichen_lowlevel::ValueExt` — `traced`, `release_obligations`; `lichen_lowlevel::Release`.
- `lichen_language_lex::lex_resume(prev, old, new, line_starts, base, a, b)` — the
  incremental re-lex, with `a`/`b` and the token ranges **absolute** in the file the code
  is a region of.
- `lichen_language_parser::path` — `Step`, `Path`, `children`, `root_children`, `resolve`,
  `for_each`, `Node`.
- `lichen_language_server::analysis::{Artifacts, index}` — the editor index over the
  frontend artifacts, shared by the one-shot path (`analyze`, crate-private) and the
  compile worker, so the incremental path and the one-shot path cannot drift.
- `lichen_language_server::server::{Worker, WorkerState}` (crate-private) — the thread the
  sessions live on; `Analysis` is what crosses back.

## 13. How to verify

```
cargo check --workspace --all-targets
cargo test -p lichen-lowlevel -p lichen-highlevel -p lichen-language -p lichen-language-server -p lichen-compute
```

The influenced set is those five crates, plus the lexer (`lex_resume`'s base) and
`lichen-package`/`lichen-compiler` (which freeze whole modules through the same
`freeze_set`). The temporary probes the design was measured with are gone; to re-take a
reading, write one as an `examples/` binary and delete it after.

Two probes are worth re-creating first, because they are the oracles: the
**differential** one — every prefix of an edit sequence against a fresh compile, comparing
value *and* diagnostics — and a **span** one — a failing expression in the cloned suffix,
comparing the session's diagnostic span against a fresh compile's, shape by shape. The
first found a window-projection bug, the second a stale-span bug; a count-only or
value-only reading misses both. A third, for anything touching the server: drive the
binary and read the `lichen/analysis` notifications (§6.4) — an edit that stops reusing
cells leaves the diagnostics correct, so the telemetry is the only thing that shows it.

A session probe reads the value by **consuming** the session: a `Build`'s module is
evaluated through a `&mut`, and the report's `Arc` is only unique once the session's own
clone is gone. On the key-reused path it is still shared, and the probe prints
`<still shared>` — that is the probe's limit, not a bug.
