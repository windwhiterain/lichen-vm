# Proposal: one query surface for types and classes

> Status: **proposal — not started.** Written for a session that will build it;
> nothing here is implemented except where marked "exists".
> Companions: [compute-param-struct-handoff](compute-param-struct-handoff.md)
> (the in-flight kernel work this unblocks), [floating-point](floating-point.md)
> §4.2/§4.4/§5.1 (the class model), [lowlevel-low-types](lowlevel-low-types.md)
> (the seed → pass → read chain).
> Touches: `crates/lichen-highlevel/src/shape.rs` (the home),
> `crates/lichen-compute/src/compute.rs` (the main consumer),
> `crates/lichen-compute-gpu/src/spirv.rs` (the second).

## 1. The complaint, stated as evidence

Reading a value's type out of the IR is done by hand, in more places than the
encoding has readers, and each place re-derives what another already decided.
Four measurements, all from the current tree.

**(a) `shape` has predicates but no accessors.** The public surface is
`is_struct_type_any`, `is_function_type_any`, `is_positional_type_any`,
`struct_names_any`, `struct_fields_by_shape`, `low_type_of`, `low_type_of_slot`.
There is no "the field list of this type" and no "the field at index *k*". The
one function that decodes values — `low_type_of` — **deliberately refuses
structs**:

> `low_type_of` is *not* used to decide — it answers `Unknown` for every struct
> by design (`lichen_highlevel::shape`), because a nominal struct has no low
> shape.

So a consumer that meets a nominal struct has no semantic question left to ask
and drops to the encoding. Three sites do exactly that today, each walking the
**same** type→kind→marker→names path:

| Site | How it walks it |
|---|---|
| `shape::struct_term_parts` (`shape.rs:719`) | `array_items` at each of the three steps |
| `Checker::lazy_index_path` + `structs.rs:142`/`:218` (`STRUCT_KIND_NAMES_PATH`, `STRUCT_TYPE_NAMES_PATH`) | builds the `Index` chain |
| `struct_type_names` (`compute.rs:5455`) | `type_term_slot` at each step — written for this branch |

`struct_type_names` is that duplication in the small: ~40 lines that re-express
a walk an authority already owns.

