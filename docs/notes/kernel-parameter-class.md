# The kernel parameter's class: two measured losses

> Status: **current, and both defects are fixed.**  Two defects that both surface
> as `the kernel parameter's class is not decided when the kernel is compiled`,
> measured on `dev` at `fb62d96`.  Neither is the defect the two parked
> `#[ignore]` reasons in `crates/lichen-language/tests/compute.rs` named, and
> neither is what [function-type-merge](function-type-merge.md) said it was; the
> corrections are in [what this refutes](#what-this-refutes).  **Defect II** (the
> decoder could not read a written arrow's node) is fixed in `7e795f9`, and the
> case it parked is un-parked.  **Defect I** (a frozen module's applied struct
> lost its field-type cells) is fixed by making the frozen node mirror the
> ordinary one: the maintainer's ruling was that an applied kernel struct's type
> **must** carry the signature and that a class write **must** cross the frozen
> boundary, and `StaticNode` now carries the fields that let the materialize walk
> carry the answer it already solved.  Points at:
> `crates/lichen-highlevel/src/shape.rs` (`low_type_of`, `low_type_of_slot`),
> `crates/lichen-lowlevel/src/{equality,static_module}.rs`,
> `crates/lichen-compute/src/compute.rs` (`compile_fragment`, `kernel_domain`,
> `argument_kind`), `crates/lichen-render/src/render/value_printer.rs`
> (`raw_cells`).

## Defect I — a frozen module's applied struct loses its field-type cells *(fixed)*

A struct built by a **frozen** (imported) module, whose field argument is the
frozen callee's **own parameter cell**, gets field-type cells that hold nothing.

Two files, no operator, no `.native`, no `jit` — `b.lichen` is frozen because it
is imported:

```lichen
S = _x => struct<.I _, .O _>
mk = x => (S _)(.I x, .O x)
```

`main.lichen`:

```lichen
---
w = import "b.lichen"
---
s = w.mk Int
s
```

Measured: `(raw Int, raw Int): struct<.I raw[?a, ?b], .O raw[?c, ?d]>`, and the
field read `s.I` is `raw Int: raw[?a, ?b]`.  The **same two lines in one file**
give `(Int, Int): struct<.I Type, .O Type>` and `s.I` is `Int: Type`.

Dumped from the module, the difference is one cell: the applied struct's
field-list entries are two-cell arrays whose `slot` is empty and whose `class` is
`None` in the frozen case, and `slot = TypeValue(TypeType)` with
`class = Some(TypeValue(TypeType))` in the local one.  So the field *read* and
the *printer* agree because they read the same node
(`checker/structs.rs:159-170`'s `field_type`, `value_printer.rs:264`'s
`element_any`), and the hole is upstream of both.

**It is not tuple-specific and not compute-specific.**  Necessary and sufficient
among the variants measured: the struct constructor is in a frozen module *and*
the field argument is the frozen callee's own parameter.  Either one removed
decides the cell:

| program | render |
|---|---|
| frozen `mk 5` (a scalar argument) | `(raw 5, raw 5): struct<.I raw[?a, ?b], .O raw[?c, ?d]>` |
| frozen `(w.S _)(.I Int, .O Int)` (a literal argument, no parameter indirection) | `(Int, Int): struct<.I Type, .O Type>` |
| frozen `mk <Int, Int>` | `(raw[raw[raw Int, …]], raw[raw[raw Int, …]]): struct<.I raw[?a, ?b], .O raw[?c, ?d]>` |
| the `compute.jit` shape with `.native 1` instead of a kernel | the parked defect, no compute and no operator |
| the same `jit` local | `(1, Int, Int): struct<.native Int, .I Type, .O Type>` |

**Which step — measured, and the answer is not the one the first reading guessed.**
The frozen template's field-type entries and the applied function's parameter
type cell are **literally one class**: for the two-line repro, `mk`'s parameter
type cell `n47` has class `{47, 68, 71}`, and `n68`/`n71` are `S`'s two
field-type entries.  The apply's clone map holds exactly **one** member of it
(`n47`); the others have no in-edge except the **cached value of the residual
operation node** (`n58 = [n67, n74]`, with `op_operand = n57`), and
`static_node_apply` drops a residual operation's cached answer — only the operand
is walked (`static_module/apply.rs:149`).  So the class is never reconstructed:
identity is re-established only through the items the walk happens to clone
(`regroup_clones` is not at fault — a class whose members both got cloned is
re-united, measured on `{60, 46}`).  The applied struct's type therefore holds
clones of a **different generation** — the frozen `S` re-applied at run time —
which are cells nothing binds.

Neither candidate recorded here first holds: **no field-type item stays an inline
static ref** (measured per item; every one is a `Dynamic` clone made by the
`undecided` arm), and the frozen template's four cells are all `undecided`, not
baked.  The loss is on the clone/class side.

**The local path differs at exactly that step, and the difference is the repair.**
The dynamic walk **carries a residual operation's own answer** when the template's
operator produced it and the deep pass ran (`function.rs:524`) and then **maps the
answer's items onto this call's clones** (`value_apply`, `function.rs:577`); the
static walk dropped the answer and re-ran the frozen callee instead.  It dropped it
because it **could not ask the question**: the frozen node stored one collapsed
flag, `evaluated_deep.is_none_or(|e| e.undecided)`, where the rule tests two facts
apart, `runned && evaluated_deep.is_some()`.

**The fix is the mirror the maintainer ruled for.**  `StaticNode` carries `runned`
and `evaluated_deep` exactly as `Node` does; `undecided()` is *derived* from the
latter rather than stored beside it; `freeze` copies both instead of collapsing
them; and `static_node_apply` runs the dynamic clone rule itself, with the policy
stated once — `apply::answer_elements_are_undecided`, read by both walks.  The
persisted artifact encodes the two fields and `ARTIFACT_FORMAT_VERSION` went `9` →
`10`: a version-`9` artifact's collapsed byte cannot be read as the verdict it was
derived from, so the bump turns that into the recompile the version check intends.

Measured on a clean rebuild: the two-line repro's imported render is
**byte-identical to its local control**, `(Int, Int): struct<.I Type, .O Type>`,
from `(raw Int, raw Int): struct<.I raw[?a, ?b], .O raw[?c, ?d]>`.  The
expectation that pinned the old render, `a_kernel_value_and_type_render_by_name`,
is re-pinned to
`(raw Kernel, Int, Int): struct<.native raw[?a, ?b], .I Type, .O Type>` — each
signature field's **type** is `Type` (the field holds a *type value*, and the
universe is the type of one) while its **value** is that type.  Unchanged:
`lichen-lowlevel --test basic` 155/0, `lichen-highlevel --test checker` 87/0,
`compute` 57/0/3, `graph_jit` 8/0/1, `graph_structure` 4/0, `examples` 1/0,
`persist` 15/0, `artifact_transitivity` 1/0, `frozen_function_type` 1/0,
`cargo check --workspace` clean.

