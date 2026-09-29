# Checker encoding: unstable at the `lichen-compute` boundary

> Status: current — describes the coupling as it stands and the decision that
> accepted it (D5 of [type-system-cleanup-plan](type-system-cleanup-plan.md));
> the design that would remove the label is
> [lowlevel-low-types](lowlevel-low-types.md), which is only `proposed`.
> Points at: `crates/lichen-highlevel/src/shape.rs` (the encoding authority)
> and `crates/lichen-compute/src/compute.rs` (the one component that reads
> around it).

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

- `value_of_node` (`compute.rs:1406-1440`) follows a `value_of` extraction — an
  `Index(pair, 0)` over a pair node — down to the pair's value slot, the same
  access path the checker itself uses to reach most values.
- The native operators read a kernel signature's domain and codomain through
  raw `Index` chains — `Index(sig.ty, 0)`, then `Index(sig_shape, 0)` /
  `Index(sig_shape, 1)`.

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
narrowed the label to the read side rather than removing it.

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

[lowlevel-low-types](lowlevel-low-types.md) (status `proposed`, not approved)
proposes giving the lowlevel **low types** — the `LowValue` variant tag of a
node's value, refined by observation and by abstract interpretation — and
making the JIT read only those. A JIT that reads low types never reads the
`[value, type]` pair encoding, which is precisely the reason this label
exists: once the JIT is on low types, re-encoding stops being a hazard for it,
and this note becomes the historical record of a coupling that is gone instead
of a live warning. That design is neither approved nor implemented, so the
label stands.
