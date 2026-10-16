# lichen-compute: kernels as `.native`/`.I`/`.O` structs (drop `TypeKernel`)

> Status: **current** — supersedes the earlier `.domain`/`.codomain`/`.kernel` proposal.
> Companion to [lichen-compute.md](lichen-compute.md).
> Points at: `crates/lichen-compute/src/compute.lichen`,
> `crates/lichen-compute/src/compute.rs` (`JitOp`/`LaunchOp`/`CallOp`/`ParallelOp`/
> `ParLaunchOp`/`ReadOp`/`WriteOp`/`BufferCollectOp`, `kernel_id_of`, `native_ops`),
> `crates/lichen-compute/src/lib.rs` (`compute_native_ops!`),
> `crates/lichen-compute/src/language.rs`, `crates/lichen-compute/tests/lichen_compute/compute.rs`.

## Problem

Kernels used to be typed `[signature, [TypeKernel, Type]]` — a dedicated "kind" that
mirrored a function type, with a `TypeKernel` marker in the value vocabulary and a
per-language `ValueType::is_function_kind` hook so the renderer could spell `Kernel : Int -> Int`.
That special kind marker drove two kinds of special-casing the design wanted to eliminate:
a per-language render hook (`is_function_kind`) and a vocabulary kind (`TypeKernel`/
`TypeParKernel`) that leaks the kernel's "shape" into the type system.

## Design (shipped): kernels are generic structs; the wrapper parses them

Drop `TypeKernel`/`TypeParKernel` and `is_function_kind`. A kernel is an ordinary lichen
**struct** `struct{.native _, .I _, .O _}`:

- `.native` — the opaque compiled wasm artifact (a `Kernel(KernelId)` / `ParKernel(KernelId,
  backend)` value), typed `_` (a fresh placeholder cell).
- `.I` — the function's **domain**, and `.O` its **codomain**, so the concrete signature
  rides in the struct, not in a kind marker. These two replaced the single `.sig` field the
  earlier version of this note argued for: the domain and the codomain are read separately,
  and a launch's result is `.O`.

A **buffer** is the same kind of wrapper, `Buf = T => struct{.native _, .element T}` — the
`.native` field is the packed payload and `.element` its element type — where it used to be
a bare extension value the type system special-cased. There is no `TypeBuffer`/`TypeWrite`
marker either.

Because the vocabulary no longer special-cases a kernel kind, the renderer prints the raw
struct: a `jit` result reads `struct{.native raw[?a, ?b], .I raw[?c, ?d], .O raw[?e, ?f]}`
(domain and codomain visible in `.I`/`.O`, no core-renderer special case).

### The embedded wrapper

```
let type_of = x => {t = _; x: t; t}
KT = _x => struct{.I _, .O _}
K = _x => struct{.native _, .I _, .O _}
A = I => struct{.n Int, .I I}
P = T: KT _ => struct{.n Int, .in T.I, .out T.O}
S = T: KT _ => A T.I -> T.O
Buf = T => struct{.native _, .element T}
jit = f => {I = _; O = _; f: I -> O; (K _)(.native $jit(f), .I I, .O O)}
launch = k: (K _) => a: k.I => {r = $launch(k.native, a); r: k.O; r}
call = k: (K _) => a: k.I => {r = $call(k.native, a); r: k.O; r}
parallel = f => b: string => {I = _; O = _; f: I -> O; (K _)(.native $parallel(f, b), .I I, .O I::out)}
plrun = k: (K _) => a: k.I => {r = $plrun(k.native, a); r: k.O; r}
range = x: Int => $range(x): Int
Read = T => struct{.from (Buf T), .at Int}
Write = T => struct{.to (Buf T), .at Int, .value T}
read = (x : (Read _)) => $read(x.from, x.at): x.from.element
write = (x : (Write _)) => $write(x.to, x.at, x.value)
collect = (b : (Buf _)) => $collect(b)
graph = f => {f: _ -> _; $graph(f)}
graphrun = g => a => $graphrun(g, a)
```

`type_of` is the standard library's type read (`lichen-std/_.lichen`), repeated
here because an embedded native source cannot depend on a package; the `let`
keeps it out of the exported struct's fields.