**One prediction this section made first is refuted.**  It recorded that mirroring
the carry rule alone would not be enough, because `wire_apply_result`'s
`write_node_value` (`apply.rs:161`) would put the re-run's product into a node the
carried answer's class already held, so the reconciliation would compare the answer
with itself.  With the mirror in place that did not happen: `wire_apply_result` is
**unmodified** and the render moved to the control.  The probe that suggested
otherwise carried the answer without the `runned` claim at all, and a first attempt
at the mirror inverted that claim (`runned` set where the items' slots were
*empty*); both left the render unmoved, and with the claim stated as the dynamic
rule states it — `runned = mapped.is_some_and(|value| !undecided(value))` — it
moved.  The claim follows `mapped`, not `carried`: that was the whole of it.

**The static type-value gap is a different mechanism**, measured separately: a
*reader* cannot name a static ref — `low_type_of`'s new arm is gated on
`AnyNodeId::Dynamic` (`shape.rs:851`) and `pair_type_half` answers `None` for a
static type half (`resolve.rs:324`) — so a frozen function type decodes as
`Function(Unknown, Unknown)` while the same node local decodes fully.  It is not a
clone walk dropping an edge, neither causes the other, and the program that shows
it (`w.mk Int`'s neighbour, `w.wrap`) renders correctly.

### What it means for the parked test

`crates/lichen-language/tests/compute.rs:782`'s
`a_tuple_domain_kernel_type_renders_as_a_function` is this defect.  Its `#[ignore]`
reason is wrong in three measured ways:

1. it locates the fix at "a class for a tuple of element types" — the class
   **exists** (the tuple kind, which `compound_type` reads: `(S _)(.I <Int, Int>, .O Int)`
   renders `(<Int, Int>, Int): struct<.I TypeTuple, .O Type>`), and a decided
   field cell renders `<Int, Int>`, not the parked `raw[Int, Int]`;
2. it calls the dump tuple-specific — `mk 5` dumps the same way;
3. its "the domain **and codomain** cells dump as `raw[raw[raw Int, …]]`" is
   false for its own program: `.O` is `raw Int`, flat.  Only a *tuple* codomain
   dumps nested.

And its expectation is not producible by the value printer at all: the only
raw-list production is `raw_cells` (`value_printer.rs:386-405`), which marks
every cell ("the mark **nests**, one per reading",
[raw-rendering-mark](raw-rendering-mark.md)) — so `raw[Int, Int]`, with unmarked
cells, is the *type* printer's spelling and not a value spelling.

## Defect II — the decoder cannot read a written arrow's node

A written arrow compiles to a lambda, and a function's own type is the
self-referential node `[Function(fid), ↺]`.  `low_type_of` decodes the older
`[shape, kind]` arrow term at `shape.rs:913-928` and has no arm for the new node:
slot 0 holds a `LowValue::Function` and slot 1 is the node itself, so the decode
falls through to `LowShape::Unknown` (`shape.rs:929-930`), and
`low_type_of_slot`'s one pair-indirection retry (`:956-974`) fails too because
slot 0 is a `Function` leaf rather than an array.

Measured structure of the parameter's type cell:

| parameter | renders | `low_type_of` | node |
|---|---|---|---|
| `Int` | `Int` | `USize` | `[none, [none, ↺…]]` |
| `Int -> Float` | `Int -> Float` | **`Unknown`** | `[Function(fid), [Function(…), ↺…]]` |
| `<Int, Int -> Float>` | `<Int, Int -> Float>` | `Tuple([USize, Unknown])` | the tuple, whose second element is the row above |
| a local arrow wrapper's parameter | `raw[?a, ?b] -> raw[?c, ?d]` | **`Unknown`** | the same self-cycle |

**The arrow's node shape is the variable, not freezing**: the same node is
`Unknown` in a local module.  Downstream, `compile_fragment` seeds from
`low_type_of_slot` (`compute.rs:2384`) and `kernel_domain` refuses with
`UNDECIDED_DOMAIN` (`compute.rs:2962`), so
`compute.jit (p : <Int, Int -> Float> => p(0))` is refused **at the `jit`** even
though the parameter is annotated — the annotation is present and its first
position *is* decided.

That refusal is what parks `a_float_domain_is_permitted_at_every_position_the_walk_reaches`
(`compute.rs:1531`), whose last sub-case wants the refusal at the **launch
argument**.  Decisive support that the decoder is the whole blocker there: when
the class is supplied by an accepted shape in another position, the launch walk
already names the offending element — `compute.kernel_launch: argument element 1
is a function, not a concrete Int or Float, so the kernel's parameters could not
be filled` (`compute.rs:5539-5553`'s walk, `argument_kind` at `:5598`).

**Repair, landed and measured** (`7e795f9`).  `Module::function_type_signature`
(`crates/lichen-lowlevel/src/equality.rs`) reads the function type's domain and
codomain **type cells** — the parameter pair's type slot, not the pair, because a
template's parameter value cell is empty until an apply binds it, and
`Function::return_type` — and `low_type_of` gained the one arm that asks it
(`shape.rs`), gated on the lowlevel's own recogniser.  Both spellings of a function
type now build their shape through a single `function_shape`, and the recognisers
became `&self` reads over the no-compression `class_root`, so the decoder never
needs a mutable borrow.

Measured, and it moves the refusal to where it can name a position:

| program | before | after |
|---|---|---|
| `jit (p : <Int, Int -> Float> => p(0))` | `UNDECIDED_DOMAIN`, at the `jit` | `kernel_launch: argument element 1 is a function, not a concrete Int or Float` |
| `jit (p : <Int -> Float, Int> => p(1))` | `UNDECIDED_DOMAIN` | the same, at **element 0** |
| `jit (p : Int -> Float => p)` | `UNDECIDED_DOMAIN` | `kernel domain must be a scalar or a tuple of scalars` — the whole domain is a function, so there is no scalar lowering to refuse later |
| a float-domain kernel, an unannotated parameter, the scalar control, the function-codomain program, the frozen two-file probe | — | **unchanged** |

`a_float_domain_is_permitted_at_every_position_the_walk_reaches` therefore runs and
passes, and its `#[ignore]` is deleted: `lichen-language --test compute` is
**57 passed / 0 failed / 3 ignored**, from 56/0/4.

Two gaps the arm does **not** close, both measured: a **static** function-type node
passed directly as a `type_value` still answers `Unknown`, because the recogniser
takes a `NodeId` and the arm is gated on `AnyNodeId::Dynamic` — the frozen two-file
probe above is the case that shows it; and `pair_type_half` answers `None` for a
static type half of a dynamic parameter pair, which makes the whole signature
`None` (the same class of gap, not measured to matter anywhere).

### The message covers two conditions

`UNDECIDED_DOMAIN` (`compute.rs:2947`) is one text for two facts.  At
`compute.rs:2962` (a shape *was* read, and a position of it could not be) the text
is false: it asks the author to annotate a parameter that **is** annotated
(`p : <Int, Int -> Float>`), names no position, and the only action it suggests
cannot change the outcome.  At `compute.rs:2392` and its parallel twin
`compute.rs:2598` (the parameter's type cell is genuinely empty — an *unannotated*
parameter) the same text is accurate.

The arm above removes the function-arrow instance of the false case, so that
message is no longer reachable through it — but the conflation itself stood at
`compute.rs:2962`, which also serves the empty case (a traced-but-undecided class
reads back `Some(Unknown)` rather than `None`) and any other unreadable position.

**Split, and measured on three programs.**  `kernel_domain` now takes the fact it
cannot read for itself — `DomainStatement` — and the caller asks the parameter's
own type cell.  The three decoded shapes are why one text could not tell them
apart:

| program | decoded domain | refusal now |
|---|---|---|
| `p : In`, `In = struct<.a Int>` | `Array(Unknown, 2)` | the domain **is a struct type**, and a kernel domain must be a scalar or a tuple of scalars |
| `y => y + y` (unannotated) | `Array(Array(Unknown, 2), 2)` | `UNDECIDED_DOMAIN` verbatim — annotate the parameter |
| `p : <Int, _>` | `Tuple([USize, Unknown])` | **position 1** has no class; the position is named and nothing asks for an annotation |

The classifier reads the parameter's type cell: a struct type term
(`struct_fields_of_slot`) is `Struct`, a tuple type (`low_type_of_slot`) is
`Shape`, anything else is `Absent`.  The parallel path passes `Shape`, because its
parameter *is* the declared named struct — a leaf with no class there is a declared
field whose class the lowering could not read.  Measured unchanged: `p : <Int, Int>`
compiles and runs, and `compute` is 57 passed / 0 failed / 3 ignored.

## What this refutes

- **[function-type-merge](function-type-merge.md)'s table row for the imported
  wrapper is not reproducible.**  It records the imported wrapper as
  `Function: raw[Function, raw[?a, ?b] -> raw[?c, ?d]] -> ?b` against a generic
  local control.  At `fb62d96` the two are **byte-identical**:
  `Function: raw[?a, ?b] -> raw[?c, ?d] -> ?b` imported and local, and
  `crates/lichen-language/tests/frozen_function_type.rs` passes.  The collapse
  that note's search was about is gone, so its "the next attempt should start
  from the two-line reproduction" points at a reproduction that now agrees with
  its control.
- **The frozen-copy step is not the binder.**  `materialize_static_signature`
  (`crates/lichen-lowlevel/src/static_module.rs:267`, called from
  `equality.rs:731`) *is* reached whenever a frozen template's signature is
  unified against a lambda, but the wrapper it materializes renders generic, and
  Defect II reproduces with the module local, where that function never runs.
- **The two parked reasons' causes.**  `jit_cross_kernel_subexpr`
  (`compute.rs:473`) is Defect II's *policy* neighbour, not the frozen arrow: it
  has unannotated parameters, and stating the classes makes it run and produce
  `raw 7` (measured) — so its un-park is stating a class, not a decoder change.
- **[compute-kernel-struct](compute-kernel-struct.md)'s LSP line** claimed a
  kernel binding renders `struct<.native raw[?a, ?b], .I Int, .O Int>` while
  line 38 of the same note says the fields are raw — the decided `.I Int` is the
  state Defect I says does *not* hold across a frozen boundary.
- **[function-type-merge](function-type-merge.md)'s "What goes" list is not
  what went.**  Five items it records as deleted still exist:
  `shape.rs:429`'s `is_arrow_type_any`, `:452`'s `is_function_type` (called from
  `checker/lambda.rs:380`), `equality.rs:766`'s `unify_function_types` (called
  from `:948`), `:662`'s `is_function_type` and `:711`'s `function_signature`.
  `Ctx::arrow` is implemented with no caller outside its own impl, and the
  "arrow rendering machinery … now unreachable" claim is wrong: the
  `TypeFunction` marker is read at `type_printer.rs:146,225,539` and
  `value_printer.rs:167`.  That paragraph is a plan, not a report.

## What is still open, and who decides

1. **The static type-value reader gap.**  `low_type_of`'s arm is gated on
   `AnyNodeId::Dynamic` and `pair_type_half` answers `None` for a static type
   half, so a frozen function type decodes as `Function(Unknown, Unknown)` while
   the same node local decodes fully.  It is a *reader* that cannot name a static
   ref — a different mechanism from Defect I, neither causing the other — and no
   caller is known to need it; widening it is a separate change.
2. **The empty `.in` group's coverage.**  The capability is measured end to end on
   both backends, and the graph-placeholder hole it exposed is fixed (`7643a37`),
   but no test exercises either one; whether the fillers the tests and examples
   carry should migrate to the empty group is a maintainer's decision.
3. **The split's own coverage.**  The three refusals above are measured by probe,
   not by a test: a test for the struct-domain and the open-position texts is a
   maintainer's decision, like (2).
4. Two silent compute failures recorded elsewhere are **not** this note's and are
   still open: a bare `plrun` whose body reads a runtime scalar on a device answers
   `none` with no diagnostic, and the `graph` chain's `flat_arity` guard was read
   but never measured.

Resolved here, and no longer open: Defect I's requirement (the type **must** carry
the signature), where the fill was owed (the static materialize walk's carry rule,
which needed the frozen node's mirrored fields), whether a class write may cross
the frozen boundary (**it must**, and it does), the sibling expectation that
conflicted with the model (it pinned the defect and is re-pinned above), and the
`UNDECIDED_DOMAIN` conflation (split into the three facts it covered).
