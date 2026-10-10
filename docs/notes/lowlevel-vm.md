# Lowlevel VM

> Status: current
> Points at: `crates/lichen-lowlevel/src/` — `evaluation.rs` (reads and the run
> gates), `apply.rs` / `function.rs` (the apply clone walk), `equality.rs`
> (unification and the write rules), `module.rs` (the node arena and the function
> shell), `gc.rs`, `table.rs`, `assert.rs`, `low_type.rs` (the low-type pass),
> `resolve.rs` and `loop_conversion.rs` / `loop_run.rs` (template graph facts and
> loop conversion), `static_module.rs` + `static_module/` (freezing),
> `registry.rs` (the shared static store).

`lichen-lowlevel` is the runtime the whole system is built on. It evaluates a
`[value, type]` program and, because unification runs here, it is also the
typechecker — "the runtime is the typechecker."

## The graph

A `Module<P>` is a dense id-keyed arena of nodes (`NodeId`). Functions, arrays and
pairs are **shared** nodes, not copies — a binding is graph sharing, so a name's
every use is the same node. The `Program` trait parameterizes the instance over
the value, operator, global-extension and package-metadata vocabularies, so one
runtime serves every layer; a program's value type *composes* the lowlevel's own
`LowValue` rather than replacing it.

Values and types are one runtime kind. `LowValue` is the structural vocabulary —
`USize`, `Float`, `Str`, `Array`, `Table`, `Function`, the unit `None`, and
`Error` — and a type constant is an ordinary value in it. `AnyNodeId = Dynamic |
Static` names a node in the building module or in a frozen one, so a value can
reference both.

**The lowlevel is untyped by design.** It knows the `[value, type]` pair only
where the pair is an *operand of the operation in hand*; it holds no type
predicate, no `is_type`, and no notion of a kind marker. A policy that needs the
highlevel's type theory is stated by the layer that owns it
([checker-encoding-instability](checker-encoding-instability.md)).

`Node::depth` is equivalently the length of the `origin` chain to a node the checker
built, memoized at construction. It cannot be a frame counter: a frame counter
measures the walk, and the lazy deep pass walks an expansion breadth-first at depth
one, while the stamp comes from the apply node the instantiation is for.

Several walks descend the node graph and must cut a cycle rather than loop: the
unification comparison, the deep pass's reconciliation of a forced computation
against the value its class committed, the table key comparison, and the type/value
printers in `lichen-render`. Each asks whether something is on the **current recursion
path**, which is an *ancestor* relation and not a visited mark — a node or pair met
again in a sibling subtree is not on the path and must be visited again, so
membership is removed on the way out. What the guard accepts is distinct (the caller
tests before it inserts), so a set answers the same question under the same
insert/remove discipline. `AncestorNodes` is the one-node form and `AncestorPairs` the
unordered-pair form; both orientations of a pair are stored, which is what lets one
probe answer the symmetric test.

### Why a node list is not SSA

A `NodeId` is not a value. The apply clone walk **unifies** a substituted parameter with
its argument, so two node ids can be one value; and a cell the deep pass left undecided
resolves through its equality class to a node that *computes* it, which is not one of
its operands. An ordered list of nodes therefore cannot say which node defines a value,
and any consumer that tries re-derives the same answers differently.

`Module::define_in` is the rule: it takes any node naming a value and answers what
defines it — a computation, one of the function's parameter leaves, or nothing
computable — so a consumer builds a body over the answers instead of re-deriving them.
`Module::parameter_leaves` is its other half, decoding the domain's leaves **in one
place** so a caller cannot decode the pair differently. `Module::defining_member` walks
the class's own member list rather than the module's node table, which holds every
kernel's nodes while one kernel is compiled — a table scan made codegen quadratic in
the number of kernels.

This graph is a **DAG of values**: every node is computed once and cached, and
`evaluate_node_deep` answers a `LowValue` for every node it reaches. A CFG says *some
of this is not computed once* — a header is entered many times and its block params
differ each time — so a CFG is a **program** fact, and the program (a kernel body)
belongs to `lichen-kernel-ir` and `lichen-compute`. Putting one in the lowlevel would
make a DAG claim to hold something it does not. A parameter is named by its node and
the consumer maps the leaf to its own slot: the slot index is the kernel ABI's arity
convention — the flattening of a domain into arguments — and belongs to whoever lays
out a call, not to the graph.