**(b) A type has no identity, and the ambiguity is a real bug.** A type is a raw
`AnyNodeId` that is *either* a type expression (the `[shape, kind]` pair) or a
node that *holds* one (an expression's type cell). The two are told apart by
doc-comment. The cost was measured in this branch: for an unresolved parameter,
`low_type_of_slot` returns a **constant** — `Static(10)`, the `TypeInt` term —
rather than an index node. `array_items` accepts both, so an unwrap that assumed
"a term is a two-slot node whose slots are index nodes" silently took the
constant's value apart and returned the wrong answer. Diagnosing that took
several instrumented runs; a newtype would have made it unrepresentable.

**(c) There is no "is this a `Float`?" predicate.** You write a comparison
against a shape:

```rust
// crates/lichen-highlevel/src/checker/operators.rs:138 and :173
low_type_of_slot(&self.module, AnyNodeId::Dynamic(ty)) == LowShape::Float
```

That is a *shape* question standing in for a *class* question, and it is the
only spelling available. It also cannot be answered for a struct, so a class
question about a nominal type has no answer at all through this route.

**(d) The class rule exists four times.** The question "which class is this
value" is answered at:

| Site | Channel |
|---|---|
| `operand_class` (`dev`'s validator) / `node_class` (this branch, `compute.rs:4141`) | the value or its low type |
| `param_leaf_classes` (this branch) | the parameter's **slot** shape |
| `bin_class` (`dev`'s SPIR-V emitter) | same three arms as `operand_class`, verbatim, in another crate |
| `fragment_class` + `local_class` (`dev`) | the fragment, then the parameter list |

Two crates, two channels, one question — and the file already warns what that
does, about `element_stride`:

> A literal here would be a second copy of the rule, and it is exactly the drift
> that one produced: this target read a float buffer at four bytes while the host
> wrote it at eight, and a float fragment was undispatchable until the width moved
> onto the class.

That change moved the fact onto the class so it would not be recomputed. The
class itself has not had the same treatment.

**The live symptom.** On `feature/kernel-param-struct`, three conversion tests
fail for exactly this reason. `emit_node` binds
`let class = node_class(module, node)` (`compute.rs`) and `node_class` reads the
value/low-type channel — but a kernel is lowered from a template, so a parameter
leaf's type cell is an undecided `_` and the channel states nothing. The leaf's
class is the **slot's** (`param_shape`), which only the parameter list knows. In
`(x : Float => int2float (x > 1.0))`, `x` comes back `Int`, `int2float` emits
`Int → Float`, and the comparison's operands are then read as `Int` against
`Float` and refused as a malformed fragment.

## 2. What is being proposed

Two levels. The first is what unblocks the failing tests; the second is what
stops the duplication rather than relocating it.

### Level 1 — a query surface, in the encoding authority

**Home: `lichen-highlevel::shape`.** It already declares itself "the single
authority for the highlevel's pair/type encoding", and the leaf crates already
depend on the crate. The boundary rule to hold:

- **`lichen-lowlevel` must not learn the encoding** — it stays a generic graph.
- **`shape` must not learn `ScalarClass`** — the class is the leaf vocabulary's,
  not the encoding's.

That second rule splits the surface: `shape` answers *type* questions, and a
small adapter in `lichen-compute` answers *class* questions on top of it.

#### In `shape.rs`

```rust
/// A type as a *slot* (a node holding a type) versus a *term* (the `[shape,
/// kind]` pair itself).  The distinction the encoding has and the type system
/// does not: resolving one into the other is this module's decision, so no
/// caller unwraps a slot by hand again.
pub enum TypeRef { Slot(AnyNodeId), Term(AnyNodeId) }

impl TypeRef {
    pub fn resolve<P: Program>(module: &Module<P>, id: AnyNodeId) -> Option<TypeRef>;
    pub fn term<P: Program>(self, module: &Module<P>) -> Option<AnyNodeId>;
}

/// The field-type list of a value type: entry 0 of the term's shape.  The
/// accessor `struct_fields_by_shape` half-provides and three sites walk.
pub fn field_list<P: Program>(module: &Module<P>, ty: TypeRef) -> Option<AnyNodeId>;

/// The field type at position `k` of a value type.
pub fn field_type<P: Program>(module: &Module<P>, ty: TypeRef, k: usize) -> Option<AnyNodeId>;

/// The field names of a named struct type, in field order.  Currently
/// `struct_type_names` in `compute.rs`, re-expressing `struct_term_parts`.
pub fn field_names<P: Program>(module: &Module<P>, ty: TypeRef) -> Option<Vec<Option<&'static str>>>;

/// Where a name sits in a value type's field list — the fold a named read
/// needs, and the one thing the checker's `named_field_index_any` does that no
/// lowering-side reader can currently do.
pub fn field_index<P: Program>(module: &Module<P>, ty: TypeRef, name: &str) -> Option<usize>;
```

#### In `lichen-compute` (an adapter, since only it can see both vocabularies)

```rust
/// The class a value node is.  **One question, two channels, one answer**: the
/// parameter slot first, because a template's parameter type cell is undecided
/// and only `param_shape` states the leaf; the value/low-type channel second.
/// This is the function whose absence produced the failing tests.
fn class_of<P: Program>(module: &Module<P>, params: &[ParamSlot], node: NodeId) -> ScalarClass;

/// Whether a type slot is the `Float` type — the predicate that replaces
/// `low_type_of_slot(...) == LowShape::Float`.
fn is_float_slot<P: Program>(module: &Module<P>, ty: AnyNodeId) -> bool;
```

The **channel ordering is the contract**, not an implementation detail: slot
facts outrank value facts because the ABI typed the slot and the template did
not type the value. `class_of` is the only place that ordering is written down.

### Level 2 — a fragment plan

Reading the encoding once is good; recomputing a decided fact three times is the
actual cost. The class of every value in a kernel body is decided during
emission, and then re-derived by:

1. the validator (`check_flow`/`check_instr`, to refuse a mix),
2. the wasm emitter (`lower_instrs`' representation stack, for `Conv`),
3. the SPIR-V emitter (`Kind::Scalar`/`Kind::Literal`, for the same).

Proposal: lowering emits **one plan** alongside the body —

```rust
/// What one instruction's value is, decided once at emission.
pub struct ValueFacts {
    /// The class the value is, as the language names it.
    pub class: ScalarClass,
    /// The representation the value is *held* in, when the ABI typed it: a
    /// parameter leaf's class is its slot's, and the two differ — a float
    /// fragment's index and count are `f32` locals carrying integers.
    pub representation: Option<ScalarClass>,
}
```

— and each consumer **looks up** instead of deriving. The validator becomes a
per-instruction check against the plan rather than a second analysis; the wasm
representation stack and the SPIR-V `Kind` stack both disappear.

This is deliberately the same move `element_stride` made: put the fact where it
is decided and let consumers read it.

### Why Level 2 is worth it, and why it comes second

It overlaps a change already requested for the wasm backend: type a block by the
classes of its values rather than by `WasmState.class` (a fragment-wide `T`).
Both are "stop re-deriving, read the decided value", and if Level 2 lands first
the block-type change falls out of it — the join's class is recorded rather than
recomputed. But Level 2 needs Level 1's surface to be expressible at all, so
Level 1 goes first and is independently useful.

## 3. Migration: which existing readers die

| Reader (this branch) | After Level 1 |
|---|---|
| `struct_type_names` (`compute.rs:5455`) | deleted → `shape::field_names` |
| `param_value_shape` (`compute.rs:5675`) | deleted → `shape::field_list` |
| `type_term_slot` (`compute.rs:5511`) | deleted → `shape::field_type` / `TypeRef` |
| `resolve_literal_node` (`compute.rs:4173`) | survives — it is `value_of` + `pair_value_node` + `concrete_element`, a *value* walk, not a type read |
| `scalar_literal` (`compute.rs:4200`) | survives, calls the new class adapter |
| `node_class` (`compute.rs:4141`) | becomes `class_of`, gaining the slot channel |
| `struct_field_index` / `resolve_steps` (this branch's named-read fix) | keep, but call `shape::field_index` instead of walking |
| `class_of` in `checker/structs.rs` (`named_field_index_any`) | may collapse into `shape::field_index` — the two are the same fold over different sides |

`scalar_class_of` (`compute.rs:3787`, `LowShape → ScalarClass`) stays: it is the
mapping from the *other* representation, and the proposal does not try to unify
`LowShape` with the class. They answer different questions and the notes already
say so (`lowlevel-low-types.md`).

## 4. Non-goals, and the traps

- **Do not unify `ScalarClass` and `LowShape`.** A shape is a structure
  (`Tuple`, `Array`, `Function`, `Table`, `Unknown`); a class is one bit of
  meaning (integer or float). `low_type_of` returning `Unknown` for a struct is
  *correct*, and the fix for the resulting gap is an accessor, not a wider shape.
- **Do not move the encoding down into `lichen-lowlevel`.** The lowlevel's whole
  value is being a generic graph; `shape` exists because the encoding needed a
  home above it.
- **Do not make `shape` depend on the class.** The class belongs to the leaf
  vocabulary (`floating-point.md` §4.4's "the class rides in two places" is about
  the *kernel ABI*, not the type encoding).
- **Do not delete the value walk in `compute.rs`.** `resolve_literal_node` looks
  through `value_of` extractions and materialized arrays, which is a property of
  *values*, not of types. Only the type half moves.
- **`Err` vs `None` stays the caller's distinction.** A second failure mode of
  this branch was an API that collapsed "not a struct field read" and "the type
  is undetermined" into one `None`; the refusal for the second is a deliberate
  compile error. Any new query should keep them apart — `Option` for "not this
  shape", a named refusal for "this shape, undecided".

## 5. Order of work

1. **`TypeRef` + the four accessors** in `shape.rs`, with `struct_fields_by_shape`
   and `struct_term_parts` re-expressed on them (one walk, not four).
2. **`class_of`** in `lichen-compute`, slot channel first; `check_instr` and
   `lower_instrs` take it as the authority.
3. **Delete the readers** in §3 and re-point their callers.
4. **Verify**: the three currently-failing conversion tests
   (`a_jit_kernel_crosses_the_two_classes_both_ways`,
   `a_conversion_the_body_cannot_hold_is_refused_by_name`,
   `a_varying_float_element_is_seeded_from_the_index`) must pass, and the suites
   listed in `compute-param-struct-handoff.md` §6 stay green.
5. **Then** the fragment plan (Level 2) and the block-type change it enables.

Step 4 is the acceptance test for Level 1: it is a bug this proposal's absence
produced, not a hypothetical.

## 6. Try this first

Before adopting any of it, spend twenty minutes on this: write the *use site* you
want, as code that does not compile yet.

```rust
let names = shape::field_names(module, ty)?;          // not a 40-line walk
let at    = shape::field_index(module, ty, "a")?;     // not a TableGet fold
let class = class_of(module, params, node);           // one answer, not two channels
```

If those three read as obviously right, the boundary in §2 is the one to build.
If one of them feels wrong — most likely `class_of` taking `params`, which is the
one place this proposal makes a caller pass context it would rather not — that is
the design question worth settling before writing the module.
