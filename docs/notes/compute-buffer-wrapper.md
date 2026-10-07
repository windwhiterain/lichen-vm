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

## What blocks it now, measured

`parallel` reading the result type from the parameter — `I.out`, where `I` is a
**type** — is a *named read on a type value*, and that does not resolve today.
The probe (`.probe/one.lichen`: one named-form kernel, then `collect inbuf.z`)
never reaches its `BufferCollect`, and the VM logs the two lookups that stay
undecided:

- `TableGet key="z"` — the author's `inbuf.z`, undecided because `inbuf`'s stated
  type (`plrun`'s `r: k.O`) has no name table to look `z` up in;
- `TableGet key="out"` — the same lookup written the model-faithful way
  (`r: k.I.out`), undecided because the *table* it would read is the type value
  `k.I`, whose runtime value is undecided.

So the next piece is the checker's: a named read whose target is a struct **type**
value folds to the field's type (and position) against the type's kind name table
— the table `parallel_roles` and `field_names` already read — rather than
emitting a runtime `TableGet`. Then `parallel` can say `.O I.out`, `plrun`'s
`r: k.O` names the author's `Out`, and the host's `inbuf.z` resolves.

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
blocked on the type-value named read above; (4) the notes that still spell
`.sig`/`BufferId` (`compute.rs`'s module docs, `compute-param-struct-handoff.md`,
`compute-runtime-scalars.md`).
