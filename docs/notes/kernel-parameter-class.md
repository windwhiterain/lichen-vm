# The kernel parameter's class

> Status: current — both defects this note was written around are **fixed**; a
> residue it did not need to close is listed under
> [What is still open](#what-is-still-open).
>
> What this note is: what the kernel parameter's **class** is, why a kernel
> cannot be compiled until it is decided, and the two ways a written class used
> to arrive *undecided* at the JIT. The class is the `Int`/`Float` a parameter
> leaf computes in; the JIT needs it before it compiles the body, because the
> lowered body's instructions declare their class rather than deriving it
> ([kernel-class-crossing-fixes](kernel-class-crossing-fixes.md)).
>
> Points at: `crates/lichen-compute/src/compute.rs` (`compile_fragment`,
> `kernel_domain`, `DomainStatement`, `argument_kind`),
> `crates/lichen-highlevel/src/shape.rs` (`low_type_of`, `low_type_of_slot`),
> `crates/lichen-lowlevel/src/equality.rs` (`function_type_signature`),
> `crates/lichen-lowlevel/src/static_module.rs` / `static_module/apply.rs` (the
> frozen mirror), and `crates/lichen-render/src/render/value_printer.rs`
> (`raw_cells`).

## Why the class has to be decided

`compile_fragment` **seeds** each template term's type slot, runs the low-type
fixed point, and then **reads** the domain — it never guesses a class from body
usage ([compute-jit-low-types](compute-jit-low-types.md) records the three
steps). A template is compiled before any apply, so a parameter whose annotated
type is an unresolved cell reaches the lowering with no class at all. The
refusals that follow are the honest ones:

- `UNDECIDED_DOMAIN` — the parameter's type cell is genuinely empty (an
  *unannotated* parameter): annotate it.
- `the kernel parameter's domain is a struct type …` — the type was read and it
  is a nominal struct, which has no low shape at all.
- `the kernel parameter's domain has no class this lowering can read` — a stated
  domain with no position to name (a non-tuple structure).
- **`… has a position with no class: position n.m …`** — the type was read, it is
  a tuple, and one position of it could not be. The position is named and nothing
  asks for an annotation.

`kernel_domain` takes the fact it cannot read for itself — `DomainStatement` —
from the caller, because one text could not tell those three apart. The
classifier reads the parameter's **own type cell**: a struct type term
(`struct_fields_of_slot`) is `Struct`, a tuple type (`low_type_of_slot`) is
`Shape`, anything else is `Absent`. The parallel path passes `Shape`, because
its parameter *is* the declared named struct — a leaf with no class there is a
declared field whose class the lowering could not read.

## Defect I — a frozen module's applied struct loses its field-type cells

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
field read `s.I` is `raw Int: raw[?a, ?b]`. The **same two lines in one file**
give `(Int, Int): struct<.I Type, .O Type>` and `s.I` is `Int: Type`.

**It is not tuple-specific and not compute-specific.** Necessary and sufficient
among the variants measured: the struct constructor is in a frozen module *and*
the field argument is the frozen callee's own parameter. Either one removed
decides the cell:

| program | render |
|---|---|
| frozen `mk 5` (a scalar argument) | `(raw 5, raw 5): struct<.I raw[?a, ?b], .O raw[?c, ?d]>` |
| frozen `(w.S _)(.I Int, .O Int)` (a literal argument, no parameter indirection) | `(Int, Int): struct<.I Type, .O Type>` |
| frozen `mk <Int, Int>` | `(raw[raw[raw Int, …]], raw[raw[raw Int, …]]): struct<.I raw[?a, ?b], .O raw[?c, ?d]>` |
| the `compute.jit` shape with `.native 1` instead of a kernel | the parked defect, no compute and no operator |
| the same `jit` local | `(1, Int, Int): struct<.native Int, .I Type, .O Type>` |

**Why, measured: the static apply dropped a residual operation's cached answer.**
The frozen template's field-type entries and the applied function's parameter
type cell are **literally one class**. The apply's clone map holds exactly **one**
member of it — the parameter type cell; the others have no in-edge except the
**cached value of the residual operation node**, and `static_node_apply` dropped
that cached answer, walking only the operand. So the class was never
reconstructed, and the applied struct's type held clones of a **different
generation** — the frozen `S` re-applied at run time — which are cells nothing
binds.

The local path differs at exactly that step, and the difference is the repair:
the dynamic walk **carries a residual operation's own answer** when the
template's operator produced it and the deep pass ran, and then **maps the
answer's items onto this call's clones**. The static walk dropped the answer
because it **could not ask the question**: the frozen node stored one collapsed
flag where the rule tests two facts apart, `runned` and `evaluated_deep`.

