# Incremental evaluation: dirty-flag propagation over the VM's mutating graph

> Status: **proposed** — nothing described here is implemented, and no
> measurement backs the cost claim in §1.3 (step 0 of §7 is the measurement).
> The evaluation counterpart of
> [incremental-parse-compile](incremental-parse-compile.md), whose `T1`–`T4`
> cover `lex → parse → resolve → lower → check`; this note asks what the *same*
> question costs one layer down, in the runtime.
>
> The frame is **incremental computation**: the checker's `Module` is a graph
> whose per-node answers (`value`, `evaluated_deep`) are computed values, a
> mutation should mark its consumers dirty, and a `dirty` bit should let the
> incremental pass skip what a full pass would recompute.  The obstruction is
> that **the graph's topology changes while it is being computed** (§3), and —
> the deeper obstruction — that one of the two computed values is a fixpoint
> whose current answer depends on traversal order (§4.4, **H1**).  §4 is the
> design; §9 is the list of things only the superior can decide.
>
> Points at: `crates/lichen-lowlevel/src/{evaluation,equality,function,gc,table,module,lib,utils,apply}.rs`,
> `crates/lichen-lowlevel/src/static_module/{freeze,apply}.rs`,
> `crates/lichen-highlevel/src/checker.rs` + `checker/lambda.rs`,
> `crates/lichen-language/src/{session,run}.rs`,
> `crates/lichen-registry` (the device store),
> and the notes [static-modules](static-modules.md), [artifact-cache](artifact-cache.md),
> [lowlevel-vm](lowlevel-vm.md), [code-audit](code-audit.md).

## 1. What is being asked, and what it costs today

### 1.1 There is no separate "evaluation" stage to make incremental

The request "let the evaluation system also rebuild incrementally" presumes an
evaluation stage that could be told apart from the compile. In this codebase
there is none, and that is the first thing the design has to absorb:

- **Checking is evaluation.** `Checker::build_with` builds the lowlevel `Module`
  and, in the same pass, deep-evaluates the canonical structures
  (`checker.rs:633-635`), every lambda's value node (`:657-659`), every
  function's return and asserts (`:703`, `:709-711`), every top-level
  statement (`:724`), and the root (`:741`), then drains the assert worklist
  (`:764-766`). The "definition pass" *is* the program running.
- **The run adds two more walks.** `render_build` deep-evaluates `root_val` and
  `root_ty` on the same module (`run.rs:59-60`).