**Native ops operate on the native pieces**; the lichen wrapper parses the struct. So the
native op never sees the struct — `$jit(f)` returns the bare artifact, `$launch(k.native, a)`
takes the extracted `.native` (the signature is the struct's `.I`/`.O`, so it is no longer a
separate argument), `$call(k.native, a)` the bare kernel, and so on. The buffer ops take the
`Buf` **value** and the engine takes the payload, which is the algebra
[compute-buffer-wrapper](compute-buffer-wrapper.md) records.

## Checker support required

Reading `.native`/`.I`/`.O` off a **generic** wrapper parameter (`k` in `launch = k: (K _) => …`,
whose type is a fresh `?a` in the frozen compute module) makes each field-read's TYPE a lazy
`Index(Index(?a,0), key)` that can't be forced while `?a` is undecided. Two things make this
resolve correctly:

1. **The program's unification-deferral policy** (`Program::defer_pending`, lowlevel
   `program.rs`; decided for the highlevel in `lichen-highlevel/src/shape.rs`): a pending
   `Index` field/positional read over an undecided container, unified against a *type value*,
   joins the classes (defers) instead of recording a false "expected X, found Y". "Holds a
   type" is a fact about the highlevel's pair encoding, so the rule lives with the encoding
   authority; the lowlevel itself merges only what is a generic graph fact (a pending
   computation against an all-undecided skeleton, and two pending `Index` reads). Targeted to
   `Index` reads and type values only, so real errors (a pending computation against a
   scalar) are still reported.
