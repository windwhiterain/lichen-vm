# lichen-compute: the JIT package (compile functions to wasm, launch them)

> Status: current — a **native plugin** (see [plugin-taxonomy](plugin-taxonomy.md)):
> program-generic, host-agnostic, added no syntax.
> Points at: `crates/lichen-compute/src/compute.rs` (the native core),
> `crates/lichen-language/src/program.rs` (`LangValue`/`LangOperator`/`LangProgram` composition),
> `crates/lichen-highlevel/src/native.rs` (the `NativeOp`/`NativeOps` extension point),
> `crates/lichen-language/src/package.rs` (`register_compute`, the virtual `compute.lichen`),
> and `crates/lichen-language/tests/compute.rs` (the end-to-end suite).

`lichen-compute` is a compiler plugin — a native wrapper package (think `numpy`)
that jit-compiles a lichen function to a **wasm kernel** and runs it. The user-facing
surface is an embedded `compute.lichen` that re-exports a small namespace:

```
@{ compute = import "compute.lichen" @}
k = compute.jit (x => x + 1)     -- jit: compile the lambda to a kernel (via `$jit`)
compute.launch k 5               -- launch: run it -> 6 : Int (via `$launch`)
```

A **kernel is now a plain struct** `struct<.native _, .sig sig>` — the `.native` field
holds the opaque compiled wasm artifact (a `Kernel`/`ParKernel` value), and the `.sig`
field carries the function signature. There is **no** `TypeKernel`/`TypeParKernel` kind
marker in the vocabulary; a kernel's "type" is just the struct, and the checker transfers
the whole function-apply machinery to it by reading `.native`/`.sig` through the struct's
field accessors. Because the vocabulary no longer special-cases a kernel type, a `jit`
result renders as the raw struct (`struct<.native <_>, .sig Int -> Int>`) rather than the
old `Kernel : Int -> Int` — the concrete signature lives in the `.sig` field, where the
language's shared struct printer spells it.

## 1. The vocabulary injection

The native core provides two plain, `Copy` enums, composed as sibling leaves into the
language's value/operator vocabularies with `lichen_utils::enum_ext!`:

- **`ComputeValue`** = `Kernel(KernelId)` (a compiled scalar kernel artifact) |
  `ParKernel(KernelId)` (a compiled **parallel** kernel) | `Buffer(BufferId)` (a runtime
  results buffer) | `TypeBuffer` (the kind marker of a buffer type). A `KernelId`/`BufferId`
  is a small host-owned scalar (`usize`) into the process kernel/buffer registry — never an
  arena payload, so GC / static-freeze / `ValueExt` are unchanged.
- **`ComputeOperator`** = `Jit` (function → kernel) | `Launch` (`[native, arg]` → result) |
  `Call` (a cross-kernel call on a bare native kernel) | `Parallel` (a single-arg index
  function `?cfg -> ?write` → parallel kernel) | `ParLaunch` (`[native, cfg]` → one output
  buffer, or the tuple of them) | `Range` (the loop index) | `Read` (`[buffer, index]` →
  element) | `Write` (`[n, index, value]`, a kernel-only side effect) | `BufferCollect`
  (`[buffer]` → `[?b]`), whose `OperatorExt::run` does the compile/execute.

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
  `&'static` slice. Only the compilation of the plugin's own embedded `compute.lichen`
  is run against it (`register_compute`), so a `$jit`/`$launch` is **private** to that
  module — two plugins each registering `$jit` never collide.
- `$jit(f)` / `$launch(native, sig, a)` parse to a `NativeCall`; the checker delegates to
  the matching `NativeOp::build` and adopts whatever `[value, type]` pair it returns. The
  checker knows nothing about kernels — the lichen wrapper is the *only* thing that parses
  the kernel struct.

```
{
  jit      = f => (struct<.native _, .sig (type_of f)>)(.native $jit(f), .sig _)
  launch   = k => a => $launch(k.native, k.sig, a)
  call     = k => a => $call(k.native, a)
  parallel = f => (struct<.native _, .sig (type_of f)>)(.native $parallel(f), .sig _)
  plrun    = k => a => $plrun(k.native, k.sig, a)
  range    = x => $range(x)
  read     = x => $read(x(0), x(1)) : Int
  write    = x => $write(x(0), x(1), x(2))
  collect  = b => $collect(b)
}
```

