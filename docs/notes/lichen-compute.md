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
--- compute = import "compute.lichen" ---
k = compute.jit (x : Int => x + 1)  -- jit: compile the lambda to a kernel (via `$jit`)
compute.launch k 5               -- launch: run it -> 6 : Int (via `$launch`)
```

A **kernel is a plain struct** `K = _x => struct<.native _, .I _, .O _>` — the
`.native` field holds the opaque compiled wasm artifact (a `Kernel`/`ParKernel`
value), and `.I` (the domain) and `.O` (the codomain) carry the function
signature. There is **no** `TypeKernel`/`TypeParKernel` kind marker in the
vocabulary; a kernel's "type" is just the struct, and the checker transfers
the whole function-apply machinery to it by reading `.native`/`.I`/`.O` through
the struct's field accessors. Because the vocabulary no longer special-cases a
kernel type, a `jit` result renders as the raw struct
(`struct<.native raw[?a, ?b], .I raw[?c, ?d], .O raw[?e, ?f]>`) rather than the
old `Kernel : Int -> Int` — the concrete signature lives in `.I`/`.O`, where the
language's shared struct printer spells it. A **buffer** is the same kind of
wrapper, `Buf = T => struct<.native _, .element T>` (no `TypeBuffer`/`TypeWrite`
marker, and no `BufferId`): the packed payload rides in `.native` and the element
type in `.element`.

## 1. The vocabulary injection

The native core provides two plain, `Copy` enums, composed as sibling leaves into the
language's value/operator vocabularies with `lichen_utils::enum_ext!`:

- **`ComputeValue`** = `Kernel(KernelId)` (a compiled scalar kernel artifact) |
  `ParKernel(KernelId, backend)` (a compiled **parallel** kernel, and where its runs are
  dispatched) | `Buffer(payload, class)` (a runtime results buffer's packed payload, in the
  block arena) | `DeviceBuffer(resident)` (a run's results still on the device) |
  `Graph(GraphId, backend)` (a recorded graph) | the graph's `GraphInput`/`GraphValue`
  placeholders. There is **no** `TypeBuffer`/`TypeWrite` kind marker and no `BufferId`: a
  buffer's *type* is the `Buf` struct, and `Buffer` is only its payload. A `KernelId` is a
  small host-owned scalar (`usize`) into the process kernel registry — never an arena payload
  — so GC / static-freeze / `ValueExt` are unchanged.
- **`ComputeOperator`** = `Jit` (function → kernel) | `Launch` (`[native, arg]` → result) |
  `Call` (a cross-kernel call on a bare native kernel) | `Parallel` (an index function
  `I -> O` → parallel kernel) | `ParLaunch` (`[native, cfg]` → the parameter's `.out`
  structure, one `Buf` field per output) | `Range` (the loop index) | `Read` (`[buffer,
  index]` → element) | `Write` (`[to, index, value]`, a kernel-only side effect whose `.to`
  is an output `Buf`) | `BufferCollect` (`[buffer]` → `[?b]`) | `Graph` | `GraphRun`, whose
  `OperatorExt::run` does the compile/execute.

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
- `$jit(f)` / `$launch(native, a)` parse to a `NativeCall`; the checker delegates to
  the matching `NativeOp::build` and adopts whatever `[value, type]` pair it returns. The
  checker knows nothing about kernels — the lichen wrapper is the *only* thing that parses
  the kernel struct.

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

(The listing is `crates/lichen-compute/src/compute.lichen` — its `type_of`
helper is the standard library's definition
([lichen-std/_.lichen](../../lichen-std/_.lichen)), repeated here because an
embedded native source cannot depend on a package; it is a `let` binding, so it
adds no field to the exported struct.  `K`/`Buf` and the `Read`/`Write`
arguments are ordinary structs now, so the wrapper names their fields rather
than counting positions.)

`NativeOp::build` receives the **already-compiled** arguments (`NativeArg { expr, value,
ty }`) and shapes the type through the curated `Ctx` (never raw lowlevel nodes): it
calls `ctx.fresh()`/`ctx.array_node()`/`ctx.value_node()`/`ctx.check_unify()`/`ctx.universe()`,
emits the operator via `ctx.op_node(...)`, and returns the `[value, type]` pair.

- **`JitOp::build`** / **`ParallelOp::build`** — function-ness (or arrow) gate:
  unify the argument's type with the arrow shape; emit `Jit`/`Parallel` over the argument
  value; the result is an **opaque** native artifact typed by a *fresh* cell. The lichen
  wrapper then builds the kernel struct around it (`.native` = the artifact, typed `_`,
  `.I`/`.O` = the function's domain and codomain).
- **`LaunchOp::build`** (`$launch(k.native, a)`) / **`ParLaunchOp::build`**
  (`$plrun(k.native, a)`) — emit the operator over `[native, a]`; the wrapper's
  `a: k.I` gates the argument and `r: k.O` types the result.  `LaunchOp`'s
  codomain is the callee's own `.O` — `Int` for a scalar kernel, and the
  tuple type itself for a tuple-codomain one — so `LaunchOp` pairs the result
  with the **lazy codomain** and its arity rides along for free.
  `ParLaunchOp`'s result is the parameter's `.out` structure: one `Buf` field per
  output, whose field names the result type carries because `parallel` sets `.O` to
  the parameter type's `.out` group (`I::out`) rather than to the body's own
  (unconstrained) codomain.  The host binds the result with its codomain type once
  and then reads the fields by name — see
  [compute-buffer-wrapper](compute-buffer-wrapper.md) and
  [multi-output](#multi-output).
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
- **`Parallel`** — like `Jit` for the index function, but it takes a **second
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
- **`Parallel`** — `compile_parallel_fragment` lowers the index function's body
  over the named parameter `struct<.n Int, .in …, .out …>` (the loop index from
  `compute.range`) to a wasm function whose leaves are the parameter's scalars
  followed by the index, one `BufferWriteCall` per output, and records the output
  count and the role paths on the fragment → `ParKernel(id, backend)`.
- **`ParLaunch`** — reads `[parallel_kernel, cfg]`; runs the kernel over the
  index range `[0, k.n)` and collects the results into one `Buf` **per output
  field**, placing them in the parameter's `.out` structure — several outputs are
  fields of `.out`, not a tuple, and the host reads each by name (`Read`/
  `BufferCollect` then name the field).  The range is cut
  into one contiguous chunk per **worker** (`available_parallelism`, capped by
  the index count), each owning a disjoint span of every output buffer and
  running the one cached module in its own store; below
  `SEQUENTIAL_PARALLEL_ELEMENTS` indices the whole run stays on the calling
  thread.  See [the parallel ABI](compute-buffer-wrapper.md#the-parallel-abi).
- **`Read`** / **`BufferCollect`** — read one element / collect the whole buffer.

A kernel body that calls another kernel is a **cross-kernel call**: `kernel_id_of` walks a
kernel *value* (or a `.native` field read) to its `KernelId`, and the body emitter lowers
the apply to a `CallKernel`, resolved at launch-time assembly.

The kernel registry is **process-global** (`KERNELS`/`NEXT_KERNEL_ID`), deliberately
not a `GlobalExt` component: kernels are immutable, cross-module-shared artifacts.
A buffer needs no registry at all — its payload lives in the block arena and dies
with its block — so there is no `BUFFERS`/`NEXT_BUFFER_ID` registry.

## 4. Codegen: bytecode fragments, not a module

`jit` emits the function's **body** as a `KernelFragment { param_shape, body,
inputs, outputs, input_classes, output_classes, results, int_width }` — a
`Vec<KernelInstr>` of *abstract* instructions, not raw wasm.
Splitting "emit bytecode" from "assemble a module" is what lets the launcher
resolve cross-kernel call indices after the kernel's relative launch set is laid
out.

`param_shape`'s leaf carries a `ScalarClass`, and `input_classes` /
`output_classes` are one class per buffer position or write ordinal, because a
parallel fragment's shape is the parameter's scalar leaves followed by the index,
however many buffers it reads — so
the buffers' classes cannot live on the shape. All three are in
`fragment_digest`, and a float fragment is permitted — the classes were a carrier
landed ahead of the permission, and
[floating-point](floating-point.md) §5.1 is the record of the permission landing.
What stays refused is a body that mixes the two classes in one operation, and a
crossing that does not name itself with `int2float` or `float2int`
(`docs/notes/operators.md` §7).

`inputs` and `outputs` are the two buffer spaces, and both are counted by the
emitter as it emits the positions rather than declared by hand, so neither can
disagree with the body. They are also **not** in `param_shape`: a parallel
fragment's shape is the parameter's scalar leaves followed by the index, however
many buffers it reads, because the
buffers are bound as storage buffers and reached through a read's position. How
many buffers a *dispatch* supplies is a separate fact, read at apply time from the
call site's parameter struct — see
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

```rust
// Per-value: every instruction names its operands by `ValueId`. `LocalGet` is
// **gone** — a value is read by naming it, not by naming a slot index.
enum KernelInstr {
  Const(ScalarClass, i64),      // a literal, in the class it is declared
  Bin(ScalarClass, KernelBin),  // an arithmetic/comparison/bitwise op over two values
  Conv { from, to },            // `int2float` / `float2int`
  I32WrapI64,                   // the `select` condition
  Select,                       // if c then a else b
  CallKernel(KernelId),         // cross-kernel call, resolved at assembly
  BufferReadCall(ScalarClass),  // the host `read(cfg_pos, idx)` import
  BufferWriteCall(ScalarClass)  // the host `write(out_pos, idx, val)` import
}

