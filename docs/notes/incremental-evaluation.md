# Incremental evaluation: reusing the VM's evaluated state across a rebuild

> Status: **proposed** — nothing described here is implemented, and
> no measurement backs the cost claim in §1.3 (step 0 of §5 is the measurement).
> The evaluation counterpart of
> [incremental-parse-compile](incremental-parse-compile.md), whose `T1`–`T4`
> cover `lex → parse → resolve → lower → check`; this note asks what the *same*
> question costs one layer down, in the runtime.
>
> Points at: `crates/lichen-lowlevel/src/{evaluation,equality,function,gc,table,module,lib}.rs`,
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
- **`evaluate_node_deep` is also the deep pass invoked from inside evaluation
  itself** (`evaluation.rs:279`, the operation postlude's parameterized gate).

So "incremental evaluation" is not a new stage bolted on after the checker: it
is the same problem as incremental checking, plus a question about what a reused
answer is allowed to be. §3 makes that precise.

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
feature, not two (§4.4).

### 1.3 The cost that is not reused even *within* one build

The deep pass memoizes nothing *across* entry points.

`Node::value` is memoized per node, and the deepest layer of `evaluate_node`
returns it early (`evaluation.rs:142-144`), so an operation is not recomputed.
But `evaluate_node_deep_inner` has exactly two cuts:

- a static ref is a decided leaf (`evaluation.rs:576-578`);
- `visiting && value.is_some()` cuts a structural cycle an outer frame is
  currently computing (`:593-597`).

`evaluated_deep` — the per-node verdict the pass *writes* at `:705` — is never
read back as a cut in that function. Its only readers are the operation
postlude's parameterized gate (`:285-287`), the verdict's own recursive
definition (`:741`, `:756`, `:760`, `:770`), and the apply clone walk
(`function.rs:325`). So every entry point of §1.1 re-descends the whole
value-reachable graph it covers — arrays and tables included (`:641-670`,
`:674-699`) — however much of it a previous entry point already walked.

Two consequences, both worth naming before any design:

- **Entry-point count, not graph size, multiplies the work.** A build makes
  `3 + |lambdas| + |functions| + |asserts| + |statements| + 1` deep-pass calls
  (§1.1), and a run makes 2 more. A statement's term, a lambda's value node and
  that lambda's return all reach much of the same graph, so the same subtree is
  walked once per entry point that reaches it.
- **The walk is path-bounded, not node-bounded, on a shared graph.** Nodes are
  shared (`lowlevel-vm.md`), and nothing in the walk remembers a subtree it has
  already completed, so a diamond is walked once per path. The `visiting` mark
  only cuts cycles *during* a frame; by its own contract it is a liveness flag,
  not a "was visited" flag (`module.rs:182-198`, `code-audit.md:2602-2608`).

This is a *within-build* incrementality that needs no new identity scheme and no
cross-build state — which is why §5 puts it first. It is also where the
soundness question of §4.1 comes from.

## 2. The state that would have to be reused, and why a `NodeId` is not an identity

Four kinds of state live in a `Module` (`lib.rs:939-1014`), and they have
different lifetimes:

1. **Values** — `Node::value` (`lib.rs:845`), private, written only through
   `write_node_value` (`equality.rs:273-297`). A compound value's payload lives
   in a **block `Bump` arena** and its items are `Dynamic(NodeId)` refs.
2. **Concreteness verdicts** — `Node::evaluated_deep: Option<EvaluatedDeep>`
   (`lib.rs:889`): a proof about the node's **whole reachable subtree**, `None`
   read as "unknown, therefore parameterized" by both its readers.
3. **Structure** — `blocks` (the GC unit and the arena owner), `functions`
   (templates, `Function::parent`/`nodes`), and the union-find `equality`
   classes plus the per-class `low_shape`.
4. **Diagnostic and budget state** — `unify_errors` / `eval_errors` /
   `apply_errors` / `asserts` / `assert_errors`, all append-only and never
   cleared (`lib.rs:984-1008`), plus `budget_exhausted`, which is sticky per run
   (`lib.rs:978-983`).

A rebuild today starts from `Module::new()` (`checker.rs:518`) and re-derives
all four. Reusing any of them across an edit runs into four structural facts:

**(a) `NodeId` is a versioned slot key, not a name.** `nodes` is a
`SlotMap<NodeId, Node>` (`lib.rs:952`) whose keys are allocated in insertion
order and whose slots are reused with a bumped version after a release. Editing
an IR prefix shifts every later `check_expr` allocation, so the same source
expression gets a different `NodeId` in the next build. (The *IR* is
append-stable — `incremental-parse-compile.md` §3 item 2 — but the `Module` is
allocated by the checker's walk, and nothing ties its ids to `ExprId`.)

**(b) The memo is the class, not the node.** `class_value` reads through the
union-find representative (`equality.rs:81-87`), and `write_node_value`
replicates a concrete value to every unbound pure-cell member of the class
(`equality.rs:273-297`). A value transplanted without its class is unsound, and
a class transplanted without its members is unsound.

**(c) Only one of the two ref forms travels.** A value's payload holds
`AnyNodeId = Dynamic(NodeId) | Static(StaticNodeId)`. `Dynamic` is module-local
— there is no cross-module dynamic ref anywhere in the design — while `Static`
is keyed and absolute from birth (`static-modules.md`). So a value that must
outlive its module has exactly one legal representation: **frozen**.

**(d) A verdict is a semantic input, not an optimization hint.** The apply
clone walk decides bake-vs-clone from `proven_concrete`
(`function.rs:318-338`): a node the deep pass proved concrete is *referenced in
place*, and a node flagged parameterized or never walked is *cloned fresh*.
Baking a node that should have been cloned per call breaks recursion — the
comment at `checker.rs:649-656` records that this was an observed failure mode.
So a reused verdict must be true of the current graph, not merely
plausible-looking.

Two further facts bound any scheme:

**(e) A build destroys part of its own intermediate state.** `evaluate_block`
runs the deep pass and then `garbage_collect` (`evaluation.rs:777-788`), which
moves the subtree's nodes out of the released block and re-allocates its
compound payloads into the target arena (`gc.rs:12-76`). A snapshot taken
mid-build is not a snapshot of a state the module ever rests in.

**(f) `BlockId`s are not per-statement.** The checker creates one root block
(`checker.rs:593`) and one block per lambda (`checker/lambda.rs:31`) — nothing
per top-level statement. So "the unit to drop and redo" has no existing
boundary to key on below a whole function.

## 3. The soundness frame: three requirements, whatever the mechanism

Every candidate below is an instance of the same three obligations. Stating them
once is what keeps the design space from looking like four unrelated ideas:

- **R1 — identity.** A name for an evaluation unit that is *position-independent*
  and *stable across a rebuild*, so "this is the same unit as last time" is
  decidable. `NodeId` fails (a); `BlockId` fails (f); `ExprId` is append-stable
  in the IR but its allocation shifts under an edit, and it names an
  *expression*, not an evaluation unit (a pair is several nodes plus shared
  markers plus operand arrays).
- **R2 — invalidation.** Either a **reverse dependency index** (who consumed
  this answer?) so a change propagates, or a **content key** over the unit's
  inputs so propagation is unnecessary. The lowlevel has neither today:
  `node_edges` (`checker.rs:250`) maps a node to a `Loc` for *diagnostics*, and
  there is no dependents relation anywhere. The deep verdict's subtree scoping
  (b/d) is what makes this obligation hard: invalidating a leaf invalidates the
  verdict of every node whose reachable subtree contains it.
- **R3 — representation.** How a reused answer crosses the boundary, given (c):
  either the module is kept alive and resumed (no boundary), or the answer is
  frozen to a `StaticNodeId` and read in place.

A scheme that answers all three is sound *by construction* rather than by
audit. That is the frame the recommendation in §4.4 is chosen against.

## 4. The design space

### 4.1 M0 — memoize the deep pass inside one build

The narrowest change: let `evaluate_node_deep_inner` cut on an existing
`evaluated_deep` verdict instead of re-descending. No identity scheme, no
cross-build state; R1 is "the node in this module", which is free.

It is also the one that needs a real soundness argument rather than a
mechanism, because of (b) and (d): a verdict is a statement about a subtree, and
the walk itself keeps writing into that subtree. `write_node_value`
(`evaluation.rs:476`) replicates a concrete value into every unbound pure-cell
member of the node's class, and a fresh cell starts life as
`Some(Parameterized)` (`checker.rs:927-933`) and can therefore be *upgraded*
later. So a subtree walked and proven concrete at time `t` can gain a bound cell
at `t' > t`, which makes the earlier verdict stale, and a later re-reach would
cut on it. Whether such a write can land inside an already-walked subtree
depends on the reachability of the shared cells — see **Q1**.

Two sound variants exist if the general cut cannot be argued:

- **Cut only where the verdict is closed.** A node with no unbound pure-cell
  member reachable in its subtree cannot be upgraded, so its verdict is
  permanent. Detecting that is a stronger property than `parameterized == false`
  and would be a second flag, not a reinterpretation of the existing one.
- **Cut only against this walk.** Give the pass its own completed-set for the
  duration of one call (a `HashSet<NodeId>` on the walk, or a second mark with
  walk-scoped lifetime). Within one call, a completed subtree cannot have been
  written into *by that same walk* after completion except through replication
  from a node outside it — the same residual case, but now a narrower one to
  audit. This is the shape `code-audit.md:2594` already asks any operand-edge
  walk to have, and it fixes the path-bounded blow-up of §1.3 without needing a
  permanent verdict.

### 4.2 M1 — a content-keyed value memo ("salsa at the IR level")

Key each allocation by a serialization of its inputs, and on a rebuild look the
key up in a cache of previous `(value, verdict, class, low_shape)`.

The identity half already exists: `resolve/content_key.rs` is an exact,
digest-free, injective serialization of the *lowering-visible* resolved content
(names by `BinderId`, error blocks opaque, spans dropped, version-stamped at
`content_key.rs:13`). It is the right model for R1, and it is already the
`BufferSession` reuse key.

The obstruction is R2 at value granularity, and it has two parts:

- **A syntax key is not sufficient.** A value is not a function of syntax
  alone: it is a function of the unification history. The code's own claim is
  that this is *order*-independent ("reads alias their target cells … so
  bindings propagate class-wise however they happen", `checker.rs:665-667`), so
  value = f(syntax, values of the binders reached) may well hold. But that is a
  claim to be verified before it is used as a cache contract, and the verdicts
  in (d) are explicitly *not* order-independent: a `Parameterized` answer is
  deliberately left uncached so a later pass with the key bound decides it
  (`evaluation.rs:463-467`).
- **The canonical value hash that exists is deliberately non-injective.** The
  table key machinery (`table.rs:1-70`) is a canonical content unfolding: it
  mixes in what each position *is* and never an address, a node identity, or a
  freeze index, and it explicitly survives a freeze. It is exactly the shape a
  value memo wants. But its own module doc states it is a **pre-filter** whose
  authority is `key_eq` (`table.rs:9-14`) and enumerates what it cannot tell
  apart (`:51-61`) — including *which operator sits at a position*, and any
  difference below `UNFOLD_DEPTH`. A cache key must be authoritative: serving
  the wrong value on a hash collision is not a candidate-quality loss, it is a
  wrong answer. So M1 needs either a verification step (`key_eq` on every hit,
  which is a full comparison and gives back much of the win) or a *new*,
  injective value-digest contract — a real design item, not a reuse
  (**Q4**).

M1 also does not escape R3: a memoized value holds `Dynamic` refs into the old
module, so either the old module stays alive (an arena pinning scheme the
runtime does not have) or the memo stores frozen state — which is M3.

### 4.3 M2 — same-module resume (generalizing `T3`/`T4`)

Keep the `Module` across edits: re-lower only the changed unit into fresh nodes,
invalidate its dependents, keep everything else. This is the only route that
answers R3 with "no boundary" — the state never leaves the module, so `Dynamic`
refs stay valid and no payload is copied.

It pays for that with R2 and with state the module cannot currently undo:

- **Rollback.** The union-find has no undo: `add_equality` merges destructively
  (`equality.rs:60-71`), `write_node_value` replicates destructively, and
  `close_operation_cycle` is a one-way operation definition. Invalidating a unit
  means removing its nodes and every write it caused — including writes it
  caused *into other units' classes*. `T4` ("unification-state
  checkpoint/rollback", `incremental-parse-compile.md` §5) is exactly this, and
  it is the honest size of the work.
- **Append-only side tables.** The four diagnostic vecs are documented as
  never-cleared and some carry an accompanying set that must not drift
  (`lib.rs:984-1013`); a resume needs a per-unit extent for each, plus
  `record_extension_diagnostic`'s dedup (`module.rs:311-330`) must be revisited
  once entries can be dropped.
- **Budgets.** `budget_exhausted` is sticky "because it is *the* record of why
  the last walk stopped" (`lib.rs:978-983`), and `apply_total` is cumulative, so
  a resumed module must decide what a "run" is now (`reset_apply_budget`,
  `module.rs:90-95`).
- **No unit boundary to key on** (f): per-statement ownership would have to be
  introduced before anything can be dropped per statement.

### 4.4 M3 — fine-grained frozen units (recommended)

The one mechanism whose R3 answer is already built, tested, and in production
for the cross-file case: freeze solved state into a `StaticModule` and read it
in place.

`StaticModule` is precisely "an evaluation unit's reusable result":

- `Module::static_read` returns the solved value **verbatim, no copy**, and its
  payload stays in the module's shared arena (`static_module.rs:96-102`);
- `StaticNode` carries the solved **verdict** (`parameterized`, `lib.rs:913`,
  copied from `evaluated_deep` at `freeze.rs:71-73`), so (d)'s bake-vs-clone
  decision is preserved rather than guessed;
- refs are absolute from birth, so (c) is answered;
- GC keeps static handles verbatim (`static-modules.md` "GC & asserts"), and the
  deep pass treats a static ref as a decided leaf (`evaluation.rs:576-578`), so
  a reused unit costs *no* re-walk and no re-evaluation — the two things §1.3
  says are being paid today;
- identity is content-addressed and transitive (`artifact_hash`, three
  agreeing sites in [artifact-cache](artifact-cache.md) §"Dirtiness"), so R2's
  "propagate a change" becomes "the key changed".

The costs and the changes it forces are real and must be stated:

- **Freeze is whole-module.** `StaticModule::from_module_mapped` freezes the
  *entire* module into one artifact and hands back a `NodeId → LocalNodeId` map
  (`freeze.rs:40-75`). Sub-file granularity therefore means either a
  **partial freeze** (freeze the sub-graph rooted at a node — a new entry point
  into a well-understood walk) or making each unit **its own `Module`**, which
  is the shape the package store already uses per file, applied within a file.
  The second is more machinery but has no new concept in it.
- **The source must be fully solved** (`freeze.rs:23-24`): every node holds its
  final answer or a residual `Parameterized`. A unit frozen before its
  dependencies resolve is not freezable, so freeze points have to be chosen
  after the definition pass — which interacts with the fact that the definition
  pass is *inside* the checker.
- **Payload copy.** Phase 2 already dedupes and lays out dynamic payloads into a
  flat arena (`freeze.rs:92-150`); freezing more, smaller units means more
  copies and a larger key space. Bounding it (an LRU per revision, or reusing
  the device store's slot discipline) is a decision, not a given.
- **Class sharing is lost across the boundary.** A static leaf unifies by
  materializing into a fresh dynamic leaf (`static_module.rs:142-153`). That is
  exactly how imports already behave, so it is not new — but it does mean a
  binding reused as a static leaf does not participate in the new build's
  classes the way it did in the old build. Whether that changes observable
  behaviour (a polymorphic top-level binding reused across an edit) is **Q2**.
- **M3 does not remove the need for `T3`.** To *skip* re-lowering an unchanged
  unit, the checker must resume from a scope whose entries are static refs for
  the reused binders. M3 supplies the representation; the resume is still
  `T3`'s work. This is the note's central claim: **incremental evaluation is
  incremental checking plus a representation choice, not a second feature.**

### 4.5 M4 — do nothing below the file

The artifact store already rebuilds only the changed chain
([artifact-cache](artifact-cache.md)), and the LSP already routes settled
imported packages through it ([liche-lsp-home](liche-lsp-home.md)). If
measurement (§5 step 0) shows the deep pass is not where the keystroke budget
goes, the honest answer is that M4 is already the answer and M0–M3 are
unwarranted. This option is listed because the note has no measurement, and
because `code-audit.md`'s `D6` shows the project's habit of letting a
measurement decide (`(b)`'s value was re-derived after `(a)` landed).

## 5. Recommendation and staged roadmap

The order is chosen so that each step is independently useful, falsifiable, and
smaller than the next.

0. **Measure first — no design without it.** Instrument the deep pass: entry
   points per build, nodes visited per entry point, distinct nodes reached, and
   the share of the keystroke budget that is (deep pass) vs (check) vs
   (lex+parse). `code-audit.md:3966-3970` shows the project's own probe style
   (a temporary counter inside `Module::static_module`, removed after). The
   §1.3 claim that the entry-point count dominates is a *reading of the code*,
   not a measurement, and step 0 is what would falsify it.
1. **M0, the within-build cut** (§4.1) — only after Q1 is settled. If the
   verdict cannot be cut on, implement the walk-scoped completed set, which is
   sound within one call and still removes the path-bounded blow-up. This needs
   no new identity, no freeze, no rollback, and it is the same fix for every
   entry point at once.
2. **M3 at the unit granularity Q3 selects** (§4.4) — partial freeze if a
   sub-graph freeze is acceptable, otherwise a per-unit `Module`. Deliverable:
   an edit re-evaluates only the changed unit and its transitive dependents, and
   the unchanged units cost a static read.
3. **`T3` resume** — the checker skipping a unit it can read as a static ref.
   Steps 2 and 3 together are the full "incremental evaluation" of the request;
   step 2 alone only makes the *reused* unit cheaper, not skipped.
4. **M1's value memo** (§4.2) — optional, and only if step 0 shows re-lowering
   (not re-evaluating) dominates after steps 1–3, and only once Q4's
   authoritative digest exists.

`T4` (same-module resume with unification rollback, §4.3) stays the boundary:
it is the only route that avoids freezing, and it is a change to the
union-find's contract plus the module's side tables. Nothing here needs it.

## 6. What would falsify the plan

- **Step 0 shows the deep pass is negligible.** Then the merge of §4.4 into
  "incremental checking" is the whole finding, and the note reduces to a
  documentation change: `incremental-parse-compile.md`'s `T3` is the evaluation
  incrementality item, and no new mechanism is warranted.
- **Q1 answers "no" and the walk-scoped set cannot be made sound either.** Then
  M0 is off, and only step 2+ remains. Unlikely — a walk-scoped completed set
  is monotone within one call — but it is the load-bearing assumption of step 1.
- **Q2 shows a static leaf changes observable behaviour for a polymorphic
  binding.** Then M3 needs a class-sharing story (i.e. M2), and the cheap
  mechanism is gone.
- **Freeze cost exceeds the re-evaluation it saves** (many small units, large
  payloads). Then unit granularity must be coarser than a binding, and §4.4's
  win shrinks toward M4 — measurable at step 2, before step 3 is built.

## 7. Open questions for the superior

- **Q1 — Is `evaluated_deep` reusable as a cut at all?** A verdict is scoped to
  a *subtree* and the walk keeps writing into subtrees via
  `write_node_value`'s class replication (`evaluation.rs:476`, `equality.rs:273-297`),
  which can upgrade a `Parameterized` fresh cell
  (`checker.rs:927-933`) after its subtree was proven concrete. Is the general
  cut provable, or do we restrict it to closed subtrees / to one walk's
  completed set? *This decides step 1, and it is the subtlest point in the
  note.*
- **Q2 — Does a reused binding as a static leaf preserve semantics?** A static
  leaf materializes into a fresh dynamic leaf and does not share the new
  build's classes (`static_module.rs:142-153`). Imported packages already live
  with this. Does a *top-level binding of the edited file* — in particular a
  polymorphic one — survive it, or does reuse have to keep the class alive
  (i.e. M2)?
- **Q3 — Which unit granularity?** Top-level binding / statement, a lambda body,
  or an arbitrary sub-expression. Finer units reuse more and freeze more; the
  boundary also decides whether `BlockId` (one per lambda today, `checker.rs:593`,
  `lambda.rs:31`) can carry R1 or whether a new per-unit id is needed.
- **Q4 — Is an authoritative value digest needed, and may it be a new
  contract?** The existing canonical unfolding is documented as a pre-filter
  (`table.rs:9-14`, `:51-61`). Either M1 verifies every hit with `key_eq` (and
  pays for it) or the project takes on a *second*, injective digest with its own
  format-version discipline, like `content_key`'s. Which?
- **Q5 — Which consumer is the target?** The CLI build path pays for one build
  per file; the editor path pays per keystroke but currently has *no* consumer
  for `BufferSession` (`P2-1`), so an incremental evaluator wired to a session
  nothing runs would repeat that. If the target is the editor, this note's
  steps 2–3 are `D6`'s `(b)` restated with evaluation included, and should be
  sequenced there rather than here.

## 8. Where the changes would land (if the plan is taken)

| Step | File / function | Change |
|---|---|---|
| 0 | `crates/lichen-lowlevel/src/evaluation.rs`, a temporary probe | count entry points, visits, distinct nodes; removed after measurement |
| 1 | `evaluation.rs` `evaluate_node_deep_inner` | a cut on a closed verdict, or a walk-scoped completed set |
| 2 | `static_module/freeze.rs` | a sub-graph freeze entry point (or a per-unit `Module` driven from the checker) |
| 2 | `crates/lichen-registry` | identity and slot discipline for many small units per source file |
| 3 | `crates/lichen-highlevel/src/checker.rs` | resume from a scope whose entries are static refs for reused binders |
| 3 | `crates/lichen-language/src/session.rs` | the reuse gate moves from the whole-`Build` `content_key` to per-unit keys |
| 4 | `crates/lichen-lowlevel/src/table.rs` | an authoritative digest, or a `key_eq`-verified memo |

## 9. Recorded discussion

- The request was "make the evaluation system also able to rebuild
  incrementally". The finding that reframed it: there is no separable
  evaluation stage (§1.1), so the honest object of the request is the `Module`
  the checker builds and evaluates — and the sub-`Build` granularity it lacks is
  the same one `T3` names. Recorded so a later reader does not re-derive the
  reframing.
- Rejected as the primary mechanism: **a syntax-keyed value memo alone** — a
  value is a function of the unification history, not of syntax (§4.2), and the
  only canonical value hash in the tree is documented non-injective.
- Rejected as the primary mechanism: **same-module resume** — sound in
  principle and the only route that avoids freezing, but it is `T4`'s
  unification rollback plus a new per-unit boundary plus the append-only side
  tables (§4.3). Kept as the documented boundary, not as a plan.
- Selected: **fine-grained frozen units, behind a measurement**, with the
  within-build deep-pass cut as the independent first step.