`KernelInstr` was a **stack machine**: its instructions named no values, so each
backend walked an operand stack and *derived* the form it wanted — `waffle` derives a
stack from SSA, `spirv.rs` derives SSA ids from a stack. Both paid to undo the
omission, and it was not cosmetic: a stack machine has nowhere to put a value that
outlives an expression, so a **loop-carried value had no representation at all**. The
replacement is an SSA body whose blocks declare the values they receive, so a
backedge's arguments *are* the next iteration's state. The long form of that argument
is [loop-conversion](loop-conversion.md) §8.6.

## Lazy evaluation

An expression is forced only when its result is needed; an untaken branch (for
example `[then, else][i]`) is never evaluated. Two gates decide a read:

- An **empty slot** is *undecided*, and an undecided answer is **never cached**,
  so the next read re-evaluates against whatever has since bound. That is how a
  later binding is observed.
- `Node::runned` is the other axis, read through `Module::has_no_result_yet`:
  "has this node's own computation produced an answer". A slot holding a value
  while `runned` is false means a unification wrote the value, not the operator —
  so the operator still owes its own answer and re-runs.

`LowValue::Error` is the opposite of undecided: it is the **decided** value of a
failed read (an out-of-bounds index, an apply of a not-callable, a table miss),
produced together with a recorded `EvalError` and cached, so one failed read is
one recorded error. `LowValue::None` is the unit value and is neither.

Two build-time positions hold `Error` as their marker rather than as a failure: the
anonymous struct's "no name table" slot, and the no-operand sentinel the VM hands a
nullary extension operator. Both stand where no value is, which is the absence the
variant denotes, and neither is a failed read, so neither records an `EvalError`.

Every failure mode of a read or an apply is a **user error**: recorded as an
`EvalError` and answered with the computed-nothing value, never an internal error and
never a panic. Neither a field read applied to something that is not a container nor a
subscript that is not an index is expressible in the type encoding, so the checker
cannot reject either statically; the same holds for applying a scalar, a string, a
table or the unit value, and for a callee whose undecided type the checker's
function-ness guard cannot see. An out-of-bounds index is refused the same way rather
than panicking in raw slice indexing. A late binding still reaches the position
through the `Error`/undecided arms, so nothing that could later resolve is lost.

An apply frame that exceeds its work budget returns `None` — the undecided answer —
and not `LowValue::Error`: the body never ran, so the answer is *unknown* rather than
nothing. `LowValue::Error` would instead be cached by the `evaluate_node` postlude as
a decided value, which lets the deep pass certify the node concrete and every parent
array with it — a proven-concrete claim about a computation that never happened.
`None` is the same refusal `apply_parameter_check` issues for a body it declined to
run.

Functions are first-class: a function's body is a **template**, and an apply
clones the nodes whose value could differ per call — the parameter pair and the
return cells among them — so a signature is re-instantiated per call
(let-polymorphism) and the template is never bound. Recursion is a function
referring to its own pair; the apply and evaluation depth budgets record
`Module::budget_exhausted` and return a value instead of panicking, so a
non-terminating program is a diagnostic rather than an unwind.

A depth refusal answers the computed-nothing value, not an undecided one: an undecided
answer promises "try again later", which nothing downstream can honour, because this
frame already owns the budget verdict and a later read reaches the same refusal.
`deep_depth` is a **nesting** counter rather than a cumulative budget — incremented at
entry, restored on every exit, and restored on the refusal path specifically, since
left inflated nothing else would restore it except `reset_apply_budget` and every
later `evaluate_node_deep` in the process would start past the limit. The refusal is
scoped to the subtree that is too deep: shallow siblings still walk and are decided,
and only the nodes past the limit yield `Error`. Unlike the apply frame's refusal this
value is never cached onto a node, so the node's `evaluated_deep` stays `None` — "never
ran", not "proven failed".

A shallow-marked array position is **not** descended into by the deep pass: the mark
means "this subtree is deliberately lazy", and a read inside it is what forces a
single element on demand through `Index`. A condition behind a shallow mark therefore
stays **pending**: the operator's own operand gate (`OperatorExt::run_deferred`) reads
the operand array's verdict, which the shallow flag alone makes undecided, so the gate
answers undecided and the entry is never triggered.

A node's deep verdict is read from the **value graph only**. An operation's operand
edge is deliberately not read: it is not value-reachable, so a verdict read from it
would be a fact about which walk happened to run rather than about the graph — and it
cannot be needed, because a `LowValue` is a computed answer, not a thunk, so a decided
value cannot depend on an operand its operator did not read. Each position's own
three-state verdict is read through `ref_is_undecided`, so an **in-progress** position
is assumed concrete while one the pass never ran on is not.

