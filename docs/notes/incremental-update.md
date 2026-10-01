# Incremental update: identity by path, retention by `cache`

> Status: **proposed** — the `cache` syntax is landed and **inert** (§7 step 2), and
> so is path identity (§7 step 1: the path module); no invalidation or retention
> mechanism is built. This is the *cross-build* half of the
> incrementality question. It supersedes the withdrawn cross-build halves of
> [incremental-evaluation](incremental-evaluation.md), which keeps the *within-build*
> settled cut, the deep-pass measurement and the mutation inventory; where the two
> meet, this note links there instead of restating.
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
> Points at: `crates/lichen-highlevel/src/{ir,checker}.rs` + `checker/lambda.rs`,
> `crates/lichen-language/src/{resolve,session,run}.rs`,
> `crates/lichen-lowlevel/src/{function,evaluation,equality,gc,module,lib}.rs`,
> `crates/lichen-registry`, and the notes
> [incremental-parse-compile](incremental-parse-compile.md),
> [artifact-cache](artifact-cache.md), [static-modules](static-modules.md),
> [attributes](attributes.md), and `compute-graph-jit` (branch
> `feature/graph-jit`).

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

Every "lost" row is *safe* (it over-invalidates; it never serves a stale value),
and §5.2 makes each one a diagnostic rather than a silent miss.

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
- a path that **does not resolve** in the current build is a lost cell, which is a
  diagnosable event (§5.2), not a silent reuse;
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

## 4. Invalidation: path-valued dependencies, verified on demand

### 4.1 What a retained cell records

A marked cell stores, beside its value:

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

### 5.1 Syntax

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

### 5.2 Semantics and the diagnostic surface

- On first computation, the value is frozen into the store under the cell's path and
  later read **in place** (the existing `StaticModule` read path — no payload copy).
- On a later build with the same path, the cell is verified (§4.2), reused, or
  recomputed.
- Every one of those four events is **reportable**: `created`, `reused`,
  `recomputed (because …)`, `lost (path changed / unresolved)`. This is the PCG-side
  analogue of a cook graph's "why did this re-cook", and it is also the differential
  harness's oracle.

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

| step | file / item | change |
|---|---|---|
| 1 | `lichen-language-parser/src/path.rs` | **landed**: `Step`/`Path`, the position vocabulary (`children`), dynamic `resolve`, and the descent (`for_each`) |
| 1 | the front end / checker | **not built**: recording a marked cell's read paths while it is computed |
| 2 | `lichen-language-lex` / `-parser` / `ast.rs` | **landed**: the `cache` keyword and `Binding.cached` — parsed and carried, consumed by nothing |
| 2 | `language-spec.md` §2 + `tree-sitter-lichen` (`grammar.js`, `highlights.scm`) | **landed**: the statement form, the keyword list, the highlighting |
| 3 | `lichen-lowlevel/src/static_module/freeze.rs` | **landed**: `freeze_closure` (the closure of a root set) and `freeze_set` (the shared phases); `from_module_mapped` is now "the closure of every node" |
| 3 | `lichen-lowlevel/src/lib.rs` (`Release`, `Program::release_obligations`, `StaticModule::releases` + `Drop`) | **landed**: the general ownership transfer — an artifact owns its out-of-arena resources and releases them when it is dropped |
| 3 | `lichen-registry` (the store) | **not built**: cells keyed by path; freeze/read in place; the four events reported |
| 3 | `session.rs:243` (`content_key` gate), `artifact_hash` | **demoted**: they may still move bytes between processes and feed diagnostics, but they no longer decide reuse |
| 4 | the program's own graph data (the PCG graph a program builds) | node paths and the edit descriptor. **Not** `lichen-graph-ir`: that crate is the JIT's recorded evaluation graph, a different structure with a different identity (§1.1) |

Step 4 is a *consumer*, not a mechanism: the agent edits the graph as data, and the
cells it marks are what make an edit cheap. Nothing in `lichen-graph-ir` changes for
it — the two graphs are different structures (§1.1).

### 7.1 The retention landing's shape (step 3)

The mechanism is the **existing** import path, which is what makes this step small:

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
pays for the cells it dirtied and nothing else. That needs one new entry point — a
**closure** freeze — because `from_module_mapped` maps *every* node of the module.
The three phases already work on a set of nodes; what changes is that the set is the
closure. The closure must be **closed under four edge kinds**, or the existing
`node_map[&x]` lookups panic — they assume totality, which is the built-in check:

