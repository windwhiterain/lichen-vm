# Low types for a lowlevel-based JIT

> Status: **implemented** — the layer exists and is what a backend reads: a
> **low type** is a class-routed lower bound on a value's shape (`LowShape`),
> refined by observation, by the class merge, and by an abstract-interpretation
> pass over a template. It is the answer to "what is this value's machine shape"
> that does not require reading the checker's `[value, type]` encoding.
>
> Points at: `crates/lichen-lowlevel/src/lib.rs` (`LowShape`),
> `crates/lichen-lowlevel/src/equality.rs` (class-routed storage, observation,
> the merge join, the read APIs), `crates/lichen-lowlevel/src/low_type.rs` (the
> abstract-interpretation pass), `crates/lichen-lowlevel/src/module.rs`
> (`add_node`, the second observation site),
> `crates/lichen-highlevel/src/shape.rs` (`low_type_of`,
> `low_type_of_slot` — the seed), `crates/lichen-compute/src/compute.rs`
> (`compile_fragment`, the consumer).
>
> Companions: [compute-jit-low-types](compute-jit-low-types.md) (what a backend
> traces from a shape), [checker-encoding-instability](checker-encoding-instability.md)
> (the coupling this removes, and the residue it leaves).

## 1. The idea: low types

A JIT must not be built over the **highlevel** graph: the highlevel's type
checking is materialized *as* lowlevel execution logic — the apply-time unifies
are in the graph — so a JIT reading the highlevel encoding would have to
re-implement the compilation of that logic. But a JIT reading only the
**lowlevel** graph has no types at all: a node carries a value (possibly still
undecided) and an operator, and nothing says what shape the value will take.

A **low type** closes that gap: the `LowShape` of a node's eventual value,
promoted from a compute-plugin side channel to a first-class lowlevel notion. A
layer above the lowlevel that *has* the type decodes it once, and a backend reads
the low type instead of the encoding. `LowShape` is host metadata, never a lichen
value — sibling to `ArrayItem::shallow` and `Node::evaluated_deep` — so "a type
is just a value" (`Type : Type`) is untouched.

```text
Unknown                      the bottom: traced, but nothing has decided it yet
USize | Float                the machine scalars
Tuple([t, …])                a heterogeneous fixed-arity tuple
Array(t, n)                  a homogeneous fixed-length array
Function(domain, codomain)
Table(key, value)
```

`Unknown` is the only answer a reader may get for a value the graph cannot
decide, and every backend must handle it conservatively. The join of two lower
bounds (`LowShape::join`) is the least shape at least as precise as both:
`Unknown ∨ k = k`, two equal decided shapes are unchanged, and **two different
decided shapes join to `Unknown`** — deliberately, and not an assertion (§6). The
one overlap that is resolved rather than degraded: `Tuple` of arity `n` and
`Array(_, n)` are two views of the same array value — a seeded positional domain
and an observed homogeneous one — so the **tuple view wins**, being strictly more
precise.

## 2. The mechanism

The core move: a low type is a **property of the union-find class**, read through
the representative, so identity is the class and not the node. An
`Option<LowShape>` keeps two distinguished states: `None` is untraced scaffolding
(the default; costs nothing), `Some(Unknown)` is traced but undecided.

Three routes refine it:

- **Observation** — a concrete value written to a node refines its class's low
  type from the value's top-level variant tag. O(1) and monotone: only the tag is
  read, never the payload. It is hooked at both value-write sites —
  `write_node_value` (re-binding) and `add_node` (a literal, a marker, or a
  freshly built array arrives with its value already concrete) — because hooking
  only the first would leave every literal without a low type.
- **The merge join** — `add_equality` joins the two representatives' low types
  onto the new representative.
- **The abstract-interpretation pass** — `infer_template_low_types`, on demand,
  over a function **template**. It is the only route that reaches a template's
  parameter positions, which observation cannot: a template is never evaluated
  and an apply binds the *clones*, so nothing is ever written onto the template's
  own cells.