`has_no_result_yet` and `node_evaluated_deep` answer two different questions, and they
disagree in **both** directions. An operator that ran and could not decide is *owed* —
the slot stays empty and the next read runs it again — yet *undecided*. A member
holding a value a unification propagated is *owed*, the operator still owing its own
reconciliation with it, yet *concrete*. A caller asking "is this answer concrete" must
ask the value axis (`node_evaluated_deep`), never the slot's emptiness. That axis'
`None` likewise means *unknown*, not *proven concrete*: a budget refusal, a node
reached only as an operand, and a node re-entered through a cycle cut all leave it
`None`, and the apply clone walk and the operation postlude both read `None` as
undecided.

The apply's result cell binds to the return type, and that type is resolved **before**
the bind. The deep pass resolves the node later and does not replicate to class
members, so an unresolved bind would leave the cell undecided. A lazy return type — a
body ending in a call — is an `Index` read, which already aliased its target cell at
evaluation time, so the unify joins the cell into that class and the binding
propagates regardless of when the nested apply runs. Element 1 is the type slot of a
2-wide `[value, type]` pair and of a 3-wide `[value, type, perspective]` pair alike.

The applied function's parameter is an entry point of the materialize walk in its own
right, not merely a node the return subtree happens to reach: an *ignored* parameter
still has to be satisfied, and a type annotation it pinned is invisible from the
return subtree, so a walk that reached the parameter only by following the return
would miss both.

The body's asserts are the function's own registry entries: each condition instantiates
through the shared remap, and the list is walked **by index** rather than over a clone
of it, because instantiating a condition needs `&mut module` while the list lives on
the context's module. A *baked* condition is a per-call invariant (decided at solve
time) and is not re-registered; a cloned one re-checks against the argument.

## Unification, and the write rules

Identity in the VM is the union-find **class**: `unify(a, b)` merges the two
classes, settles the value the merged class holds, and records the conflict — one
recursion, not a comparison path beside a write path. There is no separate
"undecided side" case: "this class knows nothing" is the `None` the question
every pair is asked, and the arm it lands in is its answer. A class carries one
value and one **low type**.

The arms, in order:

- **Both sides know nothing** — the merge is the whole answer.
- **One side knows a value** — the merged class holds it.
- **Both know a value** — they must be the *same* value. For an array "the same"
  is decided by unifying the elements, because an array's elements may still be
  cells and a free cell matches anything.
- **A side without a node still decides.** A bare value — an operation's answer, or
  a value a write is distributing — or a static ref, which is absolute and has no
  local cell to merge into: the question is still answered, and a **value against a
  valueless class is a write**, the class learning the value. Only a class that holds
  nothing is written, because a class that already holds a value is not a hole and
  overwriting it would mask the conflict the comparison exists to find. Every fact a
  static ref brings into a unification rides on that write: an imported
  `struct<.x Int, .y Int>`'s field types reach the importer's cells as `Int`, so a
  placeholder instantiation across the boundary never learns its field types.

Two rules decide what a class's members then hold:

- **A write is unconditional and reaches every member.** `write_node_value`
  writes the node's own slot and, for a concrete value, propagates it to the
  whole class (`propagate_class_value`). So **one class has one value**, and no
  reader has to know which member a write started from. A writer that must not
  claim an operator's answer restores the node's own slot afterwards
  (`write_node_answer`), which is how an operation-bearing member keeps its
  computation while the class's value is visible around it. A `None` value clears
  only the node's own slot — undecided is not a fact to propagate.
- **A merge fills only the holes, and overwrites nobody.** `add_equality` reads
  what either side already knew *before* the union (the union re-elects the
  representative) and fills the members that hold nothing (`fill_class_holes`). A
  member that already holds something keeps it, because a merge is where two
  decided sides meet and a class routinely holds a different value on each
  member — a term pair on one, the resolved value on another, a type cell on a
  third. Overwriting would move a node out of a slot the structure still names.
  The same merge carries the class's **low type** (joined, not overwritten) and
  the value to the members it adds.

`class_value` / `class_committed_value` read the class's value through its
representative, `class_root` walks the union-find without path compression so a
read can hold a shared borrow, and `alias_read` is how a pure read joins the
class of the cell it read, so a later bind replicates the value to the reader.
The full semantics, and the reason a read can be taken too early, are
[eval-before-unify](eval-before-unify.md)'s subject.

