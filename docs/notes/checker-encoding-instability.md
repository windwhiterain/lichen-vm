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
caller unwraps a slot by hand. The two are both two-slot arrays, so no
structural test separates them; the side that holds one always knows which it
has, and a caller that genuinely does not ask twice — once per variant — and
lets the decode answer (`struct_fields_of_slot`). The hand-written unwrap the
variant exists to stop is `Index(ty, 0)`: that is a holder's value slot *or* a
term's shape, and only the caller's own knowledge says which it just read.
`field_index` (its recorded consumer is the kernel
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

## Recovered measurements

### The canonical universe is a fixed point, not a value

The self-referential universe `[Type, ↺]` is a value whose type is itself, so
it cannot be rebuilt by an apply clone: a clone allocates a *fresh* self-loop and
unification cannot equate that loop with the canonical one — the path guard
reports a conflict instead. Proving the universe concrete before the definition
pass is what makes the apply clone walk reference it in place. The three type
constants are proved concrete for the same reason, and the two shared index
constants and the kind markers rely on the same untagged-node property.

`is_universe_any` answers a dynamic node by equality-class comparison against the
module's canonical universe node, and a static ref by content (a two-item
self-referential array). The shared node exists at all because cloning the
universe breaks unification, so the composite must be shared rather than rebuilt
— the same reason `Ctx::universe` is a reference and not a constructor. The
matching renderer facts are in
[universe-containment](universe-containment.md) §2.

### The IR is a DAG, so an expression is compiled once

Statement bindings pre-resolve every use of a name to the value's own
`ExprId`, so one expression can be referenced from several parents. Compiling
it once and reusing the pair matters because a recompile allocates fresh state
again — a struct type's nominal id comes from a per-compilation `Fresh` call —
and that silently breaks the sharing the frontend relies on.

### Why the skeleton gate is a membership test

A cycle can only form through a block-wide binding placeholder; an inline
compound term's subtree can never reference its own root. Pre-registering a
skeleton for such a term would add spurious cells that poison the apply-time
unify — a placeholder reached through an index-typed apply would stay an
undecided `?a` instead of binding to the actual type. The gate must therefore
test `block_roots` membership alone: the frontend transplants the binding value's
kind into the placeholder, so a block root may be *any* kind, and a
hand-maintained kind list silently misses a variant — a self-reference through
an unlisted kind then re-enters the check forever. For a childless kind the
skeleton is inert and the epilogue binds it away; a `Function` block root
pre-registers its own pair in `check_lam` before its body compiles, overwriting
the skeleton.

### An inference hole in either position

`_` is an inference hole in either the value or the type position, so both slots
are fresh undecided cells and whatever the context unifies them with binds them.
The kind slot must be a cell and not the universe, because a compound type's
kind slot holds a kind expression (`[FunctionType, Type]`), which would clash
with `Type` itself.

### A masked error region is an opaque leaf

A recovered-error region compiles to a pair of fresh, never-unified cells:
nothing inside it is checked, so it cannot introduce a spurious type-level
"expected X, found Y" from inside itself — the parser's own syntactic diagnostic
still fires at the parse layer — and the undecided cells never cause a cascade.
The region stays a distinct kind from a real `_`, so the frontend can mask it
for a content signature or diff.

### A constant offset is a lazy `Index` chain

`lazy_index_path` is the runtime form of a constant encoding offset: the nested
`Index` chain resolves when the base binds. It is how the checker walks the
struct name paths `STRUCT_TYPE_NAMES_PATH`, `STRUCT_KIND_NAMES_PATH` and
`STRUCT_KIND_NAMES_ORDER_PATH` (spelled in `crate::shape`). Each step's
subscript is a constant node, shared for `0` and `1`.

### What `LiteralExt::build` may return

A literal builds its `[value, type]` pair itself through `LiteralExt::build`:
the built-in int literal and the type-constant literal each build their own
value and type nodes, referencing the prebuilt singleton expressions the context
exposes, and a custom literal may build any value/type pair that references
other expressions. A type-constant literal is the exception — `Type : Type` is
built as the self-referential universe node, so it is not a `[value, type]`
pair at all.

### The `slot0_is_shape` heuristic, and what it is not

`slot0_is_shape` decides "element 0 is an array ⇒ this is a shape" rather than
an expression's `[value, type]` pair. It misfires on a pair whose **value** is
an array — a tuple value is an array, and so is a struct marker's payload
(`[payload, TypeStruct]`) — so a diagnostic path through such a pair may tag a
`Value` descent as `Shape`. It is a diagnostic rendering hint only: the unify
itself is unaffected.

### Naming the layout: role, not structure

An expression pair and a kinded type expression are distinguished by **role**,
never by structure: `[value, type, attrs…]` and `[shape, kind]` have
positionally identical slots (`0`/`1`) but mean different things, so each gets
its own constant names. The same reasoning repeats one level down, in the shape
half: an array's element position and a function's domain position are both
`"0"`, and a reader reusing one spelling for both would be reading a convention
rather than the layout.