**The fix, and the rule it rests on.** `StaticNode` carries `runned` and
`evaluated_deep` exactly as the dynamic `Node` does; `undecided()` is *derived*
from the latter rather than stored beside it; `freeze` copies both instead of
collapsing them; and `static_node_apply` runs the dynamic clone rule itself,
with the policy stated once — `apply::answer_elements_are_undecided`, read by
both walks. Measured, the two-line repro's imported render is **byte-identical
to its local control**, `(Int, Int): struct<.I Type, .O Type>`.

The rule the maintainer ruled for, and the note's load-bearing statement: **an
applied kernel struct's type must carry the signature, and a class write must
cross the frozen boundary.** The persisted artifact encodes the two fields, which
is why `ARTIFACT_FORMAT_VERSION` went `9` → `10`: a version-`9` artifact's
collapsed byte cannot be read as the verdict it was derived from, so the bump
turns that into the recompile the version check intends.

## Defect II — the decoder cannot read a written arrow's node

A written arrow compiles to a lambda, and a function's own type is the
self-referential node `[Function(fid), ↺]`. `low_type_of` used to decode only the
older `[shape, kind]` arrow term: slot 0 holds a `LowValue::Function` and slot 1
is the node itself, so the decode fell through to `LowShape::Unknown`, and
`low_type_of_slot`'s pair-indirection retry failed too because slot 0 is a
`Function` leaf rather than an array.

Measured structure of the parameter's type cell:

| parameter | renders | `low_type_of` (before) | node |
|---|---|---|---|
| `Int` | `Int` | `USize` | `[none, [none, ↺…]]` |
| `Int -> Float` | `Int -> Float` | **`Unknown`** | `[Function(fid), [Function(…), ↺…]]` |
| `<Int, Int -> Float>` | `<Int, Int -> Float>` | `Tuple([USize, Unknown])` | the tuple, whose second element is the row above |
| a local arrow wrapper's parameter | `raw[?a, ?b] -> raw[?c, ?d]` | **`Unknown`** | the same self-cycle |

**The arrow's node shape is the variable, not freezing**: the same node was
`Unknown` in a local module. Downstream, `compile_fragment` seeds from
`low_type_of_slot` and `kernel_domain` refused with `UNDECIDED_DOMAIN`, so
`compute.jit (p : <Int, Int -> Float> => p(0))` was refused **at the `jit`** even
though the parameter was annotated and its first position *is* decided.

**The fix.** `Module::function_type_signature` reads the function type's domain
and codomain **type cells** — the parameter pair's type slot, not the pair,
because a template's parameter value cell is empty until an apply binds it, and
`Function::return_type` — and `low_type_of` gained the one arm that asks it,
gated on the lowlevel's own recogniser. Both spellings of a function type now
build their shape through a single `function_shape`, and the recognisers became
`&self` reads over the no-compression `class_root`, so the decoder never needs a
mutable borrow. Measured, the refusal moves to where it can name a position:

| program | before | after |
|---|---|---|
| `jit (p : <Int, Int -> Float> => p(0))` | `UNDECIDED_DOMAIN`, at the `jit` | `kernel_launch: argument element 1 is a function, not a concrete Int or Float` |
| `jit (p : <Int -> Float, Int> => p(1))` | `UNDECIDED_DOMAIN` | the same, at **element 0** |
| `jit (p : Int -> Float => p)` | `UNDECIDED_DOMAIN` | `kernel domain must be a scalar or a tuple of scalars` |
| a float-domain kernel, an unannotated parameter, the scalar control, the function-codomain program, the frozen two-file probe | — | **unchanged** |

## What is still open

1. **The empty `.in` group's coverage.** The capability is measured end to end on
   both backends and the graph-placeholder hole it exposed is fixed
   ([compute-buffer-wrapper](compute-buffer-wrapper.md)), but no test exercises
   either one; whether the fillers the tests and examples carry should migrate to
   the empty group is a maintainer's decision.
2. **The domain refusals' coverage.** The three domain refusals above are
   measured by probe, not by a test: a test for the struct-domain and the
   open-position texts is a maintainer's decision, like (1).
3. **`pair_type_half` over a static type half of a *dynamic* pair.** The
   recogniser takes a `NodeId` and the arm is gated on `AnyNodeId::Dynamic`, so a
   static type half of a dynamic parameter pair leaves the whole signature
   `None`. Unmeasured; no caller is known to reach it.
4. Two silent compute failures recorded elsewhere are **not** this note's and are
   still open: a bare `plrun` whose body reads a runtime scalar on a device
   answers `none` with no diagnostic
   ([compute-runtime-scalars](compute-runtime-scalars.md)), and the `graph`
   chain's `flat_arity` guard was read but never measured
   ([compute-graph-jit](compute-graph-jit.md)).