- **`evaluate_node_deep` is also invoked from inside evaluation itself**
  (`evaluation.rs:279`, the operation postlude's parameterized gate).

So "incremental evaluation" is not a new stage bolted on after the checker: it
is the same problem as incremental checking, plus a question about what a reused
answer is allowed to be. §5 makes that precise.

### 1.2 What the existing incremental machinery already reuses

| Boundary | Unit | Mechanism | Reuses evaluation? |
|---|---|---|---|
| one buffer, unchanged resolved content | whole `Build` | `session.rs:249-266` (equal `content_key`) | yes, wholesale |
| one file across edits | whole artifact | `DeviceRegistry::verify` + `artifact_hash` ([artifact-cache](artifact-cache.md)) | yes, cross-process |
| one file ← its imports | imported package | `ExprKind::Static` → `Registry` + `StaticModule` ([static-modules](static-modules.md)) | yes, in place |

The first row is all-or-nothing: `BufferSession` reuses the established `Build`
**iff** the resolved content key is byte-identical, and otherwise re-lowers,
re-checks *and* re-evaluates everything. The other two are whole-file. So
sub-file, sub-`Build` reuse is exactly the gap, and it is the same gap `T3`
(memoized check) names — which is why this note ends up claiming the two are one
feature, not two (§6.3).

### 1.3 The cost that is not reused even *within* one build

`Node::value` is memoized per node, and the innermost layer of `evaluate_node`
returns it early (`evaluation.rs:142-144`), so an operation is not recomputed.
But `evaluate_node_deep_inner` has exactly two cuts:

- a static ref is a decided leaf (`evaluation.rs:576-578`);
- `visiting && value.is_some()` cuts a structural cycle an outer frame is
  currently computing (`:593-597`).

`evaluated_deep` — the per-node verdict the pass *writes* at `:705` — is never
read back as a reason to skip. Its readers (§3.1) use it as a fact about a node
they are already visiting. So every entry point of §1.1 re-descends the whole
value-reachable graph it covers — arrays and tables included (`:641-670`,
`:674-699`) — however much of it a previous entry point already walked.

Two consequences:

- **Entry-point count, not graph size, multiplies the work.** A build makes
  `3 + |lambdas| + |functions| + |asserts| + |statements| + 1` deep-pass calls
  (§1.1), and a run makes 2 more. A statement's term, a lambda's value node and
  that lambda's return all reach some of the same graph.
- **The walk is path-bounded, not node-bounded, on a shared graph.** Nodes are
  shared (`lowlevel-vm.md`), and nothing in the walk remembers a subtree it has
  already completed, so a diamond is walked once per path. The `visiting` mark
  only cuts cycles *during* a frame; by its own contract it is a liveness flag,
  not a "was visited" flag (`module.rs:182-198`).

This is a *within-build* incrementality that needs no new identity scheme and no
cross-build state — which is why §7 puts it first.

## 2. The state that would have to be reused, and why a `NodeId` is not an identity

Four kinds of state live in a `Module` (`lib.rs:939-1029`):

1. **Values** — `Node::value` (`lib.rs:845`), private, written only through
   `write_node_value` (`equality.rs:273-297`). A compound value's payload lives
   in a **block `Bump` arena** and its items are `Dynamic(NodeId)` refs.
2. **Concreteness verdicts** — `Node::evaluated_deep: Option<EvaluatedDeep>`
   (`lib.rs:889`): a proof about the node's **whole reachable subtree**.
3. **Structure** — `blocks` (the GC unit and the arena owner), `functions`
   (templates), the union-find `equality` classes, and the per-class `low_shape`.
4. **Diagnostic and budget state** — `unify_errors` / `eval_errors` /
   `apply_errors` / `asserts` / `assert_errors`, all append-only and never
   cleared (`lib.rs:984-1013`), plus `budget_exhausted` (`lib.rs:978-983`).

A rebuild today starts from `Module::new()` (`checker.rs:518`) and re-derives all
four. Reusing any of them across an edit runs into four structural facts:

**(a) `NodeId` is a versioned slot key, not a name.** `nodes` is a
`SlotMap<NodeId, Node>` (`lib.rs:952`) whose keys are allocated in insertion
order and whose slots are reused with a bumped version after a release. Editing
an IR prefix shifts every later `check_expr` allocation, so the same source
expression gets a different `NodeId` in the next build. (The *IR* is
append-stable — `incremental-parse-compile.md` §3 item 2 — but the `Module` is
allocated by the checker's walk, and nothing ties its ids to `ExprId`.)

**(b) The value memo is the class, not the node.** `class_value` reads through
the union-find representative (`equality.rs:81-87`), and `write_node_value`
replicates a concrete value to every unbound pure-cell member of the class
(`equality.rs:273-297`). A value transplanted without its class is unsound, and
a class transplanted without its members is unsound.

**(c) Only one of the two ref forms travels.** A value's payload holds
`AnyNodeId = Dynamic(NodeId) | Static(StaticNodeId)`. `Dynamic` is module-local
— there is no cross-module dynamic ref anywhere in the design — while `Static`
is keyed and absolute from birth (`static-modules.md`). So a value that must
outlive its module has exactly one legal representation: **frozen**.

**(d) A verdict is a semantic input, not an optimization hint.** The apply clone
walk decides bake-vs-clone from `proven_concrete` (`function.rs:318-338`): a node
the deep pass proved concrete is *referenced in place*, and a node flagged
parameterized or never walked is *cloned fresh*. Baking a node that should have
been cloned per call breaks recursion — `checker.rs:649-656` records that this
was an observed failure mode. So a reused verdict must be **true of the current
graph**, which is exactly the obligation §4 has to discharge.

## 3. The graph-mutation inventory

An incremental scheme needs to know what can change under it. This section is the
complete set of mutation sites, with each classified by what it costs a
dependency (`dependents`) index. It is the evidence §4 argues from.

### 3.1 The two computed values and the edges they read

**The value** (`value_is_parameterized`'s sibling: `evaluate_node_operation`,
`evaluation.rs:162-479`) is a function of the node's operator and:

- its **operand**'s value (`:174`, `:279`, `:309`, `:396`), when the value is not
  already cached (`:142-144`);
- the operand's payload where the operator reads one (an `Index` reads the
  operand array and then the element, `:176-230`; a `TableGet` reads the table and
  hashes the key, `:405-446`);
- the **class** it belongs to — `write_node_value`'s replication is what makes a
  later `bind` see a member's value (`equality.rs:273-297`).

**The verdict** (`value_is_parameterized`, `evaluation.rs:716-775`) is a function
of:

- **E-self** the node's own `value` variant (`:726`, `:727-746`, `:747-765`);
- **E-item** for an array value, every item's verdict and `shallow` flag
  (`:739-745`);
- **E-entry** for a table value, every entry's key and value verdict
  (`:754-764`);
- **E-operand** the operation's operand's verdict (`:766-774`).

So the verdict's dependency set is small, syntactic, and — as §3.2 shows —
almost entirely append-only. That is the good news the rest of the note is built
on.

### 3.2 Every site that changes topology or a computed value

| # | Site | What changes | Class |
|---|---|---|---|
| 1 | `module.rs:111-137` `add_node` | vertex; optional **E-operand**; `value`; `disjoint::make_set`; block membership; `observe_class_low_type` | append |
| 2 | `module.rs:261-272` `close_operation_cycle` | **E-operand added after the node exists**, and the verdict is **cleared** | **late** |
| 3 | `utils.rs:36-52` `alloc_array` / `alloc_table` | payload vertex + its item/entry refs, copied into the block arena | append |
| 4 | `checker.rs:942-954` `array_node`; `function.rs:406`; `static_module/apply.rs:209`; `table.rs:186` | the callers that mint payloads (the E-item/E-entry creation sites) | append |
| 5 | `equality.rs:60-71` `add_equality`, called at `:522,:555,:569,:599,:622,:637,:654,:909` | **class merge** — two subgraphs become one dependency set | **retroactive** |
| 6 | `equality.rs:273-297` `write_node_value` | `value` write **and** replication to every unbound pure-cell class member | append + retroactive |
| 7 | `gc.rs:154-165` `flatten_class` + `disjoint::rebuild` | class member list rebuilt, **representative re-elected** | destructive |
| 8 | `gc.rs:177-223` `drop_block` | **vertex deletion**, class splice, assert-worklist prune (`:220-221`) | destructive |
| 9 | `gc.rs:12-20`, `gc.rs:32-36` `garbage_collect_node` | `block` moves; payload re-alloc'd into the target arena | destructive (not a dataflow edge) |
| 10 | `module.rs:285-292` `register_in_function`; `:381-388` `finish_function`; `:390-422` `add_function` | ownership tag, `Function::nodes` / `parent` / `parameter` / `r#return` / `asserts` | append |
| 11 | `function.rs:339-357`, `:400-431` the apply clone walk | **new vertices**, payloads, and ownership re-stamping per apply | append (**unbounded at run time**) |
| 12 | `function.rs:158-161`, `:499`; `module.rs:340-346` `add_assert` | assert worklist entries, re-registered per apply | append |
| 13 | `static_module.rs:142-153` `materialize_leaf` / `as_dynamic` | new vertex holding a static value | append |
| 14 | `evaluation.rs:705` / `module.rs:271` | verdict write / verdict clear | append |
| 15 | `equality.rs:123-185` `seed`/`refine`/`observe_class_low_type`; `static_module.rs:132-136` `set_node_shape` | the per-class `low_shape` lattice | monotone refinements |

Two answers this inventory settles:

- **Payload edges are immutable once allocated.** `alloc_array` / `alloc_table`
  take `&[T]` and copy into the block's `Bump`; `items()` returns
  `&'static [ArrayItem]` / `&'static [TableItem]` (`lib.rs:331`, `:353`) and
  there is **no `&mut` accessor anywhere** in the crate. GC copies a whole
  payload verbatim into the target arena (`gc.rs:73-75`, `:105-107`); it edits
  neither the item list nor the flags. So E-item/E-entry are created exactly once
  and never change — reverse edges for them can be built at allocation and never
  maintained.
- **The only late edge is E-operand.** Row 2 is the one place a node's dependency
  set grows after the node was created, and it is already coupled to an explicit
  invalidation (`evaluated_deep = None`, `module.rs:271`). It is the hook the
  design needs, already written.

### 3.3 What each mutation class costs a dependency index

- **append** (rows 1, 3, 4, 10, 11, 12, 13, 14) — free. A new vertex starts dirty
  (its verdict is `None`, which every reader already handles); a new edge's
  reverse entry is written where the edge is.
- **late** (row 2) — one line: record the reverse edge and dirty the node **and
  its dependents**. This is the only site that needs to walk *upwards*, so it is
  the only site that needs the index to exist.
- **retroactive** (rows 5, 6) — a class merge makes two subgraphs affect each
  other. §4.3 shows this needs **no index at all**.
- **destructive** (rows 7, 8, 9) — `drop_block`'s own contract is that no live
  node outside the block references one inside it (`gc.rs:170-171`), so nothing
  outside can be dirty because of the removal: the index needs *cleanup*, not
  propagation. Row 7 constrains the design rather than costing it: the index must
  be keyed on **nodes**, never on representatives, or every class rebuild would
  have to re-key it.

## 4. Incremental computation over the DAG

### 4.1 The mechanism

Add two things to `Node`:

- `dirty: bool` — "the cached verdict may not describe the current graph";
- `dependents: Vec<NodeId>` — the reverse of E-item, E-entry and E-operand.

and one rule to the pass: given a verdict that is not dirty, do not descend.
`visiting` keeps its contract untouched (`module.rs:182-198`) — a dirty bit is
not a liveness mark, and the two must not be conflated, exactly as the audit
already separates them (`code-audit.md:2602-2608`).

The propagation primitives are then three, and each has a place to live:

1. `dirty(node)` — set `dirty` and enqueue; the *transitive* upward walk over
   `dependents` happens on the reverse edges.
2. `link(parent, child)` — record the reverse edge; called at `add_node` (row 1),
   at each payload allocation (rows 3-4, one `link` per item), and at
   `close_operation_cycle` (row 2, which also dirties its dependents).
3. `unlink(node)` — called from `drop_block` (row 8) for the removed nodes.

### 4.2 The hooks are two, and one of them already exists

- `close_operation_cycle` (`module.rs:261-272`) already clears the verdict for
  exactly the reason this design needs ("the proof predates this operand edge and
  no longer describes the node's graph", `:256-260`). It gains `dirty` +
  propagation to `dependents` + the reverse edge.
- `write_node_value` (`equality.rs:273-297`) is where a value change propagates.
  It **already walks the class member list** (`:286-294`), so "mark each member's
  dependents dirty" is a flag set inside an existing loop, not a new traversal.

Everything else is either free (append) or cleanup (destructive), per §3.3.

### 4.3 The class hyperedge needs no index — the key simplification

A class merge looked like the killer: `add_equality` splices two member lists,
so a write to a node can affect members that were strangers a moment ago, and the
class is an unbounded-arity hyperedge that is rebuilt at GC with a re-elected
representative.

It costs nothing here, for a structural reason:

- **The dependency set of a value write is already materialized.** It is the
  class member list, and `write_node_value` walks all of it. So propagation does
  not need a reverse *class* edge — it needs the members, which the walk is
  already visiting.
- **A merge changes no `value` and no verdict by itself.** `add_equality` moves
  links and joins the two classes' `low_shape` (`equality.rs:60-71`, row 15's
  separate monotone channel); it writes no `value` and no `evaluated_deep`, so
  nothing becomes stale at merge time. The staleness appears at the *next* write,
  and that write's walk covers both former classes.
- **The index is node-keyed**, so the GC class rebuild (`gc.rs:154-165`,
  `flatten_class` + `disjoint::rebuild`) does not touch it. Had the index been
  keyed on the representative, every rebuild would need a re-key pass.

So the "retroactive" class is retroactive for the *existing* code too, and the
existing code already handles it at the one place that matters.

### 4.4 The three things that are actually hard

**H1 — the verdict is a fixpoint whose current answer depends on traversal
order.** `Option<EvaluatedDeep>` is read three different ways in the crate:

| read | `None` means | sites |
|---|---|---|
| `is_none_or(\|e\| e.parameterized)` | parameterized | `evaluation.rs:286`, `freeze.rs:73` |
| `is_some_and(\|e\| e.parameterized)` | **not** parameterized (i.e. concrete) | `evaluation.rs:742`, `:756`, `:761`, `:772`, `table.rs:219` |
| `is_some_and(\|e\| !e.parameterized)` | not proven concrete | `function.rs:325` |

The documented contract is the first row: "`None` means concreteness is
*unknown*, which a reader must treat as parameterized, **never** as proven
concrete" (`module.rs:200-213`, `lib.rs:882-889`). The second row is the
array/table/operand arm of the verdict's own definition, and it reads the
opposite. During a normal walk the descent precedes the verdict
(`evaluation.rs:641-670` before `:704`), so a non-shallow dynamic item *does*
have a verdict — **except** when the child returned early at the structural-cycle
cut (`:593-597`) or the walk refused on budget before writing (`:599-625`).
Reading `None` as concrete at those positions is what lets a cyclic value be
proven concrete at all (the coinductive "assume concrete until disproven" step);
reading it as parameterized there would make every cycle unprovable. The code
compensates elsewhere by proving the canonical cycles first, in a fixed order
(`checker.rs:633-635`) and by sorting the definition pass by `ExprId`
(`:685-690`, documented as user-visible order).

For the design this is not a detail, it is the precondition: **if the verdict is
a function of where the walk started, then a dirty-flag recomputation — which
restarts somewhere else — can disagree with a full walk.** Either the verdict
becomes an explicit greatest fixpoint over each strongly-connected component
(recompute a whole SCC atomically, in a fixed order), or the DFS order is
declared part of the contract and the incremental pass must reproduce it. **Q6**
is this decision; §8 lists it as the first falsifier.

**H2 — the apply clone walk is a materializing consumer, not a dataflow node.**
`function.rs:318-338` reads `proven_concrete` and then *builds new topology*: it
bakes the proven node in place or clones it into a fresh node. That decision
cannot be un-done by a later dirty flag — the clones exist. Today the ordering
avoids the hazard: lambda value nodes are proven at `checker.rs:657-659`, before
the definition pass runs applies at `:691-712`. An incremental scheme must either
keep that ordering guarantee (verdicts of the template's members are settled
before any apply consumes them) or treat "the clone set of one apply" as an
atomic unit that is recomputed wholesale when any source verdict inside its
template is dirtied. This is the one place where "the graph is incrementally
updated" is false, and it is a *consumer* fact, not an edge fact — the inventory
of §3 cannot see it, because §3 lists what changes and this is about who reads.

**H3 — recomputation is observable, not merely cheaper.** `budget_exhausted` is
sticky per run "because it is *the* record of why the last walk stopped"
(`lib.rs:978-983`), `apply_total` is cumulative (`apply.rs:31-32`), and
`checker.rs:718-751` turns a refusal into a `NonTerminating` diagnostic. Skipping
a recomputation a full walk would have performed changes **whether the budget is
exhausted**, hence the diagnostics. A differential harness (§7 step 3) has to
compare diagnostics, not only values. `record_extension_diagnostic`'s dedup
(`module.rs:311-330`) already assumes a node is deep-evaluated several times —
fewer passes is fine there, but each append-only vec has to be audited for the
same "recorded per attempt" character.

### 4.5 Push or pull, and what it costs

**Pull, with a dirty bit and a lazily consumed dirty set** — the recommendation.
A verdict is recomputed only when the pass actually reaches the node, which keeps
the laziness, the budget accounting (the walk still does the work a full walk
would, minus clean subtrees) and the `visiting` cycle cut intact. A mutation
marks; the pass discovers.

Rejected: a **global revision counter alone** (any write invalidates every
verdict, so within a build it degrades to today's behaviour); **eager push in
topological order** (needs an SCC-ordered worklist, contradicts the lazy
structure and the budget guards, and cannot be reconciled with `visiting`'s
frame-scoped lifetime); **no reverse index, relying on dirty-up during the pass**
(a node cannot name its parents, which is precisely what the index is for).

Cost: `dependents` is `O(E)` memory over the payload and operand edges — the same
order as the module's own node/payload storage, but a second graph, in a runtime
whose performance items (`P4-2`, `P4-6`) are about removing per-visit work. A
cheaper shape exists if it proves too heavy: store no lists and rebuild the
reverse edges for a *region* when that region is first dirtied (a repair pass),
trading memory for a scan. That is a measurement, not a design, decision.

### 4.6 Relationship to the earlier "cut" framing

The previous framing asked whether `evaluated_deep` can be used as a *cut*
("stop descending when a subtree is already proven"). Dirty propagation
**subsumes** it: the cut is the degenerate case `dirty == false`, and the
question it failed to ask is the load-bearing one — *what invalidates a verdict*.
With §3's inventory and §4.3's simplification that question has an answer; with
H1 it also has a precondition.

## 5. The three reuse requirements

For the cross-build half (§6) the same three obligations apply, and stating them
once is what keeps the design space from looking like four unrelated ideas:

- **R1 — identity.** A name for an evaluation unit that is *position-independent*
  and *stable across a rebuild*. `NodeId` fails §2(a); `BlockId` fails (f) below;
  `ExprId` is append-stable in the IR but names an *expression*, not an
  evaluation unit (a pair is several nodes plus shared markers plus operand
  arrays).
- **R2 — invalidation.** A reverse dependency relation (§3, §4) or a content key
  over the unit's inputs. The lowlevel has neither today: `node_edges`
  (`checker.rs:250`) maps a node to a `Loc` for *diagnostics*, and there is no
  dependents relation anywhere in the crate.
- **R3 — representation.** How a reused answer crosses a boundary, given §2(c):
  either the module is kept alive and resumed (no boundary), or the answer is
  frozen to a `StaticNodeId` and read in place.

**(f) `BlockId`s are not per-statement.** The checker creates one root block
(`checker.rs:593`) and one block per lambda (`checker/lambda.rs:31`) — nothing per
top-level statement. So "the unit to drop and redo" has no existing boundary to
key on below a whole function.

## 6. The cross-build design space

### 6.1 M1 — a content-keyed value memo ("salsa at the IR level")

Key each allocation by a serialization of its inputs (§3.1's edge set is the
input set), and on a rebuild look the key up in a cache of previous
`(value, verdict, class, low_shape)`.

The identity half already exists: `resolve/content_key.rs` is an exact,
digest-free, injective serialization of the *lowering-visible* resolved content
(names by `BinderId`, error blocks opaque, spans dropped, version-stamped at
`content_key.rs:13`). It is the right model for R1, and it is already the
`BufferSession` reuse key.

The obstruction is R2 at value granularity:

- **A syntax key is not sufficient.** A value is a function of the unification
  history, not of syntax alone. The code claims this is *order*-independent
  ("reads alias their target cells … so bindings propagate class-wise however
  they happen", `checker.rs:665-667`), so `value = f(syntax, values of the
  binders reached)` may well hold — but H1 shows the *verdict* is not
  order-independent, and a `Parameterized` value is deliberately left uncached so
  a later pass decides it (`evaluation.rs:463-467`).
- **The canonical value hash that exists is deliberately non-injective.** The
  table key machinery (`table.rs:1-70`) is a canonical content unfolding that
  survives a freeze and mixes in no address or node identity — the right shape for
  a value memo. But its own doc states it is a **pre-filter** whose authority is
  `key_eq` (`table.rs:9-14`) and enumerates what it cannot tell apart
  (`:51-61`), including *which operator sits at a position* and anything below
  `UNFOLD_DEPTH`. A cache key must be authoritative: serving the wrong value on a
  collision is a wrong answer, not a lost candidate. So M1 needs `key_eq` on every
  hit (a full comparison, giving back much of the win) or a *new* injective
  value-digest contract (**Q4**).

M1 also does not escape R3: a memoized value holds `Dynamic` refs into the old
module, so either the old module stays alive (an arena pinning scheme the runtime
does not have) or the memo stores frozen state — which is M3.

### 6.2 M2 — same-module resume across edits

Keep the `Module` across edits: re-lower only the changed unit, invalidate its
dependents (the §4 index is exactly this), keep everything else. This is the only
route that answers R3 with "no boundary" — the state never leaves the module, so
`Dynamic` refs stay valid and no payload is copied.

It pays with state the module cannot currently undo:

- **Rollback.** `add_equality` merges destructively (`equality.rs:60-71`),
  `write_node_value` replicates destructively, `close_operation_cycle` defines an
  operation once. Removing a unit means removing every write it caused —
  including writes into *other units' classes*. `T4` ("unification-state
  checkpoint/rollback", `incremental-parse-compile.md` §5) is exactly this.
- **Append-only side tables.** The four diagnostic vecs are never-cleared and
  some keep a companion set that must not drift (`lib.rs:984-1013`); a resume
  needs a per-unit extent for each.
- **Budgets.** A resumed module must decide what a "run" is now
  (`reset_apply_budget`, `module.rs:90-95`) — see H3.
- **No unit boundary to key on** (§5(f)).

### 6.3 M3 — fine-grained frozen units (recommended for the cross-build half)

The one mechanism whose R3 answer is already built and in production for the
cross-file case: freeze solved state into a `StaticModule` and read it in place.

`StaticModule` is precisely "an evaluation unit's reusable result":

- `Module::static_read` returns the solved value verbatim, no copy
  (`static_module.rs:96-102`);
- `StaticNode` carries the solved verdict (`parameterized`, `lib.rs:913`, copied
  at `freeze.rs:71-73`), so §2(d)'s bake-vs-clone decision is preserved;
- refs are absolute from birth, so §2(c) is answered;
- GC keeps static handles verbatim (`static-modules.md`), and the deep pass
  treats a static ref as a decided leaf (`evaluation.rs:576-578`) — so a reused
  unit costs **no re-walk and no re-evaluation**, and its verdict is trivially
  clean under §4 (an immutable module cannot be dirtied, which is the cleanest
  statement of why freezing is the right boundary);
- identity is content-addressed and transitive (`artifact_hash`, three agreeing
  sites in [artifact-cache](artifact-cache.md)), so R2's "propagate a change"
  becomes "the key changed".

Costs, stated:

- **Freeze is whole-module.** `from_module_mapped` freezes the *entire* module
  and returns a `NodeId → LocalNodeId` map (`freeze.rs:40-75`). Sub-file
  granularity means either a **partial freeze** (a new entry point into an
  existing walk) or making each unit **its own `Module`** — the shape the package
  store already uses per file, applied within a file.
- **The source must be fully solved** (`freeze.rs:23-24`): every node holds its
  final answer or a residual `Parameterized`. So freeze points must be chosen
  after the definition pass — which is *inside* the checker.
- **Payload copy.** Phase 2 dedupes and lays payloads into a flat arena
  (`freeze.rs:92-150`); more, smaller units means more copies and a larger key
  space.
- **Class sharing is lost across the boundary.** A static leaf unifies by
  materializing into a fresh dynamic leaf (`static_module.rs:142-153`) — exactly
  how imports already behave. Whether that is observable for a *top-level binding
  of the edited file* (a polymorphic one) is **Q2**.
- **M3 does not remove the need for `T3`.** To *skip* re-lowering an unchanged
  unit the checker must resume from a scope whose entries are static refs. M3
  supplies the representation; the resume is `T3`'s work. **Incremental
  evaluation is incremental checking plus a representation choice, not a second
  feature.**

### 6.4 M4 — do nothing below the file

The artifact store already rebuilds only the changed chain, and the LSP already
routes settled imports through it ([liche-lsp-home](liche-lsp-home.md)). If
measurement (§7 step 0) shows the deep pass is not where the budget goes, M4 is
already the answer. Listed because the note has no measurement, and because
`code-audit.md`'s `D6` shows the project's habit of letting a measurement decide.

## 7. Recommendation and staged roadmap

0. **Measure first — no design without it.** Instrument the deep pass: entry
   points per build, nodes visited per entry point, distinct nodes reached, and
   the share of the keystroke budget that is (deep pass) vs (check) vs
   (lex+parse). `code-audit.md:3966-3970` shows the project's own probe style (a
   temporary counter, removed after). §1.3 is a *reading of the code*.
1. **Settle the verdict's contract (H1 / Q6) before touching the pass.** Make
   `parameterized` an explicit function of the graph — an SCC-atomic greatest
   fixpoint — or declare the DFS order part of the contract. Without this,
   "incremental == full" is not provable and step 3 has no oracle.
2. **The reverse index and the two hooks** (§4.1-4.2): `dirty` + `dependents`,
   `link` at `add_node` and at each payload allocation, propagation in
   `close_operation_cycle` and `write_node_value`, cleanup in `drop_block`.
3. **Make the pass consult `dirty`**, and prove it with a **differential
   harness**: run the incremental and the full pass over the same corpus and
   compare every verdict, every value, and every diagnostic (H3). The existing
   test suites are the corpus; the only new test is the comparison.
4. **Then the cross-build half** — M3 at the granularity Q3 selects, plus `T3`
   resume.
5. **Then, optionally, M1's value memo** — only if step 0 shows re-lowering (not
   re-evaluating) dominates after steps 1-4, and only once Q4's authoritative
   digest exists.

`T4` (same-module resume with unification rollback, §6.2) stays the boundary:
the only route that avoids freezing, at the cost of the union-find's contract
plus the module's side tables. Nothing here needs it — note that step 2's index
is the *within-build* dependency relation, which T4 would also need, so step 2 is
not wasted if T4 is ever taken.

## 8. What would falsify the plan

- **H1 cannot be settled.** If the verdict stays a function of traversal order,
  incremental and full walks can legitimately disagree, step 3 has no oracle, and
  only the cross-build half (§7 step 4) survives.
- **Step 0 shows the deep pass is negligible.** Then §6.4 is the whole finding
  and the note reduces to a documentation change: `T3` is the evaluation
  incrementality item.
- **The `dependents` graph costs more than the re-walk it saves** (§4.5) on a
  real program's edge count. Then the region-repair variant, or nothing.
- **Q2 shows a static leaf changes observable behaviour for a polymorphic
  binding.** Then M3 needs a class-sharing story (i.e. M2), and the cheap
  cross-build mechanism is gone.
- **Freeze cost exceeds the re-evaluation it saves.** Then unit granularity must
  be coarser than a binding, and §6.3's win shrinks toward M4.

## 9. Open questions for the superior

- **Q6 — What is the verdict's contract? (new, and the precondition for
  everything else.)** `Option<EvaluatedDeep>` is read three ways (§4.4 H1); the
  documented contract is "`None` ⇒ parameterized, never proven concrete"
  (`module.rs:200-213`), while the verdict's own array/table/operand arms read
  `None` as concrete (`evaluation.rs:742`, `:756`, `:761`, `:772`,
  `table.rs:219`). Is that the deliberate coinductive step that makes cyclic
  values provable (so the *documentation* is what needs fixing), or a
  conservatism hole (so the *reader* is)? Either answer is actionable — but the
  design in §4 needs to know whether `parameterized` is a function of the graph
  or of the walk before it can be maintained incrementally.
- **Q1 — Is the reverse index acceptable at `O(E)` memory?** §4.5's cheapest
  sound shape stores a `dependents` list per node, in a runtime whose open
  performance items are about removing per-visit work (`P4-2`, `P4-6`). The
  alternative (no lists, rebuild a region's reverse edges when it is first
  dirtied) trades memory for a scan; which does the project prefer?
- **Q2 — Does a reused binding as a static leaf preserve semantics?** A static
  leaf materializes into a fresh dynamic leaf and shares no class with the new
  build (`static_module.rs:142-153`). Imports live with this today. Does a
  *top-level binding of the edited file* — in particular a polymorphic one —
  survive it, or must reuse keep the class alive (i.e. M2)?
- **Q3 — Which unit granularity?** Top-level binding / statement, a lambda body,
  or an arbitrary sub-expression. This decides whether `BlockId` (one per lambda
  today) can carry R1 or a new per-unit id is needed.
- **Q4 — Is an authoritative value digest needed, and may it be a new
  contract?** The existing canonical unfolding is documented as a pre-filter
  (`table.rs:9-14`, `:51-61`). Either M1 verifies every hit with `key_eq`, or the
  project takes on a second, injective digest with its own format-version
  discipline like `content_key`'s. Which?
- **Q5 — Which consumer is the target?** The CLI pays one build per file; the
  editor pays per keystroke but currently has *no* consumer for `BufferSession`
  (`P2-1`), so an incremental evaluator wired to a session nothing runs would
  repeat that. If the editor is the target, §7 steps 4-5 are `D6`'s `(b)`
  restated with evaluation included, and should be sequenced there.

## 10. Where the changes would land (if the plan is taken)

| Step | File / function | Change |
|---|---|---|
| 0 | `crates/lichen-lowlevel/src/evaluation.rs`, a temporary probe | count entry points, visits, distinct nodes; removed after measurement |
| 1 | `evaluation.rs` `value_is_parameterized` (+ the §4.4 H1 readers) | an SCC-atomic or order-documented verdict function |
| 2 | `lib.rs` `Node`, `module.rs` `add_node`/`drop_block`, `equality.rs` `write_node_value`, `utils.rs` `alloc_*` | `dirty` + `dependents`; `link`/`unlink`/`dirty` |
| 2 | `module.rs` `close_operation_cycle` | the one late edge: reverse link + propagate to dependents |
| 3 | `evaluation.rs` `evaluate_node_deep_inner` | consult `dirty` instead of unconditional descent |
| 3 | `crates/lichen-lowlevel/tests/` (new file) | the differential harness (incremental vs full: verdicts, values, diagnostics) |
| 4 | `static_module/freeze.rs` | a sub-graph freeze entry point (or a per-unit `Module` driven from the checker) |
| 4 | `crates/lichen-highlevel/src/checker.rs` | resume from a scope whose entries are static refs for reused binders |
| 4 | `crates/lichen-language/src/session.rs` + `crates/lichen-registry` | the reuse gate per unit instead of per whole `Build`; identity/slot discipline for many small units |
| 5 | `crates/lichen-lowlevel/src/table.rs` | an authoritative digest, or a `key_eq`-verified memo |

## 11. Recorded discussion

- The request was "make the evaluation system also able to rebuild
  incrementally". The first reframing: there is no separable evaluation stage
  (§1.1), so the object is the `Module` the checker builds and evaluates, and the
  sub-`Build` granularity it lacks is the one `T3` names.
- The second reframing — the superior's, and it changed the mechanism: this is
  **not** a "can we cut on the cached verdict" question but an
  **incremental-computation** question, "a DAG that keeps being incrementally
  updated, with dirty-flag propagation — the problem being that the topology
  changes". Accepted. Under it, §3's inventory replaces "can we cut" and §4.3
  answers the mutation class that looked fatal.
- Rejected as the primary within-build mechanism: **cut-on-verdict** — subsumed
  by dirty propagation (§4.6), and it never asked what invalidates a verdict.
- Rejected as the primary cross-build mechanism: **a syntax-keyed value memo
  alone** — a value is a function of the unification history, not of syntax
  (§6.1), and the only canonical value hash in the tree is documented
  non-injective.
- Rejected as the primary cross-build mechanism: **same-module resume** — sound
  in principle and the only route that avoids freezing, but it is `T4`'s
  unification rollback plus a new per-unit boundary plus the append-only side
  tables (§6.2). Kept as the documented boundary; its dependency index is step 2.
- Selected: **dirty-flag propagation over the maintained dependency index** for
  the within-build half, behind a measurement and behind Q6; **fine-grained
  frozen units** for the cross-build half, behind `T3`'s resume.
- Left open, and flagged as the subtlest point: **H1** — the verdict's three
  readings of `None` and the traversal-order dependence they create.
