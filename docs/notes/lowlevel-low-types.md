# Low types for a lowlevel-based JIT

> Status: **implemented** — the layer exists and is what a backend reads: a
> **low type** is a class-routed lower bound on a value's shape (`LowShape`),
> refined by observation, by the class merge, and by an abstract-interpretation
> pass over a template. It is the answer to "what is this value's machine shape"
> that does not require reading the checker's `[value, type]` encoding.
>
> Points at: `crates/lichen-lowlevel/src/lib.rs` (`LowShape`),
> `crates/lichen-lowlevel/src/equality.rs` (class-routed storage, observation,
> the merge's refinement, the read APIs), `crates/lichen-lowlevel/src/low_type.rs` (the
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
USize | Float                the machine scalars
Tuple([Option<LowShape>, …]) a heterogeneous fixed-arity tuple
Array(Option<LowShape>, n)   a homogeneous fixed-length array
Function(Option<LowShape>, Option<LowShape>)
Table(Option<LowShape>, Option<LowShape>)
```

**The shape has no "unknown" variant** (the superior's verdict, round 2): a
position nothing states is `None`, so *getting* a low type answers
`Option<LowShape>` and every nested position is an `Option` too. Two statements
about one class are combined by picking whichever is `Some`; the highlevel
invariant says a class's seed and its observation agree, so a disagreement is a
`debug_assert` (§6), never a widening to a third value. The widening join and the
structural `refine` this note used to define are **deleted**.

**A disagreement is between views of one value, never between two values.** A merge
only happens where the two sides are one value or one side is undecided: `unify`
compares two decided sides and records its error *without* merging (`unify_inner`,
the `(Some, Some)` arm; the merge arms are `(None, None)` and one decided side), and
[lowlevel-vm](lowlevel-vm.md) records that precondition for `fill_class_holes`. So
"two different decided values in one class" is a unify error, not a class; and the
**highlevel invariant** is that what it seeds for a value and what observation
derives from that value agree up to the view rule above.

## 2. The mechanism

The core move: a low type is a **property of the union-find class**, read through
the representative, so identity is the class and not the node. The class stores an
`Option<LowShape>`; `None` is "nothing states it", the one undecided state there
is, and a statement is only ever filled in once. Observation states a **scalar**
only (a shape with positions is the checker's to state, from its type), so the two
can never disagree.

Three routes refine it:

- **Observation** — a concrete value written to a node refines its class's low
  type from the value's top-level variant tag. O(1) and monotone: only the tag is
  read, never the payload. It is hooked at both value-write sites —
  `write_node_value` (re-binding) and `add_node` (a literal, a marker, or a
  freshly built array arrives with its value already concrete) — because hooking
  only the first would leave every literal without a low type. A value the
  vocabulary has no shape for states nothing at all: the `Str` literal, the unit
  `None` and an empty `Error` all observe as `None`, which is what keeps
  observation from ever widening a class it knows more about.
- **The merge** — `add_equality` refines the two representatives' low types onto
  the new representative.
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

**The superior's verdict (round 2): a polymorphic template is not jittable.** A
`jit`/`parallel` whose parameter domain is undecided is **refused** with a
diagnostic that names the annotation as the fix; it must not stay lazy, and there
is no "a later pass compiles it". The earlier behaviour — `RoleUnread::Undecided`
returning `None` at the `parallel` operator, leaving the launch to retry — is
gone: an undecided domain is an error at the operator that needed it.

Per-call-site specialization — static abstract interpretation over call sites, or
launch-time specialization — is a separate future feature to be decided on its
own; the freeze/persist roadmap independently requires pre-apply compilation,
which is why the pre-apply case is the mainstream one.

## 4. Phases

| Phase | Content | Touch points |
|---|---|---|
| 3a | `LowShape` is scalar/tuple/array/function/table with `Option` positions; class-routed storage; observation hook in `write_node_value`; refinement in `add_equality`; read APIs | `lowlevel/src/equality.rs`, `lib.rs` |
| 3b | The abstract-interpretation pass: seeds, the transfer table, the extension hook, the template fixed point | `lowlevel/src/low_type.rs` |
| 3c | highlevel `low_type_of` (answers `Option`); `compile_fragment` rewired to seed → pass → read; the raw domain walks deleted; polymorphic `jit` → refused (the superior's verdict) | `highlevel/src/shape.rs`, `compute/src/compute.rs` |

## 5. Open questions

- **The exact lattice** — recursive shapes: a tuple domain requires
  `Tuple([t, …])`, so shapes stay recursive.
- **Who runs the pass, and when** — the `Jit` operator, on demand, per jitted
  template (pay-per-use).
- **Whether `LowShape` is absorbed** — yes: `LowShape` **is** the low type, plus
  `Unknown`. The dead variants and the write-only channel are resolved by giving
  the channel real traffic, not by removal.

## 6. What the implementation found

**The shape has no unknown, and one statement per class.** The superior's verdict
(round 2): `Unknown` is deleted, a position nothing states is `None`, and two
statements about one class combine by picking whichever is `Some` —
`LowShape::join` and `LowShape::refine` are both gone, replaced by the
`debug_assert` that a second statement equals the first. The assertion then
*measured* what had to change for the invariant to hold:

- the shared **wildcard** node was seeded from whatever pair listed it — skipped (it
  states nothing);
- a **data tuple** was read as a term pair because it has two items — the seed now
  requires a pair's **layout** (`docs/notes/array-attribute-view.md`), which width
  cannot tell;
- an **operation node whose own value is a pair** (an apply's result) was stated both
  by the seed (the pair view) and by the transfer (the bare result) — the transfer
  now **declines for pair-valued nodes**, and `Index(pair, 0)`'s transfer derives the
  result from the seeded pair;
- **observation** stated arrays (the physical view) while the checker stated their
  types (the tuple view) — observation now states **scalars only**; a shape with
  positions comes from the type alone (`low_type_of`), which is the authority.

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
  reads no layout — which is the coupling this design exists to remove. The two
  are told apart by **whether the decode succeeds**, not by a structural guess,
  which would not be sound: a term pair and a type value have the same two-slot
  silhouette. What separates them is the terminal marker — the type slot of a
  term names the *type of an expression*, which for a type expression is the
  `Type` marker, and `Type` is the one kind marker the decode refuses — so a
  pair never decodes directly and a bare type value always decodes on the first
  try. An undecided answer is `LowShape::Unknown` and never a fallback: a
  polymorphic parameter has no domain at this boundary, and a backend that
  invented one would compile a kernel for a type nobody wrote.

**The superior's verdict (round 2): a struct is interpreted as a tuple.** Its
fields are positional, so `low_type_of` reads a struct type exactly as it reads a
tuple type — the machine shape of a nominal struct is the tuple of its field
shapes. The nominal identity lives in the encoding (the marker payload), not in
the low vocabulary; the earlier refusal ("answering `Tuple(..)` would drop the
identity that makes it a struct") is retracted. Consequences: a named struct
kernel parameter's domain is a `Tuple`, so the struct refusal in
`lichen_compute::kernel_domain` (`DOMAIN_IS_A_STRUCT`) and the
`DomainStatement::Struct` classifier are obsolete.

The decode still refuses two things, for two different reasons: a `string` is not
a machine scalar, and a **type** is not a value at all. Whether a *kernel* can
lower a shape the decode does accept is a separate question
(`lichen_compute::kernel_domain`, [floating-point](floating-point.md) §3.8).

**The seed is not an optimisation — without it nothing in a body has a shape.** A
template is never evaluated and an apply binds the clones, so nothing in a body is
ever *observed*; the checker, though, wrote its decision into every term's type
cell. The transfers cannot see those cells (the pass never learns the
`[value, type]` layout) and observation is silent on a template, so an unseeded
body reads `Unknown` throughout and every class falls back to `Int`. A cell
nothing decides stays undecided, and a disagreement between a seed and a transfer
degrades to `Unknown`.

## 7. The other axis on the same slot

Low types are built on the value slot's **decided or not** axis: `Unknown` is the
conservative read of a class whose value is not decided yet (§2's lattice). That
slot carries a second, orthogonal axis: **has the computation run**. A node with
an operation and no cached value has not answered; an operation node whose answer
was undecided **has** run (the attempt happened and could not resolve, and the
slot stays empty), while a pure cell with an empty slot has nothing to run at
all.

The axis is a **stored field** (`Node::runned`, whose value is the value epoch the
operator ran at — zero for "never ran") with one named read,
`Module::has_no_result_yet` — the evaluator's own run gate, so the definition and
its only use cannot drift apart — and `Node::value`'s own doc states both axes. The
epoch rides the same field because it answers the next question the run axis is
asked: whether a *stalled* operator's reads would still decide the same
([stalled-rerun-epoch](stalled-rerun-epoch.md)).
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
