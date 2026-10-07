# lichen-compute: kernels as `.native`/`.I`/`.O` structs (drop `TypeKernel`)

> Status: **current** — supersedes the earlier `.domain`/`.codomain`/`.kernel` proposal.
> Companion to [lichen-compute.md](lichen-compute.md).
> Points at: `crates/lichen-compute/src/compute.lichen`,
> `crates/lichen-compute/src/compute.rs` (`JitOp`/`LaunchOp`/`CallOp`/`ParallelOp`/
> `ParLaunchOp`/`ReadOp`/`WriteOp`/`BufferCollectOp`, `kernel_id_of`, `native_ops`),
> `crates/lichen-compute/src/lib.rs` (`compute_native_ops!`),
> `crates/lichen-language/src/program.rs`, `crates/lichen-language/tests/compute.rs`.

## Problem

Kernels used to be typed `[signature, [TypeKernel, Type]]` — a dedicated "kind" that
mirrored a function type, with a `TypeKernel` marker in the value vocabulary and a
per-language `ValueType::is_function_kind` hook so the renderer could spell `Kernel : Int -> Int`.
That special kind marker drove two kinds of special-casing the design wanted to eliminate:
a per-language render hook (`is_function_kind`) and a vocabulary kind (`TypeKernel`/
`TypeParKernel`) that leaks the kernel's "shape" into the type system.

## Design (shipped): kernels are generic structs; the wrapper parses them

Drop `TypeKernel`/`TypeParKernel` and `is_function_kind`. A kernel is an ordinary lichen
**struct** `struct<.native _, .I _, .O _>`:

- `.native` — the opaque compiled wasm artifact (a `Kernel(KernelId)` / `ParKernel(KernelId,
  backend)` value), typed `_` (a fresh placeholder cell).
- `.I` — the function's **domain**, and `.O` its **codomain**, so the concrete signature
  rides in the struct, not in a kind marker. These two replaced the single `.sig` field the
  earlier version of this note argued for: the domain and the codomain are read separately,
  and a launch's result is `.O`.

A **buffer** is the same kind of wrapper, `Buf = T => struct<.native _, .element T>` — the
`.native` field is the packed payload and `.element` its element type — where it used to be
a bare extension value the type system special-cased. There is no `TypeBuffer`/`TypeWrite`
marker either.

Because the vocabulary no longer special-cases a kernel kind, the renderer prints the raw
struct: a `jit` result reads `struct<.native raw[?a, ?b], .I raw[?c, ?d], .O raw[?e, ?f]>`
(domain and codomain visible in `.I`/`.O`, no core-renderer special case).

### The embedded wrapper

```
let type_of = x => {t = _; x: t; t}
KT = _x => struct<.I _, .O _>
K = _x => struct<.native _, .I _, .O _>
A = I => struct<.n Int, .I I>
P = T: KT _ => struct<.n Int, .in T.I, .out T.O>
S = T: KT _ => A T.I -> T.O
Buf = T => struct<.native _, .element T>
jit = f => {I = _; O = _; f: I -> O; (K _)(.native $jit(f), .I I, .O O)}
launch = k: (K _) => a: k.I => {r = $launch(k.native, a); r: k.O; r}
call = k: (K _) => a: k.I => {r = $call(k.native, a); r: k.O; r}
parallel = f => b: string => {I = _; O = _; f: I -> O; (K _)(.native $parallel(f, b), .I I, .O I::out)}
plrun = k: (K _) => a: k.I => {r = $plrun(k.native, a); r: k.O; r}
range = x: Int => $range(x): Int
Read = T => struct<.from (Buf T), .at Int>
Write = T => struct<.to (Buf T), .at Int, .value T>
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

`kernel_id_of` now walks a kernel **struct value** `[native, I, O]` (by value, and through an
`Index(struct,0)` field read) to its `KernelId`, so a kernel body that refers to another
kernel by value lowers its call to a `CallKernel`, assembled at launch time.

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

- `tests/compute.rs` renders a kernel as the raw struct
  `struct<.native raw[?a, ?b], .I raw[?c, ?d], .O raw[?e, ?f]>` and `launch` results resolve
  their codomain lazily (`6 : Int`, `12 : Int`).
- LSP renders a kernel binding as `(raw Kernel, raw parameterized) : struct<.native raw[?a, ?b], .I Int, .O Int>`
  (all three fields are raw readings, so each carries the mark — the `.native` artifact's own
  pair likewise — and the generic `compute.jit`/`compute.launch` wrappers as plain
  `Function` types.

## The parallel parameter struct

A parallel kernel's parameter **is** the named struct
`struct<.n Int, .in …, .out …>`, built by `compute.P (compute.KT _)(.I In, .O Out)`, so that a
read names its buffer (`k.in.x`) instead of counting a position and the kernel can take
runtime scalars (`k.alpha`) beside its count. The retired tuple form's `cfg(0)` is `k.n` and
its `cfg(1)(k)` is a field under `.in`; `Write`'s `.to` is an output `Buf`, not the count. The
parameter's **role paths** (`scalars`/`inputs`/`outputs`, `KernelRoles` in `lichen-kernel-ir`)
are read from the parameter's type by `parallel_roles` and travel in the fragment, hashed by
`fragment_digest` like every other field. The author's type lambdas (`KT`/`A`/`P`/`S`) are in
`compute.lichen`, and the JIT'd signature has to come from the author, because a struct type's
identity is the occurrence it is written at
([applied-struct-nominal-id](applied-struct-nominal-id.md)).

**It runs** — both blockers were diagnosed and fixed. The current state, the two
blockers as they were diagnosed, the reproduction and the orientation map are in
[compute-param-struct-handoff](compute-param-struct-handoff.md) — that note is the
one to read; this section is the pointer.

_Footnote: the earlier proposal split the invocation into `call`/`launch`/`run` (a `.kernel`
field-based 3-field struct). The shipped v1 keeps `launch`/`plrun` two-step and uses the
`.native`/`.I`/`.O` struct; `call` remains the cross-kernel-body form, applied to the bare
`.native`._