// And the body they live in: blocks with parameters, not a flat list.
struct BasicBlock { params: Vec<ValueId>, instrs: Vec<KernelInstr>, terminator: Terminator }
enum Terminator { Return { values: Vec<ValueId> }, Br(Br), CondBr { cond, if_true, if_false } }
```

**Every instruction declares the class it produces**, and the class check
(`compute/wasm/mixed.rs`, shared with SPIR-V) refuses a body that mixes the two in
one operation. The class is read off the **operands**, not off the node the
checker hung them on — a float body's index and count are `Int` positions, so an
operator adding two floats computes in `Float` whatever its node's own class says.
Trusting the node was a real defect: it declared `Bin(Int, Add)` over two float
values.

`Lower` walks the simple kernel-safe subset — integer constants, every
`KernelBin` operator (the arithmetic, comparison and bitwise sets the language
has: `kernel_bin` is the one conversion from `TypeOperator`, and `None` for the
one operator no body can contain), the parameter read (`Index(param_pair, 0)`), a
`value_of` extraction (`Index(pair, 0)`), and a 2-element conditional, plus a
cross-kernel `Launch`/`Call` (lowered to `CallKernel`).

The arithmetic is **unsigned** in both backends — `I64DivU`/`I64RemU` and the
`U` comparisons, never the `S` siblings — because an `Int` is a machine-sized
unsigned integer: a signed reading agrees with the interpreter below `2^63` and
diverges above it, silently. The choice is stated once, in `KernelBin`. See
[operators](operators.md).

### The JIT walks the value graph, not the types

The design decision that keeps this safe: the JIT reads the lowlevel **value dataflow**
(parameter reads + scalar operators), gets the I/O contract from the kernel signature
(the struct's `.I`/`.O`), and ignores the type halves and union-find classes. It compiles the
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
emitted by `Lower`, so each is its own SSA value. The walk is the same shape
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
does, `cross_kernel_call` refuses naming the callee and its arity
(`CROSS_KERNEL_RESULT_ARITY`) rather than truncating to the first result.

### The body's lowering: a graph walk that emits SSA

`crates/lichen-compute/src/compute/body.rs` holds `Lower`, the one walk that turns a
checked graph into a `KernelBody`. It replaced `emit_node`, a recursive walk that
pushed `KernelInstr`s onto a `Vec`. Instructions named no values there, so the
walk's *recursion* was its ordering, and three things followed that were properties
of the IR rather than of any kernel:

- **a shared subexpression was emitted once per use**, because nothing could name
  it after the first;
- **every consumer reconstructed the operand stack**, each deriving the form it
  wanted — the wasm backend derives a stack from SSA, `spirv.rs` derives SSA ids
  from a stack;
- **the walk recursed over the graph**, so it needed a depth budget
  (`MAX_KERNEL_BODY_DEPTH`, a refusal rather than a panic) and `#[stacksafe]` to
  survive a deeply *expanded* body.

