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
  launch's result is the callee's own codomain (`Int` for a scalar kernel, and the
  tuple type itself for a tuple-codomain one).  This needs **no change** for a
  tuple codomain, and the asymmetry with `ParLaunchOp` is the point: the codomain
  is *read* here as a type value the checker has already built, so its arity rides
  along for free.  `ParLaunchOp` cannot read it: its result is a `Buffer` for a
  one-output index function
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
  lazy (`Parameterized`) — those are *reported* type errors, not panics. A `jit` kernel
  carries **no backend**: it is launched one invocation at a time, and a device is for the
  thousands a dispatch runs at once.
- **`Parallel`** — like `Jit` for the curried index function, but it takes a **second
  argument, the backend** (`"cpu"` or `"gpu"`), and the kernel value records it
  (`ParKernel(id, backend)`). There is no default and no `auto`: a program says where its
  parallel runs go, and a named backend that cannot be used is a *named* refusal rather
  than a silent fall back — see [lichen-compute-gpu](lichen-compute-gpu.md#the-language-selects-the-backend-on-parallel-only).
  The name rides on the **value**, not the fragment, so it is not hashed by
  `fragment_digest` and the same body compiled for two backends shares one id.
- **`Launch`** / **`Call`** — reads `[kernel, arg]`; flattens the argument to an `i64`
  vector (`collect_args`); `run_kernel` assembles and runs; returns the `USize` result. A
  non-scalar/non-literal argument stays lazy. Every way that can happen records why, under
  the launch path's own category `compute.kernel_launch` — shared by the two arms because
  they refuse on the same three facts: an argument element that is not a concrete `Int`
  (named by its path into the argument, e.g. `argument element 1.1`), an argument that is
  not a parameter vector at all (named by what it is — a string, a function, an undecided
  value), and a run that failed.  The last one is checked in `run_kernel` before `wasmi` sees
  it, because compute holds both numbers — the callee's registered domain flattened by
  `flat_arity`, and the vector built from the argument — so a wrong-arity call reads
  `the callee kernel 3 takes 2 arguments, but this call supplied 3`; the other run failures
  (assembly, the `main` export, a trap) are facts only `wasmi` has and its message is
  propagated as it stands.  `launch` and `call` differ only in how their argument is
  checked — `launch` unifies it against the kernel's signature domain, `call` against a
  fresh cell — so an argument a checker rejects reaches `call` but not `launch`, and the
  arity check is the only one a `call` ever gets.
- **`Parallel`** — `compile_parallel_fragment` lowers a single-arg index function
  (`cfg = (n, (buffer…))`, the loop index from `compute.range`) to a
  `(n, index) -> i64` wasm function, one `BufferWriteCall` per output, and
  records the output count on the fragment → `ParKernel(id)`.
- **`ParLaunch`** — reads `[parallel_kernel, cfg]`; runs the kernel over the
  index range `[0, cfg(0))` and collects the results into one buffer **per
  output** — a single output is a bare `Buffer(id)`, several are the tuple of
  them (`Read`/`BufferCollect` then address each by ordinal).  The range is cut
  into one contiguous chunk per **worker** (`available_parallelism`, capped by
  the index count), each owning a disjoint span of every output buffer and
  running the one cached module in its own store; below
  `SEQUENTIAL_PARALLEL_ELEMENTS` indices the whole run stays on the calling
  thread.  See [the parallel run](compute-parallel-buffer-read-write.md#the-run-is-parallel).
- **`Read`** / **`BufferCollect`** — read one element / collect the whole buffer.

A kernel body that calls another kernel is a **cross-kernel call**: `kernel_id_of` walks a
kernel *value* (or a `.native` field read) to its `KernelId`, and the body emitter lowers
the apply to a `CallKernel`, resolved at launch-time assembly.

The kernel/buffer registries are **process-global** (`KERNELS`/`BUFFERS`/`NEXT_KERNEL_ID`/
`NEXT_BUFFER_ID`), deliberately not a `GlobalExt` component: kernels are immutable,
cross-module-shared artifacts.

## 4. Codegen: bytecode fragments, not a module

`jit` emits the function's **body** as a `KernelFragment { param_shape, body,
inputs, outputs, input_classes, output_classes, results, int_width }` — a
`Vec<KernelInstr>` of *abstract* instructions, not raw wasm.
Splitting "emit bytecode" from "assemble a module" is what lets the launcher
resolve cross-kernel call indices after the kernel's relative launch set is laid
out.

`param_shape`'s leaf carries a `ScalarClass`, and `input_classes` /
`output_classes` are one class per buffer position or write ordinal, because a
parallel fragment's shape is `(config, index)` however many buffers it reads — so
the buffers' classes cannot live on the shape. All three are in
`fragment_digest`, and a fragment with a float anywhere in it is **still refused
by name**: the classes are a carrier landed ahead of the permission, not the
permission. See [floating-point](floating-point.md) §4.4.

`inputs` and `outputs` are the two buffer spaces, and both are counted by the
emitter as it emits the positions rather than declared by hand, so neither can
disagree with the body. They are also **not** in `param_shape`: a parallel
fragment's shape is `(config, index)` however many buffers it reads, because the
buffers are bound as storage buffers and reached through a read's position. How
many buffers a *dispatch* supplies is a separate fact, read at apply time from the
call site's cfg tuple — see
[compute-graph-jit.md](compute-graph-jit.md#how-many-buffers-a-fragment-reads-is-not-in-its-shape-and-the-check-that-asked-was-wrong).

That IR lives in **`lichen-kernel-ir`**, a dependency-free crate, not in this one.
The split is by stage: **lowering** a checked graph to a fragment needs the lowlevel
(it reads it), while **emitting** a fragment needs the fragment and nothing else —
and the emitting stage is the one with more than one plausible implementation. A
dependency in the IR crate would be a dependency every backend cannot avoid, which
is how a backend ends up unable to exist without pulling in another backend's
runtime. A second backend (a GPU one being the obvious candidate) would depend on
`lichen-kernel-ir` and not on `wasmi`.

Two things follow from the IR being its own crate rather than a set of private
types here:

- **`param_shape` is `KernelShape`** (`Scalar` / `Tuple`), not `LowShape`. The IR
  carries only the two shapes a kernel domain can be — the ones `kernel_domain`
  admits — because a backend needs the domain's *structure* to flatten it and
  nothing else. `kernel_shape` in `compute.rs` is the single conversion point, and
  it is deliberately total: a tuple's element is not re-checked, so a shape with
  no IR counterpart can still arrive, and it folds to `Scalar` with the same
  one-leaf arity the local `flat_arity` filler gives those shapes. The
  compiler-stage `ParamSlot.shape` stays a `LowShape`, because `flatten_offset`
  and `sub_shape` walk the full lattice.
- **`int_width` is a declared fact, not an assumption.** The language says
  "integer" and stops: an `Int` reaches the lowered graph as a machine-sized
  integer, so the width in a kernel is a property of the host the compiler ran on.
  A fragment that assumed 64 bits would bake that host's choice into every backend
  and leave a backend whose target has a different (or absent) native width with
  no way to know it must convert at its boundary. A backend that cannot represent
  the declared width refuses the fragment **by name** rather than narrowing,
  because narrowing is a semantic change to a program nobody wrote. `I64` is the
  only variant because it is the only width this compiler produces; a second one is
  a decision it has not made, so it is not spelled as a variant nothing produces.

`outputs` and `results` are two different counts and must not be conflated:
`outputs` is how many **output buffers** a fragment writes (a parallel kernel's
output-buffer count; `0` for a scalar `jit`), and `results` is how many `i64`
values the body leaves on the **stack** — the wasm function's result arity. A
parallel fragment has `results: 1` (its dummy `Const(0)`); a scalar kernel has
`results: 1`; a tuple-codomain kernel has one per leaf. Both are compiled
properties read off the fragment rather than discovered at run time, and both are
in `fragment_digest`, so two fragments differing only in either cannot intern to
one id and be served each other's module. `int_width` is hashed there for the same
reason: `fragment_digest`'s invariant is that *every* field is hashed.

The fragment ids the digest produces are **process-local** — `write_operator`
refuses to serialize any compute operator ("a compute operation is a runtime form
with no on-disk representation"), and a frozen module carries no kernel — so a
change to the digest is a cold start and has no on-disk consequence.

```
enum KernelInstr {
  Const(i64),           // i64.const
  Bin(KernelBin),       // an arithmetic/comparison/bitwise op over the top two i64
  LocalGet(u32),        // a flattened parameter read
  I32WrapI64,           // the `select` condition
  Select,               // if c then a else b
  CallKernel(KernelId), // cross-kernel call, resolved at assembly
  BufferReadCall,       // the host `read(cfg_pos, idx)` import
  BufferWriteCall       // the host `write(out_pos, idx, val)` import
}
```

`emit_node` walks the simple kernel-safe subset — integer constants, every
`KernelBin` operator (the arithmetic, comparison and bitwise sets the language
has: `kernel_bin` is the one conversion from `TypeOperator`, and `None` for the
one operator no body can contain), the parameter read (`Index(param_pair, 0)` →
a `local.get`), a `value_of`
extraction (`Index(pair, 0)`), and a 2-element conditional (a wasm `select`), plus a
cross-kernel `Launch`/`Call` (lowered to `CallKernel`).

The arithmetic is **unsigned** in both backends — `I64DivU`/`I64RemU` and the
`U` comparisons, never the `S` siblings — because an `Int` is a machine-sized
unsigned integer: a signed reading agrees with the interpreter below `2^63` and
diverges above it, silently. The choice is stated once, in `KernelBin`. See
[operators](operators.md).

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

A **tuple codomain** is the mirror: `compute.jit (p : <Int, Int> => (p(0), p(1)))`
compiles to wasm `(i64, i64) -> (i64, i64)`. `codomain_leaves` resolves the body's
return value into the leaves to emit — a bare value is the one-leaf form, a
materialized tuple value one leaf per element, in source order — and each leaf is
emitted by `emit_node`, so each is its own stack slot. The walk is the same shape
as `emit_tuple_leaves`'s (the cross-kernel **argument** walk), reached through the
same peel chain: a `value_of` extraction, a `Parameterized` cell, then the node
itself. A *nested* tuple is refused by name rather than flattened, because each
leaf must leave exactly one value or the stack slots interleave.

The count of leaves becomes `KernelFragment::results`, and it is what types the
assembled function and sizes the launch's output buffer. `assemble_module`
therefore keys its type index on the **pair** `(parameter arity, result arity)`,
not on the parameter arity alone: two fragments can share a parameter arity and
differ in result arity (`(i64) -> i64` against `(i64) -> (i64, i64)`), and wasm
function types are indexed by type — keying on one arity would hand the second the
first's single-`i64` signature and the module would not validate. On the way back,
`kernel_results_value` gives a bare `USize` for one result and the **tuple** of
them for several (the same array value `ParLaunch` builds for its output buffers),
so `r(0)`/`r(1)` index a returned tuple with no special case.

A **callee that returns several values** is refused by name inside a kernel body.
A wasm `call` pushes one value per result, where the caller's body expects one, so
an `N`-result callee would leave `N` values in a one-value position — the silent
miscompilation this crate never allows (the same reason a conditional write is
refused rather than emitted). Re-materialising a tuple would mean spilling those
values into locals, a primitive `KernelInstr` has no encoding for; so until it
does, `emit_cross_kernel_call` refuses naming the callee and its arity
(`CROSS_KERNEL_RESULT_ARITY`) rather than truncating to the first result.

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

The **multi-value** group pins a **tuple codomain** end to end — the mirror of the
multi-arity domain: two leaves returning `(5, 3): <Int, Int>`, a per-leaf body
(pin `5, 8`) so a leaf emitted from the wrong stack slot would show, a returned
tuple indexed back with `r(0)`/`r(1)`, a three-leaf codomain `(7, 8, 9)` (the
distinct wasm signature that forces `assemble_module` to key its type index on the
arity *pair*), and the same multi-value run reached through the untyped
`compute.call` path. One test pins the **decision** about a multi-value callee
called from inside another body: it is refused, and the refusal names its cause
("more than one value") rather than silently truncating to the first result.

The **multi-output** group pins the parallel tuple codomain: one `plrun` producing two buffers read
by ordinal (`(2, 4): <Int, Int>`), three outputs with `collect` on a middle one, and the two
refusals that keep the every-ordinal-written invariant — a `compute.write` inside a
conditional, and a codomain position that is not a write (refused *by position*).

The **parallel-run** group pins that the worker partition is invisible in the result: the
same two-output kernel over a count below the sequential threshold and over one above it
agrees element for element, and the run over the threshold produces the exact expected
value at the first, a middle and the last index of **both** buffers — the chunk boundaries
between two workers, where a wrong rebase or a skipped chunk would show.  Which regime a
count is in is `parallel_worker_count`, and the fact that a fan-out happened at all is
`parallel_launch_workers`; both are unit-tested in `lichen-compute` next to
`chunk_bounds`/`partition_outputs`, because a parallel result is bit-identical to a
sequential one and no end-to-end *value* can distinguish a working fan-out from a dead
code path.

The **refusal** group pins that a `launch`/`call` refusal says which of its three causes it
was: an argument element that is not a concrete `Int` (by its path), an argument that is not
a parameter vector (by what it is), and a run that failed (by the callee's expected arity and
the count actually supplied — both numbers, since this is `call`'s only arity check). All
three are reached through `compute.call`, whose argument gate is a fresh cell and therefore
admits the values `launch`'s signature gate rejects — the `Launch` arm's three recordings are
the same sites, not three more.

## 8. v1 scope

The kernel-safe subset is the language's **unsigned** scalar arithmetic —
`+ - * / %`, the six comparisons and the bitwise trio, plus the conditional — over
a scalar or tuple-of-scalars domain ([operators](operators.md) is the set and the
rules that come with it); the
codomain is a scalar `i64` or a **tuple** of them — a tuple codomain's body is flattened to
one stack slot per leaf, the wasm function returns one `i64` per leaf
(`KernelFragment::results`), and the launch yields their tuple. A **nested** tuple codomain
is refused, and a callee that returns several values is refused inside a body (it cannot be
read as one value). A cross-kernel callee may have **any** scalar-or-tuple domain:
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

### The parallel run

A `plrun` is **data-parallel for real**: the index range is cut into one contiguous
chunk per worker, and each worker runs the one cached module in its own `Store`/
`Linker`/instance and writes only its own **disjoint span of every output buffer** —
a partition built with `split_at_mut`, so there is no lock on the write path.  The
kernel is handed a *global* index either way and the `write` import rebases it by
the worker's base, so the emitted wasm does not know it was split.  Inputs are never
partitioned (a read is by a global index).

Two bounds keep it from being a pessimisation: the worker count is
`min(available_parallelism(), count)`, and a run below `SEQUENTIAL_PARALLEL_ELEMENTS`
indices stays sequential, because a spawn plus a store per worker is only repaid once
a worker owns enough indices for the interpreted calls to dominate it.

The result is **bit-identical to the sequential loop's**: the partition depends only on the
count and the worker count (never on a schedule or a timing), no slot is written by two
workers, and there is no reduction to reorder — so regrouping the indices across a
different number of workers cannot change what is computed.  A worker that fails is
joined and reported with its cause rather than dropped for a partial result.  See
[compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md#the-run-is-parallel).

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
