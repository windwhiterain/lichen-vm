# Incremental update: identity by path, retention by `cache`

> Status: **proposed** — the `cache` syntax is landed and **inert** (§7 step 2);
> no identity, invalidation or retention mechanism is built. This is the *cross-build* half of the
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

A step is a **name** wherever the position has one in the source — a binding name, a
lambda's parameter list position, a field name — and a **child index** only for
positions the syntax leaves anonymous (the `i`-th element of a tuple or array
literal, an argument of a `$op(…)` form).

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

**Nothing is registered when a node is allocated.** No `Path → NodeId` table exists,
and the checker's allocation path is untouched. A path is resolved **on demand** by
walking the same navigable structure name resolution already walks — the IR tree and
the scopes built over it — one step at a time, exactly as `SyntaxNodePtr` is
resolved into a node when someone needs one.

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

- **Graph edits use a descriptor.** A graph is edited *as data* — add / remove /
  rewire / re-parameterize a node — so the edit names the node paths it touched and
  the invalidation is exact. No diffing is involved.
- **Source edits use file granularity.** A source edit dirties the file; the file's
  marked cells are re-derived, and §4.3's backdating is what keeps the rest of the
  program, and the graph, from re-running. Sub-file identity is *not* needed for
  this, which is why no content key is needed either.

## 7. What changes where

| step | file / item | change |
|---|---|---|
| 1 | `lichen-highlevel/src/ir.rs` + the frontend | IR nodes carry their occurrence path (name-preferred steps) |
| 1 | resolution / checker | resolve a path on demand against the live IR + scopes; record each marked cell's read paths while it is computed |
| 2 | `lichen-language-lex` / `-parser` / `ast.rs` | **landed**: the `cache` keyword and `Binding.cached` — parsed and carried, consumed by nothing |
| 2 | `language-spec.md` §2 + `tree-sitter-lichen` (`grammar.js`, `highlights.scm`) | **landed**: the statement form, the keyword list, the highlighting |
| 3 | `lichen-registry` (the store) | cells keyed by path; freeze/read in place; the four events reported |
| 3 | `session.rs:243` (`content_key` gate), `artifact_hash` | **demoted**: they may still move bytes between processes and feed diagnostics, but they no longer decide reuse |
| 4 | the graph side (`feature/graph-jit`'s IR crate and node set) | node paths, the edit descriptor, and per-node residency — this note constrains that crate's shape |

Step 4 is the reason this design is written before that crate: its node set and
residency rules are exactly what path identity and the `cache` mark constrain.

Step 2 landed **inert rather than rejected**: the mark parses, the AST carries it,
and no consumer reads it — so the spec and `Binding.cached`'s own doc both say so,
because a user who writes `cache` must not be led to believe a value is retained.
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
  by `drop_block` does not release (`compute-graph-jit`'s landmine list). Eviction
  must release explicitly, or an agent editing for hours leaks VRAM.

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

- **The step grammar**: exactly which syntactic positions are named, and what a path
  looks like across an import boundary (the package's file identity is
  path-derived — `is_lichen_file_id` / `file_id_hash(file_id: &str)`,
  `lichen-registry/src/device.rs:57,85` — which is a name, not a content hash, and
  must stay that way).
- **Eviction and residency policy** for cells that hold device buffers.
- **The graph-side descriptor's shape**, and whether a graph node's path is
  expressed in the same step vocabulary as a source node's.
- **Per-revision diagnostics and budget semantics** — inherited from
  `incremental-evaluation.md` §7's pending list, since recomputation is observable
  through the budgets.