The conflict record itself is a flat, ordered sequence of `UnifyStep`s: the
structural path from the unified root operands (`root_a`/`root_b`) down to the
failing pair, which is the "step by step" a layer above uses to pinpoint which
component of a compound type conflicts. A static child resolves to
`NodeId::default()` because it materializes into a fresh leaf, so only `index` is
authoritative and a reader re-reads the path through the graph when it needs
positions. `steps` is empty for a top-level (non-array) clash, and `root_a`/`root_b`
are the operands the trigger framed — the checker's source-meaningful sides, or the
cloned parameter and argument of an apply-time check — which is where a diagnostic is
attributed, the raw `a`/`b` leaves being the deep conflict.

A raw `UnifyError` records the deep conflict leaves, so the two **top-level** sides of
a failed parameter unify — the declared parameter type and the argument's own type —
are dropped; `ApplyError` is the record that keeps them, alongside the argument node
(the fallback span source), the apply operation node, and the index of the first unify
error this check produced. The apply node is the identity of the *edge* whose structure
the checker recorded (`Build::apply_edges`), so keying by it lets a diagnosis reach the
argument's source span even when the argument node is shared. One error is recorded
per apply node, so a later re-read of the same apply does not record it again.

The recursion guard has two halves, one per kind of pair. `path` guards only pairs
that have **nodes**; a pair of bare values has no class to name it — a
self-referential structure reached through the node-less arms (`[cell, self]`, the
term pair a type is) would repeat forever — so `depth` bounds that descent instead,
and a pair past `MAX_VALUE_DEPTH` (64) is given the benefit of the doubt exactly as
the unifier's cycle guard does.

A structural cycle cut marks the node it re-entered, and the two cases a `None` verdict
covers — never ran, versus in progress and assumed concrete — are told apart by that
mark, never by the verdict field alone. `close_operation_cycle` clears the verdict
*and* the cycle-cut assumption together, because both predate the operand edge and no
longer describe the node's graph.

**A function type is the one special arm.** Two function types unify by
descending the two functions' own signature cells positionally, the way two
arrays do; a function type and a self-cycle that is not a function type (the
universe, a recursive struct) are refused. `is_self_referential` is a *generic*
graph fact — a 2-element array one of whose elements points back at its own class
— and two such cycles merge successfully instead of tripping the cycle guard. See
[function-type-merge](function-type-merge.md).

## Block GC, tables, asserts

- **Block-level GC.** Values are collected by block (`garbage_collect(block_root)`),
  following the block's reachable nodes and dropping the rest; the static arena is
  never moved or compacted.
- **Tables** (`table{…}` / `t{k}`) are immutable and built once (`build_table`).
  Each entry stores its key's precomputed deep-content hash, the items are sorted
  by that hash, and a read binary-searches and verifies the equal-hash run with
  `key_eq`.
- **Asserts** are the recorded-constraint channel: a `PendingAssert` carries the
  condition, the live pair and the template it came from. `check_asserts`
  deep-evaluates the condition and requires `USize(1)`; a condition that stays
  undecided is pending, not failed, and the apply clone re-checks it per call.
  That is the mechanism a refinement rides
  ([operator-polymorphism](operator-polymorphism.md) §3).
- A **traced program value** may carry a handle into an arena and may carry nodes the
  lowlevel cannot see on its own: an operator's result is cached, and a cached node's
  operand is not followed, so a value holding a node has to name it or the node dies
  with the block the walk is vacating. The walk therefore enters through the value's
  own trace (`ValueExt::traced`) into **a scratch the GC owns**, and the walked value is
  then discarded — as the array and table arms discard theirs, because a node keeps its
  id across the move and only its block changes. The shape of that API is the borrow
  rule: the value looks through a shared `TraceContext` and the walk then mutates, so
  the two never hold a borrow of the module at once, and collecting into a scratch means
  a value never has to hold its references as one contiguous run of its own.

### Deep content keys

A table's keys are **deep content**: two keys compare equal when their evaluated
structures are coinductively equal. `Module::key_eq` is that comparison and the
**authority**; the stored hash only *finds candidates*. A read binary-searches the
payload for the equal-hash run and verifies every candidate with `key_eq`, so the one
direction the hash owes is *equal keys hash equal* and a collision is always sound —
every number in a payload is a pre-filter, never a decision.

The hash is the key's **canonical content unfolding**: it walks the same structure the
comparison walks, mixing in what each position *is* and never an address, a node
identity, or a freeze-assigned index. That is what lets it survive a freeze, so a
dynamic key and the static key the freeze files it as unfold to the same number.

