# Checker encoding: unstable at the `lichen-compute` boundary

> Status: current — the label **stands, in a narrowed scope**. The kernel
> *domain* read is gone: `kernel_param_shape`/`element_shape` were deleted and
> `compile_fragment` now seeds, passes, and reads the parameter's **low type**
> ([lowlevel-low-types](lowlevel-low-types.md)). What remains is the body's
> graph walk, and that is what this note now describes.
> Points at: `crates/lichen-highlevel/src/shape.rs` (the encoding authority)
> and `crates/lichen-compute/src/compute.rs` (the one component that still
> reads around it).

lichen's type representation is an untyped node graph with positional
conventions: every expression compiles to a `[value, type, attrs…]` pair, every
kinded type expression to a `[shape, [marker, universe]]` pattern, and every
type spine ends at the self-referential universe `K = [Type, ↺]`. **Inside the
project that encoding has one owner** — `crates/lichen-highlevel/src/shape.rs`
holds the layout as named constants and typed accessors (`shape_of`, `kind_of`,
`is_function_type`, `attr_slot`), and the highlevel checker, the renderer and
the language composition read the layout through it. The lowlevel does not hold
it either: it became honestly untyped in Phase 2 (decision D1), keeping only
generic graph facts such as "this class is a self-referential cycle", so there
is no second copy anywhere to keep in step.

`lichen-compute`'s JIT is the one component that does not use that authority.
It reads the same conventions out of raw nodes:

- `value_of_node` (`compute.rs`) follows a `value_of` extraction — an
  `Index(pair, 0)` over a pair node — down to the pair's value slot, the same
  access path the checker itself uses to reach most values.
- The native operators read a kernel signature's domain and codomain through
  raw `Index` chains — `Index(sig.ty, 0)`, then `Index(sig_shape, 0)` /
  `Index(sig_shape, 1)`.

**The domain read is gone.** Phase 3c of the low-type design deleted
`kernel_param_shape` and `element_shape` — the two functions that re-derived a
kernel's domain by walking the type half of the parameter pair. The domain now
comes from the parameter's low type, and the *seed* for that low type is read
once through the authority (`shape::low_type_of_slot`), which is a single named
call rather than a convention the JIT re-derives. That was the read with the
most reach: a `string` domain used to decode as a machine scalar, so the old
walk was not merely coupled, it was wrong where it was silent.

**The write side was closed in Phase 5.**  All four native operators used to
assemble a function type by hand out of array positions —
`ctx.array_node`, `ctx.universe()`, `ctx.value_node(P::Value::function_type_marker())`
— and two of them hand-built the `[marker, universe]` kind outright rather than
going through `Ctx::kind_expr`.  They now all call the one constructor,
`ctx.arrow(domain, codomain)` (`Checker::arrow_parts` behind it), with the same
three nodes allocated in the same order, so the artifact is byte-identical.
The **read** side is deliberately still raw: a signature that is not yet bound
has to resolve at apply time, so those two sites must keep the lazy `Index`
chain and cannot use `shape::function_type_parts`, which needs a bound type.

None of that goes through `shape`, so an encoding change does not stop the JIT
from compiling: it makes it compile against an encoding that no longer exists,
and the damage is wrong kernel code rather than a build error. This is the "any
encoding change breaks it silently" the cleanup plan records
([type-system-cleanup-plan](type-system-cleanup-plan.md) §1). Decision D5
deferred cleaning this component up and asked for this label instead; Phase 5
narrowed the label to the read side, and Phase 3c of the low-type design
narrowed it again, to the body's graph walk.

## What "unstable" means here

- **Inside the project** the encoding is owned, named, and still free to
  change: the cleanup's Phases 0–3 re-encoded it (the `shape` authority, the
  kind-marker registry, single-sourced codec tags and attribute slots), and
  every internal reader followed. Nothing inside the repository reads the
  layout except through the one module that states it.
- **Outside the project** the pair and kind layout is **not** a frozen
  contract, and this note is the statement of that. The two parts of it that
  *are* frozen are the ones an already-persisted artifact carries: the
  attribute order (a pair's arity is its schema tail) and the persisted codec
  tag of each kind marker — see [attributes](attributes.md) and the registry in
  `shape.rs`.
- **The instability is scoped to the JIT boundary.** Anything that builds a
  program through the documented `Program` / `Ctx` extension points — a
  compiler plugin, a package — is not reading the encoding by hand and is not
  affected; see [compiler-plugin](compiler-plugin.md) and
  [plugin-taxonomy](plugin-taxonomy.md).

## What would remove the label

[lowlevel-low-types](lowlevel-low-types.md) gave the lowlevel **low types** —
the `LowValue` variant tag of a node's value, refined by observation, by the
class merge, and by an abstract-interpretation pass over a template — and moved
the JIT's domain read onto them. A JIT that reads low types never reads the
`[value, type]` pair encoding, which is precisely the reason this label exists:
once the JIT is on low types, re-encoding stops being a hazard for it.

That is done for the domain; the **body** is the residue. The emitter reaches a
value's computation by following `Index` chains from a known pair, and reaches
a parameter element by its index path. A backend that read each body node's low
type instead — which the pass now computes and stores — would remove the last
of it. That is recorded as the open item in
[compute-jit-low-types](compute-jit-low-types.md), not as work in flight.