The struct marker is where the naming pays off: it is an ordinary
`[value, type]` pair whose **type** read is the `TypeStruct` atom, so "is this a
struct marker?" is a check of a type constant rather than a guess about the arity
of an open encoding. Its payload is `[TypeId, names, names_in_order]`, and the
last field exists for the one reader a name→index table cannot serve: a named
instantiation through an **unresolved** callee reorders its arguments when the
struct type resolves, and that reorder needs the field's *name* at each
definition position — the table's inverse, which a lazy `Index` cannot derive.

### The definition-order names path is a deliberate follow-up

The deferred named instantiation reads a TypeStruct kind's definition-order
names through `STRUCT_KIND_NAMES_ORDER_PATH` (`kind[0][0][2]`) rather than
through the `STRUCT_TYPE_NAMES_PATH` spelling, because reading *the term's*
slot 1 would pull the shape half into the read's operand chain. That hazard
belonged to the old forced pass, which walked every element of an operation's
operand array rather than only the selected one — the shape half, which holds
the deferred reorder's own field-type probe, would then have been forced
mid-read and the read would have met itself. **The operand forcing is gone**, so
whether the `STRUCT_TYPE_NAMES_PATH` spelling is safe again is an open
follow-up rather than a re-derived fact; the path is kept as the direct read of
the kind node.

### Arrow terms, and the guard that skips them

`is_arrow_type_any` recognises a concrete **arrow-term** function type —
`[shape, [FunctionType, K]]`, the spelling a written `A -> B` lowers to —
because the checker's function-ness guard skips these: only concretely
*non*-function types are caught statically. A function's **own** type
(`f : f`) needs no recognition here, because the unifier already descends into
the two functions' cells when it reaches one.

### The set instance's exact encoding

A set instance is typed `[members, [[element type], [TypeSet, Type]]]`, whose
shape is the element type *alone* — a set has no length, so `set{a}` and
`set{a, b}` share one type, and that is what separates the set kind from
`array<T, n>`. The value is an ordinary array node, exactly the node an array
literal's elements make, and nothing tags it.

### The kind-marker registry

`ValueType::is_kind_marker` is the **honest tag test** behind "is this atom a
type-level marker", as opposed to a structural guess (an array of the right
arity). `TypeId` is not a kind marker and is tested through
`ValueType::type_id` instead.

### Two child lists, and which one is the authority

`IR::range_children` is the variadic arena only, so a descent built on it would
skip every named operand (`Apply`, `BinOp`, `Index`, …) and miss most of the
graph; `ExprKind::children` is the complete list, which is what a walk over the
IR needs. `range_children` is the **open** end of the encoding — a new kind that
stores its children as a `ChildRange` must be added to the range arm, and a
wildcard would let it compile and then panic at run time — and `children` names
its non-variadic kinds rather than catching them by a wildcard, because a new
kind that forgets to list its children is a walk that silently skips it. The
checker's own `range_children` is the authority, not `IR::range_children`: it
already spelled the list out, *including* `Set`, which the first draft of the
`IR` method left out. The `IR` method is kept as the *closure* end, so a
caller's mistake reads as a `debug_assert` naming itself rather than a wrong
child list.

### The read forms and the tables they walk

`NamedField` (`a.name`) reads the name table out of the container type's
**kind** (`container_ty[1][0][1]`); `RawNamedField` (`X::a`) reads it directly
from the container's **type**, which must be a TypeStruct (`container_ty[0][1]`),
and yields the field's *type* as a value — `S::a` on
`struct<.a Int, .b string>` is `Int : Type`. The raw form's requirement is a
check-time unify (a concretely non-TypeStruct container is a diagnostic), so it
is *not* raw in the no-validation sense of `RawIndex`. `RawIndex` (`X<e>`) is
the genuinely unvalidated read — no array-type pinning, no guard, no bounds
assert — reading a component of a type-as-value or of any expression's value,
both lazily, so an undecided container resolves at the apply; this is the form
the `T<e>` array-type postfix used to be, and the array type is now `TypeArray`,
spelled `array<T, n>`. `Field` (`a(k)`) works over a tuple element or a struct
field because both shapes are positional type lists and the nominal struct id
lives in the kind. The read *forms* are distinguished syntactically and the
source spelling is the whole rule: the adjacent single-expression paren
(`a(1)`) is a slot read, the glued comma-disciplined paren (`a(1,)`, `a(1,1)`,
`a()`, `a(,)`) is an instantiation, a spaced paren is an application, and the
adjacent brace (`t{k}`) is a table lookup whose entry has a key deep-content-equal
to the given one. `Instantiate` checks the value's element types against the
struct's field list and the expression's type is the struct type itself; a
non-struct callee is an `InstantiateCallee` diagnostic. `Record` is its
anonymous sibling — the checker builds the struct type from the value's element
types — and a `let` field never reaches it, being a block-local.
