# lichen-compute: wrap kernels in a struct + split call / launch / run

> Status: **proposal** — not yet implemented.  Companion to
> [lichen-compute.md](lichen-compute.md) (the current design this changes).
> Points at: `crates/lichen-compute/src/compute.lichen`,
> `crates/lichen-compute/src/compute.rs` (`JitOp`/`LaunchOp`, `native_ops`),
> `crates/lichen-compute/src/lib.rs` (`compute_native_ops!`),
> `crates/lichen-language/tests/compute.rs`.

## Problem

A kernel value is typed `[signature, [TypeKernel, Type]]` — a "kind" that mirrors a
function type, which is exactly why it is *directly callable* (`k x`): the apply
machinery dispatches on the callee's kind, reads the `[domain, codomain]` signature, and
the JIT body emitter lowers a kernel `Apply` to a `CallKernel`.  In lichen a **struct** is
a *different*, non-callable kind, so wrapping the kernel in a struct must not rely on the
kernel staying directly callable.

Separately, the tooling (LSP hover) renders this raw kernel type poorly: the `TypeKernel`
marker and the frozen signature cells render as opaque `?`, so `compute.jit` hovers as
`? -> ? -> [[?, ?], ?]`.

## Design: kernels are generic structs; invocation is via explicit native functions

Wrap every kernel in a struct whose fields carry the signature, and invoke kernels through
named native functions instead of a direct `k x` apply.  The internal kernel encoding
(`[sig, [TypeKernel, Type]]`, the `Kernel(KernelId)` value) is **unchanged**; it just lives
inside a struct field.

### The kernel struct (generic-struct-inside-a-function shape)

Built by `$jit` (the native op), because only it can see the `d`/`c` signature cells:

```
{ domain : P1, codomain : P2, kernel : RawKernel }
```

- **field names** `.domain`/`.codomain`/`.kernel` and the **kind** (a struct) are stable;
- **field types** `P1`/`P2` are **placeholder fresh cells** — bound to the concrete
  signature at each kernel's synthesis, so `jit (y => y + y)` yields
  `struct<.domain Int, .codomain Int, .kernel …>` while a generic `jit` hovers as
  `struct<.domain ?a, .codomain ?b, .kernel …>` (the "looks like a generic struct" pattern:
  a struct defined in a function scope, field types parameterized, kind shared);
- **field values**: `.domain`/`.codomain` hold the signature's **type** (a type-as-value),
  `.kernel` holds the raw `Kernel(KernelId)` value.

### The API (all typed, separate steps)

```
jit    f    -> wrapped kernel                 // $jit: compile f, wrap into the struct
call   k a  -> cross-kernel call result       // $call: a kernel body calling kernel k
launch k    -> launched (assembled) kernel    // $launch: assemble k + its callees
run    lka  -> result                         // $run: run a launched kernel on a
```

`compute.lichen`:

```
{
  jit    = f => $jit(f)
  call   = k => a => $call(k, a)
  launch = k => $launch(k)
  run    = lk => a => $run(lk, a)
}
```

- `$jit` makes the struct; the raw kernel stays inside `.kernel`.
- `$launch` **assembles** the relative launch set (BFS the reachable `call` set) into a
  launched unit — one module, root exported (`run_kernel`/`assemble_module`).
- `$run` **executes** a launched unit on `a` (`wasmi` instantiate + call `main`), returning
  the codomain result.
- `$call` is the cross-kernel form a kernel body uses (replaces direct `k x`), which
  `launch` assembles.

## Encoding changes

1. **`JitOp::build`** — after the function-ness gate (`f : d -> c`), build a nominal struct
   value + struct type:
   - shape = `[P1, P2, raw_kernel]` (placeholder field-type cells `P1`/`P2`, and the raw
     kernel kind `[sig, [TypeKernel, Type]]`);
   - kind = `[TypeStruct{id, names}, Type]` with `names` mapping `.domain`→0, `.codomain`→1,
     `.kernel`→2;
   - value = `[domain_type_value, codomain_type_value, op]` where the first two are the
     domain/codomain signature types as values, `op` is the `ComputeOperator::Jit` node;
   - result type = the struct type; unify the kernel's d/c against the struct's field cells
     (`P1 ≍ d`, `P2 ≍ c`) so the signature flows into the struct.
2. **New `CallOp` (`$call`)** — kernel-ness gate on `.kernel`; unify `a` against `.domain`,
   result `.codomain`; emit the kernel call (same lowering the current `launch` uses).
3. **`LaunchOp` (`$launch`)** — kernel-ness gate on `.kernel`; produce a launched unit
   typed `struct<.kernel …>` (or a `Launched` kind) without running it.  The assembly
   (`run_kernel`/`assemble_module`) moves to the run step.
4. **New `RunOp` (`$run`)** — receive the launched unit, unify the argument against its
   domain, emit `ComputeOperator::Launch`-style execution, result the codomain.
5. **Native registry** — `[jit, call, launch, run]` (`compute_native_ops!`, the
   `NativePlugin` op structs).
6. **Runtime** — `OperatorExt::run`: read the kernel from `.kernel` structurally; assemble
   on `launch`, execute on `run`; a `call` inside a body is a `CallKernel`.

## Test / tooling consequences

- `tests/compute.rs` — drop the bare `k x` form in favour of `call k x`; single-step
  `launch k a` becomes `launch k` / `run lk a`; keep the `value: type` assertions.
- LSP analysis (`crates/lichen-language-server`) — kernels are now real `struct<…>` types,
  so drop the `field_type_in_struct` parser and the opaque-`?`/`uninformative_type` guards
  added for the raw kernel kind.
- The `[sig, [TypeKernel, Type]]` encoding and `Kernel(KernelId)` are preserved, so the
  wasm codegen / cross-kernel assembly is unchanged.

## Implementation

Do this in a **separate git worktree** on its own branch so the current `v1` worktree stays
green: `git worktree add ../lichen-compute-kernel-struct -b feature/compute-kernel-struct`.
Land it in a commit per area (source wrapper, native ops + registry, runtime, tests, LSP)
and validate against `cargo test -p lichen-language --test compute` after each.