- a value's items/entries (arrays, tables, ext handles);
- `operation.operand` — a residual node must be able to re-run later, so unlike the
  GC's walk, which deliberately does not follow a cached value's operand, this one
  must;
- the **whole equality class** (`parent`/`next`/`tail` all go through `node_map`, and
  half a class is a broken class);
- the **whole function template** (`StaticFunction.nodes` is the template's member
  list, and a missing member is a broken template).

**Per-cell freeze is an ownership transfer, and it is general** — the superior's
correction, and the reason this is not merely a smaller `from_module`:

- a **handle-type** value (an array or table whose items live in the module's block
  arena) is **copied** into the artifact's arena, as phase 2 already does; the module
  keeps its own copy, so nothing dangles;
- **everything else a value owns outside the arena** is transferred by *obligation*,
  and the mechanism is deliberately not a device-buffer special case.  The artifact
  (`StaticModule`) owns a list of release obligations, filled at freeze time, and
  dropping the artifact — its eviction — runs them.  The hook is on **`Program`**, not
  on `ValueExt`:

  ```rust
  pub trait Release { fn release(self: Box<Self>); }
  // on Program:
  fn release_obligations(value: Self::Value, out: &mut Vec<Box<dyn Release>>) { … }
  ```

  `ValueExt` is the wrong place because it deliberately carries no `P` (see its own
  doc: keeping `P` off it is what saves a program parameter being threaded through
  every `ValueType` bound), while `Program` already hosts the policy hooks — the
  unification policy is the precedent.  A program that owns nothing takes the default
  and pays nothing.

  **Landed** in `lichen-lowlevel`: the `Release` trait, the `Program` hook,
  `StaticModule::releases`, and the artifact's `Drop`, which the freeze fills once per
  frozen value.  An artifact loaded from the device's store owns none
  (`persist/container.rs`), because a resource handle is process-local and cannot be
  in the bytes — the store's own eviction is the only path that has an obligation to
  run.

  This composes with the existing rule rather than fighting it: a value dropped by
  `drop_block` **still does not release** (the deliberate no-per-value-release
  decision), the context still owns the memory until it drops, and the artifact's
  obligations are the **early** release path.  Because the module's own copy never
  releases, taking the obligation cannot double-release; adding per-value release
  later is what would have to revisit this, not the other way round.

Two rules follow:

- **only a solved cell can be frozen**: a `Parameterized` cell has no answer to keep,
  so it is not retained — a diagnostic, not a silent freeze of nothing;
- **cells freeze in dependency order**: `freeze_mapped` already asserts that every
  referenced key is registered, the same discipline packages use, and a cell that read
  another cell's frozen reference satisfies it by construction.

**Measured · a temporary probe in the lowlevel's own test harness, since removed.**
On a four-node module — an array of two constants, plus one node no root reaches —
the closure of the array root filed **3 nodes of 4**, the unreachable node was not
pulled in, and the frozen artifact read both items (10 and 20) back through its own
arena; adding that node as a second root grew the closure to all 4. That is the
per-cell claim in its smallest form: the artifact is as big as the value it keeps.

A second reading, from the same harness: a value that **reports a node through
`traced`** and owns one obligation produced a closure of **2 nodes** — the value and
the node it reports, which is that edge doing its job — and an artifact carrying
**1 obligation**, which ran **exactly once** when the artifact was dropped.

The dirty input for this landing is the caller's, and it is the granularity the
superior chose for source edits: the edit declares which file changed, and every file
that **imports** it, transitively, is dirty with it (the reverse import closure, the
files whose values can depend on the changed one). A cell is reused iff its own file
is not in that closure. That is sound **without recording a single read**, which is
exactly why it comes before §4's path-valued dependencies: §4 makes the cut finer
(an unchanged cell downstream of a changed file is reused), it does not make this
landing sound.

**Step 1 measured · a temporary probe (`cargo run -p lichen-language --example
path_probe`), since removed.** Over seven shapes — top-level bindings, a binding
inside a lambda body, a record program, struct fields and named arguments, a table
and an `if`, a record block, a shallow array with an assert — it derived every
binding/field position with `for_each`, resolved each path back with `resolve`, and
compared the resolved position with the one it started from: **13 positions, 0
mismatches**. The two readings that matter:

- a binding inside a lambda body is reached as `f/2/y` — the reserved role slot for
  the body, then the binding's own name;
- **a binding's path is stable under an insertion before it**: `b`'s path is `b`
  both in `a = 1 / b = 2 / b` and in `z = 0 / a = 1 / b = 2 / b`, which is the whole
  reason the step is a name;
