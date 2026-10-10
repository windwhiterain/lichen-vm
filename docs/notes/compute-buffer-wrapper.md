# A buffer is a struct wrapper, and a parallel result is the parameter's `.out`

> Status: current — a parallel result's fields are read by name only after the
> host annotates the run's result once with its codomain type; an unannotated
> result stays `?a`, and the residue is recorded under
> [What is still open](#what-is-still-open).
>
> What this note is: the model a parallel kernel's parameter and its buffers
> have. A **buffer is a lichen struct wrapper**, exactly as a kernel is; a
> parallel kernel's parameter is a **named struct** whose `.out` group is the
> run's result; and the JIT walks that parameter's type and refuses by name
> whatever it does not support.
>
> Points at: `crates/lichen-compute/src/compute.lichen` (the `Buf`/`Read`/`Write`
> algebra), `crates/lichen-compute/src/compute.rs` (`parallel`, `parallel_roles`,
> `param_path`, `buf_payload`, `is_buf_shape`, `compile_parallel_fragment`,
> `build_graph`), `crates/lichen-kernel-ir/src/lib.rs` (`KernelRoles`), and
> `crates/lichen-language/tests/compute.rs`.
>
> Companions: [compute-kernel-struct](compute-kernel-struct.md) (the kernel this
> mirrors), [compute-runtime-scalars](compute-runtime-scalars.md) (the per-leaf
> ABI), and [compute-graph-jit](compute-graph-jit.md) (the recorded path).

## The model

A buffer is not a bare extension value the type system special-cases. It is a
**struct wrapper**, the same shape a kernel has:

```lichen
K   = _x => struct<.native _, .I _, .O _>       # a kernel: the artifact + its signature
Buf = T => struct<.native _, .element T>        # a buffer: the payload + its element type
```

The type level **allows anything under `I` and `O`, at any depth**: a buffer is
simply a field that happens to be a `Buf`, and it may sit anywhere in those two
structures. Finding them is the **JIT's** job — it walks `I`/`O`, reads what it
finds, and **refuses what it does not support by name with its path**, rather
than ignoring it or lowering it into something else.

## Why the kernel's shape, and not a kind marker

A kernel carries **no** `TypeKernel`/`TypeParKernel` marker: its struct's field
names are the whole type-level story, and the checker and the shared renderer
treat it as an ordinary struct. The buffer follows the same rule, and
`TypeBuffer`/`TypeWrite` were the marker pair it would otherwise have needed.
Giving the buffer type a *field* to read (`.element`) makes a marker unnecessary
rather than half-implemented: the type's own shape says what the value is, so
nothing has to declare a kind beside it.

## The prelude algebra

```lichen
Buf     = T => struct<.native _, .element T>
Read    = T => struct<.from (Buf T), .at Int>
Write   = T => struct<.to (Buf T), .at Int, .value T>
read    = (x : (Read _))  => $read(x.from, x.at): x.from.element
write   = (x : (Write _)) => $write(x.to, x.at, x.value)
collect = (b : (Buf _))   => $collect(b)
```

- The ops take the **`Buf` value**, and the engine takes the payload: the path
  the checker resolves for `x.from` is then *the same path* the role walk records
  for that field, which is what lets the emitter match a read against
  `roles.inputs`. Spelling `.native` in the prelude put the source's path one
  step deeper than the role table and had the emitter decline a read that is
  plainly an input buffer. `buf_payload` (`compute.rs`) is where the payload is
  taken — the wrapper's first item, whose order `is_buf_shape` fixes — and the
  run's input decoding and the two VM arms that consume a buffer (`Read`,
  `BufferCollect`) each fall back to the operand itself, so nothing that already
  names a payload breaks.
- `read`'s result is **`x.from.element`**, the buffer's own element cell: a fact
  of the value, now *stated* where it lives instead of left to the call's fresh
  cell. A program that only forwards a buffer still prints `?a` — the cell is
  open until the run binds it — but one whose buffer's element is decided reads
  precisely.