- **A cycle is cut by the depth bound, not by a path check.** The walk unfolds to a
  fixed depth and every position at that frontier mixes in the same `FRONTIER_TOKEN`. A
  revisited-node token carrying *its own* revisit depth disagrees with `key_eq`, which
  cuts on the *pair* of nodes on its path: `[1, ↺]` and `[1, [1, ↺]]` are equal under
  it while their revisits sit at different depths. Truncating both at one depth is
  invariant under that equality, so the bound costs candidate quality and can never
  hide a key that is there.
- **A table and a function unfold to their content, not their identity.** `key_eq` keys
  a table by identity and a function by its id, and neither identity crosses a freeze: a
  dynamic handle's address and a `FunctionId` are process-local. A table unfolds to the
  fold of its entry keys' content hashes — the numbers the payload is already sorted by,
  so it is the key *set*'s — and a function to the shape of its template, whose content
  is what it returns and asserts. A template is unfolded in a second mode
  (`UnfoldMode::Template`) because a template is *expected* to hold undecided cells and
  is never compared by `key_eq`: that walk is total and reads a node's operation edge in
  preference to its memoized value, so its answer does not depend on how far the
  definition pass has run.

A coarser answer costs candidate quality and nothing else, since `key_eq` decides every
candidate offered. What the unfolding cannot tell apart: two keys differing only below
`UNFOLD_DEPTH`; two function templates differing only in which operator sits at a
position, since a program's operator vocabulary is not something the lowlevel can name
(they are still distinct functions, and `key_eq` keys a function by its id); two tables
with the same key set and different values; and two of the program's own value
variants, which are one opaque token each.

Keys are deep-evaluated when the table is built, so a stored key is fully concrete and
its hash stable for the table's whole life. A key that cannot be decided, or whose
content is an empty value (`LowValue::Error`), records an
`EvalError::TableKeyUndecided` and drops the entry; values are stored as lazy refs and
read on demand, like array items.

### Guards and what bounds them

The VM has three bounds and they catch three different shapes:

- **`apply_depth_limit` bounds nesting.** An unrolling self-apply nests one level per
  application and is refused here.
- **`apply_total_limit` bounds work.** A recursion whose return is a *cached pair*
  containing the recursion (`f(x) = [f(x), 0]`) evaluates each apply to its cached
  value and returns, so every apply sits at depth 1 and the nesting guard never fires;
  only the total-application budget catches it. The same bound turns an endless but
  non-nesting loop into a diagnostic instead of a hang.
- **`evaluate_depth_limit` bounds the walk.** A value that grows without nesting —
  `f(x) = [x, f(x)]`, where every apply level terminates but deep evaluation descends
  an unbounded tree — is caught here.

Two refusals are silent by construction: the deep pass returns **before** writing the
`evaluated_deep` flag for a frame it refused on, so a refused frame has no verdict at
all and stays undecided; and an undecided answer is never cached, so a block root that
is still lazy leaves that block's compaction with nothing to move.

On the union-find side, union by size **bounds the parent-chain depth** a class can
reach at a given size, so a deep chain cannot be produced through the public API — it
would take a hand-installed `parent` link, which `add_equality` refuses. When GC splices
a block's dead members out of a class the survivors keep the binding; when the
**representative** is the one that dies it is re-elected from the survivors and their
parents re-point at it.

## Static modules and low types

A frozen module is registered in the process-wide `Registry<P>`
([static-modules](static-modules.md)); a static node is read in place and never
moved, so materializing a frozen value means copying it into fresh dynamic leaves.

A **low type** is a class-routed lower bound on a value's shape (`LowShape`, with
`Unknown` as the lattice bottom). It is refined by observation at the two value
write sites, by the merge join, and by an abstract-interpretation pass over a
template (`infer_template_low_types`, in `low_type.rs`) that a caller seeds with
`seed_class_low_type` before running. It gives a backend a type answer without
reading the checker's encoding — [lowlevel-low-types](lowlevel-low-types.md).

A compiled function's **parameter and return are `[value, type]` pair cells**, and
that is the lowlevel's own convention rather than a format it consumes: `Function`
calls the parameter node a *pair*, the apply resolves its arity, and the evaluator
peels `Index(pair, 0)`. Every read of either node therefore goes through
`Module::pair_value_half` (the sibling `pair_type_half` reads element 1 by the same
rule), and the pair is two **or three** wide — the 3-wide form carries a perspective,
with the value at element 0 in both. `resolve.rs`'s `pair_value_half` is the one place
that knows this layout; the low-type pass in `low_type.rs` learns none of it.