- a path that no longer names anything resolves to `None` rather than to a
  neighbour.

Step 2 landed **inert rather than rejected**: the mark parses, the AST carries it,
and no consumer reads it — so the spec and `Binding.cached`'s own doc both say so,
because a user who writes `cache` must not be led to believe a value is retained.
A record block's field carries it too (`RecordField.cached`), since a record block's
fields *are* its statements; without that the mark vanished in the statement →
field conversion, which is how it was found.
The mark is accepted in every scope and combined freely with `let` (the two are
orthogonal); no diagnostic is emitted, so nothing here can be mistaken for a
refusal. Making it *refuse* would be a semantic decision the mechanism has not
taken yet.

## 8. Costs and failure modes

- **Path churn is the whole risk.** If an agent's edits keep moving nodes, paths keep
  changing and retention never hits. That is measurable, and §9 says what it would
  falsify.
- **A read that is not recorded is a silent stale reuse** — the same class of
  obligation as `ValueExt::traced` ("nothing checks the answer, because nothing can").
  The mitigation is that the read sites are enumerable choke points, already
  inventoried in `incremental-evaluation.md` §3 (`class_value`, the operand read,
  payload items, table entries, a `StaticNode` reference), and that the differential
  harness is the oracle rather than an argument.
- **Instance explosion.** A `cache` inside a lambda instantiated per recursion step
  creates one cell per instance. The retention policy must be able to evict, and a
  cell inside a cyclic instance set should be refused (or bounded) rather than
  silently multiplied.
- **Store lifetime.** Cells hold device buffers (`ResidentId`), and a value dropped
  by `drop_block` does not release (`compute-graph-jit`'s landmine list). §7.1's
  obligation list is what makes an eviction release early instead of leaking VRAM for
  the life of the context.

## 9. What would falsify it

- **Paths churn more than they persist**: if ordinary agent edits resolve to "lost"
  often enough that the retained cone is small, the mechanism buys little and the
  honest conclusion is that the graph's cost is not in the marked cells.
- **The marked granularity is wrong**: if the expensive work is *inside* one marked
  cell rather than *across* cells, freezing the cell's boundary saves nothing, and
  the retention unit needs to be finer (or the work needs its own incremental
  operator).
- **A missed read shows up as a value divergence** in the harness. Then the read
  sites must be widened before anything else.

## 10. Decisions taken, and alternatives rejected

- **No keys** (the superior) — so identity is allocated (§1) and change detection is
  "recompute and compare" (§4.3) rather than "hash and look up".
- **Identity = occurrence path, name-preferred** (the superior) — chosen over a
  user-written name only (no identity for anonymous nodes, §2.4) and over pure
  child indices (an insertion invalidates the whole suffix, §2.2).
- **Resolution is dynamic** (the superior): no `Path → NodeId` registration; a path
  is resolved on demand (§2.3).
- **The tree is the AST, not the IR** (decided while landing step 1): the IR is a
  graph with no binding names, and the AST is the tree the resolver already walks by
  name. A path therefore names a position, which is what a cell is (§2.3). Landing it
  there also kept the front end's compilation and the checker's allocation path
  untouched, which a name-carrying IR would not have.
- **`cache` is a keyword, not an attribute** (the superior): a slot is a runtime
  value; the mark is static and identity-selecting (§5.1).
- **`cache` is allowed in any scope** (the superior): an instance reaches a
  free-variable-free scope in which a name/path is again stable (§3).
- **Small step first**: retained cells in a store, `Module::new()` per build
  unchanged. A live module with universal node-level tracking is the next segment of
  the same route, not a second design.
- Rejected: content-keyed reuse (the requirement), and the fine-grained
  `StaticModule` units of the earlier draft (their identity was a content key; the
  representation survives as §5.2's freeze-and-read-in-place, with the path as key).

## 11. Open questions

- **A path across an import boundary.** The step vocabulary is settled and landed
  (§2.2); what is not is how a package's file identity prefixes a path — the
  registry's file identity is path-derived (`is_lichen_file_id` /
  `file_id_hash(file_id: &str)`, `lichen-registry/src/device.rs:57,85`), which is a
  name, not a content hash, and must stay that way.
- **Eviction and residency policy** for cells that hold device buffers.
- **The graph-side descriptor's shape**, and whether a graph node's path is
  expressed in the same step vocabulary as a source node's.
- **Per-revision diagnostics and budget semantics** — inherited from
  `incremental-evaluation.md` §7's pending list, since recomputation is observable
  through the budgets.
