# Low types: a variant-tag layer for a lowlevel-based JIT

> Status: **approved** — the mechanism below is the decided design (recorded
> 2025, direction set in the ComputeJIT review: pre-apply JIT is the mainstream
> scenario, `LowShape` gains `Unknown` and becomes the low type). Relates to
> decision D5 in [type-system-cleanup-plan](type-system-cleanup-plan.md), to
> [lichen-lowlevel-shape](lichen-lowlevel-shape.md), and removes the coupling
> labelled in [checker-encoding-instability](checker-encoding-instability.md).
> Implementation is phased (§4); each phase updates this status as it lands.

## The problem

A JIT must not be built over the **highlevel** graph: the highlevel's type
checking is materialized *as* lowlevel execution logic (the apply-time
unifies are in the graph), so a JIT reading the highlevel encoding would have
to re-implement the compilation of that logic — and indeed the current
lichen-compute JIT already reverse-engineers the checker's pair encoding from
raw nodes (`compute.rs:1410-1630`), silently breaking on any encoding change.

But a JIT reading only the **lowlevel** graph has no types at all.

A measured fact about the current state: the `Node::low_shape` channel is
**write-only** — the only writer is the compute JIT itself
(`compute.rs:689`, `compute.rs:742`) and no reader exists anywhere; the JIT
carries shapes in its own `KernelFragment::ParamSlot` instead. The whole
`LowShape` mechanism is plumbing with no traffic, which is what this design
turns into the real low-type layer.

## 1. The idea: low types

Give the lowlevel **low types**: the `LowValue` variant tag of a node's value
(`USize`, `Str`, `Array`, `Table`, `Function`, `None`, `Void`, …) plus,
recursively, element/shape information (`Array(t)`, `Tuple([t, …])`). This is
the existing `LowShape` concept (`lowlevel/src/lib.rs`) promoted from a
compute-plugin side channel to a first-class lowlevel notion, with one added
variant: **`Unknown`**.

Some low types are known when the graph is **built** (literals, constructors);
others only after unification/evaluation binds the cells (a parameter's value
is `Parameterized` until the apply clones and unifies it). So a low type is a
**lattice** `Unknown → Known(shape)`, refined monotonically.

## 2. The mechanism (decided)

The core move: a low type is a **property of the union-find class, maintained
by the VM**, not a fact the JIT must ask for at the right moment. The JIT
reads the class's current lower bound whenever it runs; the lattice's
monotonicity guarantees it can never read wrong, only `Unknown`. **No
notification mechanism exists or is needed**, because the JIT reads at its two
pre-existing moments (`jit` and `launch`).

Identity in the VM is the equivalence class, so the low type lives on the
class (read through the representative, the same `&self` walk
`class_value` already uses, `equality.rs:71-77`), stored in the existing
`Node::low_shape` field. Two writers maintain it:

- **Observation — hooked into `write_node_value`** (`equality.rs:93`), the
  single choke-point every value write already flows through (evaluation
  results, apply-clone bindings, launch-argument unification). When a
  concrete value binds, the class's low type refines from the value's variant
  tag — O(1), monotone, no extra traversal. Deep shapes are not unfolded at
  write time; a read recurses into element classes, which refine
  independently as they bind.
- **Class merge — hooked into `add_equality`** (`equality.rs:59`): a union
  joins the two representatives' low types onto the new representative.
  `Unknown ∨ k = k`; two equal `Known`s are unchanged; two different `Known`s
  are unreachable on a checked graph (the checker already proved
  `value : type` consistent — consistency comes free), so a debug_assert
  suffices.

And one computation route for what observation cannot reach:

