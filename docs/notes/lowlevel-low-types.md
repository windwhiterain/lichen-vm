# Low types: a variant-tag layer for a lowlevel-based JIT

> Status: **implemented** (Phases 3a–3c landed; `feature/low-types`) — the
> mechanism below is the decided design (recorded 2025, direction set in the
> ComputeJIT review: pre-apply JIT is the mainstream scenario, `LowShape`
> gains `Unknown` and becomes the low type). Relates to decision D5 in
> [type-system-cleanup-plan](type-system-cleanup-plan.md) and to
> [compute-jit-low-types](compute-jit-low-types.md); it removes the coupling
> labelled in [checker-encoding-instability](checker-encoding-instability.md),
> which that note now tracks in its narrowed scope.
>
> Three things the implementation found that the design below did not say, all
> recorded in §6.

## The problem

A JIT must not be built over the **highlevel** graph: the highlevel's type
checking is materialized *as* lowlevel execution logic (the apply-time
unifies are in the graph), so a JIT reading the highlevel encoding would have
to re-implement the compilation of that logic — and indeed the current
lichen-compute JIT already reverse-engineers the checker's pair encoding from
raw nodes (`compute.rs:1410-1630`), silently breaking on any encoding change.

But a JIT reading only the **lowlevel** graph has no types at all.

A measured fact about the state before this landed: the `Node::low_shape`
channel was **write-only** — the only writer was the compute JIT itself
(`compute.rs:689`, `compute.rs:742`) and no reader existed anywhere; the JIT
carried shapes in its own `KernelFragment::ParamSlot` instead. The whole
`LowShape` mechanism was plumbing with no traffic, which is what this design
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
`class_value` already uses, `equality.rs:81`), stored in the existing
`Node::low_shape` field. Two writers maintain it:

- **Observation — hooked into `write_node_value`** (`equality.rs:178`) and into
  `add_node` (`module.rs:110`), the single choke-point every value write already
  flows through
  (evaluation results, apply-clone bindings, launch-argument unification). When a
  concrete value binds, the class's low type refines from the value's variant
  tag — O(1), monotone, no extra traversal. Deep shapes are not unfolded at
  write time; a read recurses into element classes, which refine
  independently as they bind.
- **Class merge — hooked into `add_equality`** (`equality.rs:60`): a union
  joins the two representatives' low types onto the new representative.
  `Unknown ∨ k = k`; two equal `Known`s are unchanged; two different `Known`s
  join to `Unknown` — which the design expected to be unreachable, and is not
  (§6).

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
    seeds the parameter class and runs the pass. The authority also resolves
    the type slot's one indirection (`low_type_of_slot`), so the caller still
    reads no layout at all (§6).
  - **Transfers**: every binary operator (`Add`/`Sub`/`Mul`/`Div`/`Rem`, the
    comparisons, the bitwise set) transfers to `USize`;
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

## 4. Phases (all landed)

| Phase | Content | Touch points |
|---|---|---|
| 3a | `LowShape` gains `Unknown`; class-routed storage (read through the representative); observation hook in `write_node_value`; join in `add_equality`; read APIs (`class_low_type`, recursive `low_type_of_node`) | `lowlevel/src/equality.rs`, `lib.rs`; clone/freeze/codec plumbing already exists (`function.rs:303`, `static_module/freeze.rs`) |
| 3b | The abstract-interpretation pass: seeds, `LowOperator` transfer table, `OperatorExt` hook, template fixed point | new lowlevel module |
| 3c | highlevel `low_type_of` in `shape.rs` (Unknown explicit); `compile_fragment` rewired to seed → pass → read; `kernel_param_shape`/`element_shape` raw walks deleted; polymorphic `jit` → honest lazy + diagnostic | `highlevel/src/shape.rs`, `compute/src/compute.rs` |

`Option<LowShape>` keeps its two distinguished states: `None` = untraced
scaffolding (default, costs nothing), `Some(Unknown)` = traced but undecided.