Deep shapes are never unfolded at write time; a **read** recurses into element
classes (`low_type_of_node`), which refine independently as they bind. The reads
and the seed are `class_low_type`, `low_type_of_node` and `seed_class_low_type`;
`refine_class_low_type` is the single write side all three routes share, so
refinement is monotone by construction.

### 2.1 The pass

Seeding is the caller's job. A layer above the lowlevel decodes the parameter's
**type slot** through the encoding authority (`shape::low_type_of_slot`) and
calls `seed_class_low_type`; the pass never learns the pair layout and never
invents a parameter's type. That is what keeps the lowlevel honest — the graph
facts live here, the type facts live with whoever owns the encoding.

The transfer function reads a node's operation and the low types of its operand
array's elements: every structural binary operator transfers to `USize`;
`Index(Tuple(ts), k)` yields `ts[k]` (the index must be a decided scalar, or a
mis-selected element would be a wrong type rather than an undecided one);
`Apply(Function(_, c), _)` yields `c`; `TableGet(Table(_, v), _)` yields `v`; an
extension operator states its own result through the vocabulary's `low_type`
hook. The pass is monotone on a finite lattice and re-runs a node only when one
of its operands' classes actually moved, so it halts at a fixed point even on a
cyclic template. It runs at `Jit` time — pay-per-use, so a program that never
jits never pays.

## 3. The hard boundary: polymorphic templates

Apply binds **clones**, not the template. A polymorphic template's parameter
class is therefore never refined by observation: observation is silent on
templates (they are not evaluated), and applies bind only the clones. No
mechanism can compile a polymorphic kernel pre-apply with a correct domain —
this is a fact of the language (names bind per apply), not a mechanism gap.

The decided behaviour: a `jit` of a template whose domain is `Unknown` at jit
time stays lazy and reports honestly, guiding the user to annotate the parameter.
Per-call-site specialization — static abstract interpretation over call sites, or
launch-time specialization — is a separate future feature to be decided on its
own; the freeze/persist roadmap independently requires pre-apply compilation,
which is why the pre-apply case is the mainstream one.

## 4. Phases

| Phase | Content | Touch points |
|---|---|---|
| 3a | `LowShape` gains `Unknown`; class-routed storage; observation hook in `write_node_value`; join in `add_equality`; read APIs | `lowlevel/src/equality.rs`, `lib.rs` |
| 3b | The abstract-interpretation pass: seeds, the transfer table, the extension hook, the template fixed point | `lowlevel/src/low_type.rs` |
| 3c | highlevel `low_type_of` (Unknown explicit); `compile_fragment` rewired to seed → pass → read; the raw domain walks deleted; polymorphic `jit` → honest lazy + diagnostic | `highlevel/src/shape.rs`, `compute/src/compute.rs` |

## 5. Open questions

- **The exact lattice** — recursive shapes: a tuple domain requires
  `Tuple([t, …])`, so shapes stay recursive.
- **Who runs the pass, and when** — the `Jit` operator, on demand, per jitted
  template (pay-per-use).
- **Whether `LowShape` is absorbed** — yes: `LowShape` **is** the low type, plus
  `Unknown`. The dead variants and the write-only channel are resolved by giving
  the channel real traffic, not by removal.

## 6. What the implementation found