- **Abstract interpretation — the pre-apply engine, run on demand.** A
  fixed-point pass over a function *template* computes low types **before any
  apply**, which is what makes pre-apply JIT (the mainstream scenario) work:
  - **When**: at `Jit` time, pay-per-use — a program that never jits never
    pays. This settles the open question "who runs the pass, and when".
  - **Seeds**: a template's parameter positions are the one thing the value
    graph can never decide (the template is never evaluated, so observation
    is silent on it). The seed is the parameter's **type slot**, decoded by a
    highlevel-side `low_type_of(type_node) -> LowShape` living in `shape.rs`
    — the encoding authority — with `Unknown` explicit instead of the current
    silent `USize` fallback (`compute.rs:937-964`). The lowlevel pass itself
    never learns the pair layout; the caller (the JIT's `compile_fragment`)
    seeds the parameter class and runs the pass.
  - **Transfers**: `Add`/`Sub`/`Leq`/`Eq → `USize`;
    `Index(Tuple(ts), k) → ts[k]`; `Apply(Function(d, c), _) → c`; the
    lowlevel owns the `LowOperator` transfers, extension operators go through
    an `OperatorExt` hook — the same `Program`-hook home as D1's deferral
    policy.
  - Unseeded positions stay `Unknown` and propagate conservatively; recursive
    templates converge at `Unknown` (compute v1 rejects recursion anyway).

**Consistency comes free.** The low type is a function of the value, and the
checker's unification already proves `value : type` consistent, so a low type
can never contradict the highlevel type. The JIT must still keep a
conservative fallback for `Unknown` (nodes still `Parameterized` after
checking, e.g. lazy branches).

**JIT boundary.** The JIT reads only low types, never the `[value, type]`
pair encoding — so Phase 1 of the cleanup plan (re-encoding freedom) stops
being a hazard for the JIT.

## 3. The hard boundary: polymorphic templates

Apply binds **clones**, not the template. A polymorphic template's parameter
class is therefore never refined by anything: observation is silent on
templates (they are not evaluated), and applies bind only the clones. No
mechanism can compile a polymorphic kernel pre-apply with a correct domain —
this is a fact of the language (names bind per apply; compare D3's "a
per-call-site fact by construction" in
[type-system-cleanup-plan](type-system-cleanup-plan.md)), not a mechanism gap.

The decided behaviour: `jit` of a template whose domain is `Unknown` at jit
time stays lazy and reports honestly, guiding the user to annotate the
parameter. Per-call-site specialization (static abstract interpretation over
call sites, or launch-time) is a separate future feature, to be decided on
its own; the freeze/persist roadmap (static kernel artifacts) independently
requires pre-apply compilation, which is why it is the mainstream.

## 4. Phases

| Phase | Content | Touch points |
|---|---|---|
| 3a | `LowShape` gains `Unknown`; class-routed storage (read through the representative); observation hook in `write_node_value`; join in `add_equality`; read APIs (`class_low_type`, recursive `low_type_of_node`) | `lowlevel/src/equality.rs`, `lib.rs`; clone/freeze/codec plumbing already exists (`function.rs:303`, `static_module.rs:539`) |
| 3b | The abstract-interpretation pass: seeds, `LowOperator` transfer table, `OperatorExt` hook, template fixed point | new lowlevel module |
| 3c | highlevel `low_type_of` in `shape.rs` (Unknown explicit); `compile_fragment` rewired to seed → pass → read; `kernel_param_shape`/`element_shape` raw walks deleted; polymorphic `jit` → honest lazy + diagnostic | `highlevel/src/shape.rs`, `compute/src/compute.rs` |

`Option<LowShape>` keeps its two distinguished states: `None` = untraced
scaffolding (default, costs nothing), `Some(Unknown)` = traced but undecided.

## 5. Open questions (resolved)

- ~~The exact lattice~~ — recursive shapes, decided: tuple domains require
  `Tuple([t, …])`, so shapes stay recursive.
- ~~Who runs the abstract-interpretation pass, and when~~ — the `Jit`
  operator, on demand, per jitted template (pay-per-use).
- ~~Whether `LowShape` is absorbed~~ — yes: `LowShape` **is** the low type,
  plus `Unknown`. The dead variants and the write-only channel are resolved
  by giving the channel real traffic (Phase 3a), not by removal.
