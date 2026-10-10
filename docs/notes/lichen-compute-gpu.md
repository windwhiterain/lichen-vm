# The GPU backend for the lowered-kernel IR

> Status: current — the arithmetic/select subset of a kernel runs on a real
> device, a `jit`/`plrun` chain can be **recorded and submitted once**
> (`run_chain`, and the `compute.graph`/`compute.graphrun` surface), and a
> fragment may **cross-call another kernel** — the module holds one function per
> fragment in the caller's launch set. What is *not* here is listed under
> [Not yet](#not-yet) rather than left implied.
>
> What this note is: the second consumer of the lowered-kernel IR. It exists
> because that IR is target-neutral, so a backend other than the wasm one can
> consume it without dragging in `wasmi` — and it records what that actually
> cost, because the answer was not the one first assumed.
>
> Points at: `crates/lichen-compute-gpu/src/spirv.rs` (the emitter and its
> refusals), `crates/lichen-compute-gpu/src/dispatch.rs` (the context, the pool
> of submission slots, staging), `crates/lichen-kernel-ir/src/lib.rs`
> (`ParallelBackend`, `LaunchSet`, `Pending`, `KernelFragment`), and
> `crates/lichen-graph-ir/` (the executor).
>
> Companions: [compute-graph-jit](compute-graph-jit.md) (the recorded path and
> the scheduling model), [lichen-compute](lichen-compute.md) (the IR and the
> wasm backend), [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md) (what the
> primitive cannot express, reached by writing algorithms against it), and
> [gpu-algorithms-ladder](gpu-algorithms-ladder.md) (the evidence).

## What the split bought, and what it did not

`lichen-compute-gpu` depends on `lichen-kernel-ir` and `ash` and nothing else in
the workspace, and nothing in the workspace depends on it. A build that never
dispatches a kernel to a device never links a Vulkan loader.

**The IR needed no change to be emitted for a completely different target.** The
same SSA `KernelBody` lowers to wasm bytes and to SPIR-V, and the one place the
two disagree is the clearest evidence it is genuinely neutral:

| | wasm backend | this backend |
|---|---|---|
| buffer access | a host import, `call` a function | `OpAccessChain` + `OpLoad`/`OpStore` on a storage buffer |
| comparison result | an `i64`, narrowed to `i32` for `select` | a `bool` already |
| `KernelInstr::I32WrapI64` | **emitted** | **a no-op** *when the condition is a comparison* |

`I32WrapI64` exists *solely* because the wasm MVP's `select` takes an `i32`
condition; SPIR-V's comparison operations yield `OpTypeBool`, which is what
`OpSelect` takes, so the narrowing has nowhere to go. An IR that had encoded
either target's choice would have made the other backend wrong.

**Neither backend tracks an operand stack**, which is what let the table above
survive the SSA rewrite that deleted `LocalGet`, `Flow` and `BlockId`. Both walk
the body's blocks; the wasm one maps a `ValueId` to a slot and a type, this one
to a `Slot`.

**Structured control flow exists here too, and it is the SSA body's.** The emitter
plans blocks and emits `OpBranch`, `OpBranchConditional`, `OpLoopMerge`,
`OpSelectionMerge` and **one `OpPhi` per block parameter**, which is what a
block's `params` were always for. A loop header is recognised from the graph — a
block some block it transitively reaches branches back to — and its exit is the
arm that cannot reach it again; a header with no exit, or with both arms reaching
back, is a `ControlFlow` refusal naming which. What a loop changes for a
**dispatch** is the write rule (see the tail-lane obligation below), and that is
the only place this target is still narrower than the IR.

### The two type facts the IR does not carry

The IR is untyped, and this target is not: `OpTypeBool` and `OpTypeInt` are
distinct, and a module that mixed them is rejected by the driver. Two facts
therefore live in the emitter rather than in the IR, and both are the *same*
fact — that the language says a comparison is the `0`/`1` scalar, which is not
what this target's comparisons produce:

- **A `bool` reaching a scalar position is materialised** (`as_class`:
  `OpSelect` over the class's `1` and `0`), and a **scalar condition is
  converted** (`as_condition`: `OpINotEqual` against zero — non-zero is true,
  which is what wasm's `select` means by its `i32`). Each slot on the emitter's
  stack records which of the two it holds, because "was this a comparison" is
  knowledge only the walk has. The common shape — an `if` over a comparison —
  needs neither, and that is why `I32WrapI64` is a no-op there; the operators in
  [operators](operators.md) are what made the other shapes reachable
  (`(a < b) & c`, a stored predicate, a comparison of a comparison).
- **The fragment's integer type is unsigned** — `OpTypeInt 64 0`. This is not a
  style choice: `OpUDiv`, `OpUMod` and `OpULessThan` require operands whose
  signedness is `0`, so a signed type with those opcodes is an invalid module
  rather than a wrong answer. It is also simply true — an `Int` is a
  machine-sized unsigned integer, so `/`, `%` and the order comparisons are the
  unsigned ones, and declaring the type signed would have *required* the signed
  opcodes and their silent divergence above `2^63`.

A module's arithmetic class is baked in — it selects the opcodes a `Bin` reaches
and is the fallback for a slot its class lists do not reach — while a buffer's
element type, stride, block struct and variable type are read per buffer. Every
module declares both element types, so **a crossing always has the type it
needs**; the price is that a float module's `Int` data is 32-bit where the wasm
target's is 64-bit, so the two diverge past 2³². See
[kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) for the class rules
and the recorded truncation.

**`IntWidth` is a declared fact, not an assumption.** A buffer *index* needs no
capability — SPIR-V addressing is 32-bit by default with a 4 GB range, and one
`plrun` is bounded at 8 MiB. Only the *data* width needs a device capability
(`shaderInt64` on Vulkan), so a device without it gets `RunError::MissingInt64`
rather than a silent narrowing. `needs_int64` is what a caller checks before it
builds a pipeline.

## The language selects the backend, on `parallel` only

`compute.parallel` takes the backend as a second argument, and there is **no
default and no `auto`**:

```lichen
k = compute.parallel f "cpu"
out = compute.plrun k (4,)

k2 = compute.parallel g "gpu"
```

Two decisions carry most of the weight:

- **Only `parallel` takes one.** A `jit` kernel is launched a single invocation
  at a time, and a device is for the thousands a dispatch runs at once, so there
  is nothing for a backend to choose between. `ComputeValue::Kernel` therefore
  has no backend field at all, and `launch` is untouched.
- **No "try either" value.** A named backend that declines is a *named* failure.
  There is no third value to fall back to, because overriding the author's
  choice is the one place the language's explicit dataflow would quietly stop
  being explicit. The refusals, all by name:

  | refusal | when |
  |---|---|
  | `…is not a compute backend; name one of "cpu" or "gpu"` | a name that is neither |
  | `a compute backend must be the string "cpu" or "gpu", not …` | a non-string — caught statically, as a check error |
  | `this parallel kernel was compiled for the "gpu" backend, but no compute backend is installed…` | `"gpu"` with no backend installed |
  | `the "stub" backend declined this run: <its reason>` | a backend that declines |

The name lives on the **value**, not on the fragment, so it is *not* hashed by
`fragment_digest`. Two programs that compile the same body for different backends
share one fragment id, and the choice can never make a cache serve one backend's
module for another's.

A host program installs the backend with `lichen_compute_gpu::install_default()`.
That is the composition step, and it is the only place the two crates meet —
`lichen-compute` still does not name this crate. `install_default` is what
`lichen-language`'s tests and examples call; it is a **dev-dependency** of
`lichen-language`, so the library half still never links a GPU loader.

The chain from source to a device is proved in two halves, on purpose:
`lichen-compute`'s tests install a **stub** backend and pin the routing (that a
`"gpu"` kernel reaches the installed backend, that its answer is the answer, and
that a decline is reported rather than swallowed), while this crate's tests run a
real `KernelFragment` on the device against a hand-written expectation. The seam
test lives in `lichen-language`, because it is the one crate that can compile a
lichen program and install a backend: a first kernel writes `[10, 11, 12]`, a
second reads that buffer and doubles it, and the whole thing is checked against
the rendered output.

**A green device run is evidence that nothing crashed, not that a device
agreed.** Every device-backed test returns early and prints the reason when it
cannot open a context — and that is **any** failure to open one, so a genuinely
broken backend on a GPU machine skips rather than fails. `--nocapture` is how a
skip is read, because libtest captures stderr for a passing test.

## The emitter is hand-written, and that is a deliberate trade

Roughly twenty opcodes. A builder crate would remove the spelling mistakes but
**not** the failures that actually happen, which are all in the *module
contract*: the section order, the declared capabilities, the decorations, and
what the entry point's interface lists. A builder validates none of that.

What removes that risk is offline validation, and it is why `spirv-val` is the
development tool for this crate rather than a runtime hope. `tests/spirv_validation.rs`
**runs** the validator rather than depending on it: it emits a fragment, pipes the
words to `spirv-val --target-env vulkan1.1 -`, and asserts the module validates.
That check needs no device, and it **skips loudly** (rather than passing quietly)
when the tool is not on `PATH` — with the caveat that the fragment in the test is
a copy of the one in `examples/emit-spv.rs`, so a change to the example must be
mirrored there.

Three module shapes are validated beside it, each because it is a shape the module
*contract* rather than an opcode list decides: a fragment whose buffers are of two
classes (the id-range collision above), a body with structured control flow, and a
launch set holding a cross-kernel call (two functions, one `OpFunctionCall`).

The errors that shape actually produced, each named by the validator or the
device, are the module contract in list form:

- `OpConstant` emitted **inside** the function, where it is not module-scope.
- A `StorageBuffer` variable typed as a bare `OpTypeRuntimeArray` — Vulkan
  requires a struct (`VUID-StandaloneSpirv-Uniform-06807`), so a runtime array is
  a struct's final member and the struct is decorated `Block` and laid out.
- `OpUConvert`'s result type signed, where it must be unsigned.
- **A `StorageBuffer` access chain needs three indices**, not two — the buffer
  struct's member, the element, *and* the element struct's member, because the
  element is itself a struct. This one the validator could not see; the device
  caught it.

## Two passes, because a module's sections are ordered

SPIR-V requires instructions in a fixed section order, and the entry point —
which must name the function and list the globals that function reaches — comes
*before* the types, variables and body it names. Ids are therefore **all
pre-allocated** before anything is emitted, the body is walked first to learn
which buffers it touches, and only then is the module written in section order.
Emitting in one pass would mean either forward-referencing the entry point or
emitting it twice. `assemble`'s numbered sections are that order made explicit:
capabilities, memory model and entry point, annotations, types and constants,
variables, then the functions.

The emitter keeps **one id per `ValueId`** — SPIR-V is SSA and this emitter does
not pretend otherwise.

**Every module-scope id comes from one cursor, and that is not a style choice.**
The constants used to sit at a hand-written range `16..21`, which stayed correct
only while a module declared one buffer class: a second chain reached into that
range, and `spirv-val` answered `Id 16 is defined more than once` for a module
whose two buffers were of two classes. The device accepted it, which is the
worse half — the emitter's output is checked offline precisely because a driver
is not the authority on the specification.

**Function-local ids come from the same cursor and are never reused.** SPIR-V
scopes an id to the function that defines it, so reuse is legal; what it is not
is *free*, because a module-scope constant and a local would then be one id
inside the function that reused it. Handing out from one cursor that only moves
forward makes that inexpressible rather than merely avoided.

## The SSA emitter: how a body becomes structured SPIR-V

`plan_body` lays the body's blocks out as structured control flow, and everything
it decides is a structural fact rather than a target preference:

- **A loop header is recognised, not marked.** SSA has no `While`: a block is a
  loop header when an edge on the active depth-first path returns to it, and that
  is the same rule that makes it the dominator of everything the loop holds. A
  `CondBr` that is not a loop header is a selection, whose merge block is the
  immediate post-dominator of its arms.
- **Which arm stays in the loop is the one that can reach the header again**, so
  the exit is decided by the same edge the header was. `OpLoopMerge`'s continue
  target must differ from the header and be dominated by it, and a header with no
  exit or with both arms reaching back is a `ControlFlow` refusal naming which.
- **A selection's merge block must be dominated by the selection.** A join the
  selection is inside — a loop header, an outer join — is not, so the selection
  gets a pass-through block of its own and branches the join from there.
  `redirect` resolves a declared target through those pass-throughs, taking the
  **innermost** enclosing selection (a header that dominates the one already
  chosen is deeper).
- **Depth-first pre-order is what makes a definition precede its uses**: a
  dominator is an ancestor in the depth-first tree, so it is written first. The
  immediate post-dominators are computed over the graph with one virtual exit
  every return reaches; that exit is the fallback answer only when no real block
  is a strict post-dominator.

On top of that plan:

- **One `OpPhi` per block parameter**, filled in as the edges that reach the block
  are emitted — a predecessor's terminator is emitted before the block it enters.
- **The index parameter is the one entry-block parameter this target can place**:
  it is the invocation id rather than a value of the fragment's domain, so it is a
  *binding* rather than an instruction to interpret, and any other parameter is
  refused where it is read (`SpirvRefusal::NonIndexParameter`) rather than
  silently given some id. `index_local` finds it as the last leaf of the flattened
  parameters, recognised structurally so a caller cannot disagree with a fragment
  about which parameter it is.
- **An `OpPhi` has one type however its edges were written**, so `resolved` fixes a
  literal's kind to the module's class. A constant is emitted once per
  (class, value) — SPIR-V requires every id to be defined exactly once — and since
  `OpConstant` is module-scope the body's literals are collected in pass 1. The
  same `Const(0)` is a buffer position in one place and a float's bit pattern in
  another, so the payload rides in a `Literals` pool until a position reads it and
  is materialised per class on demand.
- **The returned values are the terminator's list**, so the result arity is a fact
  of the body rather than of whatever happened to be left over.

Three class facts the walk carries:

- **A comparison's operands and result need not be scalars** — `(a < b) == c`
  compares the scalar a comparison means — so both are materialised where the
  position demands it. A comparison itself yields a `bool`, which is what `Select`
  consumes and needs no widening.
- **Float `==`/`!=` compares bit patterns.** The language routes both through
  `ValueExt::value_eq`, which for a float compares `to_bits`, so `0.0 == -0.0` is
  `0` and `NaN == NaN` is `1`. `OpFOrdEqual` is the trap here — right for IEEE and
  wrong for this language — so a float equality reinterprets both operands in their
  32-bit reading and compares the integers, which is `to_bits` exactly.
- **A crossing is representable in both module classes**, because both element
  types are declared (below). The operand is the class the conversion is *from*,
  and a crossing of one class to itself is a reclassification that emits nothing:
  `Int → Float` is `OpConvertUToF` and `Float → Int` is `OpConvertFToU`. An access
  chain's index is an integer, so a float in that position is refused by name
  rather than converted.

`OpFDiv` is emitted plainly with no zero-divisor guard: a zero divisor is
undefined here and IEEE on wasm, the recorded price of admitting floats
([floating-point](floating-point.md) §4.4).

One position rule: a buffer operation's *position* selects **which**
storage-buffer variable to reach, so it is read as a number rather than as an id.
A position is a `Const` computed immediately before the call — a value that was
*computed* is not an ordinal however constant its value happens to be, and
reading one as a position would address a buffer the caller never named.

## Several functions in one module

A `KernelInstr::CallKernel` names **another compiled kernel** rather than an
inlined body, so no backend can resolve one from the calling fragment: the
callee's *position* and the callee's *own domain* are facts the caller holds and
the fragment does not. Both backends therefore take the same thing —
`LaunchSet`, the fragments in the order the module lays them out plus each kernel
id's position in them (`crates/lichen-kernel-ir/src/lib.rs`) — and
`lichen-compute` discovers it in one place (`ordered_launch_set`) for the wasm
link and for `ParallelBackend::run` alike. **That is what a caller owes the
emitter**: assemble the callees into the set, or the call is refused by name.

The emitter takes `&LaunchSet` and a `Binding` — this is the one signature the
single-fragment form could not keep — and `dispatch`'s pipeline cache is keyed on
the **whole set's** digests rather than the root's, because two sets that share a
root and differ in a callee are two modules.

What the module then holds:

- **One `OpFunction` per fragment**, in the set's order, position `0` being the
  entry point the pipeline names `main`. Every function id is allocated before any
  body is walked, so a call may name a function the module declares later.
- **`OpFunctionCall` names the callee's function id**, and is typed by that
  callee's own `result_classes` — which is the other half of why the callee's
  fragment, not merely its position, has to reach the emitter.
- **A callee's arity is its own domain**, read off `param_shape.flat_arity()`.
  `KernelInstr::arity` answers `None` for a call because the IR does not carry it,
  so a call whose argument count disagrees with the callee is refused by name
  (`CrossKernelArity`, naming both counts) rather than emitted.
- **A callee takes its whole domain as `OpFunctionParameter`s and ends with
  `OpReturnValue`**, in the class its `result_classes` names. Only the entry point
  reads the invocation id and ends with a bare `OpReturn`, because a compute
  shader communicates through its bound buffers. So `NonIndexParameter` is about
  the *entry point* alone: a callee's parameters are ordinary parameters, which is
  the one place the two kinds of function genuinely differ.
- **A call to a kernel the set does not hold is refused by name**
  (`CalleeNotInLaunchSet`), naming which kernel: the set is the caller's to
  assemble, so that is where the fix is.
- **The buffer variables are the module's, not a function's.** A called function
  reaches the same bound buffers through the same descriptor set, so a slot's
  element class is read through the **root** fragment — a slot is one variable and
  a variable has the one class the host binds, which is what `dispatch` stages it
  at. A callee that wants the other class crosses inside its own body, the same
  `Conv` path any mixed body uses.
- **`needs_int64` asks the whole set**, because a callee is a function in this
  module: an integer buffer it reads is an integer chain this module declares.

Transitivity is the set's, not the emitter's: `k1` calling `k2` calling `k3` is
one module with three functions as soon as the caller assembles the closure, and
the closure is breadth-first over `CallKernel` so a callee is always at a position
above its caller.

What this does **not** admit is a *parallel* callee. A parallel fragment's
parameter is `(config, index)`, and only the index is a value a body can name — the
`config` group is not reachable from inside a kernel — so a call to one has no
argument list to write and is refused as a non-index parameter read. A parallel
body may call a `jit` kernel, which is the shape the language produces and the one
the cross-backend test runs.


A module's *arithmetic* class and a buffer's *element* class are two different
facts, and a mixed fragment is the case that separates them.

- **`module_class` is the arithmetic class.** Buffer element classes are read
  first — they are what the body reads and writes, and the positions data crosses
  the ABI at — then the parameter leaves **except the index**, since a parallel
  fragment's `param_shape` is `(config, index)` and both leaves are integers
  whatever its buffers hold. A fragment with neither is an integer module. It no
  longer refuses a fragment whose buffers disagree: it is one module's arithmetic
  class, and the first buffer class wins.
- **`buffer_classes` is the set of classes the module declares a chain for.** A
  class no buffer holds is not declared: an unused `OpTypeStruct` is legal but it
  would be a second copy of a rule nobody asked for, and a fragment that declared
  it would have no way to say which of its chains a given binding means.
- **The chain itself** (`BufferTypes`) is the element struct, the runtime array
  over it, the block struct that wraps the array, and the two pointers the body
  reaches through. Two of the module-contract errors above are why it has that
  shape: a `StorageBuffer` variable must be typed as a struct (or an array of
  one), and a runtime array may only be a struct's final member, so the buffer
  takes two struct levels. `ptr_elem` points at one element *in the
  storage-buffer storage class*, and its pointee is the class's own scalar.
- **One chain per class in use**, keyed by `ScalarClass::index`, five ids each in
  `ScalarClass::ALL` order, allocated with every other module-scope id before the
  bodies are walked; `chain_of` is only ever read for a class `buffer_classes`
  reported. An id nothing defines is legal — the id bound is an upper limit, not a
  count — so a float module reserves `ulong` and leaves it undefined. The
  function-local ids come from the same cursor, continuing above the last
  module-scope id, and the module's id bound is where it stopped.
- **`buffer_class_of(slot, fallback)`** reads slots inputs-first-then-outputs, the
  order a `Binding` and every `Buffer{Read,Write}Call` position use. It falls back
  to `module_class` for a slot the fragment's lists do not reach, and it is
  `pub(crate)` because the **dispatch path asks the same question**: the bytes
  staged for a buffer are that buffer's class's `byte_width()`, so a caller and the
  module it binds cannot disagree about how wide a buffer is. **In a module with
  several functions it is read through the launch set's root**, because the slots
  are the root's: a callee reaches the same bound buffers and does not bind any of
  its own.
- **A buffer's variable is typed with its own class's block struct**, which is what
  makes a mixed module bindable: a dispatch binds a descriptor against the type the
  shader declares for that binding, so the `Int` variable must be the `Int` chain
  and the `Float` variable the `Float` one.
- **Both scalar types are declared in every module**, because a body may hold
  values of either class and cross between them through `Conv`. `Float32` is core
  SPIR-V and carries no capability, so it is unconditional; the 64-bit integer is
  what costs `Int64`, declared whenever **anything** in the fragment is 64-bit —
  an integer buffer counts, which is the difference a mixed fragment makes
  (`needs_int64` asks the same question). A float fragment therefore needs no
  device capability at all: its index is 32-bit and no value in it is a 64-bit
  integer.
- **The element struct's only member is that buffer's own scalar**, which is where
  "does this buffer hold floats" is decided — per buffer, not per module. The
  `ArrayStride` decoration is that same width — eight bytes for the integer ABI's
  `i64`, four for an `f32` — and it is the *only* place the module states it: the
  runtime array it decorates is the buffer a dispatch binds, and a dispatch reads
  the stride off the buffer it is binding rather than off the module, so one
  stride per class in use is enough.
- **A storage-buffer access chain needs three indices**, not two: the block
  struct's member, the element within the runtime array, and the element struct's
  member. A read yields that buffer's element class; a write stores that buffer's
  element class, whichever class the body computed the value in.

## Two position spaces, and the silent bug they caused

A read's position (`cfg_pos`) counts over the **inputs**. A write's position
(`out_pos`) counts over the **outputs** — the IR documents it as the write's
*emission ordinal*, the index function's codomain position. They are two
distinct spaces, and collapsing them into one list of buffers is a bug that
produces **no error at all**: a write at position `0` reaches **input 0**, every
run writes its result into its own input, and the output comes back as
plausible-looking zeroes with no Vulkan error and no diagnostic. `tests/refusals.rs`
pins the separation, including the negative case where a write names an output
that does not exist.

Related and worth knowing: a parallel fragment's `param_shape` is
`(config, index)` — **two leaves, however many inputs there are**. Extra inputs
are reached through a read's position, not through further parameters, so the
index is always parameter 1, and a fragment with three parameter leaves is not a
shape the compiler produces. (This is the fact the graph runner's old
`flat_arity - 1` input check got wrong; see
[compute-graph-jit](compute-graph-jit.md).)

## The tail-lane obligation

A dispatch covers `LOCAL_SIZE_X` (64) invocations per workgroup, so the last
workgroup runs lanes past the end. The shader has **no bounds test**, and that is
deliberate: a bounds test is a branch every lane takes, on the hottest path, to
guard indices the host already knows.

Instead every buffer is allocated rounded up to a whole number of workgroups and
the padding is zeroed — inputs so the surplus lanes' reads are in bounds and
return `0`, outputs so their writes land in padding rather than past the end.
Inputs are zeroed **on the device**, by a `cmdFillBuffer` recorded next to the
upload: only the `count` real elements cross the bus, and the tail is cleared
where it already lives rather than uploaded as zeroes the host had copies of
anyway. Only the first `count` elements are read back. Out-of-range access is
impossible by construction rather than by a runtime test, and the shader stays
branch-free. The tests pin **both** ends of that: a count that is not a multiple
of 64 (so the padding is exercised) and one that is exactly a multiple (so it is
not).

**An uninitialised output buffer is safe only because a write is reached by every
invocation.** `dispatch` allocates output buffers and does not initialise them, so
a lane that never reaches its `BufferWriteCall` leaves a hole the reader would
take for data. A **selection** arm is fine: it runs on exactly the lanes that took
it, and a `select` is a value rather than a branch. **A loop body is not**, because
a zero-trip count is a lane that never runs it — so a write inside a loop is
refused by name (`SpirvRefusal::WriteInsideLoop`) rather than emitted, and the
straight-line body the invariant rests on is the one the subset produces.

## Acceptance

The criterion is bit-for-bit agreement, checked three ways per run: against a
**hand-written expected vector**, and against an independent CPU reading of the
IR. The expected vector is the point — comparing the GPU only against another
implementation of the same reading would pass a fragment that both misread.

The refusal tests need no device, deliberately: a refusal that only appeared once
a GPU was present would be untestable on a machine without one.

## Staging must be cached, or the run is refused

The single biggest performance fact about this backend is not in the dispatch at
all — it is in which Vulkan memory type the staging lands in, and the rule is
easy to get wrong in a way that still produces correct results.

On the development machine, the memory types on offer are:

| index | flags | heap | what it is |
|---|---|---|---|
| 1 | `0x0001` | VRAM | device-local, not host-visible |
| 3 | `0x0006` | system RAM | host-visible + coherent, **no cache** |
| 4 | `0x000e` | system RAM | host-visible + coherent + **cached** |
| 5 | `0x0007` | VRAM | BAR aperture, device-local + host-visible |

Selecting "the first type that is host-visible and host-coherent" picks **index
3**, which on a discrete GPU is system RAM reached over PCIe with nothing behind
it. Every host touch on it is a round trip, so a run pays two uncached transfers
per element and the whole backend lands ~15× behind a scalar loop — while
reporting success at every step. The fix is one flag (`HOST_CACHED`), and on the
development machine the difference is 55 ms against 4.8 ms at a million elements.

So `cached_memory_type` **requires** `HOST_CACHED` and refuses a device that has
none, rather than falling back. A fallback would be *correct* and would silently
put the backend back where it started, with nothing in the output to say so. This
is the same rule the rest of the crate follows: no silent fallback onto a path
that changes what the numbers mean.

## What a host-side run refuses, and what it assumes

- **A parameter the body *reads* is refused by name** (`RunError::ScalarsNotPushed`):
  the device path pushes no leaf at all — the extent and the index and nothing
  else — so a read parameter would have no value to arrive, and every lane would
  compute from the wrong value
  ([compute-runtime-scalars](compute-runtime-scalars.md) §3). **A declared leaf the
  body never reads is not missing**, and that is the whole of the condition: a
  parameter declares its leaves whether or not the body names one, so an input
  group's filler is a leaf nothing demands
  ([compute-buffer-wrapper](compute-buffer-wrapper.md)). `spirv` draws the same
  line at `SpirvRefusal::NonIndexParameter`; this side's test is "is this operand a
  *different* entry-block parameter", since a parameter read is an operand naming a
  block parameter.
- **A host input shorter than the run's count is refused**, each against its own
  class's width rather than one module-wide answer. Only a host slot can be too
  short: a resident buffer already holds what an earlier run put there, and its
  length is that run's business.
- **The outputs are not initialised.** They are what the caller keeps, so they
  become resident, and they go into the same descriptor list as the inputs — the
  binding layout is inputs-then-outputs in one set, so a run's outputs occupy the
  slots after its inputs rather than a second set. Each is allocated and recorded
  at its own class, the width a fetch reads it back at.

## What the GPU actually costs, and where the crossover is

Two numbers decide the design, and both are measured on the development machine:

- **A dispatch's fixed cost is the round trip, and the kernel inside it is not.**
  An empty dispatch — one workgroup, nothing but the dispatch itself — reads
  **0.04–0.05 ms** across runs of the same binary. A dispatch reading a **host**
  input pays about 0.007 ms more than one reading a **resident** buffer, because
  the host input `memcpy`s into staging and the device copies it out in the same
  submission. Using the host floor on a chain link — which never uploads —
  credits the link with an upload it never did, and the resulting figure can
  exceed 100%, which is a wrong divisor rather than a noisy one.
- **The per-link cost barely moves with count** — roughly 0.04 ms to 0.12 ms
  across a 1024× range — because the round trip is fixed. So the same fixed cost
  is ~92% of a 16-link chain's link at a thousand elements and ~30% of one at a
  million: **recording a chain as one submission is worth 65–81% of a 16-link
  chain at counts up to 65 536, and ~10% at a million.** The single row every
  early estimate used — a million elements — is the one least favourable to
  fusing.

`run_chain` records a chain into one command buffer, submits once and waits once,
and it is faster than the unfused chain at every count measured: **5.7× at 1 024
elements, 5.4× at 4 096, 4.7× at 16 384, 3.0× at 65 536, 1.5× at 262 144, 1.23× at
1 048 576.** It is verified against the kernel's **closed form** (`2^n * x +
(2^n − 1)`), derived rather than run — a second CPU copy of the same loop would
agree with a mis-ordered chain just as cheerfully.

Three design facts the measurement bought, all of them load-bearing:

- **A recycled buffer needs no clearing**, because every consumer writes all of
  it: an output is written across `[0, padded)` by the shader's straight-line
  body, and an input is uploaded across `[0, count)` with the tail cleared on the
  device. The pool is keyed by exact element count **and class**, so a chain at a
  fixed size and class is served from it forever, while a different size — or the
  same size in the other class, whose elements are half as wide — simply
  allocates. A cap keeps that from turning a release into a permanent VRAM
  reservation.
- **Allocating one output buffer per link is not merely bigger, it is three times
  slower than not fusing at all.** A link reads the buffer the link before it
  wrote, so two buffers ping-ponged round a chain are enough; a pool that has to
  hold a buffer per link is empty at the start of every call, so every call
  allocates the lot and discards most of it. The allocation is the cost, and the
  same lesson arrives from the other direction: recycling released device buffers
  was the largest single win in the fixed cost (about 4×), while reusing seven
  per-dispatch objects was about 10%.
- **There is deliberately no minimum-count gate.** The measurement says the
  answer is not a count: 65 536 elements loses at every chain length while
  1 048 576 wins from four links on. A count threshold would be a number invented
  to look careful about a quantity that is not the one that decides it.

The GPU still loses on a **single cold dispatch**, and that is not a bug: the
first upload is 8 MB across PCIe, which costs about what the scalar loop costs
outright. The crossover is at **about four links** at a million elements, and it
climbs from there. Read the ratio columns loosely — one sample per point is not
monotonic in links, and the CPU denominator stands in for a thread pool that is
not being measured.

## Two corrections, because the first answers were wrong

Two claims about GPU compute shaped this work, and **both were wrong at first**.
They are recorded because the wrong versions are the plausible ones.

**"A GPU backend needs the kernel body to be index-free."** False. A compute
shader indexes freely, and a dispatch giving each invocation one index maps
straight onto the existing index-based parallel kernel — one invocation per
index, the body unchanged. Being index-free is a precondition only for
*vectorising across a lane group*, which is a different and much larger win than
parallelism. See [attributes.md](attributes.md#nothing-consumes-it-yet).

**"The 64-bit integer width is the blocker."** Mostly false, in the way the
`IntWidth` rule above states: a buffer index needs no capability at all, and only
the data width does.

## Not yet

Named rather than implied, because each is a decision rather than a gap:

- **More than one result.** A compute shader communicates through its storage
  buffers, so a fragment whose body leaves one value is required here
  (`ResultArity`). A callee is held to the same rule, by name.
- **A non-index parameter read.** A buffer is *bound*, not passed, so there is no
  value for it to hold (`NonIndexParameter`).
- **A runtime scalar.** A dispatch pushes the extent alone, so a fragment with a
  runtime scalar is **refused by name** (`RunError::ScalarsNotPushed`) rather
  than dispatched with a leaf missing. The CPU path passes the leaf list; the
  device half is not written. See
  [compute-runtime-scalars](compute-runtime-scalars.md).
- **`Batch` as a submission policy.** A graph under `Batch` is refused by name,
  because a backend can only fuse if the contract can hand it several dispatches
  for one command buffer, and `submit` records one run. `run_chain` is that
  fusion at depth one; the general capability is not in the contract.
- **A general graph builder.** The `compute.graph` operator, the `Graph` value
  and the lowering exist; what remains is the shape the design records
  ([compute-graph-jit](compute-graph-jit.md) §"Not yet").
- **Freeing a resident buffer from the language.** There is **no per-value
  release**, and this is deliberate rather than an oversight: the value set has
  no per-value destructor, and a `Buffer`'s arena payload has no individual free
  either — so a device buffer is scoped exactly like a host one. The cost is real
  and worth stating plainly: **resident buffers accumulate for the life of the
  backend.** The backstops are `GpuContext::drop`, which reclaims everything, and
  a refused allocation once the device is full, which names itself.

## The pool of submission slots, and the pending submission

`ParallelBackend::submit` records a run, hands it to the queue, and returns a
`Box<dyn Pending>` **without waiting**. The ids it will produce are on
`Pending::outputs`, and their contents the device has not promised to have
written — they exist so the next node can be recorded against this one's
buffers, and that is the chaining the overlapping schedule needs. **A demand
point is `wait` followed by an ordinary fetch**, so ids a *host* ever sees have
been waited for while the ids the *executor* chains on have not.

**`submit` has a default that calls `run`.** A backend that cannot overlap
anything still satisfies the contract; it just never collects from it. Requiring
an implementation would mean every stub and every future backend wrote a method
whose only correct body is the one that does nothing, and a caller could not tell
"cannot overlap" from "has not implemented it yet". The GPU is the only backend
here that overrides it, and it can: it has a pool, so a submission handed back is
a **different slot** from the one the next submission takes.

`GpuConfig { slot_depth }` defaults to **2** and is configurable only from Rust:
`GpuContext::new()` takes the default and `GpuContext::with_config` takes the
rest. A depth of zero is refused by name, for the same reason a zero-link chain
is: it is not a smaller pool, it is none.

**One slot is a command buffer, a fence, a staging buffer and a descriptor pool,
and they live in one struct because they all die at the same moment.** That is
the whole design: a command buffer cannot be recorded while a previous recording
of it is running, a staging mapping cannot be written while a copy out of it is
in flight, and a descriptor pool cannot be reset while a dispatch bound to its
sets is executing. So "staging is per slot", "the descriptor pool is per slot and
reset only in `acquire`", and "`fetch` acquires a slot like everything else" are
properties of the type rather than rules someone has to remember.

**The claim is the lock.** `acquire` hands back a `Segment` carrying the pool's
`MutexGuard` for its whole life, so "two threads never record into one command
buffer" is something the borrow checker sees. `Segment` has three submit methods
that differ in what they leave the caller holding: `submit_and_wait` gives the
slot back, `submit` hands back a `Token` and keeps the slot claimed, and
`submit_and_read_back` reads its own staging *inside* the claim. `Token` is
consumed by the wait, so one submission is waited for exactly once and a second
wait is inexpressible.

What depth costs is VRAM and host RAM — each slot carries its own staging — not
round-robin time: at depth 2 with callers that all wait before returning (which
is every caller today), `acquire` never waits. `acquire`, `Segment` and `Token`
are private; the public entry point is `ParallelBackend::submit`, which returns a
`Pending` rather than ids, so the backend's representation never leaks into a
public signature.

## Invariants a graph must not break

Stated here because they are silent-wrong-answer rules rather than slow ones:

- **One descriptor set per dispatch, and no pool reset under a running command
  buffer.** A set is read when the submission *executes*, so one set cannot serve
  two dispatches, and a reset frees every set — including ones an earlier
  dispatch in the same command buffer is still bound to. A submission resets the
  pool once, at its own start. `MAX_DESCRIPTOR_BINDINGS` is a *per-dispatch*
  limit; the number of dispatches per submission is a second, separate limit.
- **The trailing barrier names both possible readers.** It was
  `SHADER_WRITE → TRANSFER_READ`, right only while every dispatch was followed by
  a `fetch` in another submission. A dispatch recorded after this one in the same
  command buffer reads those results as a *shader*, and a consumer outside the
  destination scope reads undefined data. The scope is therefore
  `SHADER_WRITE → SHADER_READ | TRANSFER_READ`. It over-covers the
  single-dispatch case, which is a cost — measured as not measurable, but not
  free.

  A **third** case made the destination wide rather than naming a third reader: a
  chain that hands the same buffer round again — `run_chain` ping-pongs two
  buffers rather than allocating one per link — has this dispatch **reading** a
  buffer a later dispatch **writes**, a write-after-read hazard the other two
  scopes do not order at all. Each of the three is a case where leaving it out
  does not slow anything down, it makes the chain read undefined data: not stale
  data, and not a crash. The widening was measured rather than assumed: 16 links
  at 1 048 576 elements cost 7.79 ms before it and 7.17 ms after, and an empty
  dispatch 0.046 against 0.048, both inside the run-to-run spread. That is "not
  measurable", not "free". **The write-after-read half is not measured that way**:
  only a chain that reuses a buffer needs it, nothing on the `run` path depends on
  it, and it is paid on the strength of the specification. What does *not* imply
  this barrier is needed is `spirv`'s write-reachability invariant: that says
  every invocation reaches its write, and nothing about ordering between
  dispatches.
- **A slot is not released until its fence signals.** A recorded-but-unsubmitted
  command buffer is clobbered by the next recording into it, and a command buffer
  whose submission is still in flight cannot be recorded into at all. This is the
  whole reason the pool exists.
- **`BufferSlot::Host` inputs are staged by `memcpy` before recording.** Reserve
  for the whole submission up front, and never share staging between slots: a
  submission that grows staging after recording has begun is a use-after-write on
  the mapping, and with several slots in flight a *different* slot growing or
  writing staging is the same hazard across slots.
- **A `DeviceBuffer` value dropped by `drop_block` never calls `release`.** Its
  memory lives until `GpuContext::drop`, the same rule as the deliberate
  no-per-value-release decision above.

## Recovered measurements

**`examples/bench.rs` measures the whole program, and that is a different question
from this note's tables.** The number it reports is everything a lichen program
pays — building the input, the kernels and the read-back — while the tables above
measure a dispatch and a fetch separately. The two do not contradict each other: a
single dispatch never repays itself (the first upload costs about what the scalar
loop costs outright), and a chain does.
