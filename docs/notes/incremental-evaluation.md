# Incremental evaluation within one build: the settled cut

> Status: **proposed, and in scope correction** — step 0 is measured (below); the
> mechanism in §4.1 is not built yet, and §4.2/§4.3/§5/§7 are to be rewritten.
> The evaluation counterpart of
> [incremental-parse-compile](incremental-parse-compile.md), whose `T1`–`T4`
> cover `lex → parse → resolve → lower → check`; this note asks what the *same*
> question costs one layer down, in the runtime.
>
> **The requirement this note must serve is *external mutation*:** a host may
> write any node's value at any moment and expects the consequences to be
> re-derived incrementally.  That does not retire §4.1's `settled` — a host cannot
> interleave with a pass that holds `&mut Module`, so `settled` is still final
> *within* a pass — but it does retire §4.2's judgement that the reverse index is
> unnecessary: across calls, `Module::write_node_value` (`equality.rs:273`) is a
> public write that is unconditional on the node it names (only the *replication
> targets* are filtered by `is_unbound`), so an edit invalidates a settled verdict
> and invalidation must be **tracked**, not excluded.  §1.3's measurement, §3's
> mutation inventory and §3.3's three predicates stand as they are; the rewrite
> depends on four decisions (edit semantics for already-materialized clones,
> per-revision diagnostics, budget semantics, and cyclic fixpoints) that are
> recorded in the session and not yet taken.
>
> **Scope, decided by the superior: no cross-rebuild reuse.**  Nothing here keys a
> sub-expression or reuses a solved unit in a later build; those questions (and the
> fine-grained `StaticModule` machinery they needed) are withdrawn rather than
> deferred, and there is no `T5`.  What is
> left is the *within-build* half: one build makes many deep-pass entry points,
> each re-descending subtrees its predecessors already decided (§1.3), and the
> question is what such a pass may safely skip.
>
> The frame is still **incremental computation with a `dirty` flag**, as the
> superior framed it — but in this scope the flag turns out to be a *stability*
> property rather than a tracked invalidation (§4.2): a verdict that cannot be
> written into needs no reverse index to protect it, and the writes that could
> invalidate one are exactly the ones such a subtree cannot contain.
>
> Points at: `crates/lichen-lowlevel/src/{evaluation,equality,function,gc,table,module,lib,utils,apply}.rs`,
> `crates/lichen-highlevel/src/checker.rs` + `checker/lambda.rs`,
> `crates/lichen-language/src/run.rs`,
> and the notes [lowlevel-vm](lowlevel-vm.md), [static-modules](static-modules.md),
> [code-audit](code-audit.md) (`P1-31`, done).

## 1. What is being asked, and what it costs today

### 1.1 There is no separate "evaluation" stage to make incremental

The request presumes an evaluation stage that could be told apart from the
compile. In this codebase there is none:

- **Checking is evaluation.** `Checker::build_with` builds the lowlevel `Module`
  and, in the same pass, deep-evaluates the canonical structures
  (`checker.rs:633-635`), every lambda's value node (`:657-659`), every
  function's return and asserts (`:703`, `:709-711`), every top-level statement
  (`:724`), and the root (`:741`), then drains the assert worklist
  (`:764-766`). The "definition pass" *is* the program running.
