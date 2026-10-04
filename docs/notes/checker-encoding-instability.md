# Checker encoding: unstable at the `lichen-compute` boundary

> Status: current — the label **stands, in a narrowed scope**. The kernel
> *domain* read is gone: `kernel_param_shape`/`element_shape` were deleted and
> `compile_fragment` now seeds, passes, and reads the parameter's **low type**
> ([lowlevel-low-types](lowlevel-low-types.md)). What remains is the body's
> graph walk, and that is what this note now describes. **The lowlevel's own
> pair decoding is inventoried below and its body-graph read is closed**
> (`control_flow` takes the domain and codomain values from its caller).
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
the language composition read the layout through it. **That "one owner" claim was
wrong about the lowlevel.** It does not hold the *constants*, but it decodes
positions — element 0 is a value, element 1 is a type, a pair is 2- or 3-wide —
in six sites, listed in "The lowlevel readers too" below. What Phase 2 (decision
D1) did remove was the lowlevel's own *type* representation: there is no
`type_marker`, no `type_of`, no `is_type` anywhere in the crate, so a lowlevel
reader with a type question has nothing but the pair to answer it with. That is
why the sites are there, and why they cannot be closed by deleting a helper.

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

### The lowlevel readers too — six sites, and the defect among them is closed

An audit of `crates/lichen-lowlevel` for the pair convention. Six readers, and the
distinction that matters is not *how much* each one knows but **whether the value
it reads is an operand of the operation being performed**:

| site | what it reads | verdict |
|---|---|---|
| `apply.rs:105`, `apply.rs:111` | element 1 of a **parameter** node — the declared type, for the `ApplyError` attribution only | **Wiring.** The apply pass built or cloned the pair it is reading; element 1 is an operand of the check it is performing. |
| `apply.rs:160` | element 1 of the **return pair** it just cached, to bind the checker's result cell | **Wiring.** `wire_apply_result` is constructing the result; the cell is the third operand of the apply. |
| `resolve.rs` (`pair_value_half`) | a **body's** parameter or return, and a 2..=3 width test, to decide which half is the value | **Encoding reader, and now the lowlevel's own convention.** The caller-stated form (`define_in(domain, node)`) is the one to prefer *where the caller has the value*; where the analysis **starts** from `Function`'s two nodes — `parameter_leaves`, and `loop_conversion.rs`'s state and spine roots — there is no caller to state it, and `Function`'s own docs call those nodes pairs. |
| `loop_conversion.rs` (`parameter_value_path`, `loop_conversion`) | a template's parameter, to resolve a read to the path into the parameter's value (`s(0)`), and its return, for the spine's roots | **The same standing as `parameter_leaves`.** The loop's analysis is a template-shape question — which node is a branch of the return spine — so it reads the pair the template carries; the caller states only that the function is marked. |
| `equality.rs:479-488` (`is_static_universe_id`) | a 2-element array whose element 1 points back at the same module and index | **Mixed.** The `[Type, ↺]` universe is the canonical instance, but the predicate is stated as a *generic* graph shape and unifies any two such cycles. It decodes the positions, and reads no meaning. |
| `evaluation.rs:554` (`table_get_operands`) | element 0 and 1 of a `TableGet`'s operand, which the arm's own check already proved is a 2-element array | **Not an encoding reader.** It is a destructure of `[table, key]`; no pair is involved. |

`low_type.rs:13-18` states the boundary correctly, and it is worth reading exactly:
*"The pass never learns the `[value, type]` pair layout"* is a claim about **that
pass**, not about the crate. `apply.rs` and `control_flow.rs` are in the same crate
and do learn it. A per-analysis boundary is the honest description; a crate-wide one
is not.

**The defect is fixed by a caller contract, not by a lowlevel change, and the
rows above are what is left of it.** A function's `parameter` and `r#return` are
`NodeId`s that hold the checker's pair; `Function` says nothing about their shape
(`lib.rs:1083-1084`). So [`Module::define_in`] takes the **domain value as an
argument** — the JIT's own applied parameter, which `ParamSlot` already resolves —
and the analysis that *starts* from the pair reads it deliberately rather than
sniffing for a width: `parameter_leaves` (a function's own leaves) and
`loop_conversion.rs` (a template's state paths and return spine). A caller that
states the value has done the decoding; where there is no such caller, the read is
the lowlevel's own convention about `Function`, which is a stated fact and not a
heuristic standing in for one. The apply sites stay: their pair is an operand of
the operation in hand, which is the same standing `lichen-compute`'s
`value_of_node` has.

**How the sites got there: they were never introduced.** `41bfa9e` ("crates
refactor") shows `src/lowlevel/{equality,evaluation,function}.rs` arriving as pure
renames, pair reading intact; `apply.rs` first appears with its type-slot doc
comment already written (`7b65015`). `PAIR_TYPE_SLOT` was not named until
`fc856a7` (Phase 1a) — the bare `items[1]` predates any constant by weeks. There is
no earlier revision in which the lowlevel was free of the layout, because the
lowlevel **is** the evaluator the checker's graph was written for: `P::Value` for
the lichen frontend is the checker's own term, so the pair is inside the VM's value
domain rather than a format it consumes.

## What "unstable" means here

- **Inside the project** the encoding is owned, named, and still free to
  change: the cleanup's Phases 0–3 re-encoded it (the `shape` authority, the
  kind-marker registry, single-sourced codec tags and attribute slots), and
  every internal reader followed. One module states the layout; the sites that
  read it without going through that module are the ones listed above, and they
  are readable sites rather than a second statement of the encoding — no constant
  or accessor is duplicated.
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

**One part of that residue is closed ahead of it.** Building a body's graph no
longer needs the pair: the graph facts moved to `resolve.rs` without a
control-flow graph, and the pair reads that remain there are the two the analysis
*starts* from (`parameter_leaves`, `loop_conversion.rs`) rather than a value the
caller already held. What remains of this label is `lichen-compute`'s emitter
walking the pair on the way *into* that graph, which is the item above and is
deliberately not in flight.
