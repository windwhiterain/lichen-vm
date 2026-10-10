# Checker encoding: unstable at the `lichen-compute` boundary

> Status: **current — the label stands, narrowed to one component.** The
> encoding has one owner and a usable query surface; what remains unstable is
> `lichen-compute`'s emitter walking the `[value, type]` pair on the way into a
> kernel body. The kernel *domain* read is gone (it comes from the parameter's
> **low type**, [lowlevel-low-types](lowlevel-low-types.md)), and the lowlevel's
> own pair decoding is inventoried below with the defect among it closed.
> **Open:** the body's graph walk.
> Points at: `crates/lichen-highlevel/src/shape.rs` (the encoding authority) and
> `crates/lichen-compute/src/compute.rs` (the one component that still reads
> around it).

lichen's type representation is an untyped node graph with positional
conventions: every expression compiles to a `[value, type, attrs…]` pair, every
kinded type expression to a `[shape, [marker, universe]]` pattern, and every
type spine ends at the self-referential universe `K = [Type, ↺]`.

## The one owner

**Inside the project that encoding has one owner**: `crates/lichen-highlevel/src/shape.rs`
holds the layout as named constants and typed accessors, and the highlevel
checker, the renderer and the language composition read the layout through it.
The cleanup's re-encoding (the `shape` authority, the kind-marker registry,
single-sourced codec tags and attribute slots) ran through it, and every internal
reader followed. Two parts of the layout are **frozen contracts** because an
already-persisted artifact carries them: the attribute order (a pair's arity is
its schema tail, [attributes](attributes.md)) and each kind marker's persisted
codec tag (the registry in `shape.rs`).

**Outside the project** the pair and kind layout is **not** a frozen contract,
and this note is the statement of that. The instability is scoped to the JIT
boundary: anything that builds a program through the documented `Program` /
`Ctx` extension points — a compiler plugin, a package — is not reading the
encoding by hand and is not affected
([compiler-plugin](compiler-plugin.md), [plugin-taxonomy](plugin-taxonomy.md)).

## The query surface, and the field read it decided

`shape` answers *type* questions; the class vocabulary belongs to the leaf crates,
so an adapter answers *class* questions on top of it. Built and consumed:

| Item | Consumer |
|---|---|
| `TypeRef::{Term, Slot}` + `term` | `struct_fields_of_slot` (`compute.rs`), the one caller that does not know which it holds |
| `field_list` | the same, and `field_type` |
| `field_type` | `Checker::slot_read` — a concrete container's field read |
| `field_names` | `struct_fields_of_slot` |
| `name_table_index` | `Checker::named_field_index_any` — the fold over a name table, shared so the two gates cannot decode an entry differently |

A `TypeRef` is the distinction the encoding has and the type system does not: a
type **term** is the `[shape, kind]` pair itself, a type **slot** is a node
holding one. Resolving one into the other is the authority's decision, so no
caller unwraps a slot by hand. `field_index` (its recorded consumer is the kernel
specialize pass) and the compute-side `class_of` adapter are **not built**: a fold
with no caller is surface this tree does not keep, and the field read below
removed the case that needed `class_of`.

**The symptom the accessors fixed, measured in plain lichen.** A field read's
*type* used to be `Index(Index(container_ty, 0), key)` — an operation node — even
when the container's type was concrete and the checker had already resolved the
field's position for its own guard. A *cell* reader cannot see through that:
`low_type_of_slot` reads a node's value, so `check_binop`'s class question found
neither operand concretely `Float`, pinned the operation to the `Int` default,
and then refused its own operands against it. `Checker::slot_read` now reads the
field's type straight out of the container type's field list when the position is
known (`shape::field_type` on a `TypeRef::Term`), and keeps the lazy `Index` when
it is not.

| Program | Before | After |
|---|---|---|
| `Par = struct<.n Int, .alpha Float>; (x : Par) => x.alpha + x.alpha` | `expected Int, found Float` (both operands) | `1.0: Float` |
| `(x : <Int, Float>) => x(1) + x(1)` | `expected Int, found Float` | `1.0: Float` |
| `Outer = struct<.in Inner>; (x : Outer) => x.in.a + x.in.a` | `expected Int, found Float` | `1.0: Float` |
| the same three with one concrete `Float` operand (`x.alpha * 2.0`) | already worked | unchanged |

The last row is why this is the *class* question and not the unify: a unify
against a decided `Float` forces the lazy `Index` and passes, while the predicate
that chooses the class cannot.

### The boundary rules

- **Do not unify `ScalarClass` and `LowShape`.** A shape is a structure (`Tuple`,
  `Array`, `Function`, `Table`, `Unknown`); a class is one bit of meaning
  (integer or float). `low_type_of` returning `Unknown` for a struct is correct,
  and the fix for the resulting gap is an accessor, not a wider shape.
- **Do not move the encoding down into `lichen-lowlevel`.** The lowlevel's value
  is being a generic graph; `shape` exists because the encoding needed a home
  above it.
- **Do not make `shape` depend on the class.** The class belongs to the leaf
  vocabulary; the class riding in two places is a kernel-ABI fact
  ([floating-point](floating-point.md) §4.4), not a type-encoding one.
