# Low types: a variant-tag layer for a lowlevel-based JIT

> Status: proposed — a design discussion (recorded 2025, from the type-system
> cleanup review), not approved yet; relates to decision D5 in
> [type-system-cleanup-plan](type-system-cleanup-plan.md) and to
> [lichen-lowlevel-shape](lichen-lowlevel-shape.md).

## The problem

A JIT must not be built over the **highlevel** graph: the highlevel's type
checking is materialized *as* lowlevel execution logic (the apply-time
unifies are in the graph), so a JIT reading the highlevel encoding would have
to re-implement the compilation of that logic — and indeed the current
lichen-compute JIT already reverse-engineers the checker's pair encoding from
raw nodes (`compute.rs:1410-1630`), silently breaking on any encoding change.

But a JIT reading only the **lowlevel** graph has no types at all.

## The idea: low types

Give the lowlevel **low types**: the `LowValue` variant tag of a node's value
(`USize`, `Str`, `Array`, `Table`, `Function`, `None`, `Void`, …) plus,
recursively, element/shape information (`Array(t)`, `Tuple([t, …])`). This is
the existing `LowShape` concept (`lowlevel/src/lib.rs`) promoted from a
compute-plugin side channel to a first-class lowlevel notion.

Some low types are known when the graph is **built** (literals, constructors);
others only after unification/evaluation binds the cells (a parameter's value
is `Parameterized` until the apply clones and unifies it). So a low type is a
**lattice** `Unknown → Known(shape)`, refined monotonically.

## Design points from the discussion

- **Attach to the equivalence class, not the node.** Identity in the VM is
  the union-find class; `write_node_value` refines the class's low type when
  it binds — monotone, no extra traversal.
- **Two computation routes, used together:**
  - *Observation* — `evaluate_node` records the variant of every value it
    computes. Zero cost, always correct, but only knows evaluated nodes; an
    unapplied function template's parameter positions stay `Unknown`.
  - *Abstract interpretation* — each operator gets a low-type transfer
    function (`Index : Array(t) × USize → t`, `Add : USize × USize → USize`;
    plugin operators via an `OperatorExt` hook), and a fixed-point pass over
    the graph computes low types *before* apply, enabling call-site
    specialization (polymorphic-inline-cache style).
- **Consistency comes free.** The low type is a function of the value, and
  the checker's unification already proves `value : type` consistent, so a
  low type can never contradict the highlevel type. The JIT must still keep a
  conservative fallback for `Unknown` (nodes still `Parameterized` after
  checking, e.g. lazy branches).
- **JIT boundary.** The JIT reads only low types, never the `[value, type]`
  pair encoding — so Phase 1 of the cleanup plan (re-encoding freedom) stops
  being a hazard for the JIT.
- **Same hook as D1.** The low-type maintenance is, like the unification
  deferral rules, policy growing inside the lowlevel; both belong to the
  `Program`-hook design of Phase 2 (decision D1, option A).

## Open questions

- The exact lattice: are `Array(t)`/`Tuple([t…])` recursive shapes, or is a
  flat variant tag enough for the JIT's calling conventions?
- Who runs the abstract-interpretation pass, and when (per apply? per kernel
  launch? once per template)?
- Whether `LowShape` is absorbed into (renamed) the low-type type, and what
  happens to its currently dead variants (`Array`/`Function`/`Table` are
  matched but never constructed).
