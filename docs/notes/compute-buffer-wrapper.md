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
- **`O`'s fields are buffers.** `plrun`'s result is the kernel's own codomain —
  the structure the author wrote, with `Buf` fields — which is why `plrun` can
  state `r: k.O` and does not need a separate "bare buffer or tuple of buffers"
  shape.

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
read    = (x : (Read _))  => $read(x.from.native, x.at): x.from.element
write   = (x : (Write _)) => $write(x.to.native, x.at, x.value)
collect = (b : (Buf _))   => $collect(b.native)
```

- The ops see only the **payload** (`.native`), the way `$launch`/`$call` see
  `k.native` — the struct is the type level's business, the artifact is the
  op's.
- `read`'s result is **`x.from.element`**, the buffer's own element cell: a fact
  of the value, now *stated* where it lives instead of left to the call's fresh
  cell (`ReadOp`'s note explains why the cell used to stay open). A program that
  only forwards a buffer still prints `?a` — the cell is open until the run
  binds it — but one whose buffer's element is decided reads precisely.
- `Read`/`Write` keep their `_` instantiation spelling at the call sites
  (`(compute.Read _)(…)`): the element cell is what the buffer's `.element` binds.

## What the JIT owes this

`parallel_roles` (`compute.rs:2045`) already walks the **parameter's type** —
not a value — finds `.in`/`.out` **by name**, and its own comment says it: "A
field under `.in`/`.out` is a buffer" (`compute.rs:2071`). Two things follow:

- Its enumeration is **one level** (`count_under` reads the sub-struct's field
  count, `compute.rs:2078`), while the checker's `param_path` (`compute.rs:4636`)
  already resolves a named read of *any* depth against the parameter's type. The
  enumerator has to recurse and keep collecting **paths** — which is already the
  shape it stores (`ParallelRoles::inputs: Vec<Vec<usize>>`).
- A leaf under `.in`/`.out` is no longer a buffer *by position*: it is a buffer
  when it **is** a `Buf`, and anything else is a leaf the walk must either lower
  (a runtime scalar) or refuse **by name** — the model this note opened with.

The walk and the fragment's `param_shape` (`kernel_domain`/`emit`) must be the
same enumeration, or the ABI's leaf order and the walk's paths can disagree.

## The migration, measured

`compute.read` 103, `compute.write` 127, `compute.collect` 48, `compute.plrun`
145 call sites, in `lichen-compute/src/compute.rs` (126), and the examples'
crates: `algorithms.rs` 115, `recursion.rs` 44, `graph_jit.rs` 41, `bench.rs` 29,
`crossbackend.rs` 22, `graph_structure.rs` 14. Host code only ever **passes a
`Buf` on** (the first one comes from `plrun`), so the migration is retyping what
it passes, not constructing wrappers by hand.

Phases: (1) this file; (2) the JIT walk, the `Buf`-shaped results the run
produces, and the marker deletion; (3) the call sites and their expectations;
(4) the notes that still spell `.sig`/`BufferId` (`compute.rs`'s module docs,
`compute-param-struct-handoff.md`, `compute-runtime-scalars.md`).