(The listing is `crates/lichen-compute/src/compute.lichen` verbatim.  `read`
destructures its single array argument with positional slot reads, because the
parser's `f [a, b]` is one array argument rather than a curried two-arg call.)

`NativeOp::build` receives the **already-compiled** arguments (`NativeArg { expr, value,
ty }`) and shapes the type through the curated `Ctx` (never raw lowlevel nodes): it
calls `ctx.fresh()`/`ctx.array_node()`/`ctx.value_node()`/`ctx.check_unify()`/`ctx.universe()`,
emits the operator via `ctx.op_node(...)`, and returns the `[value, type]` pair.

- **`JitOp::build`** / **`ParallelOp::build`** — function-ness (or curried-arrow) gate:
  unify the argument's type with the arrow shape; emit `Jit`/`Parallel` over the argument
  value; the result is an **opaque** native artifact typed by a *fresh* cell. The lichen
  wrapper then builds the kernel struct around it (`.native` = the artifact, typed `_`,
  `.sig` = `type_of f`).
- **`LaunchOp::build`** (`$launch(native, sig, a)`) / **`ParLaunchOp::build`**
  (`$plrun(native, sig, a)`) — gate the signature value (a function type),
  unify the argument against the domain, and emit the operator over
  `[native, a]`.  `LaunchOp` pairs the result with the **lazy codomain**, so a
  launch's result is the callee's own codomain (`Int` for a scalar kernel).
  `ParLaunchOp` cannot: its result is a `Buffer` for a one-output index function
  and a *tuple* of buffers for a several-output one, and the arity that decides
  which is not knowable here — `build` runs once on the frozen wrapper template,
  where `.sig` is still an unbound cell, and a tuple type is a value node with
  one element per position.  So the result type is a **fresh cell**: `read` and
  `collect` accept each buffer by ordinal.  That costs static precision, not
  safety — a non-existent ordinal is still refused at check time with a span
  (*"index 5 out of bounds (array length 2)"*).  The *shape* of the
  codomain is checked by the emitter instead, which is where the writes are —
  see [multi-output](#multi-output) and
  [compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md).
- **`CallOp::build`** (`$call(k.native, a)`) — only gates the argument against a fresh
  domain cell and types the result as a fresh codomain cell (the callee signature is read at
  launch-time assembly by `kernel_id_of`).

See [compiler-plugin](compiler-plugin.md) for the general model.

## 3. Runtime dispatch

The VM dispatches structural `LowOperator`s (`Index`/`Apply`/`TableGet`) through
`AsEnum` first; everything else falls to the program's `OperatorExt::run`. The compute
arm:

- **`Jit`** — `compile_fragment` lowers the function's body to a `KernelFragment`, stores
  it in the process-global `KERNELS` registry under a fresh `KernelId`, returns
  `Kernel(id)`. A non-function target or a body outside the kernel-safe subset stays
  lazy (`Parameterized`) — those are *reported* type errors, not panics.
- **`Launch`** / **`Call`** — reads `[kernel, arg]`; flattens the argument to an `i64`
  vector (`collect_args`); `run_kernel` assembles and runs; returns the `USize` result. A
  non-scalar/non-literal argument stays lazy.
- **`Parallel`** — `compile_parallel_fragment` lowers a single-arg index function
  (`cfg = (n, (buffer…))`, the loop index from `compute.range`) to a
  `(n, index) -> i64` wasm function, one `BufferWriteCall` per output, and
  records the output count on the fragment → `ParKernel(id)`.
- **`ParLaunch`** — reads `[parallel_kernel, cfg]`; runs the kernel over the
  index range `[0, cfg(0))` and collects the results into one buffer **per
  output** — a single output is a bare `Buffer(id)`, several are the tuple of
  them (`Read`/`BufferCollect` then address each by ordinal).
- **`Read`** / **`BufferCollect`** — read one element / collect the whole buffer.

A kernel body that calls another kernel is a **cross-kernel call**: `kernel_id_of` walks a
kernel *value* (or a `.native` field read) to its `KernelId`, and the body emitter lowers
the apply to a `CallKernel`, resolved at launch-time assembly.

The kernel/buffer registries are **process-global** (`KERNELS`/`BUFFERS`/`NEXT_KERNEL_ID`/
`NEXT_BUFFER_ID`), deliberately not a `GlobalExt` component: kernels are immutable,
cross-module-shared artifacts.

## 4. Codegen: bytecode fragments, not a module

`jit` emits the function's **body** as a `KernelFragment { param_shape, body,
outputs }` — a `Vec<KernelInstr>` of *abstract* instructions, not raw wasm.
Splitting "emit bytecode" from "assemble a module" is what lets the launcher
resolve cross-kernel call indices after the kernel's relative launch set is laid
out.

```
enum KernelInstr {
  Const(i64),          // i64.const
  Bin(KernelBin),      // add/sub/leq/eq over the top two i64
  LocalGet(u32),       // a flattened parameter read
  I32WrapI64,          // the `select` condition
  Select,              // if c then a else b
  CallKernel(KernelId) // cross-kernel call, resolved at assembly
}
```

`emit_node` walks the simple kernel-safe subset — integer constants, `Add`/`Sub`/`Leq`/
`Eq`, the parameter read (`Index(param_pair, 0)` → a `local.get`), a `value_of`
extraction (`Index(pair, 0)`), and a 2-element conditional (a wasm `select`), plus a
cross-kernel `Launch`/`Call` (lowered to `CallKernel`).

### The JIT walks the value graph, not the types

The design decision that keeps this safe: the JIT reads the lowlevel **value dataflow**
(parameter reads + scalar operators), gets the I/O contract from the kernel signature
(struct's `.sig`), and ignores the type halves and union-find classes. It compiles the
**template shape** — the parameter value (`Index(param_pair, 0)`) is a *symbolic* wasm
local, constants are concrete `i64.const`, and every operator is concrete. There is no
snapshotting of a bound argument and no class-rep routing, so the launch-argument
`Parameterized` handling (§6) does not apply to a pure shape walk. The "runtime is the
typechecker" guarantee holds because the checker's gates run first; the JIT only lowers a
graph the checker has already accepted.

A **multi-arg (tuple) kernel** is just a wider shape: `compute.jit (p : <Int, Int> =>
p(0) + p(1))` compiles to wasm `(i64, i64) -> i64`. The arity comes from the parameter's
**low type** — seeded from the parameter's type cell by the encoding authority, then read
back off the parameter's class (see [compute-jit-low-types](compute-jit-low-types.md)) — which
drives the wasm parameter list (`Vec![ValType::I64; arity]`) and the per-element reads:
`param_path`/`is_param_value` recognise `Index(param_pair, 0)` → `local.get 0` (scalar) and
`Index(Index(param_pair, 0), k)` → `local.get k` (tuple element k), via `flat_arity`/
`flatten_offset`. On the launch side the tuple argument is unwrapped by `collect_args` and
the now multi-arity `main` is called through the dynamic `wasmi::Func::call` API.

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
- **Style 3 — the wrapper launch** (`compute.launch k x`, the typed cross-module form): the
  `ComputeOperator::Launch` is lowered exactly like a kernel `Apply`, and its result is
  typed `Int` (the codomain resolved by `LaunchOp` reading `.sig`).

For style 3, `launch` is a **native two-step** — assemble the module, then call it — so
the argument arrives at codegen time as a bare `Parameterized` cell (expected; it's only
concrete at run time). That cell is **unified** with the defining computation (e.g. the
`x + 1` `Add`) in its equality class, and `emit_node` resolves collapsed cells through
the class (`class_computation_node`) to emit the real expression.

## 7. Tests

`tests/compute.rs` covers scalar/tuple domains, all the safe ops, conditionals, closure
constants, cross-kernel calls (bare, sub-expression, and the wrapper form), inline lichen
functions (nested), and the parallel `parallel`/`plrun`/`range`/`read`/`write`/`collect`
family — as `value: type` end-to-end runs. The cross-kernel group pins the multi-arity argument
mechanisms separately: a concrete tuple argument, a whole-parameter pass-through, a
sub-tuple pass-through against a nested callee domain, and the same tuple argument through
the wrapper `launch` (whose argument arrives as a `Parameterized` cell). The assertions pin
the struct rendering (`struct<.native <_>, .sig Int -> Int>`) and the lazily-read codomain
resolution (`6 : Int`, `12 : Int`).

The **multi-output** group pins the tuple codomain: one `plrun` producing two buffers read
by ordinal (`(2, 4): <Int, Int>`), three outputs with `collect` on a middle one, and the two
refusals that keep the every-ordinal-written invariant — a `compute.write` inside a
conditional, and a codomain position that is not a write (refused *by position*).

## 8. v1 scope

The kernel-safe subset is scalar arithmetic over a scalar or tuple-of-scalars domain; the
codomain is a single `i64`. A cross-kernel callee may have **any** scalar-or-tuple domain:
its argument is flattened into one `i64` per leaf of that domain, either from a concrete
tuple value or passed through from the caller's own parameter (see
[below](#multi-arity-cross-kernel-calls)). Beyond that: higher-order kernels, recursion
inside the compiled region, and a `GlobalExt`-based compute global (the registries are
currently process-global).

### Multi-output

A parallel kernel's codomain may be a **tuple of `Write`s**, so one `plrun` yields several
output buffers from a single pass: the `k`-th `compute.write` is emitted with the
compile-time constant `out_pos = k` (the same treatment `read`'s `cfg_pos` gets), the
output count is the codomain's arity read at compile time and carried on the fragment as
`KernelFragment::outputs`, and the launch allocates exactly that many buffers and returns
them as a tuple. The **every-ordinal-written invariant** — ordinal `k` is written on every
index — holds structurally: the lowered body is straight-line, and the subset's only
conditional is a value `select`, which a write (a side effect with no value) cannot sit in.
The emitter refuses the constructs that could break it, each naming its own cause: a write
inside a conditional, and a codomain position that is not a write. See
[compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md).

The one thing multi-output cannot have is a **check-time result type**: the arity is a
run-time fact of the kernel, so `plrun`'s result is an unconstrained cell rather than an
N-tuple of buffer types (see `ParLaunchOp::build` in §2). The cost is that a positional read
of a `plrun` result is checked at run time rather than at check time.

### Multi-arity cross-kernel calls

The callee's domain is a fact of the **callee's registration**, read from the registry rather
than from the call site, so the caller pushes `flat_arity(domain)` stack values and the
assembler's per-arity type section already matches. Two argument shapes cover it:

- **A whole-parameter read passes through** as the parameter's own locals. A domain's leaves
  are contiguous in the flattened layout (what `flatten_offset` counts), so `r(1)` under a
  `<Int, <<Int,Int>, Int>>` domain starts at local 1 and supplies the callee's three leaves
  from locals 1, 2, 3. The read's own sub-shape must flatten to the callee's arity — a
  shorter read would push the *next* parameter's local as the callee's last argument, so a
  mismatch is refused by arity rather than trusted.
- **A concrete tuple value** is emitted element by element, recursively for a nested domain.
  A scalar element goes through `emit_node`, so a constant, a parameter read, or a
  cross-kernel call result all keep working inside a tuple argument.

The two **encodings** of the argument are not told apart by shape: a bare kernel apply
carries the `[value, type]` pair whose element 0 is the argument, while a `launch` argument
arrives as a bare `Parameterized` cell — and a pair has exactly as many elements as the
two-element tuple it wraps. So each encoding is *emitted* and the first that produces one
leaf per domain element is kept. That is not a guess: the leaves must emit anyway, and a pair
read as a tuple fails on its second element, which is a type cell. Each refusal names its own
cause (wrong encoding / wrong element count / wrong read arity) and none falls back.