Here the walk emits into a `KernelBody` — SSA values, blocks with parameters — and
the memo `Lower::values` is what makes a node emit **once**. `Lower::value` is the
one placement mechanism: a literal, a parameter read at an index path, or a
`define_in` that names the definition. Inside a `@loop` nest the roles are the
binding instead: a node of the marked function's template is a read of its carried
state — the conversion's paths say which slot — or a computation over such reads,
the same role the host loop resolves through `Instantiation::node_of`
(`loop_run.rs`). A nest's own role is emitted in the nest's current block, and a
value the nest reaches but does not own at the nest's entry, the one block that
dominates all of it.

**Which value a node names is the lowlevel's answer, not this file's.** That is
`define_in` and `selection_of` in `lichen_lowlevel::resolve`, because it is a fact
about cells and equality classes. The lowering used to re-derive it on every bare
cell it reached, through `equality_rep` and `class_computation_node`; both moved
down and neither exists here.

**The one thing that is still a count is `Positions`** — the buffer ordinals a
parallel body reads and writes. It is filled by the walk rather than by a consumer
because it is a *shared* fact: the fragment's `input_classes` and `output_classes`
and the refusal wording a backend uses have to agree, and two counters that
disagreed is the defect `mixed_classes`'s contract exists to prevent.

#### The parameter count is the declared shape's

