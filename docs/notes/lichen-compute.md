# lichen-compute: the JIT package (compile functions to wasm, launch them)

> Status: current (implemented; the live contract is the `crates/lichen-language/src/compute.rs` rustdoc)
> Points at: `crates/lichen-language/src/compute.rs` (the native core),
> `crates/lichen-language/src/program.rs` (`LangValue`/`LangOperator`/`LangProgram` composition),
> `crates/lichen-highlevel/src/native.rs` (the `NativeOp`/`NativeOps` extension point),
> `crates/lichen-language/src/package.rs` (`register_compute`, the virtual `compute.lichen`),
> and `crates/lichen-language/tests/compute.rs` (the end-to-end suite).

`lichen-compute` is a compiler plugin — a native wrapper package (think `numpy`)
that jit-compiles a lichen function to a **wasm kernel** and runs it. The user-facing
surface is an embedded `compute.lichen` that re-exports two functions:

```
@{ compute = import "compute.lichen" @}
k = compute(0) (x => x + 1)     -- jit: compile the lambda to a kernel (via `$jit`)
compute(1) k 5                  -- launch: run it -> 6 : Int (via `$launch`)
```

`compute(0)` is `jit`, `compute(1)` is `launch`, exported as the positional tuple
`(jit, launch)`. A kernel's **type is its signature** (`Kernel<Int -> Int>`), so the
whole function-apply machinery transfers to kernels.

## 1. The vocabulary injection

The native core provides two plain, `Copy` enums, composed as sibling leaves into the
language's value/operator vocabularies with `lichen_utils::enum_ext!`:

- **`ComputeValue`** = `Kernel(KernelId)` (a compiled artifact id) | `TypeKernel` (the
  kind marker of a kernel type). A `KernelId` is a small host-owned scalar (`usize`) into
  the process kernel registry — never an arena payload, so GC / static-freeze / `ValueExt`
  are unchanged.
- **`ComputeOperator`** = `Jit` (function → kernel) | `Launch` (`[kernel, arg]` → result),
  whose `OperatorExt::run` does the compile/execute.

```
LangValue    = LowValue + TypeValue + ComputeValue
LangOperator = LowOperator + TypeOperator + GcdOp + ComputeOperator
LangProgram  = ProgramImpl<LangValue, LangOperator, Perspective>
```

`enum_ext!` generates the `From<X>` / `AsEnum<X>` pair each extension needs, so the
vocabularies are flat unions with no `Ext` wrapper. The program marker (`LangProgram`)
is what fixes the vocabulary for the whole frontend.

## 2. Native operators as `$name(args)` + an embedded source wrapper

Ops are bound to source through a **general native-call IR** (`ExprKind::NativeCall`),
not callee-type dispatch:

- The plugin registers a private `NativeOps` table (`native_ops()`), a name→`NativeOp`
  `&'static` slice. Only the compilation of the plugin's own embedded `WRAPPER_SOURCE`
  is run against it (`register_compute`), so a `$jit`/`$launch` is **private** to that
  module — two plugins each registering `$jit` never collide.
- `$jit(f)` / `$launch(k, a)` parse to a `NativeCall`; the checker delegates to the
  matching `NativeOp::build` and adopts whatever `[value, type]` pair it returns. The
  checker knows nothing about kernels.

```
jit   = f => $jit(f)                 -- : (F -> G) → Kernel
launch = k => a => $launch(k, a)     -- : Kernel → F → G   (curried, two-step)
(jit, launch)                        -- the exported positional tuple
```

`NativeOp::build` receives the **already-compiled** arguments (`NativeArg { expr, value,
ty }`) and shapes the type through the curated `Ctx` (never raw lowlevel nodes): it
calls `ctx.fresh()`/`ctx.array_node()`/`ctx.value_node()`/`ctx.check_unify()`/`ctx.universe()`,
emits the operator via `ctx.op_node(...)`, and returns the `[value, type]` pair.

- **`JitOp::build`** — function-ness gate: unify `f`'s type with an arrow `[d, c]`;
  re-head it with `TypeKernel` to get `[sig, [TypeKernel, Type]]`; emit
  `ComputeOperator::Jit` over `f.value`; pair with that kernel type.
- **`LaunchOp::build`** — kernel-ness gate: `k`'s type is `[sig, [TypeKernel, Type]]`;
  unify the argument against the domain `d`; emit `ComputeOperator::Launch` over
  `[k.value, a.value]`; pair with the codomain `c` (so the result is `Int`).

See [compiler-plugin](compiler-plugin.md) for the general model.