- `Read`/`Write` keep their `_` instantiation spelling at the call sites
  (`(compute.Read _)(…)`): the element cell is what the buffer's `.element` binds.

## The parameter is a named struct, and its `.out` group is the result

A parallel kernel's parameter **is** the named struct
`struct<.n Int, .in …, .out …>`, built by `compute.P (compute.KT _)(.I In, .O
Out)`. A body reads the extent at `k.n`, each input as a field under `k.in`, and
names its outputs under `k.out`. The retired tuple form's `cfg(0)` is `k.n` and
its `cfg(1)(k)` is a field under `.in`.

Three consequences, and they are the decisions the model rests on:

- **The tuple `cfg` form retires.** So `Write`'s `.to` is an **output buffer**,
  not the count: in the tuple form `.to` was the length `n` and the output was
  chosen by the write's *ordinal*; in the named form it is the buffer itself,
  which is what the emitter already works from (it never read the first operand
  at all — the emitter reads the buffer and the index operands and takes the
  ordinal from its own write tally).
- **`O` is the *parameter's* `.out` group, not the body's codomain.** A parallel
  body **produces no value**: it dispatches writes, and the result is the
  structure the author declared under `.out`. So `parallel` sets the kernel
  struct's `.O` to the parameter type's `.out` field — `(K _)(.native
  $parallel(f, b), .I I, .O I::out)`. Setting it to the body's codomain is what
  left `k.O` bound to the body's own (unconstrained) value cell, which is why
  `plrun`'s `r: k.O` could not name the output fields.
- **The ABI tells the same story from the other side.** A parallel fragment's
  emitted function "only has side effects", and `compile_parallel_fragment`
  appends a trailing `Const(0)` purely so the `-> i64` signature has a result.
  The language has **no unit type** to spell `P -> None` with (the kind markers
  are int/float/string/`Type`/function/tuple/array/struct/table/set), so the
  prelude leaves a parallel `f`'s codomain unconstrained (`f: I -> _`) and the
  writes' roles are the real contract.

The parameter's **role paths** (`scalars`/`inputs`/`outputs`) are one
`KernelRoles` value read from the parameter's *type* by `parallel_roles` and
carried in the fragment, hashed by `fragment_digest` like every other field,
because the run side has the cfg **value** and no type: the run reads its leaves
and inputs at those paths, and builds the result structure by placing each output
buffer where the walk found it. The walk, the ABI's leaf order and the emitter's
reads are then one enumeration rather than three.

## The parallel ABI

`range`/`read`/`write` are the **parallel buffer-map primitives**: a body is a
single index function over `[0, n)`, `compute.range n` supplies the current index,
`compute.read` reads one element of an input `Buf`, and `compute.write` dispatches
into an output `Buf`.

- **The count is the parameter's first scalar leaf** — `k.n`, the retired tuple
  form's `cfg(0)`. `plrun` reads the extent there and runs `[0, k.n)`; the emitter
  reads the loop index from `compute.range`'s own argument, and it must be that
  same leaf.
- **How many buffers a body reads is not in `param_shape`.** The shape is the
  parameter's scalar leaves followed by the index, **however many buffers the
  body reads**, because a buffer is bound as storage rather than passed as a
  parameter — so the index is always the last leaf. The count is
  `KernelFragment::inputs`, counted by the emitter as it emits the read positions
  and hashed by `fragment_digest`; it is a *max*, not a tally, because the read
  positions are a sparse space.