A kernel's parameter is a struct carrying a native wrapper and a signature, so the
node a caller hands over is the wrapper's cell rather than the tuple the domain
declares. `domain_arity` counts the parameter slots' `flat_arity` summed — the
ABI's count — where reading the count off the domain node gave one leaf for a
two-leaf domain, and every tuple read then failed to place. `leaf_classes` is the
same list in the same order, because a leaf's class is the parameter's and not the
body's.

A read of the domain at a path is matched **from the chain of `Index` nodes, not
from the class of its base**: the deep pass may have unified the parameter with the
argument it was passed, so the node the read names is a computation and only the
chain says where it came from. `Index(param_pair, k)` is the pair's value half
rather than a step of the domain's path — it is how a read reaches the parameter at
all, and counting it would descend the parameter's *wrapper* struct as though it
were the domain's shape — and the cursor still stops **on** the pair, because the
pair is the base the slot check has to see. The base is matched against the
parameter slots themselves (each slot's pair and its value) *or* one of the
domain's leaves: a read of `p` reaches the wrapper's field first, so the chain ends
at a leaf rather than at the domain, and matching the slots alone placed six
tuple-domain reads nowhere.

The slot is the path **flattened**, not summed. A flat tuple's positions sum,
because every element before the one named holds exactly one leaf. A nested tuple's
do not: in `<<Int, Int>, Int>`, `p(1)` names the outer element at position 1, which
starts after the inner tuple's **two** leaves — offset **2**, not 1. Summing read
the wrong parameter and the fragment answered `2 + 3 + 3`. The steps come out
innermost-first — the walk descends from the read — so they are reversed before
flattening.

`Lower::arguments` is the other half of `Module::operands_of`: that answers which
nodes a *definition* depends on, so for a program operator the whole operand array
is one dependency (it is built before the operator that indexes it), while
`arguments` answers what the operator *reads* — whatever the array holds. Which is
which is the lowlevel's to answer, because `LowOperator`'s enum documents the
structural shapes and says nothing about a program's own.

A **static operand** is a constant the apply clone carried across, not a value this
graph computes: a routed operator's arguments include the frozen residual of the
call it was lowered from, and the only part of a static module a kernel can carry
is a scalar, so it becomes a `Const` and anything else is refused by name.

#### A call's argument is one of three shapes

`callee_args` reads a cross-kernel call's argument as the callee domain's scalar
leaves, trying three shapes in the order the graph can rule them out:

1. a read of the domain at a path — `k x` and `k x(0)` are the caller's own
   parameters, contiguous in the flattened layout, so they pass through, matched by
   `parameter_path_of`;
2. the `[value, type]` pair's value half — a bare kernel apply carries the pair and
   the argument is its element 0;
3. a concrete tuple value, element by element, recursing for a nested domain.

A wrapper's `launch` argument is a bare `Parameterized` cell, concrete only at run
time, so the tuple it stands for is not an array value here and never will be: the
argument's class is where it is. Measured on
`jit_cross_kernel_tuple_argument_through_the_wrapper`, the pair's value half is its
own singleton class and holds no array, while `class_root(arg)` holds one — the peel
loses the link, not the cell. The old walk tried four encodings in a loop and kept
the first that worked; that is a guess that happens to be checked, where the three
above are shapes, each ruled out by a fact rather than by a later one failing.

#### The four cases an `Apply` is

1. **A routed operator.** The routing lowers `x + 1` to a call of the prelude's
   binding, so the frozen callee is a body this module cannot walk and the
   **residual** the lowlevel's clone wrote for the call is emitted. The residual is
   written only when the argument is decidable, and a loop's step argument never is
   — but the callee is a frozen function whose body is reachable by `FunctionId`
   whether or not anything evaluated, so `routed_operator` reads the operator out of
   the artifact's structure. That is why it answers where the class channel cannot:
   the argument is a read the kernel emits and the host never decides.
2. **A cross-kernel call** — a callee that is a kernel value.
3. **A marked recursion whose shape converts** — the loop nest, over the roles
   `Module::loop_conversion` names, emitted instead of interpreted. Every block of
   the nest receives the whole carried state as its `params`, so a read of the
   marked function's parameter means one thing in all of them; the outermost test is
   the header, because a merge block is joined from the header and a backend's
   structured control flow requires the header to be the block that chooses. The
   caller's remaining instructions land in the merge block, which is where the nest
   leaves its result. See [loop-conversion §8.6](loop-conversion.md#86-where-the-conversion-lives-lowlevel-and-the-jit-reads-it).
4. **An ordinary lichen-function call** — Style 1 — which is refused by name rather
   than inlined, because a cycle cannot be inlined.

#### The class is recorded against the node being defined

`Lower::defining` is set at the top of `Lower::value` rather than in
`Lower::definition`, because a literal is emitted straight from `value` and both
routes have to record their class. Recording it against the wrong node is how a
float constant ended up unrecorded and its operator fell back to the node's own
class. `Lower::emitted_class` is the read side of the same map.

#### The pieces a class crossing needs

- **A literal converts in the language's own classes.** The conversion is the one
  instruction whose operand and result differ, so a literal the checker already
  decided folds rather than being emitted as a constant of the operand's class and
  converted at run time. The direction is the operator's, so a literal of the class
  it does not name means the graph and the word disagree.
- **An out-of-range conversion is refused by name.** The interpreter answers with
  `operator.out_of_range`; a kernel has no channel to record a diagnostic — wasm
  traps and SPIR-V is undefined — so a literal this layer can see is refused where
  the answer is the same for both backends.
- **A parameter's class is the ABI's**, recorded so an operator that reads one
  learns what class it is without the fragment.
- **A read's and a write's element class is declared, not inferred**: the buffer is
  bound by the host, and a write's element is the class of the value written. A
  write's ordinal is taken **before** its operands are emitted, so a write nested
  inside another write's value still consumes an ordinal of its own, which the
  codomain count check refuses.

#### Refusals that name what they saw

`Lower::index`'s refusal reports the target's kind, the index's kind, the number of
chain steps, the base the chain reached, and the parameter slots with their class
roots, because a refusal that says only "cannot place" leaves the reader guessing
between a target that is not an array, an index that is not a constant, and an arm
count that is not two — three different defects. `Lower::opaque` likewise describes
the node rather than numbering it: a kernel is compiled from a template before any
apply, so a binding the body would fill in at run time is still empty — a `let`
alias fed by a buffer read, a helper defined in the body rather than at module
level, and a `compute.call`'s wrapper all have that shape.

A **parallel index function** is the same walk with the writes as its outputs: the
outputs are emitted in position order, so write `k` takes the ordinal `k`. The
writes produce nothing (`KernelInstr::produces` is `0` for a write), so the
terminator returns a `Const` of the fragment's class as a dummy to match the
declared result — stated rather than left implicit, because a body whose returned
value was supposed to mean something would be indistinguishable from one where it
does not. The fragment's domain is the config's leaves *plus* the invocation index,
which is not part of the config parameter's own shape and is therefore added rather
than counted from a shape that does not contain it.

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
  typed `Int` (the codomain resolved by `LaunchOp` reading `.O`).

For style 3, `launch` is a **native two-step** — assemble the module, then call it — so
the argument arrives at codegen time as a bare `Parameterized` cell (expected; it's only
concrete at run time). That cell is **unified** with the defining computation (e.g. the
`x + 1` `Add`) in its equality class, and `define_in` resolves collapsed cells through
the class to emit the real expression.

## 7. Tests

`tests/compute.rs` covers scalar/tuple domains, all the safe ops, conditionals, closure
constants, cross-kernel calls (bare, sub-expression, and the wrapper form), inline lichen
functions (nested), and the parallel `parallel`/`plrun`/`range`/`read`/`write`/`collect`
family — as `value: type` end-to-end runs. The cross-kernel group pins the multi-arity argument
mechanisms separately: a concrete tuple argument, a whole-parameter pass-through, a
sub-tuple pass-through against a nested callee domain, and the same tuple argument through
the wrapper `launch` (whose argument arrives as a `Parameterized` cell). The assertions pin
the struct rendering (`struct<.native raw[?a, ?b], .I raw[?c, ?d], .O raw[?e, ?f]>`) and
the lazily-read codomain resolution (`6 : Int`, `12 : Int`).

The **multi-value** group pins a **tuple codomain** end to end — the mirror of the
multi-arity domain: two leaves returning `(5, 3): <Int, Int>`, a per-leaf body
(pin `5, 8`) so a leaf emitted from the wrong stack slot would show, a returned
tuple indexed back with `r(0)`/`r(1)`, a three-leaf codomain `(7, 8, 9)` (the
distinct wasm signature that forces `assemble_module` to key its type index on the
arity *pair*), and the same multi-value run reached through the untyped
`compute.call` path. One test pins the **decision** about a multi-value callee
called from inside another body: it is refused, and the refusal names its cause
("more than one value") rather than silently truncating to the first result.

The **multi-output** group pins the parallel `.out` structure: one `plrun` producing two `Buf`
fields read by name (`(2, 4)`), three output fields with `collect` on a middle one, and the two
refusals that keep the every-field-written invariant — a `compute.write` inside a
conditional, and an `.out` position that is not a write (refused *by position*).

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
inside the compiled region, and a `GlobalExt`-based compute global (the kernel registry is
currently process-global).

### The every-field-written invariant

A parallel body dispatches writes and produces no value, so output field `k` is written on every
index. That holds **structurally**: the lowered body is straight-line, and the subset's only
conditional is a value `select`, which a write — a side effect with no value — cannot sit in.
The emitter refuses the constructs that could break it, each naming its own cause: a write
inside a conditional (`CONDITIONAL_WRITE`), and an `.out` position that is not a write (refused
*by position*). Nothing is ever defaulted to `0` and nothing is silently reduced to a single
output. The parameter shape, the output ordinals and the run's result structure are
[compute-buffer-wrapper](compute-buffer-wrapper.md)'s model.

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
  A scalar element goes through `Lower::value`, so a constant, a parameter read, or a
  cross-kernel call result all keep working inside a tuple argument.

The two **encodings** of the argument are not told apart by shape: a bare kernel apply
carries the `[value, type]` pair whose element 0 is the argument, while a `launch` argument
arrives as a bare `Parameterized` cell — and a pair has exactly as many elements as the
two-element tuple it wraps. So each encoding is *emitted* and the first that produces one
leaf per domain element is kept. That is not a guess: the leaves must emit anyway, and a pair
read as a tuple fails on its second element, which is a type cell. Each refusal names its own
cause (wrong encoding / wrong element count / wrong read arity) and none falls back.
