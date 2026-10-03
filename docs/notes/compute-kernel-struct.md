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
  let type_of = x => {t = _; x: t; t}
  jit      = f => (struct<.native _, .sig (type_of f)>)(.native $jit(f), .sig _)
  launch   = k => a => $launch(k.native, k.sig, a)
  call     = k => a => $call(k.native, a)
  parallel = f => (struct<.native _, .sig (type_of f)>)(.native $parallel(f), .sig _)
  plrun    = k => a => $plrun(k.native, k.sig, a)
  pget     = b => i => $pget(b, i)
  pcollect = b => $pcollect(b)
}
```

`type_of` is the standard library's type read (`lichen-std/_.lichen`), repeated
here because an embedded native source cannot depend on a package; the `let`
keeps it out of the exported struct's fields.

**Native ops operate on the native pieces**; the lichen wrapper parses the struct. So the
native op never sees the struct — `$jit(f)` returns the bare artifact, `$launch(native, sig, a)`
takes the extracted `.native` and `.sig`, `$call(k.native, a)` the bare kernel, and so on.

## Checker support required

Reading `.native`/`.sig` off a **generic** wrapper parameter (`k` in `launch = k => a => …`,
whose type is a fresh `?a` in the frozen compute module) makes each field-read's TYPE a lazy
`Index(Index(?a,0), key)` that can't be forced while `?a` is unbound. Two things make this
resolve correctly:

1. **The program's unification-deferral policy** (`Program::defer_pending`, lowlevel
   `program.rs`; decided for the highlevel in `lichen-highlevel/src/shape.rs`): a pending
   `Index` field/positional read over an unbound container, unified against a *type value*,
   joins the classes (defers) instead of recording a false "expected X, found Y". "Holds a
   type" is a fact about the highlevel's pair encoding, so the rule lives with the encoding
   authority; the lowlevel itself merges only what is a generic graph fact (a pending
   computation against an all-unbound skeleton, and two pending `Index` reads). Targeted to
   `Index` reads and type values only, so real errors (a pending computation against a
   scalar) are still reported.
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
- LSP renders a kernel binding as `(Kernel, parameterized) : struct<.native raw[?a, ?b], .sig Int -> Int>`
  (the `.native` artifact's own pair is a raw reading, so it is marked `raw[…]`) and the
  generic `compute.jit`/`compute.launch` wrappers as plain `Function` types.

## The parallel parameter struct

A parallel kernel's parameter is being moved from `cfg = (n, (buffers…))` to a named
struct, so that a read names its buffer (`k.in.x`) instead of counting a position and
the kernel can take runtime scalars (`k.alpha`) beside its count. The two shapes are
decoded by a **role table** read from the parameter's type (`parallel_roles`), and the
author's type lambdas are in `compute.lichen`:

```
KT = _x => struct<.I _, .O _>                      # the input/output pair
A  = I  => struct<.n Int, .I I>                    # the JIT'd input
P  = T: KT T => struct<.n Int, .in T.I, .out T.O>  # the author's parameter
S  = T: KT T => A T.I -> T.O                       # the JIT'd signature
```

`parallel_sig f b s` / `jit_sig f s` declare the JIT'd signature in the kernel struct's
`.sig`, which `plrun`'s gate already unifies the host's argument against. **The
signature has to come from the author**: a struct type's identity is the occurrence it
is written at ([applied-struct-nominal-id](applied-struct-nominal-id.md)), so a type
the gate *built* could never be the type the host writes at the call site — only an
occurrence the author supplies, evaluated by the checker, is shared with it. Nothing
in the gates changes; declaring it in the type position is enough.

Measured, and working: `Sig = S (KT _)(.I In, .O Out)` and the host's `(A In)(…)` are
one type (`(f : Sig) => f ((A In)(…))` checks), the role table holds the input paths
in declaration order (`[[1, 0]]` for `k.in.a` when `.in` is field 1), and the wasm
signature is the scalar fields followed by the index.

**Two blockers stop a struct-shaped kernel from running**, both measured:

1. **`param_path` cannot resolve a named-field path.** The read arm peels the
   wrapper's argument array and then asks `param_path` for the path from the parameter;
   it answers `None` because an `Index`'s *selector is not an evaluated constant* at
   that point (`usize_value` → `None`) — for the wrapper's own `x(0)` and for the
   author's `k.in.a` alike. The role table and the answer therefore disagree while both
   are correct in their own terms, and the refusal names that mismatch:
   `read's buffer argument is not an input buffer of the parallel parameter (the
   parameter's inputs are [[1, 0]])`. The old tuple shape never hits this: its position
   is a constant the *body* wrote, read off the node.
2. **`parallel_sig`'s extra curried parameter makes the `$parallel` operand
   `Parameterized`.** `compute.parallel f "cpu"` with the *same* struct-shaped `f`
   reaches the lowering (and reports blocker 1), while `compute.parallel_sig f "cpu"
   Sig` fails earlier, at the operand check in `ComputeOperator::Parallel`'s run. The
   third parameter is the only difference; `.sig s` is not the cause (reverting it to
   `.sig (type_of f)` does not move the failure).

_Footnote: the earlier proposal split the invocation into `call`/`launch`/`run` (a `.kernel`
field-based 3-field struct). The shipped v1 keeps `launch`/`plrun` two-step and uses the
smaller 2-field `.native`/`.sig` struct; `call` remains the cross-kernel-body form, applied
to the bare `.native`._