The reads and the seed are `Module::class_low_type` (through the
representative), `Module::low_type_of_node` (the recursive read), and
`Module::seed_class_low_type`; `node_shape`/`set_node_shape` are gone, because
a node's own slot is no longer where the answer lives. `refine_class_low_type`
is the single write side all three routes share.

## 5. Open questions (resolved)

- ~~The exact lattice~~ — recursive shapes, decided: tuple domains require
  `Tuple([t, …])`, so shapes stay recursive.
- ~~Who runs the abstract-interpretation pass, and when~~ — the `Jit`
  operator, on demand, per jitted template (pay-per-use).
- ~~Whether `LowShape` is absorbed~~ — yes: `LowShape` **is** the low type,
  plus `Unknown`. The dead variants and the write-only channel are resolved
  by giving the channel real traffic (Phase 3a), not by removal.

## 6. What the implementation found, and what it is still open

Three corrections to the design above, all from measurement rather than
argument, and all still true of the code.

**The join's "unreachable" case is reachable, so it is not an assertion.**
The design reasoned that two different decided low types on one class cannot
happen, because the checker proved `value : type` consistent — and a
`debug_assert` sufficed. The first run of the compute suite fired it on 21 of
24 programs. The reason is that a unification **deferral** merges two classes
whose values were never compared: a pending computation against a skeleton, a
deferred field read, a type round-trip (`equality.rs`, `shape::defer_pending`).
Two arrays of different arity then legitimately share a class, and the class
is only reconciled later, if at all. The join therefore answers
`LowShape::Unknown`, which is the design's own safety argument: a reader
degrades to "undecided" rather than to a wrong shape. The cost is bounded by
what reads low types — a class whose writers disagree is a class the *encoding*
arrays live on (a pair, a kind, a tuple type's element list), none of which a
backend compiles against. One overlap *is* resolved rather than degraded: a
seeded `Tuple(..)` of arity `n` and an observed `Array(_, n)` are two views of
the same array value, so the tuple view wins.

**Observation needs two sites, not one.** The design names
`write_node_value` as the choke-point. It is the choke-point for *re-binding* a
node, but a literal, a kind marker, or a freshly built array arrives through
`add_node` with its value already concrete — so observing only at
`write_node_value` would have left every one of them without a low type and
made the channel blind to everything but the evaluated spine. Both are hooked.

**A type value is not read the way a kinded type looks.** Two encoding facts
that only the compiler could settle, and that the first version of
`shape::low_type_of` got wrong (all 24 compute tests answered `Unknown`):

- an **atomic** type's shape slot *is* its marker — `int` is literally
  `[int, K]` — while a **compound** type's marker lives in its kind. Reading
  the kind's marker for both classifies every scalar as an unrecognised kind.
- the parameter's type slot is not always a type value: the checker unifies an
  *annotated* parameter's type cell with the annotation expression's own
  `[value, type]` term pair, so the slot holds that pair, while an *inferred*
  one is bound straight to a type value. `low_type_of_slot` peels that one
  indirection, in the authority, so the caller reads no layout — which is the
  coupling this design exists to remove.

**Open, deliberately not started:**

- **Rendering the diagnostic.** `Module::extension_diagnostics` is the general
  channel (every other channel on a `Module` is a fact the VM itself can
  describe, and a plugin refusing to lower a computation has no such home),
  and `compute.jit` records into it instead of discarding the reason. Nothing
  renders it yet: deciding which extension diagnostics are user-facing, and
  attributing them to a source span, is a separate call.
- **Attribution.** `ExtensionDiagnostic::node` is `None` for the JIT's records,
  because `OperatorExt::run` is not handed the operator's node. Threading it
  through would be a public-trait break for a field nothing reads yet.
- **Per-call-site specialization** — the §3 future feature, still undecided.
- **A backend that reads the body's low types.** v1 reads the parameter
  domain; the pass computes every body node's low type and stores them, which
  is what a follow-on emitter would read instead of walking operand chains.