- **Do not delete the value walk in `compute.rs`.** A literal read looks through
  `value_of` extractions and materialized arrays, which is a property of *values*,
  not of types. Only the type half moves.
- **`Err` versus `None` stays the caller's distinction.** "Not this shape" is
  `Option`; "this shape, undecided" is a named refusal, never a collapsed `None`.

## The lowlevel readers too

An audit of `crates/lichen-lowlevel` for the pair convention. Six readers, and
the distinction that matters is not *how much* each one knows but **whether the
value it reads is an operand of the operation being performed**:

| site | what it reads | verdict |
|---|---|---|
| `apply.rs` (two sites) | element 1 of a **parameter** node — the declared type, for `ApplyError` attribution only | **Wiring.** The apply pass built or cloned the pair it is reading; element 1 is an operand of the check it is performing. |
| `apply.rs` (`wire_apply_result`) | element 1 of the **return pair** it just cached, to bind the checker's result cell | **Wiring.** The cell is the third operand of the apply. |
| `resolve.rs` (`pair_value_half`) | a **body's** parameter or return, and a 2..=3 width test, to decide which half is the value | **Encoding reader, and the lowlevel's own convention.** `define_in(function, node)` derives the domain itself, through `parameter_leaves`, from the pair the template carries; where the analysis starts from `Function`'s two nodes — `parameter_leaves`, and `loop_conversion.rs`'s state and spine roots — there is no caller to state it. |
| `loop_conversion.rs` | a template's parameter, to resolve a read to the path into the parameter's value, and its return, for the spine's roots | **The same standing as `parameter_leaves`.** The loop's analysis is a template-shape question, so it reads the pair the template carries. |
| `equality.rs` (`is_static_universe_id`) | a 2-element array whose element 1 points back at the same module and index | **Mixed.** The `[Type, ↺]` universe is the canonical instance, but the predicate is stated as a *generic* graph shape and unifies any two such cycles. It decodes the positions and reads no meaning. |
| `evaluation.rs` (`table_get_operands`) | elements 0 and 1 of a `TableGet`'s operand, which the arm's own check already proved is a 2-element array | **Not an encoding reader.** A destructure of `[table, key]`; no pair is involved. |

A **per-analysis** boundary is the honest description; a crate-wide one is not:
`low_type.rs` states "the pass never learns the `[value, type]` pair layout",
which is a claim about *that pass*, while `apply.rs` and `control_flow.rs` are in
the same crate and do learn it.

**The defect among the readers is answered by a stated convention, not by a
sniff.** A function's `parameter` and `r#return` are `NodeId`s that hold the
checker's pair, and `Function` says nothing about their shape. `Module::define_in`
derives the domain itself — `parameter_leaves` reads the pair the template
carries — and the analyses that *start*
from the pair read it deliberately rather than sniffing for a width. Such a
read is the lowlevel's own stated convention about `Function`, not a heuristic
standing in for one. The lowlevel is the evaluator the checker's graph was
written for — `P::Value` for the lichen frontend is the checker's own term — so
the pair is inside the VM's value domain rather than a format it consumes, and
there is no revision in which the lowlevel was free of the layout.

## What `lichen-compute` still reads

`lichen-compute`'s JIT is the one component that does not use the authority. It
reads the conventions out of raw nodes:

- `value_of_node` follows a `value_of` extraction — an `Index(pair, 0)` over a
  pair node — down to the pair's value slot, the same access path the checker
  itself uses to reach most values.
- The native operators read a kernel signature's domain and codomain through raw
  `Index` chains — `Index(sig.ty, 0)`, then `Index(sig_shape, 0)` /
  `Index(sig_shape, 1)`.

**The write side is closed.** The native operators all build a function type
through the one constructor, `ctx.arrow(domain, codomain)` (with the same three
nodes in the same order, so the artifact is byte-identical). The **read** side is
deliberately still raw: a signature that is not yet bound has to resolve at apply
time, so those sites must keep the lazy `Index` chain and cannot use the
authority's bound-type reader (`Module::function_type_signature`).

**The domain read is gone.** `kernel_param_shape` and `element_shape` — the two
functions that re-derived a kernel's domain by walking the type half of the
parameter pair — were deleted. The domain now comes from the parameter's low
type, and the *seed* for that low type is read once through the authority
(`shape::low_type_of_slot`), a single named call rather than a convention the JIT
re-derives. That was the read with the most reach: a `string` domain used to
decode as a machine scalar, so the old walk was not merely coupled, it was wrong
where it was silent.

What remains is the **body's graph walk**: the emitter reaches a value's
computation by following `Index` chains from a known pair, and reaches a parameter
element by its index path. Building a body's graph no longer needs the pair — the
graph facts moved to `resolve.rs` without a control-flow graph, and the pair
reads that remain there are the two the analysis *starts* from
(`parameter_leaves`, `loop_conversion.rs`) rather than a value the caller already
held.

## What would remove the label

A backend that reads each body node's **low type** instead of walking the pair
would remove the last of it: the abstract-interpretation pass computes and stores
every body node's low type, and [lowlevel-low-types](lowlevel-low-types.md) gave
the lowlevel the layer. That is recorded as the open item in
[compute-jit-low-types](compute-jit-low-types.md), not as work in flight.
