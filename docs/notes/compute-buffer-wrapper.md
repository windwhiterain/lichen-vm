# A buffer is a struct wrapper, like a kernel

**Status: decided, first step landed (`compute.lichen`); the JIT and the call
sites are the work `docs/notes/compute-param-struct-handoff.md` and this note
hand over.**

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

## The migration, measured

`compute.read` 103, `compute.write` 127, `compute.collect` 48, `compute.plrun`
145 call sites, in `lichen-compute/src/compute.rs` (126), and the examples'
crates: `algorithms.rs` 115, `recursion.rs` 44, `graph_jit.rs` 41, `bench.rs` 29,
`crossbackend.rs` 22, `graph_structure.rs` 14. Host code only ever **passes a
`Buf` on** (the first one comes from `plrun`), so the migration is retyping what
it passes, not constructing wrappers by hand.

Phases: (1) this file; (2) the JIT walk, the `Buf`-shaped results the run
produces, the marker deletion, and the role paths in the fragment — landed, with
`lichen-language --test compute` still at 35 passed / 21 failed / 5 ignored, the
21 being the tuple-form call sites; (3) the call sites and their expectations,
under way: the language tests first (three migrated, the rest in flight), then
the examples whose programs only dispatch (`algorithms`, `recursion`, `bench`,
`crossbackend`), and **the graph tests last** — `graph_jit` is where the recorded
body's placeholders meet the named parameter (26 graph call sites), so it waits
on the recording path rather than on spelling; (4) the notes that spelled
`.sig`/`BufferId` (`compute.rs`'s module docs — done — and the
`lichen-compute*`/handoff/runtime-scalars/graph-jit notes).