2. **Lazy signature reads** in `LaunchOp::build`/`ParLaunchOp::build`: the domain/codomain
   (and a parallel run's element type) are read lazily out of the kernel struct's `.I`/`.O`
   fields — `launch` reads the codomain from `.O`, a parallel run reads its scalar leaves and
   input/output paths from the parameter type the kernel carries — rather than from fresh
   cells, so the frozen wrapper template's generic kernel parameter resolves the real cells
   once a concrete kernel struct binds at apply time — the argument gate and the result type
   both check against the *actual* signature.

## Runtime / codegen

`kernel_id_of` walks a kernel **struct value** `[native, I, O]` (by value, and through an
`Index(struct,0)` field read) to its `KernelId`, so a kernel body that refers to another
kernel by value lowers its call to a `CallKernel`, assembled at launch time.

**A kernel `@loop` nest carries a run-time state, so the kernel reader has to build
the nest.** Its header's `params` are the state, the base test branches out of it,
and the exit hands the carried value on — and because that state is a **run-time
value**, the checker cannot expand the recursion. The state is a buffer read, so it
is an instruction the entry block computes; until the wasm lowering decided value
types over the whole body rather than block to block, such a body was refused with
"block 1 has parameters but no branch reaches it with a value".

**A `plrun` result's element class stays undecided in the result type — a
documented limit, not a defect** (pinned end to end by `tests/lichen_compute/compute.rs`); what the
undecided cell costs a host read is
[compute-runtime-scalars](compute-runtime-scalars.md)'s.

## Encoding summary

- `ComputeValue` = `Kernel(KernelId)` | `ParKernel(KernelId, backend)` | `Buffer(payload, class)` |
  `DeviceBuffer(resident)` | `Graph(GraphId, backend)` | the graph's `GraphInput`/`GraphValue`
  placeholders — **no** `TypeKernel`/`TypeParKernel`, and no `TypeBuffer`/`TypeWrite`
  counterpart either: a buffer is a `Buf` struct, not a kind marker.
- `ComputeOperator` = `Jit` | `Launch` | `Call` | `Parallel` | `ParLaunch` | `Range` |
  `Read` | `Write` | `BufferCollect` | `Graph` | `GraphRun`.
- `ValueType::is_function_kind` (and the `FunctionKind` trait it was refactored into) is
  **removed**; kernels are ordinary structs for the checker and the renderer.

## Consequences

- `tests/lichen_compute/compute.rs` renders a **generic** kernel struct as
  `struct{.native raw[?a, ?b], .I raw[?c, ?d], .O raw[?e, ?f]}` — the frozen wrapper's
  fields are undecided cells, so each carries the raw mark — and `launch` results resolve
  their codomain lazily (`6 : Int`, `12 : Int`).
- An **applied** kernel binding renders its `value: type` by name
  (`a_kernel_value_and_type_render_by_name`):
  `(raw Kernel, Int, Int): struct{.native raw[?a, ?b], .I Type, .O Type}`. Each signature
  field's **type** is `Type`, not `Int`: the field holds a *type value* — the domain, the
  codomain — and the universe is the type of one, so its **value** half reads as that same
  `Int`. `.native` is the artifact's own dump (`raw Kernel`), because the struct's type names
  no class for it. The two field cells are **decided**, and that is the property a frozen
  boundary has to carry — see
  [the kernel parameter's class](kernel-parameter-class.md), which records the defect where
  they arrived empty instead.
- A **tuple** domain renders `(raw Kernel, tuple{Int, Int}, Int): struct{.native raw[?a, ?b], .I
  TypeTuple, .O Type}`: a tuple of element types is itself a tuple, so its type is the tuple
  universe. Only a *tuple* codomain dumps nested.

## Kernel bodies are SSA, and a body may compute its own state and branch back

The body of a kernel is `lichen-kernel-ir`'s SSA form: a value is a `ValueId`, an
instruction names its operands, and control is blocks with terminators —
`Br { target, args }`, `CondBr`, and `Return { values }`. **A block's `params`
are the values it receives**, and that one rule serves a function's arguments and
a loop's carried state alike: a header's carried values *are* its block
parameters, reading one is an ordinary read of a named value, and a backedge's
arguments *are* the next iteration's state. `KernelBody::validate` requires a
branch's `args` to match its target's `params` exactly, so a phi cannot be built
incompletely. The entry block's parameters are the fragment's own domain leaves,
in flattening order — "read the function's argument" and "read the loop's
carried state" are the same operation, because a parameter *is* a value the
block received.

That expressiveness is the capability a loop conversion needs: **a body may
compute its next state and reach the loop's backedge.** The earlier stack-machine
IR could not express it — `LocalGet` named a parameter leaf and nothing named
element `k` of a loop's state, so a body could arrive at a header but could not
carry anything new, and every loop the IR could build ran zero trips or forever.
The SSA rewrite is what closed it, not a terminator variant added to the old
form.

The **`passed_out` contract** that goes with a loop header is what the loop
conversion must satisfy, and the IR's own doc states it: the loop's whole state
is the tuple of carried values the header's block parameters are, and **the exit
receives the header's own top `passed_out` values** rather than the body's. That
is what gives a zero-trip loop a defined result — a trip count of zero never runs
the body, so a `passed_out` the body had to compute would have no source on that
path. `passed_out ≤ carried` therefore holds, and the conversion refuses a loop
that breaks it by name.

**A `@loop`-marked recursion is lowered to this nest** (`compute/body.rs`'s
`lower_loop`, over the roles `Module::loop_conversion` names in
`lichen-lowlevel`), so a non-straight-line `KernelBody` is produced in production
and both backends emit it. What remains open is the recursions whose steps apply
routed operators, which the kernel body has no node for
([loop-conversion](loop-conversion.md) §8.5 item 4, §8.6 item 6).

## The parallel parameter struct

A parallel kernel's parameter **is** the named struct
`struct{.n Int, .in …, .out …}`, built by `compute.P (compute.KT _)(.I In, .O Out)`, so that a
read names its buffer (`k.in.x`) instead of counting a position and the kernel can take
runtime scalars (`k.alpha`) beside its count. The retired tuple form's `cfg(0)` is `k.n` and
its `cfg(1)(k)` is a field under `.in`; `Write`'s `.to` is an output `Buf`, not the count. The
parameter's **role paths** (`scalars`/`inputs`/`outputs`, `KernelRoles` in `lichen-kernel-ir`)
are read from the parameter's type by `parallel_roles` and travel in the fragment, hashed by
`fragment_digest` like every other field. The author's type lambdas (`KT`/`A`/`P`/`S`) are in
`compute.lichen`, and the JIT'd signature has to come from the author, because a struct type's
identity is the occurrence it is written at
([function-type-merge](function-type-merge.md)).

The two blockers this shape was diagnosed through are **fixed**: a named read resolves through
the parameter's type by the two-pass `param_path` walk, and a nested static closure keeps its
captures' bindings (`StaticNode` mirrors the dynamic node's carry rule). So the shape runs, and
what remains open about it — the result type not reaching a host read's container cell, an
interleaved parameter the ABI cannot express — is recorded in
[compute-buffer-wrapper](compute-buffer-wrapper.md); that note is the model, and this section
is the kernel-struct side of it.

_Footnote: the earlier proposal split the invocation into `call`/`launch`/`run` (a `.kernel`
field-based 3-field struct). The shipped v1 keeps `launch`/`plrun` two-step and uses the
`.native`/`.I`/`.O` struct; `call` remains the cross-kernel-body form, applied to the bare
`.native`._
## Named access is *not* normalized away, and compute matches names

**Measured against the question "can compute analyze a kernel body that contains structs, on the
assumption that by function normalization every named access is already an ordinary tuple index" — the
assumption is false, in the direction that matters.**

**There is no normalization pass.** A named access is turned into a subscript **once, per expression,
at check time, and only conditionally**: `check_named_field`
(`crates/lichen-highlevel/src/checker/structs.rs:273-292`) chooses a `USize(at)` constant cell when the
container's type is concrete and the field is in its name table, and otherwise emits a **lazy
`TableGet(names, "name")`** that is committed to the graph and resolved per apply clone. The name to
position fold is `crates/lichen-highlevel/src/shape.rs:657` — a lookup, not a rewrite. `LowOperator` is
exactly `Index | Apply | TableGet`, so there is nothing for a later stage to fold into, and the
checker's own comment says why the constant exists at all: *"A constant is not an optimisation: a kernel
body is walked by operands, and a `TableGet` is no value to resolve."*

**What compute reads, measured by instrumenting the walk:** for the **parameter struct**, `k.in.d` and
`k.out.z` arrive as `TableGet` and are resolved **by name** (`compute.rs:3851` and `:3903-3916`, through
`IndexStep::Named`), and `parallel_roles` (`compute.rs:1823-1863`) decides where the buffers are by
**comparing strings against the parameter's own name table** — `position(|name| *name == Some("in"))`.
Renaming the parameter struct to `.input`/`.output` refuses by name ("a parallel kernel's parameter
holds a buffer outside .in/.out …"), which is the proof: these are names at the compute layer, not
positions. For a **struct that is not the parameter**, compute reads **nothing at all** — `Lower::apply`'s
residual branch (`compute/body.rs:884-896`) leaves the field's value in place, so no field-shaped node
ever exists in the caller.

**Behaviour, all on the serial/`"cpu"` path:** a struct local with a literal field, with a computed
`Float` field, and a **depth-2** named read on a non-parameter struct all build and answer correctly;
two same-typed buffers read as `k.in.b` then `k.in.a` answer per the **name** rather than the field
order, which is the sharpest positive check. Refused by name: a struct **returned by a cross-kernel
call** read as `s.p`; and a **nested struct group under `.in`/`.out`** (`In = struct{.g Inner}`, read
`k.in.g.b`) — which is the first of two defects below, because `b` *is* a field of `Inner`.

**Two defects, both false refusals and neither a wrong answer** (the guards are what rule the wrong
answer out: `resolve_steps` returns an `Err` naming the step instead of guessing a position, and the
non-parameter case has no node to get wrong):

- **A nested struct group under `.in`/`.out` is refused with the wrong reason.** The descent in
  `resolve_steps` (`compute.rs:3921-3935`) kept the selected field's **type node** as the next level's
  **field list**, so a step past the first level indexed the two-element `[shape, kind]` term instead of
  the field list — measured: the node it carried had two elements where the field list had one. The
  `slot_type`-then-`.unwrap_or(node)` step then answered the same node back, and the next named step saw
  no name table and reported "that type's fields are `None`". `walk_role` walks to any depth;
  `param_path` reads two names deep.
- **`scalar_leaf_classes` assumes a scalar role is a direct field** — its comment says so at
  `compute.rs:1761-1762` — while `walk_role` emits paths of any depth. A `Float` scalar under a grouped
  `.in` is therefore forced to `Int` and refused at launch ("a parallel parameter's scalar leaf is Int
  here and the launch passes a Float for it"), which misnames the cause; the `Int` case runs correctly.
  The launch still compares the real class against the declared one, so this degrades to a refusal.

**The decisive file is the checker's `check_named_field`**, which the field-naming work is actively
changing; if its constant-versus-`TableGet` choice moves, the node-kind evidence above must be re-measured,
though the behaviours would survive a shape change more readily than the refusal texts.


## The requirement: arbitrary struct nesting, with `.in`/`.out` the only special case

A kernel function must be able to **nest structs to any depth and read them by name** — in its body, in
its locals, in values a call returns, and inside the parameter's own fields — and the **only** place
where a name is treated specially is the parameter's `.in`/`.out`, which compute matches against the
name table to find the buffers. **That special parse stays**: it is load-bearing (renaming those two
fields refuses by name, as measured), so "normalize the names away" is neither possible nor wanted at
this layer.

**Measured today** (the diagnosis's matrix, all on the serial/`"cpu"` path): a struct local with a
literal field, with a **computed** `Float` field, and a **depth-2** named read on a **non-parameter**
struct all build and answer correctly; two same-typed buffers read out of name order answer per the
**name**; a struct **returned by a cross-kernel call**, read as `s.p`, refuses by name; and an
**anonymous** nested struct under `.in` refuses at check — as does a **named** nested group under `.in`,
with a message that misnames the cause.

**Two halves, deliberately separate.** The **parameter half** — a nested group under `.in`/`.out`, and a
`Float` scalar inside one — is the walk descending by the field's own type at every depth instead of
stopping where a shape element carries no element slot. The **everywhere-else half** — arbitrary nesting
in the body, in locals, in call results, to any depth — is the next slice, because both halves change
the same walk and two slices in one function collide. **Nothing in the tree tests any of this**: the
matrix above is the specification to grow into, and every case it gains wants a test of its own.

**The parameter half is done, and measured.** The descent kept the field it selected as the next level's
*field list*, but a field carries a *type*, and a type's own field list is a different node — measured:
the node it carried had two elements where the field list had one. The step after that indexed the
two-element `[shape, kind]` term instead of the fields, so `slot_type` answered the same node back through
`.unwrap_or` and the next named step saw no name table. The walk now carries the `TypeRef` and reads each
level from that type's own field list and name table. `scalar_leaf_classes` read a scalar's class from
`path.first()`, assuming a direct field of the parameter; it now descends to the type at the leaf's own
path. Both refusal sentences above are unreachable for these two shapes, which
`crates/lichen-compute/tests/lichen_compute/nested_role.rs` builds.

**The mixed group is fixed too** (`d5993c84`). A group holding **both** a buffer and a scalar —
`In = struct{.b (compute.Buf Float), .s Float}` — used to refuse with "a kernel body's index cannot be
placed: its target is a computation and its index is the constant 1", depth-independently; it now builds
and answers exactly what the flat control answers. The refusing decision was `Lower::parameter_path`'s
offset-flattening loop (`crates/lichen-compute/src/compute/body.rs`), and the reason is worth keeping:
**a buffer field is elided from the slot's `shape` but not from the path**, so a group's scalar sits at
field 1 while the shape has no second level — and a scalar-only group never asks for one, which is why
only the mixed shape broke. `KernelRoles` had recorded both all along (`scalars [[0], [1, 1]]` and
`inputs [[1, 0]]`, and both parameter structs reduce to the same two scalar locals in the same order), so
the ABI was never the obstacle. The walk now asks the roles for the offset the path names, with the shape
flattening as the fallback — the rule `param_read_offset` already applied on the same slot.

**What still refuses, and what is unsound.** `parameter_path` still consults a parameter's **first** slot
only, so a multi-slot parameter's second slot is resolved by the fallback alone — pre-existing, and
outside every defect above. And a path the roles table does not record still falls back to positional
shape flattening, which is **unsound for any future shape where a buffer field precedes a scalar one**:
that is the very shape this fix exists for. Nothing else in the everywhere-else half is missing —
buffers, scalars and mixed groups all nest to any depth, and `tests/lichen_compute/nested_role.rs` plus
`tests/lichen_compute/mixed_group.rs` build them.

**The repair is a temporary measure.** The name→position fold belongs after the apply clone, once for
every reader, not in one reader at compile time — see `specialize-normalize.md`.
