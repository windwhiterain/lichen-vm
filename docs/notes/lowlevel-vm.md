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

Functions are first-class: a function's body is a **template**, and an apply
clones the nodes whose value could differ per call — the parameter pair and the
return cells among them — so a signature is re-instantiated per call
(let-polymorphism) and the template is never bound. Recursion is a function
referring to its own pair; the apply and evaluation depth budgets record
`Module::budget_exhausted` and return a value instead of panicking, so a
non-terminating program is a diagnostic rather than an unwind.

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