**The join's "unreachable" case is reachable, so it is not an assertion.** The
design reasoned that two different decided low types on one class cannot happen,
because the checker proved `value : type` consistent — and a `debug_assert`
sufficed; measurement says otherwise. A unification **deferral** merges two
classes whose values were never compared (a pending computation against a
skeleton, a deferred field read, a type round-trip), so two arrays of different
arity can legitimately share a class, and the class is reconciled later, if at
all. The join therefore answers `Unknown`, which is the design's own safety
argument: a reader degrades to "undecided" rather than to a wrong shape. The cost
is bounded by what reads low types — a class whose writers disagree is a class
the *encoding* arrays live on (a pair, a kind, a tuple type's element list), none
of which a backend compiles against.

**The exception is a pair, and it is seeded deliberately.** The JIT seeds each
template term's type cell onto two things: the term's value slot takes the cell's
class, and the **pair** takes `Tuple([shape, Unknown])`. The pair is one of the
arrays above, and it is seeded anyway because a body term is reached through
`Index(pair, 0)` — an `Index` reads its *container*, so without the pair's half
the seed lands on a channel nothing reads. The cost is that a pair's class is now
*stated* rather than degraded; what buys the safety back is that the statement is
the checker's own conclusion (`value : type`), so it cannot contradict a
transfer — and where a seed and a transfer do disagree, the lattice still joins
them to `Unknown`.

**Observation needs two sites, not one.** `write_node_value` is the choke-point
for *re-binding* a node, but a literal, a kind marker, or a freshly built array
arrives through `add_node` with its value already concrete — so observing only at
`write_node_value` would have left every one of them without a low type and made
the channel blind to everything but the evaluated spine.

**A type value is not read the way a kinded type looks.** Two encoding facts,
both of which an earlier structural decoder got wrong (every compute test
answered `Unknown`):

- An **atomic** type's shape slot *is* its marker (`int` is literally
  `[int, K]`), while a **compound** type's marker lives in its kind. Reading the
  kind's marker for both classifies every scalar as an unrecognised kind.
- The parameter's type slot is not always a type value: an *annotated*
  parameter's type cell holds the annotation expression's own `[value, type]`
  term pair, while an *inferred* one is bound straight to a type value.
  `low_type_of_slot` peels that one indirection, in the authority, so the caller
  reads no layout — which is the coupling this design exists to remove.

## 7. The other axis on the same slot

Low types are built on the value slot's **decided or not** axis: `Unknown` is the
conservative read of a class whose value is not decided yet (§2's lattice). That
slot carries a second, orthogonal axis: **has the computation run**. A node with
an operation and no cached value has not answered; an operation node whose answer
was undecided **has** run (the attempt happened and could not resolve, and the
slot stays empty), while a pure cell with an empty slot has nothing to run at
all.

The axis is a **stored field** (`Node::runned`) with one named read,
`Module::has_no_result_yet` — the evaluator's own run gate, so the definition and
its only use cannot drift apart — and `Node::value`'s own doc states both axes.
The axis was first *derived* from the slot through a pair of predicates that the
class scans read; what broke that derivation is the distinction the class channel
needs: an answer an operator *produced* versus a value a unification *asserted*
into its slot, which the slot cannot tell apart. So the field answers it, the
derived predicates are gone, and of the class scans only `class_committed_value`
survives. What the slot still decides is whether a *read* runs the operator, and
it must: an empty slot is re-read, which is how a later binding is observed. The
only invariant the field needs (an operation node's *cached* answer is decided or
absent; it may still *compute* undecided) is stated where it is enforced,
`evaluate_node_operation`'s postlude.

## 8. What is deliberately open

- **Rendering the diagnostic.** `Module::extension_diagnostics` is the general
  channel for "a plugin declined to lower this computation" (every other channel
  on a `Module` is a fact the VM itself can describe), and `compute.jit` records
  the reason into it instead of discarding it. Nothing renders it yet: deciding
  which extension diagnostics are user-facing, and attributing them to a source
  span, is a separate call. `ExtensionDiagnostic::node` is `None` for the JIT's
  records because the operator's node is not handed to the extension hook;
  threading it through would be a public-trait break for a field nothing reads.
- **Per-call-site specialization** — §3's future feature, still undecided.
- **A backend that reads the body's low types.** The first consumer reads the
  parameter domain; the pass already computes and stores every body node's low
  type, which is what a follow-on emitter would read instead of walking operand
  chains ([checker-encoding-instability](checker-encoding-instability.md)).
