# Lowlevel VM

> Status: current
> Points at: `crates/lichen-lowlevel` — see the module rustdoc for `evaluation`,
> `apply`, `function`, `equality`, `gc`, `table`, `assert`, `static_module`.

`lichen-lowlevel` is the runtime the whole system is built on. It evaluates a
`[value, type]` program and, because unification runs here, it is also the
typechecker — "the runtime is the typechecker."

## What it computes

- **Lazy evaluation.** An expression is forced only when its result is needed; an
  untaken branch (e.g. `[then, else][i]`) is never evaluated.
- **First-class functions, closures, higher order.** Functions are values.
- **Recursion.** Recursive function application, with depth/total guards.
- **Unification.** The equality machinery that keeps the pair consistent; the apply
  and index guards run through it.
- **Block-level GC.** Values are collected by block; the static arena (below) is never
  moved or compacted.
- **Normalization, asserts, and tables** (`table{…}` / `t{k}`).
- **Values and types are one runtime kind** (`LowValue`): numbers, arrays, tuples,
  functions, structs, and type constants all live side by side.

## A node graph, not a tree

A `Module<P>` is a dense id-keyed arena of nodes (`NodeId`). Functions, arrays and
pairs are **shared** nodes, not copies — a binding is graph sharing, so a name's every
use is the same node. The `Program` trait parameterizes the instance over
value/attribute/operator/literal types, so one runtime serves every layer.

## Key types

- `Module<P>`, `NodeId`, `LowValue` — the evaluator.
- `Registry<P>` — the process-wide shared store of static modules (see
  [static-modules](static-modules.md)).
- `AnyNodeId = Dynamic(NodeId) | Static(StaticNodeId)` — a ref that may live in a
  dynamic (building) module or a frozen static one.

## Observing it

- `evaluation::evaluate_node` / `evaluate_node_deep` force a node to a value.
- The `tests/basic/` integration tests cover evaluation, equality, GC, recursion,
  tables, asserts and static modules.
