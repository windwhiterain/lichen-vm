# A buffer is a struct wrapper, like a kernel

**Status: landed and pushed.**  `compute.lichen` defines the wrapper; the JIT walks
the parameter's roles and carries them in the fragment (`KernelRoles`), a run
reads the launch's leaves and inputs at those paths and builds the parameter's
`.out` structure, the recorder reads its argument the same way and hands back the
body's return, and every call site the migration touched is on the named
parameter.  What is left is recorded in the body of this note and in the notes
below, not in this status.

> Companion notes: [compute-kernel-struct](compute-kernel-struct.md) (the kernel
> this mirrors), [compute-param-struct-handoff](compute-param-struct-handoff.md)
> (the parameter's fields, resolved), [compute-runtime-scalars](compute-runtime-scalars.md)
> (the per-leaf ABI), [compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md)
> (the primitive's design, whose parameter shape this note supersedes), and
> [compute-graph-jit](compute-graph-jit.md) (the recorded path).

## The decision

A buffer stops being a bare extension value the type system special-cases, and
becomes a **lichen struct wrapper, exactly as a kernel is**:

```lichen
K   = _x => struct<.native _, .I _, .O _>       # a kernel: the artifact + its signature
Buf = T => struct<.native _, .element T>        # a buffer: the payload + its element type
```

The type level then **allows anything under `I` and `O`, at any depth** — a
buffer is simply a field that happens to be a `Buf`, and it may sit anywhere in
those two structures. Finding them is the **JIT's** job: it walks `I`/`O`, reads
what it finds, and **refuses what it does not support by name** (with the path),
rather than ignoring it or lowering it into something else.

Two more decisions came with it:

- **The tuple `cfg` form retires.** A parallel kernel's parameter is the named
  struct (`struct<.n Int, .in …, .out …>`), not `(n, (buffers…))` with `cfg(0)`/
  `cfg(1)(k)` positional reads. So `Write`'s `.to` is an **output buffer**, not
  the count: in the tuple form `.to` was the length `n` and the output was chosen
  by the write's *ordinal*; in the named form it is the buffer itself, which is
  what the emitter already works from (it never read the first operand at all —
  `compute.rs:3993` reads `items[1]` and `items[2]` and takes the ordinal from
  `tally.writes`).
- **`O`'s fields are buffers, and `O` is the *parameter's* `.out` group.** A
  parallel body **produces no value**: it dispatches writes, and the result is the
  structure the author declared under `.out` (`P = T: KT _ => struct<.n Int, .in
  T.I, .out T.O>`, so `O` is the author's own `Out`, whose fields are `Buf`s).
  So `parallel` must set the kernel struct's `.O` to the parameter type's `.out`
  field — `(K _)(.native $parallel(f, b), .I I, .O I.out)` — and *not* to the
  body's codomain. Setting it to the body's codomain is what left `k.O` bound to
  the body's own (unconstrained) value cell, which is why `plrun`'s `r: k.O`
  could not name the output fields.
  The ABI already tells this story from the other side: a parallel fragment's
  emitted function "only has side effects", and `compile_parallel_fragment`
  appends a trailing `Const(0)` purely so the `-> i64` signature has a result.
  The language has **no unit type** to spell `P -> None` with (the kind markers
  are int/float/string/`Type`/function/tuple/array/struct/table/set), so the
  prelude leaves a parallel `f`'s codomain unconstrained (`f: I -> _`) and the
  writes' roles are the real contract.

## Why the kernel's shape, and not a kind marker

The kernel has **no** `TypeKernel`/`TypeParKernel` marker: its struct's field
names are the whole type-level story, and the checker and the shared renderer
treat it as an ordinary struct (`compute.rs:13-18`). `TypeBuffer`/`TypeWrite`
were the buffer's counterpart to those markers, and they were **never
constructed**: the whole workspace named them in five places — the leaf kind
marker (`LeafKindMarkers`, whose only consumer is the type-form check), the
artifact codec's two tags, `graph.rs`'s two error descriptions, and
`render.rs`'s vocabulary hook. Nothing ever built `[?b, [TypeBuffer, Type]]`, so
"a buffer's type is `[element_type, [TypeBuffer, Type]]`" was documentation for
a form no code produced. Giving the type a *field* to read (`.element`) makes the
marker unnecessary rather than half-implemented.

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
  cell (`ReadOp`'s note explains why the cell used to stay open). A program that
  only forwards a buffer still prints `?a` — the cell is open until the run
  binds it — but one whose buffer's element is decided reads precisely.
- `Read`/`Write` keep their `_` instantiation spelling at the call sites
  (`(compute.Read _)(…)`): the element cell is what the buffer's `.element` binds.

## The pipeline runs, measured

A two-kernel probe (`.probe/named2.lichen`) — a producer whose body writes
`i + 10`, a consumer that reads the first kernel's output field and writes
`v * 2` — now runs the whole way and prints

```
raw[raw 20, raw 22, raw 24]
```

which is the correct answer: the producer's `[10, 11, 12]` doubled. So the
engine half is verified end to end: the recursive walk finds the roles, the
fragment carries them, the run reads its cfg by those paths, builds the `Buf`
wrapper for each output (`[payload, element type]`), places them into the
parameter's `.out` structure, and the host's `collect` of a field reads the
payload back.

**One thing is still missing, and it is the type's, not the engine's**: the
kernel's *result type* does not reach the host read's container cell. The same
probe with the host read written `inbuf.z` fails (the read stays a lazy
`TableGet` whose name table is never materialised, and `build_outputs`' answer
never lands where the pair reads look). Measured, in this order:

- the container's type cell is a lazy `Index` chain over empty nodes (the
  callee's returned `k.O`), so `type_is_concrete` is false and the checker has no
  field position to write as a constant;
- writing the read as `(inbuf : Out).z` — the author's own type, stated — prints
  the values above, and so does annotating the **binding** instead
  (`inbuf = (compute.plrun kg … : Out)`), after which every other read stays a
  plain named one.

So the **migration recipe** is: annotate a parallel run's result once, at its
binding, with the codomain type the author already declared (`Out`), and read its
fields by name as usual. The annotation is not a workaround that the type fix
would invalidate — it states what the kernel's `O` already means — so call sites
migrated this way stay correct when the propagation is fixed.

## What the JIT owes this (landed)

`parallel_roles` already walked the **parameter's type** — not a value — and its
own comment said it: "A field under `.in`/`.out` is a buffer". Its enumeration was
**one level** while the checker's `param_path` already resolves a named read of
*any* depth against the parameter's type, so it now recurses and keeps collecting
**paths** (which was already the shape it stored). A leaf under `.in`/`.out` is no
longer a buffer *by position*: it is a buffer when it **is** a `Buf`-shaped struct,
a runtime scalar when it is a scalar, and anything else is refused **by name with
its path** — the model this note opened with.

The walk's paths travel in the fragment (`KernelRoles` in `lichen-kernel-ir`,
hashed by `fragment_digest` like every other field), because the run side has the
cfg **value** and no type: the run reads its leaves and inputs at those paths, and
builds the result structure by placing each output buffer where the walk found it.
The walk, the ABI's leaf order and the emitter's reads are then one enumeration
rather than three.

## The kernel compiles before its parameter's annotation resolves

A parallel kernel is compiled by `$parallel`'s own run, and at that moment the
parameter's annotated type may still be an unresolved cell.  The walk then
declines and the fragment is interned **with no roles** — measured on a graph
probe, where the same kernel compiles twice (`roles=None`, then
`Some((1, 1, 1))`) and the recording resolves its dispatches by identity to the
**role-less** fragment, so the recorder finds no extent and no input paths:

```
PROBE fragment roles: recording=false roles=None
PROBE fragment roles: recording=false roles=Some((1, 1, 1))
PROBE record roles: id=0 scalars=0 inputs=0 outputs=0
```

Making the decline an *error* instead (tried) fixes the wrong-fragment half but
turns the ordering into a hard diagnostic on the first attempt, which is equally
wrong.  What is needed is the distinction the walk cannot currently draw:

- an **unresolved** parameter type (no value in the cell yet) must leave the
  operator *undecided*, with no diagnostic, so a later pass compiles it with
  roles;
- a **decided** parameter type that is not the named struct is the retired tuple
  form and is refused by name.

The same distinction is what the checker's `type_is_concrete` draws for its own
guards, so it is a shaping question rather than new machinery.

## The graph path

Recording a body with a **named** parameter now runs end to end, and the pieces
it needed are landed:

- the **recorder** (`record_launch`) reads its argument by role path rather than
  by position: the extent at the first scalar leaf, each input at its buffer path
  (the placeholder rides in the `Buf` wrapper's payload), and it builds the
  result as the parameter's `.out` structure, one `Buf`-wrapped placeholder per
  declared output;
- the **return walk** descends that structure and skips the wrapper's type slot
  (`.element`), which names no value;
- `build_graph` and `graphrun` **share one numbering**: the placeholders are
  built at the parameter's own paths with the slots numbered depth-first in field
  order (lexicographic in the paths), and a run flattens its argument the same
  way, skipping the type slot — so the two ends agree by construction rather than
  through a stored table. A parameter that is not a named struct keeps the flat
  ceiling, because a positional body's reads are exactly that shape.

Measured on a named-form probe: the recording finds its roles, the return is read
through the structure, and the run is reached — where the standalone compiler
stops for the environment's reason ("this graph dispatches to a device, but no
compute backend is installed"), so the test harness that installs one is where
the graph tests exercise it once their programs are migrated.

**An empty group is spellable now, and a group may also be absent.**  `struct<>`
is valid source (`1c488bc`; its names slot is a present, empty table, `388e624`),
so a shape with no members has a spelling — and `parallel_roles` already treats a
*missing* reserved name as "no leaves of that role" (`b95dc77`), so both routes
exist.  The filler leaf a producer kernel used to declare is not a lie on either
path: the device refuses only a leaf a body **reads** (`bce8d55`), so a leaf
nothing reads is not a missing one.

Measured end to end, the filler and the empty group give **byte-identical**
answers on **cpu and on the device** — a real device, no skip: `In1 = struct<>`
with `(compute.A In1)(.n 3, .I In1())` runs, reads `10, 11, 12` and collects
`[10, 11, 12]`, exactly as `In1 = struct<.a Int>` with `.I In1(.a 0)` does,
including through `compute.graph`/`compute.graphrun`.  The minimal producer
spelling is two edits — the group's type, and the launch leaf:

```lichen
In1  = struct<>                                    # was struct<.a Int>
...
p = (compute.plrun k1 ((compute.A In1)(.n 3, .I In1())) : Out1)   # was .I In1(.a 0)
```

Two things the author still writes, and one defect this exposed:

- **The empty struct is nominal, so its occurrence cannot be inlined.**  One
  occurrence has to be bound and reused: `Par1 = compute.P (compute.KT _)(.I In1,
  .O Out1)` with `.I In1()` checks, while the same program inlined (`.I struct<>`
  and `struct<>()`) is refused with `expected raw[Int, struct<>#0], found
  raw[Int, struct<>#2]` — three occurrences, three nominal types.
- **The empty instance is still an argument**, because `A = I => struct<.n Int,
  .I I>` keeps the field.
- **A graph step's parameter with an empty group before a later role path loses
  the whole value, silently.**  With `GArg = struct<.n Int, .in In1, .out Out1>`
  and `In1 = struct<>` the graph answers `raw none: ?a` with **no diagnostic**, on
  cpu and on the device; the same program with a filler under `.in` answers
  `[20, 22, 24, 26]`, and the same program with the fields **reordered**
  (`struct<.n Int, .out Out1, .in In1>`) answers `[20, 22, 24, 26]` as well — so
  the failure is positional, not the empty group as such.  `assemble_result`
  (`compute.rs:3511-3528`) derives a group's arity from the paths that exist
  (`max(index)+1`) and a prefix with no path under it makes the `?` return `None`,
  which `build_graph` (`:6131`) propagates out of the `Graph` operator as `none`.
  The fix belongs where the roles are known (`build_graph`) or where the paths are
  built (`assemble_result`).

The placeholders are wrapped **per role**: a buffer field's cell is a `Buf` around
the `GraphInput` (a dispatch reads the wrapper's payload, `buf_payload`), while a
scalar field's cell is the bare `GraphInput` (the extent is a number).  The body's
own return is what the graph hands back — a bare `Buf` when it returns one
dispatch's result — and the host annotates the run's result accordingly
(`(compute.graphrun built (…) : (compute.Buf _))`).

**The unannotated parameter is still supported, and one test depends on it.**
`build_graph` falls back to the flat ceiling when a recorded body's parameter is
not a named struct, which is what an unannotated `step = ins => …` gets — so a
body that reads `ins(0)` still records and runs.  That form is not leftover
spelling to be cleaned up: migrating it to the named struct would *retire* the
test that relies on it (`graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one`),
because the named struct types the count (`.n Int`), so a closed-over buffer in
the count position becomes a *type* error and the graph's own count filter never
speaks.  Measured: migrating it left that test failing at its first assertion, and
reverting it restored 8 passed / 0 failed / 1 ignored.

## The migration, measured

`compute.read` 103, `compute.write` 127, `compute.collect` 48, `compute.plrun`
145 call sites, in `lichen-compute/src/compute.rs` (126), and the examples'
crates: `algorithms.rs` 115, `recursion.rs` 44, `graph_jit.rs` 41, `bench.rs` 29,
`crossbackend.rs` 22, `graph_structure.rs` 14. Host code only ever **passes a
`Buf` on** (the first one comes from `plrun`), so the migration is retyping what
it passes, not constructing wrappers by hand.

Phases, all four landed and pushed: (1) this file; (2) the JIT walk, the
`Buf`-shaped results the run produces, the marker deletion, and the role paths in
the fragment; (3) the call sites and their expectations — every target migrated,
with `graph_structure` at 4/0, `graph_jit` at 8/0/1, and the examples printing real
answers where the environment allows one; (4) the notes that spelled
`.sig`/`BufferId`, and the Rust doc comments that did the same — the `src/`,
`tests/` and `examples/` comments are the last of it and are in the sweep this
note's status does not cover.

**One correction, and it is the reason this note no longer quotes a pass count.**
Phase 3 was reported as leaving `lichen-language --test compute` at 51 passed /
5 failed / 4 ignored, with the five described here as "the pre-existing
float/backend/GPU reds … never migration's".  That was wrong.  Measured against
this work's own before-image — `git worktree add .worktrees/prechange-check
2e2255a`, the parent of `53f9d49` — the suite was **56 passed / 0 failed /
5 ignored**, and all five of those tests are `ok` there:
`a_float_fragment_agrees_across_the_two_backends`,
`an_integer_fragment_agrees_across_the_two_backends`,
`a_varying_float_element_is_seeded_from_the_index`,
`a_body_may_compute_in_one_class_and_cross` and
`a_gpu_program_chains_two_kernels_on_a_device`.  So the five are this refactor's
own regressions, not pre-existing reds, and they fail in the shared helper —
`tests/common/mod.rs:164`'s "right element … has no value" for four of them and
`:120`'s "an array element has a value" for the fifth.

**What caused it — found after the first account of it was wrong twice.** It is
one level above where it looks: **the read arm never ran at all**.
`GpuContext::stage_run` refused *every* fragment whose parameter declared a second
leaf (`param_shape.flat_arity() > 2`) — a condition written when "declares a
runtime scalar" and "needs one" were the same thing.  The named parameter broke
that reading: `struct<.n Int, .in T.I, .out T.O>` cannot express an *empty* group
(`struct<>` is not source), so a **producer** kernel — one whose body only writes —
declares a filler under `.in` (e.g. `In1 = struct<.a Int>`), which `walk_role`
records as a leaf and `compile_parallel_fragment` puts into `param_shape`: three
leaves where the retired tuple parameter had two.  Each refused `plrun` recorded
`ScalarsNotPushed` and returned `None`, so its result stayed undecided and every
later read of it found no value.  Fixed by refusing only what a dispatch cannot
run — a fragment whose **body reads** a parameter beside the index (`bce8d55`).

Two things hid it, and both are worth keeping:

- the refusal **was** recorded, and swallowed: the test helper's gate panics on a
  recorded diagnostic only when the root value is nothing or an error, and these
  programs' root is an array literal;
- **the symptom's location is not the cause's.**  Naming each silent exit in the
  read arm found nothing, because none of them was reached; nor did the device
  count gate, the inline ascription, or the `.native` operand spelling.  "An element
  has no value" says *where* a value is missing, not *which* decision stopped it
  arriving.

The fix could have gone the other way: rewrite the five programs' producer kernels
to omit `.in`, which is the shape this note's empty-group wart endorses and
`examples/recursion.rs` already ships.  That edits five test programs to work
around a gate refusing fragments nothing demands, so the gate was narrowed
instead; if an empty group becomes expressible, the gate change stays correct and
the source shape can follow.

The lesson is worth more than the count was: a pass/fail number is only evidence
when it is compared against the same measurement taken before the change, and the
cheap way to get that is a worktree at the parent commit rather than a memory of
what the reds "were".