- **The run adds two more walks** on the same module (`run.rs:59-60`).
- **`evaluate_node_deep` is also invoked from inside evaluation itself**
  (`evaluation.rs:279`, the operation postlude's parameterized gate).

So the object is the `Module` the checker builds and evaluates, and the passes it
runs over it.

### 1.2 What is already reused

| Boundary | Unit | Mechanism |
|---|---|---|
| one buffer, unchanged resolved content | whole `Build` | `session.rs:249-266` (equal `content_key`) |
| one file across edits | whole artifact | `DeviceRegistry::verify` + `artifact_hash` ([artifact-cache](artifact-cache.md)) |
| one file ← its imports | imported package | `ExprKind::Static` → `Registry` + `StaticModule` ([static-modules](static-modules.md)) |

All three are whole-`Build` or whole-file. The rows are listed only to fix the
scope: they stay as they are, and this note does not touch them.

### 1.3 The cost inside one build: the deep pass memoizes nothing across entry points

`Node::value` is memoized per node (`evaluation.rs:142-144`), so an operation is
not recomputed. But `evaluate_node_deep_inner` has exactly two cuts — a static
ref is a decided leaf (`:576-578`), and `visiting && value.is_some()` cuts a
structural cycle an outer frame is currently computing (`:593-597`) — and
`evaluated_deep`, which it *writes* at `:705`, is never read back as a reason to
skip. Every entry point therefore re-descends the whole value-reachable graph it
covers, arrays and tables included (`:641-670`, `:674-699`).

**Measured · `cargo run -p lichen-language --example deep_pass_stats`, temporary and since removed.**  The
readings were: *stamped* is the nodes one
build left a verdict on, *cheap* is the visits that returned without evaluating a
node (a static leaf or a cycle cut), and *revisit* is the visits that reached a
node **already carrying a verdict** — the headroom any cut has:

| program | walks | visits | cheap | revisit | nodes | stamped | real / stamped |
|---|---|---|---|---|---|---|---|
| `struct_recursion` | 8 | 326 | 57 | **236 (72.4%)** | 56 | 33 | **8.15** |
| `closure (nested)` | 10 | 126 | 18 | **71 (56.3%)** | 118 | 37 | **2.92** |
| `assert` | 7 | 45 | 7 | 21 (46.7%) | 28 | 17 | **2.24** |
| `let_polymorphism` | 7 | 59 | 8 | 26 (44.1%) | 51 | 25 | **2.04** |
| `recursion (fib 10)` | 991 | 3 721 | 9 | 1 826 (49.1%) | 6 103 | 1 886 | **1.97** |
| `table (deep keys)` | 9 | 81 | 10 | 28 (34.6%) | 92 | 43 | **1.65** |

Two readings:

- The redundancy is **real work, not early exits**: the cheap returns are 2–18%
  of visits, and roughly *half* of all visits land on a node that already has a
  verdict.
- The worst case is the canonical cyclic shapes. A 56-node module is walked 326
  times because each of its 8 entry points re-descends the same universe-shaped
  structures — and §4.3 says those are exactly what a cut *cannot* take.

What is still unmeasured: the pass's share of a build's **wall-clock**. These are
counters, not a profile, and §5 step 0 keeps that open.

## 2. Why the module stays in memory (and a `NodeId` is not an identity)

Four kinds of state live in a `Module` (`lib.rs:939-1029`):

1. **Values** — `Node::value` (`lib.rs:845`), private, written only through
   `write_node_value` (`equality.rs:273-297`). A compound value's payload lives in
   a block `Bump` arena and its items are `Dynamic(NodeId)` refs.
2. **Concreteness verdicts** — `Node::evaluated_deep: Option<EvaluatedDeep>`
   (`lib.rs:889`): a proof about the node's whole reachable subtree, with the
   in-progress case named separately since `P1-31`.
3. **Structure** — `blocks` (the GC unit and the arena owner), `functions`
   (templates), the union-find `equality` classes, the per-class `low_shape`.
4. **Diagnostic and budget state** — the append-only error vecs
   (`lib.rs:984-1013`) and the sticky `budget_exhausted` (`lib.rs:978-983`).

Two facts make the in-memory scope the cheap one:

- **A value's payload holds `Dynamic(NodeId)` refs**, which are module-local —
  there is no cross-module dynamic ref anywhere in the design. Anything that
  outlives its module must be *frozen* to `StaticNodeId`s first, which is what
  §1.2's whole-file rows do and what this note now declines to do per
  sub-expression.
- **The value memo is the class, not the node** (`class_value` reads through the
  union-find representative, `equality.rs:81-87`; `write_node_value` replicates
  to the class's unbound pure cells). A reused answer would have to carry its
  class, which is exactly what an in-memory scope gets for free.

And one fact is a semantic input rather than a hint: the apply clone walk decides
bake-vs-clone from `proven_concrete` (`function.rs:318-338`), and baking a node
that should have been cloned per call breaks recursion (`checker.rs:649-656`
records that as an observed failure). A reused verdict must be true of the
current graph — §4 is about when it provably is.

## 3. The graph-mutation inventory

The evidence §4 argues from: every site that changes topology or a computed
value, classified by what it costs an incremental scheme.

### 3.1 The two computed values and the edges they read

**The value** (`evaluate_node_operation`, `evaluation.rs:162-479`) is a function
of the node's operator and its **operand**'s value (`:174`, `:279`, `:309`,
`:396`) when the value is not cached, the operand's payload where the operator
reads one (`Index` `:176-230`, `TableGet` `:405-446`), and the **class** it
belongs to (`equality.rs:273-297`).

**The verdict** (`value_is_parameterized`, `evaluation.rs:716-775`) is a function
of the node's own `value` variant, every array item's verdict and `shallow` flag
(`:739-745`), every table entry's key and value verdict (`:754-764`), and the
operand's verdict (`:766-774`).

### 3.2 Every mutation site

| # | Site | What changes | Class |
|---|---|---|---|
| 1 | `module.rs:111-137` `add_node` | vertex; optional **E-operand**; `value`; `disjoint::make_set`; block membership; `observe_class_low_type` | append |
| 2 | `module.rs:261-272` `close_operation_cycle` | **E-operand added after the node exists**, and the verdict is **cleared** | **late** |
| 3 | `utils.rs:36-52` `alloc_array` / `alloc_table` | payload vertex + its item/entry refs, copied into the block arena | append |
| 4 | `checker.rs:942-954` `array_node`; `function.rs:406`; `static_module/apply.rs:209`; `table.rs:186` | the callers that mint payloads (the E-item/E-entry creation sites) | append |
| 5 | `equality.rs:60-71` `add_equality`, called at `:522,:555,:569,:599,:622,:637,:654,:909` | **class merge** — two subgraphs become one dependency set | retroactive |
| 6 | `equality.rs:273-297` `write_node_value` | `value` write **and** replication to every unbound pure-cell class member | append + retroactive |
| 7 | `gc.rs:154-165` `flatten_class` + `disjoint::rebuild` | class member list rebuilt, **representative re-elected** | destructive |
| 8 | `gc.rs:177-223` `drop_block` | **vertex deletion**, class splice, assert-worklist prune | destructive |
| 9 | `gc.rs:12-20`, `gc.rs:32-36` `garbage_collect_node` | `block` moves; payload re-alloc'd into the target arena | destructive (not a dataflow edge) |
| 10 | `module.rs:285-292` `register_in_function`; `:381-388` `finish_function`; `:390-422` `add_function` | ownership tag, `Function::nodes` / `parent` / `parameter` / `r#return` / `asserts` | append |
| 11 | `function.rs:339-357`, `:400-431` the apply clone walk | **new vertices**, payloads, ownership re-stamping per apply | append (**unbounded at run time**) |
| 12 | `function.rs:158-161`, `:499`; `module.rs:340-346` `add_assert` | assert worklist entries, re-registered per apply | append |
| 13 | `static_module.rs:142-153` `materialize_leaf` / `as_dynamic` | new vertex holding a static value | append |
| 14 | `evaluation.rs` verdict write / `module.rs:271` verdict clear | the verdict, and (since `P1-31`) the in-progress mark | append |
| 15 | `equality.rs:123-185` `seed`/`refine`/`observe_class_low_type`; `static_module.rs:132-136` `set_node_shape` | the per-class `low_shape` lattice | monotone refinements |

Two answers this inventory settles:

- **Payload edges are immutable once allocated.** `alloc_array` / `alloc_table`
  take `&[T]` and copy into the block's `Bump`; `items()` returns
  `&'static [ArrayItem]` / `&'static [TableItem]` (`lib.rs:331`, `:353`) and there
  is **no `&mut` accessor anywhere** in the crate. GC copies a whole payload
  verbatim into the target arena (`gc.rs:73-75`, `:105-107`) and edits neither the
  item list nor the flags. E-item/E-entry are created exactly once.
- **The only late edge is E-operand** (row 2), and it already clears the verdict
  when it appears.

### 3.3 The three code facts that make a stability property checkable

`is_unbound(value)` is `None` or `Parameterized` (`lib.rs:550`); everything else
is a concrete value. Three predicates then say who can write what:

- **A concrete value is not overwritten.** `bind` takes
  `concrete = if is_unbound(va) { vb } else { va }` (`equality.rs:653`) and
  replicates only `!is_unbound` values (`:661`); `write_node_value`'s replication
  targets only `is_unbound(value)` members (`:289`).
- **A node with a cached value is not pending.** `pending_op`
  (`equality.rs:853-856`) and `class_has_pending_op` (`:669-672`) select members
  with `operation.is_some() && is_unbound(value)`, which is exactly what
  `force_pending` (`:963-966`) re-runs — so a node holding a concrete value is
  never re-derived by unification.
- **A node with a value cannot gain an operand edge.** `close_operation_cycle`
  asserts the node is unbound and never evaluated (`module.rs:261-272`).

## 4. The design: a settled cut, with no index and no dirty tracking

### 4.1 The property

Give the verdict a second bit. `EvaluatedDeep { parameterized, settled }` where
**`settled` means: this node's value is concrete, no position it read was
in-progress (the `P1-31` assumption), every value-reachable item's verdict is
itself settled, and its operand — if the node has an operation — is absent, has
been dropped, or has a settled verdict.**

Two consequences fall out of the definition:

- **`settled` implies `!parameterized`**: a settled node's own value is concrete
  and every item is settled (hence non-parameterized), which is exactly what
  `value_is_parameterized` tests for. So the bit is not a second opinion about
  concreteness; it is the *proof that concreteness is final*.
- **`settled` is monotone and needs no invalidation.** By §3.3 nothing can write
  into a settled subtree, the operand edge cannot appear under it, and the
  payload edges cannot change. So `settled` never goes back to false, and it is
  not a `dirty` flag at all — it is the *complement*: a node is either still
  under construction or finished for good.

### 4.2 Why the reverse index and dirty propagation are not needed here

The earlier design for this note built a `dependents` list per node and had every
mutation mark its consumers dirty. That is the right shape when a *change* must
be propagated — but in a within-build scope the only mutations that can invalidate
a verdict are value writes and a late operand edge, and §3.3 says a **settled**
subtree admits neither. So invalidation never has to be *tracked*: it is excluded
by the property. What is dropped along with it:

- the `O(E)` `dependents` graph and its `link`/`unlink` maintenance,
- the hook in `write_node_value` (whose class walk would otherwise have carried
  the dirty propagation),
- the upward propagation from `close_operation_cycle` (which keeps doing the one
  thing it already does: clear the verdict, now also clearing `settled`),
- the cleanup in `drop_block` (nothing points at a dropped node from outside it,
  by that function's own contract, `gc.rs:170-171`).

What remains is one bit to compute and one early return to add.

### 4.3 What the cut cannot cover, stated up front

- **Cyclic regions can never be settled.** The canonical universe `[Type, ↺]` is
  reached from inside its own descent, so its verdict is computed under the
  in-progress assumption (`P1-31`) and `settled` is false there — by definition,
  not by oversight. That is also where §1.3's worst redundancy sits
  (`struct_recursion`: 8.15 real/stamped, 72.4% revisits), so **the measured
  headroom is an upper bound, not a forecast**.
- **A node whose operand was never walked cannot be settled** unless the operand's
  verdict is settled. For a *core* operator the operand is the argument array a
  layer above synthesized, and the deep pass descends value-reachable edges only,
  so "operand has no verdict" is the normal case. Cutting there would be unsound:
  a later `evaluate_node_forced` *does* walk operand edges (`evaluation.rs:633-639`),
  and if it certified such an operand parameterized the parent's `false` would be
  stale. The saving grace is that the missed cuts are cheap ones — a core
  operator's own value is usually a scalar, so there is little subtree behind it —
  while the expensive re-descents are at operation-free array and table values,
  which have no operand at all and *are* settlable.
- **It cannot cut across the apply clone walk's decisions.** `function.rs:318-338`
  materializes bake-vs-clone into new topology; that is a consumer that freezes
  its inputs, and today's ordering (lambda value nodes proven at
  `checker.rs:657-659` before the definition pass runs applies at `:691-712`) is
  what keeps it coherent. A cut only removes redundant descents; it must not
  reorder those proofs, and the differential harness (§5 step 3) is what checks
  that it has not.
- **Recomputation is observable.** `budget_exhausted` is sticky per run
  (`lib.rs:978-983`), `apply_total` is cumulative (`apply.rs:31-32`), and a
  refusal becomes a `NonTerminating` diagnostic (`checker.rs:718-751`). Skipping a
  descent a full pass would have made changes *whether the budget is exhausted*,
  so the harness must compare diagnostics, not only values. This is also the one
  place where the cut could change behaviour for the worse: a cut node is not
  re-evaluated, so it cannot contribute the refusal a re-walk would have.

### 4.4 What the flag costs

Computing `settled` needs no new traversal: `value_is_parameterized` already walks
the same positions, so the flag is a second accumulator over that walk (an item
counts as settled iff its verdict is `Some(settled)`). The cut is one early return
in `evaluate_node_deep_inner`, after the static-leaf and cycle-cut cases and
before `deep_depth` is charged. The one subtlety is that the assumption must be
recorded as *not settled* rather than as settled — which is why `P1-31`'s split
came first: without a named in-progress state there is no way to tell a
deliberately assumed position from a decided one.

### 4.5 The expected take

The headroom is 34.6–72.4% of visits. Two forces pull the take below it: the
cyclic regions (uncuttable by definition, and the headroom's largest
contributor), and the core-operator nodes whose operand was never walked
(uncuttable for soundness). The acyclic, operation-free array and table values —
the closure, table, assert and polymorphism shapes — are where the cut should
collect, and their headroom is 34.6–56.3%.

**So the honest step is to build it and re-measure, not to forecast.** The delta
on the same six shapes is the answer, and if it is small, the correct conclusion
is that the remaining redundancy is *inherent* to re-deriving cyclic verdicts per
entry point, and that nothing further should be built.

## 5. Recommendation and staged roadmap

0. **Measure first.** *Half done.* The redundancy and revisit shares are measured
   (§1.3) with temporary counters that have since been removed. Still owed:
   the pass's share of a build's **wall-clock** — the counters do not separate
   (deep pass) from (check) from (lex+parse), and without that split "1.65–8.15×
   redundant" does not say how much of a build it is.
1. **The settled bit and the cut** (§4): `EvaluatedDeep::settled`, computed inside
   the existing verdict walk, plus one early return. No reverse index, no dirty
   propagation, no hooks beyond clearing the bit where the verdict is cleared.
2. **The differential harness** — required, not optional. Run the pass with and
   without the cut over the existing test corpus and compare every value, every
   verdict and **every diagnostic**. It is the only oracle for §4.3's two
   obligations (the apply ordering, the budget observability), and it is also how
   the "settled implies not-parameterized" claim is checked rather than argued.
3. **Re-measure** the six shapes and decide. If the delta is small, stop: the
   residual redundancy is the cost of re-deriving cyclic verdicts, and an
   SCC-atomic recomputation would be the only remaining lever — a change to the
   pass's traversal contract that this scope does not obviously justify.

## 6. What would falsify the plan

- **`is_unbound`-based stability has a hole.** §3.3 names three predicates; if a
  fourth write path can land in a concrete subtree, a cut verdict can go stale and
  step 2's harness would show it as a value or diagnostic divergence. That is the
  first thing the harness is for.
- **The measured delta is small** (see §4.5). Then the redundancy is inherent to
  cyclic re-derivation, nothing further is built, and the note's finding reduces
  to the measurement plus `P1-31`.
- **The ordering guarantee breaks.** If a cut lets an apply consume an unsettled
  template member, the recursion tests should fail; they are the canary.
- **The wall-clock share is negligible.** Then even a working cut is not worth
  its complexity, and the answer is to leave the pass as it is.

## 7. Decisions and open questions

**Decided (the superior):**

- **No cross-rebuild reuse.** Nothing persists a `Module` across an edit or keys a
  sub-expression; the fine-grained `StaticModule` design (`M3`), the same-module
  resume (`M2`), the value-digest memo (`M1`) and the `T3` resume are all
  **withdrawn**, not deferred. Q2/Q3/Q4 and Q7 (what identifies a sub-expression
  across a rebuild) go with them.
- **The invalidation question is answered by the code, not by an index.**
  "Dirty" is the *tracked* form of "may have changed"; §3.3 gives the *excluded*
  form, and in this scope the excluded form is sufficient. So the reverse index
  (Q1) is **withdrawn** with the propagation hooks.
- **The CLI build path is the measurement target** (Q5, unchanged): step 0 needs
  no editor, and `BufferSession` still has no production consumer (`P2-1`).

**Done:**

- **Q6 / `P1-31`** — the verdict's `None` conflated "never ran" with "in
  progress"; the split landed (`code-audit.md` `P1-31`), and §4.4 depends on it.

**Open:**

- **Q8 (re-posed, and this note's judgement) — what may a within-build cut
  trust?** *Judgement: a stability property, not a dependency index* — `settled`
  as §4.1 defines it. The obligation that remains is the differential harness,
  not another design question. What is genuinely not known is the *take*
  (§4.5), which is a measurement, not a decision.
- **Q9 — the wall-clock split.** Those counters measured redundancy, not cost. A
  probe that separates the deep pass from the checker and the frontend is what
  would say whether any of this matters; it is the one thing step 0 still owes.

## 8. Where the changes would land

| Step | File / function | Change |
|---|---|---|
| 0 | `evaluation.rs` `evaluate_node_deep_inner` + the two entry points | **removed**: the temporary walks / visits / cheap / revisit counters that took §1.3's numbers; re-add them (or a real profile) for the wall-clock split |
| 1 | `lib.rs` `EvaluatedDeep`; `evaluation.rs` `value_is_parameterized` + `evaluate_node_deep_inner` | the `settled` bit and the early return |
| 1 | `module.rs` `close_operation_cycle` | clear `settled` with the verdict it invalidates |
| 2 | `crates/lichen-lowlevel/tests/` (new file) | the differential harness: with and without the cut, over the existing corpus |
| 3 | — | re-run the measurement; the delta is the decision |

## 9. Recorded discussion

- The request was "make the evaluation system also able to rebuild
  incrementally". First reframing: there is no separable evaluation stage
  (§1.1), so the object is the `Module` the checker builds and evaluates.
- Second reframing (the superior's, and it changed the mechanism): this is
  **incremental computation over a DAG that keeps being updated, with dirty-flag
  propagation — the topology being the problem**. Accepted, and §3's inventory is
  the result.
- Third: **no cross-rebuild reuse** (the superior's), which deleted the whole
  cross-build half and, with it, the *need* to propagate. What is left is §4: a
  cut that trusts a verdict only when nothing can write into it, which is cheaper
  than tracking what did.
- `P1-31` (named in the previous revision of this note) landed: the verdict's
  `None` now means one thing, with the in-progress case named. Two sites an
  earlier draft had named as defects were **retracted** — the operand arm is a
  deliberate exemption, and `Module::key_state` is correct because the content
  unfolding is total and self-reporting. Both retractions are recorded in
  `P1-31` rather than dropped.
- What this note deliberately does **not** claim: that the cut is worth building.
  §4.3 lists two reasons its reach is limited and §4.5 says the take is a
  measurement. The note's durable results are the measurement, the mutation
  inventory, the three stability predicates, and `P1-31`.