- **One `Buf` per output field**, and the run's result is the parameter's `.out`
  structure (the named form of "several outputs are the fields, not a tuple of
  buffers"). The ordinal that reaches the `write` import is the write's emission
  order, a compile-time constant exactly as a read's input position is.
- **The lowered function's scalar arguments are the leaves, then the index**;
  each worker runs the one cached module in its own store and the `write` import
  rebases the *global* index by the worker's base, so the emitted wasm does not
  know it was split. The **result is bit-identical to the sequential loop's** as
  long as no two indices write the same output slot: the partition depends only
  on the count and the worker count, never on a schedule or a timing. Below
  `SEQUENTIAL_PARALLEL_ELEMENTS` (4096) indices the whole run stays on the
  calling thread; above it, the worker count is
  `min(available_parallelism(), count)`. A kernel that writes a slot that is not
  its own is the one place a result stops being reproducible, which is why
  contention has to be *declared* — the algorithmic consequence, and the
  measurement it rests on, are
  [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md#44-axis-d-slot-access-and-the-invariant-it-breaks).

## How to use it

```lichen
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
g = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
kg    = compute.parallel g "cpu"
inbuf = (compute.plrun kg ((compute.A In)(.n 3, .I In(.a 0))) : Out)
```

- The kernel's signature is the function's own `f: I -> O` annotation; the JIT
  reads it back through the kernel struct's `.I`/`.O`. Nothing else has to state
  it, because a struct type's identity is the occurrence it is written at
  ([function-type-merge](function-type-merge.md)).
- The launch's argument is the parameter's scalar leaves in field order, then the
  input group. The shipped `compute.A I = struct<.n Int, .I I>` builds the
  one-scalar shape; a parameter with more scalars is spelled by the author (see
  [compute-runtime-scalars](compute-runtime-scalars.md)).
- **Annotate a parallel run's result once, at its binding, with the codomain type
  the author already declared (`Out`), then read its fields by name**
  (`inbuf.z`). This is the recipe, not a workaround: the annotation states what
  the kernel's `.O` already means, so a call site migrated this way stays correct
  when the propagation below is fixed. Without it the read stays a lazy
  `TableGet` whose name table is never materialised and the field read does not
  resolve.

## Two spelling facts, both load-bearing

- The argument to `compute.P` must be an *instantiated* pair —
  `P (compute.KT _)(.I In, .O Out)`. A pre-bound pair answers "this value is not
  a container".
- Applying a type lambda before instantiating needs parentheses:
  `(A In)(.n 3, …)`, not `A In(.n 3, …)`.

## The JIT's role walk

`parallel_roles` walks the **parameter's type** — not a value — and keeps
**paths**: `.in`/`.out` field 1 under `.n` is path `[1, 0]`, and that is exactly
what `k.in.a` resolves to. A leaf under `.in`/`.out` is no longer a buffer *by
position*: it is a buffer when it **is** a `Buf`-shaped struct, a runtime scalar
when it is a scalar, and anything else is refused **by name with its path** — the
model this note opened with. An empty group contributes no leaf, and a *missing*
reserved name is "no leaves of that role" rather than an error.

A named read resolves through the parameter's **type**, never through how the
body spelled the read. `param_path` is a two-pass walk:

1. **Collect.** Walk the value chain from the read inward, recording each level's
   selector as an `IndexStep` — a `Position` when it is a constant (`a(0)`), a
   `Named` name when it is a `TableGet` (a name is a compile-time string even
   when its index is not). A chain whose outermost `Index` reads the parameter
   pair's value slot is a whole-parameter read: the empty path.
2. **Resolve.** Walk the steps outermost-first against the parameter's type,
   carrying two parallel lists per level: the value's field *types* and the same
   value's field *names* (`struct_type_names`). A named step is its position in
   the names list; a positional step is taken as it stands.

The path is therefore the parameter **type's** field order — `k.n` → `[0]`,
`k.in.a` → `[1, 0]`, `k.out.z` → `[2, 0]` — which is exactly what the role table
holds. A named read whose field is in no name table is an error **by name**, not
a silent `None`: the index of a named field read must be a compile-time constant,
and a kernel is compiled from a concrete instantiation, so an undetermined index
is a compile error rather than something to defer.

**A kernel's parallel ABI is positional and cannot be otherwise.** The host
struct is the parameter's scalars in field order, then `.I`, and the fragment
carries no host-side role table. A parameter that interleaves scalars with `.in`
would need one, so the lowering should refuse an interleaved parameter by name
rather than mis-decode it.

### The chain walk, what it measures, and what it cannot answer

A named read reaches the emitter in **two** shapes, and a function reading only
the first returns every struct field read nameless: unspecialised it is
`TableGet(name-table, "name")` with the name as the second operand, and
specialised — which is what the evaluator hands the emitter — it is
`Index(target, "name")`, whose selector *is* the string and carries no operation.

Three measurements, because two rounds went the other way.

**A struct field read is read by the path on its own chain, not from the
parameter.** Walking *up* from the parameter cannot answer it: the parameter's
value half is an alias with no operation, and the link between it and a read is the
equality class rather than the shape. Walking **down** from a read can, because the
chain *is* the read. Measured on `a_struct_parameter_..._carrying_wrapper`:

```
chain[0] Index(921, 952)      // 952 is a *static* selector — a name
chain[1] Index(923, 926)      // 926 likewise
chain[2] 923: no operation     // the alias carries the field's class
param_pair: 22                 // never reached
```

so the caller searches the role table's own paths for the one whose node is
class-equal to the read — see `parallel_buffer_pos`. And the head of the chain and
the head of the role path disagree by one:

```
positions on the read's chain = [0, 0]
roles.inputs                   = [[1, 0]]
```

`.in` is field **1** of the parameter struct and the chain says 0, so either the
chain indexes the value's own fields rather than the struct's, or the role table
counts the struct's. **Which of the two is right is the next thing to read**, and it
is a fact about the checker's parameter layout rather than a rule the walk can
decide.

**The chain is the role path's tail, so the relation is a suffix.** Two levels of
the chain are not levels of the path. The first is the wrapper's slot-read
destructuring — the step `peeled_argument` resolves, which is why the path is read
from the *unpeeled* operand. The second is `.in`, which the alias consumed:

```
1103: Index(0) -> 1105: Index(0) -> 1107: bare cell
roles.inputs[0] = [1, 0]
```

1107 is the aliased `.in` cell, so no `Index` states it. What remains is `[0]`, and
`[1, 0]` ends with `[0]`; comparing `[0, 0]` against `[1, 0]` matches nothing, and
comparing `[0]` matches exactly one input.

**A bare value cell ends the chain; it does not void it.** Two different things
stop there: a whole-parameter read, matched by comparing the cell's class against
the slot's, and a **struct field read**, whose `TableGet` the evaluator *aliased* to
the field it resolved (`Module::alias_read`) — so the node is a bare cell carrying
the field's class rather than an operation. Returning "not a parameter path" threw
both away for a node that plainly is one; the steps collected so far are kept, and
whether they name the field is the caller's question, which it has the role table
to ask it with.

Two consequences of that shape, both of them reasons the code reads as it does.
Stepping **into the pair's value** is required for a struct parameter's fields,
because they live inside the value half and the `roles` path is made of field
positions with no step for the peel — so `cfg.I.a` is two steps in, not a
whole-parameter read. That step is **known not to reach the answer**: the value
half is an alias with no operation, so the descent stops immediately. What is kept
is the step record, because `resolve_steps` is right about what a *positional* path
means and wrong only about where a named one can be resolved.

Finally, the chain walk is the **fallback, not the answer**: it resolves a
*positional* parameter read — the `[n, (buffers…)]` shape — where the role table is
empty and there is nothing to compare against. The innermost read's target is the
parameter's value, and the test is the **equality class** (`class_root`), which is
the same thing `define_in` matches a parameter against — two tests that could
disagree would be two answers to "is this the parameter's own value".

### A scalar leaf of `.in`, which the body could not read

`k.in.a` — `a` an `Int` field of the input struct — was refused as
"a kernel body's index cannot be placed". **This was pre-existing**, measured on
`dev` at `c99d8c9` before any change here, and it reproduces on a bare
`compute.P` parameter with no annotation tricks.

The body's own walk reads each step of an `Index` chain with `usize_value`,
which answers a **positional constant** and nothing else. A struct field read
arrives as a **name** — `TableGet(name-table, "a")` — so `k.in.a` was a step the
walk could not place, and the read fell through to the refusal. It is worth being
exact about which reads this defeated, because it is **only** this one: in the
same program, `compute.range k.n`, `k.out.z` and the buffer leaves all arrive
already **folded to positions** by the checker, and place without help.

```
node 217   sel_name = Some("a")   sel_pos = None     // k.in.a     -- the only named selector
node 520   name = None            pos = 2 -> 0        // k.out.z
node 508   name = None            pos = 1 -> 0        // a buffer leaf under .in
```

So `k.n` was never "handled structurally as the index"; it was a position before
the walk saw it.

Two defects were behind the one refusal, and fixing either alone still refuses:

1. **The walk had no name resolution.** A named step now goes to
   `param_read_offset`, which is `param_path`'s own two-pass resolution — the
   one that already collects `IndexStep::Named` and looks a name up in the
   parameter type's field list. No second field-position reader was added; the
   hand-rolled `struct_type_names` was **deleted** in favour of
   `struct_fields_of_slot`/`field_names`, the encoding authority the role table
   is itself read with, so a name can no longer resolve against a different
   layout than the roles did.
2. **The pair's value-half peel was counted as a field.** `param_path` pushed
   `Index(param_pair, 0)` as a path step, so every field after it was shifted by
   one: `k.in.a` collected as `[#0, "in", "a"]`, which resolves `in` against
   `.n`'s type and then cannot find it. The pair's value half *is* the parameter,
   not one of its fields, so the peel contributes no step — the path is counted
   from the parameter's first field, which is what `KernelRoles`' paths and the
   slot's scalar shape are both written against.

A name that is **not** a field of that struct is refused by name, not answered
with a wrong number: `resolve_steps` says so and names the fields it did find.

The shape now runs end to end on the wasm backend
(`a_body_reads_a_scalar_leaf_of_its_input_struct`, four lanes of `a + i`). It is
**not** a two-backend test: a fragment carrying a runtime scalar is refused by
the `"gpu"` dispatch, which pushes the launch extent alone
(`RunError::ScalarsNotPushed`, `crates/lichen-compute-gpu/src/dispatch.rs`).
That refusal is the GPU crate's own and pre-existing; this change makes the shape
reachable for the first time, which is what exposes it.

### A placeholder under a `Buf` wrapper

A graph's placeholder reaches an operator **wrapped**: an operator that reads a
buffer names the wrapper, and the recorder puts the placeholder in `.native`, so
asking only whether an operand *is* a placeholder answers `false` for the case the
rule exists to catch. The wrapper is recognised by `buf_value`'s own construction —
two items, the second an element **type**.

The scan that asks whether any operand of an operator is a placeholder reads **one
level, and one level is enough**: every operator that reads a dispatch's output
takes it as a direct item of its operand array — `collect [b]`, `read [b, i]`,
`call [k, a]` — so a graph's value is never buried inside a structure the scan would
have to walk to find. A `plrun`'s argument *is* the placeholder structure (the
parameter's own shape, a leaf per cell), which is why the question is asked about
operators other than a parallel launch. The one level of wrapping it looks through
is the `Buf` wrapper above.

## The kernel may compile before its parameter's annotation resolves

A parallel kernel is compiled by `$parallel`'s own run, and at that moment the
parameter's annotated type may still be an unresolved cell. The distinction the
walk has to draw, and does, is between **"not yet"** and **"wrong"**:

- an **unresolved** parameter type (the annotation's cell holds no value, or a
  pin whose name table is not written yet) leaves the operator **undecided with
  no diagnostic**, so a later pass compiles the kernel once the annotation states
  the struct. `parallel_roles` answers the sentinel
  `PARALLEL_PARAMETER_UNDECIDED`, and the `Parallel` arm turns exactly that
  answer into `None` rather than a recorded diagnostic — so the same kernel can
  compile twice (`roles=None`, then `Some((1, 1, 1))`) and only the later one
  carries roles;
- a **decided** parameter type that is readable but is not the named struct — a
  readable struct missing a reserved field — is refused by name.

Turning the unresolved case into an *error* (tried) fixes the wrong-fragment half
but turns the ordering into a hard diagnostic on the first attempt, which is
equally wrong. The same distinction is what the checker's `type_is_concrete`
draws for its own guards, so it is a shaping question rather than new machinery.

**The consequence to know when reading a recording:** a recording resolves its
dispatches **by identity**, so if the first compile was the role-less one, the
recording finds **no extent and no input paths** on the fragment it interns. Code
that reads roles off a fragment has to be prepared for that, and the reason it is
not a diagnostic is the first bullet above.

## The graph path

A recording builds its placeholders from the parameter's own **cells**, one per
role path — a buffer field's cell is a `Buf` around its `GraphInput` (a dispatch
reads the wrapper's payload), while a scalar field's cell is the bare
`GraphInput` (the extent is a number). The body's own return is what the graph
hands back — a bare `Buf` when it returns one dispatch's result — and the host
annotates the run's result accordingly
(`(compute.graphrun built (…) : (compute.Buf _))`).

- The **recorder** reads its argument by role path rather than by position: the
  extent at the first scalar leaf, each input at its buffer path (the placeholder
  rides in the `Buf` wrapper's payload), and it builds the result as the
  parameter's `.out` structure, one `Buf`-wrapped placeholder per declared
  output.
- The **return walk** descends that structure and skips the wrapper's type slot
  (`.element`), which names no value.
- `build_graph` prefers a placeholder assembled from the declaration
  (`assemble_parameter`) and falls back to the path walk, refusing **by name**
  when neither yields a tuple. The two ends agree by construction rather than
  through a stored table: the slots are numbered depth-first in field order.
- **The unannotated parameter is still supported, and one test depends on it.**
  `build_graph` falls back to the flat ceiling when a recorded body's parameter
  is not a named struct, which is what an unannotated `step = ins => …` gets — so
  a body that reads `ins(0)` still records and runs. That form is not leftover
  spelling: the named struct types the count (`.n Int`), so a closed-over buffer
  in the count position becomes a *type* error and the graph's own count filter
  never speaks, which is the property
  `graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one`
  pins.

**An empty group is spellable now, and a group may also be absent.** `struct<>`
is valid source (its names slot is a present, empty table), so a shape with no
members has a spelling — and `parallel_roles` treats a *missing* reserved name as
"no leaves of that role", so both routes exist. The filler leaf a producer kernel
used to declare is not a lie on either path: the device refuses only a leaf a
body **reads**, so a leaf nothing reads is not a missing one. The filler and the
empty group give **byte-identical** answers on **cpu and on the device**: `In1 =
struct<>` with `(compute.A In1)(.n 3, .I In1())` runs, reads `10, 11, 12` and
collects `[10, 11, 12]`, exactly as `In1 = struct<.a Int>` with `.I In1(.a 0)`
does, including through `compute.graph`/`compute.graphrun`.

Two things the author still writes, and one defect the empty group exposed:

- **The empty struct is nominal, so its occurrence cannot be inlined.** One
  occurrence has to be bound and reused: `Par1 = compute.P (compute.KT _)(.I In1,
  .O Out1)` with `.I In1()` checks, while the same program inlined (`.I struct<>`
  and `struct<>()`) is refused with
  `expected raw[Int, struct<>#0], found raw[Int, struct<>#2]` — three
  occurrences, three nominal types.
- **The empty instance is still an argument**, because `A = I => struct<.n Int,
  .I I>` keeps the field.
- **A graph step's parameter with an empty group before a later role path loses
  the whole value, silently.** With `GArg = struct<.n Int, .in In1, .out Out1>`
  and `In1 = struct<>` the graph answers `raw none: ?a` with **no diagnostic**, on
  cpu and on the device; the same program with a filler under `.in` answers
  `[20, 22, 24, 26]`, and the same program with the fields **reordered**
  (`struct<.n Int, .out Out1, .in In1>`) answers `[20, 22, 24, 26]` as well — so
  the failure is positional, not the empty group as such. `assemble_result`
  derives a group's arity from the paths that exist (`max(index)+1`) and a prefix
  with no path under it makes the `?` return `None`, which `build_graph`
  propagates out of the `Graph` operator as `none`. `assemble_parameter` builds
  the placeholder from the declaration instead, and `build_graph` prefers it, so
  every recording now succeeds and `compute.graph step` alone answers
  `raw ?: ?a`; with a device installed all four `graphrun` programs answer
  `[20, 22, 24]`.

## What is still open

- **The result type does not reach the host read's container cell.** The engine
  half is verified end to end — the recursive walk finds the roles, the fragment
  carries them, the run reads its cfg by those paths, builds the `Buf` wrapper
  for each output, places them into the parameter's `.out` structure, and the
  host's `collect` of a field reads the payload back — but the kernel's *result
  type* does not reach the container cell of a host read. The same probe with the
  read written `inbuf.z` fails: the container's type cell is a lazy `Index` chain
  over empty nodes (the callee's returned `k.O`), so `type_is_concrete` is false
  and the checker has no field position to write as a constant. Writing the read
  as `(inbuf : Out).z` — the author's own type, stated — prints the right values,
  and so does annotating the **binding** instead, which is the recipe above.
- **The empty group's and the split's own coverage.** The empty `.in` group is
  measured end to end on both backends, but no test exercises it; whether the
  fillers the tests and examples carry should migrate to it is a maintainer's
  decision. Same for a test of the graph-placeholder hole the empty group
  exposed.
- **Interleaved scalars and `.in`.** The ABI cannot express them (see the JIT's
  role walk above); the lowering does not yet refuse them by name.
- **The `flat_arity` guard of the graph chain** was read but never measured.

## Where the pieces live

| Item | Where |
|---|---|
| The type lambdas and the wrappers | `crates/lichen-compute/src/compute.lichen` |
| The named-parameter walk and the prelude algebra | `parallel_roles`, `param_path`, `buf_payload`, `is_buf_shape`, `compute.rs` |
| Position resolution | `parallel_buffer_pos`, `peeled_argument`, `is_param_value`, `usize_value`, `field_name`, `param_read_offset`/`param_path`/`resolve_steps`, `compute.rs` |
| Parallel lowering | `compile_parallel_fragment`, `compute.rs` |
| Instruction emitter (read/write arms) | `emit_node`, `compute.rs` |
| Launch walk (count and buffers) | `ComputeOperator::ParLaunch`, `compute.rs` |
| The `Parallel` gate and run | `ParallelOp::build` and `ComputeOperator::Parallel`, `compute.rs` |
| Role paths in the fragment | `KernelRoles`, `crates/lichen-kernel-ir/src/lib.rs` |
| The recording and its placeholders | `record_dispatch`, `record_return`, `assemble_parameter`, `build_graph`, `compute/graph.rs` |
| Struct type reading | `struct_term_parts`, `struct_names_any`, `field_names`/`field_list`/`field_type`/`TypeRef` (`crates/lichen-highlevel/src/shape.rs`), `struct_fields_of_slot` (`compute.rs`) |