## 3. Runtime dispatch

The VM dispatches structural `LowOperator`s (`Index`/`Apply`/`TableGet`) through
`AsEnum` first; everything else falls to the program's `OperatorExt::run`. The compute
arm:

- **`Jit`** — `compile_fragment` lowers the function's body to a `KernelFragment`, stores
  it in the process-global `KERNELS` registry under a fresh `KernelId`, returns
  `Kernel(id)`. A non-function target or a body outside the kernel-safe subset stays
  lazy (`Parameterized`) — those are *reported* type errors, not panics.
- **`Launch`** — reads `[kernel, arg]`; flattens the argument to an `i64` vector
  (`collect_args`); `run_kernel` assembles and runs; returns the `USize` result. A
  non-scalar/non-literal argument stays lazy.

The kernel registry is **process-global** (`KERNELS`/`NEXT_KERNEL_ID`), deliberately not
a `GlobalExt` component: kernels are immutable, cross-module-shared artifacts.

## 4. Codegen: bytecode fragments, not a module

`jit` emits the function's **body** as a `KernelFragment { param_shape, body }` — a
`Vec<KernelInstr>` of *abstract* instructions, not raw wasm. Splitting "emit bytecode"
from "assemble a module" is what lets the launcher resolve cross-kernel call indices
after the kernel's relative launch set is laid out.

```
enum KernelInstr {
  Const(i64),          // i64.const
  Bin(KernelBin),      // add/sub/leq/eq over the top two i64
  LocalGet(u32),       // a flattened parameter read
  I32WrapI64,          // the `select` condition
  Select,              // if c then a else b
  CallKernel(KernelId) // style-2/3 cross-kernel call, resolved at assembly
}
```

`emit_node` walks the simple kernel-safe subset — integer constants, `Add`/`Sub`/`Leq`/
`Eq`, the parameter read (`Index(param_pair, 0)` → a `local.get`), a `value_of`
extraction (`Index(pair, 0)`), and a 2-element conditional (a wasm `select`). The
parameter is read by **shape** (`LowShape`, a host marker on the node), so the backend
emits bytecode without consulting the type half or forcing the value. A scalar domain is
one `i64` local; a tuple domain is flattened to per-element locals (`flat_arity`,
`flatten_offset`).

## 5. Launch-time assembly (the deferred linker)

`run_kernel` BFS's the kernel's **relative launch set** — the kernel plus every kernel it
(transitively) `CallKernel`s — into an ordered slice; `assemble_module` builds one wasm
module where `ordered[i]` is function `i`, the root exported as `main`, and each
`CallKernel` resolves to the callee's in-module `call`. `wasmi` instantiates and calls
`main`. This single step covers both a lone kernel and a cross-calling one.

## 6. The three call styles

- **Style 1 — same-module lichen function** (`helper x`): the deep pass eagerly *reduces*
  the call, so the JIT traces the reduced graph; a substituted parameter cell resolves
  to the enclosing kernel's parameter (via its equality class) and becomes a `local.get`.
- **Style 2 — kernel value** (`k x`): an `Apply` whose callee is a kernel value →
  `emit_cross_kernel_call` (an arg then a `CallKernel`), assembled at launch time. The
  bare `k x` form leaves the codomain unresolved (`?a`), so it's asserted, not typed.
- **Style 3 — the wrapper launch** (`compute(1) k x`, the typed cross-module form): the
  `ComputeOperator::Launch` is lowered exactly like a kernel `Apply`, and its result is
  typed `Int` (the codomain resolved by `LaunchOp`).

For style 3, `launch` is a **native two-step** — assemble the module, then call it — so
the argument arrives at codegen time as a bare `Parameterized` cell (expected; it's only
concrete at run time). That cell is **unified** with the defining computation (e.g. the
`x + 1` `Add`) in its equality class, and `emit_node` resolves collapsed cells through
the class (`class_computation_node`) to emit the real expression.

## 7. Tests

`tests/compute.rs` covers scalar/tuple domains, all the safe ops, conditionals, closure
constants, cross-kernel calls (bare and sub-expression), inline lichen functions (nested),
and the wrapper `$launch` form — as `value: type` end-to-end runs.

## 8. v1 scope

The kernel-safe subset is scalar arithmetic over a scalar or tuple-of-scalars domain; the
codomain is a single `i64`. Cross-kernel callees are restricted to a scalar domain
(arity 1). Beyond that: higher-order kernels, recursion inside the compiled region, and a
`GlobalExt`-based compute global (the registry is currently process-global).
