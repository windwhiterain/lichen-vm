# lichen-compute: kernels as `.native`/`.sig` structs (drop `TypeKernel`)

> Status: **current** — supersedes the earlier `.domain`/`.codomain`/`.kernel` proposal.
> Companion to [lichen-compute.md](lichen-compute.md).
> Points at: `crates/lichen-compute/src/compute.lichen`,
> `crates/lichen-compute/src/compute.rs` (`JitOp`/`LaunchOp`/`CallOp`/`ParallelOp`/
> `ParLaunchOp`/`BufferGetOp`/`BufferCollectOp`, `kernel_id_of`, `native_ops`),
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
**struct** `struct<.native _, .sig sig>`:

- `.native` — the opaque compiled wasm artifact (a `Kernel(KernelId)` / `ParKernel(KernelId)`
  value), typed `_` (a fresh placeholder cell).
- `.sig` — the function signature (a type-as-value, e.g. `Int -> Int`), so the concrete
  signature rides in the struct, not in a kind marker.

Because the vocabulary no longer special-cases a kernel kind, the renderer prints the raw
struct: a `jit` result reads `struct<.native <_>, .sig Int -> Int>` (signature visible in
`.sig`, no core-renderer special case).

### The embedded wrapper

```
{
  jit      = f => (struct<.native _, .sig (type_of f)>)(.native $jit(f), .sig _)
  launch   = k => a => $launch(k.native, k.sig, a)
  call     = k => a => $call(k.native, a)
  parallel = f => (struct<.native _, .sig (type_of f)>)(.native $parallel(f), .sig _)
  plrun    = k => a => $plrun(k.native, k.sig, a)
  pget     = b => i => $pget(b, i)
  pcollect = b => $pcollect(b)
}
```

**Native ops operate on the native pieces**; the lichen wrapper parses the struct. So the
native op never sees the struct — `$jit(f)` returns the bare artifact, `$launch(native, sig, a)`
takes the extracted `.native` and `.sig`, `$call(k.native, a)` the bare kernel, and so on.

## Checker support required

Reading `.native`/`.sig` off a **generic** wrapper parameter (`k` in `launch = k => a => …`,
whose type is a fresh `?a` in the frozen compute module) makes each field-read's TYPE a lazy
`Index(Index(?a,0), key)` that can't be forced while `?a` is unbound. Two things make this
resolve correctly:

1. **`unify_inner` deferral** (`crates/lichen-lowlevel/src/equality.rs`): a pending `Index`
   field/positional read over an unbound container, unified against a *type value*, joins the
   classes (defers) instead of recording a false "expected X, found Y". Targeted to `Index`
   reads and type values only, so real errors (a pending computation against a scalar) are
   still reported.
2. **Lazy signature reads** in `LaunchOp::build`/`ParLaunchOp::build`: the domain/codomain
   (and the element type `?b`) are read lazily out of the signature field
   (`Index(Index(sig.ty,0),0/1)` etc.) rather than fresh cells, so the frozen wrapper
   template's generic `.sig` resolves the real cells once a concrete kernel struct binds at
   apply time — the argument gate and the result type both check against the *actual*
   signature.

## Runtime / codegen

`kernel_id_of` now walks a kernel **struct value** `[native, sig]` (by value, and through an
`Index(struct,0)` field read) to its `KernelId`, so a kernel body that refers to another
kernel by value lowers its call to a `CallKernel`, assembled at launch time.

## Encoding summary

- `ComputeValue` = `Kernel(KernelId)` | `ParKernel(KernelId)` | `Buffer(BufferId)` |
  `TypeBuffer` — **no** `TypeKernel`/`TypeParKernel`.
- `ComputeOperator` = `Jit` | `Launch` | `Call` | `Parallel` | `ParLaunch` | `BufferGet` |
  `BufferCollect`.
- `ValueType::is_function_kind` (and the `FunctionKind` trait it was refactored into) is
  **removed**; kernels are ordinary structs for the checker and the renderer.

## Consequences

- `tests/compute.rs` renders kernels as `struct<.native <_>, .sig Int -> Int>` and `launch`
  results resolve their codomain lazily (`6 : Int`, `12 : Int`).
- LSP renders a kernel binding as `(Kernel, parameterized) : struct<.native [?a, ?b], .sig Int -> Int>`
  and the generic `compute.jit`/`compute.launch` wrappers as plain `Function` types.

_Footnote: the earlier proposal split the invocation into `call`/`launch`/`run` (a `.kernel`
field-based 3-field struct). The shipped v1 keeps `launch`/`plrun` two-step and uses the
smaller 2-field `.native`/`.sig` struct; `call` remains the cross-kernel-body form, applied
to the bare `.native`._