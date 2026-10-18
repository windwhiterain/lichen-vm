# Buffer precision: `Buf T` is the payload, the pack file is the precision

> Status: **design settled, and the runtime now reads it that way.** The user's verdict is recorded
> here with the mechanics measured, because where the work lands depends on two facts below that are
> not obvious from the code.

## The verdict

**`Buf T` declares the payload's shape only.** A buffer's real precision — how many bytes an
element occupies, and therefore which class a kernel is compiled for — is decided by **the
pack file together with the JIT**: the pack states the class, and the JIT compiles the node for
it.

This is why the language needs no `bf16` type, and why Qwen's weights can stay at their own
width without a third scalar type appearing in `Buf T`. The user put the same sentence a second way
when the question of a `bf16` buffer came up: **the language's `Float` is a precision-agnostic
concept** — it says a buffer holds real numbers, and says nothing about how many bits one takes.

`check_input_classes` now enforces exactly that: it compares the **shape** the declaration names with
the shape the fragment reads, so two real classes agree (`Float` beside `BFloat16`) and only a
real/integer disagreement is refused, which is the one that would convert nothing and mean nothing.
Two things follow, and both are the caller's: the fragment's class comes from a pack, and the bytes
that reach the buffer are the caller's to match — the runtime cannot tell a `bf16` payload from an
`f32` one, only a real one from an integer one.

## A class is the buffer's, and an edge is where two nodes must agree

Measured while narrowing a real projection: the pack's class reaches the node it names — the landed
fragment reads and writes `BFloat16`, its body's accesses are `BFloat16`, and the emitted module declares
`ArrayStride 2` — and a `BFloat16` payload round-trips through a dispatch unchanged. What is **not** in
place is the edge: a graph whose second node still reads the first node's output as `Float` reads four
bytes of a two-byte-per-element buffer and answers a different number without refusing.

So the sentence "the pack decides the precision of a node's buffers" has a consequence the tree does not
carry yet: when two nodes share a buffer, the class is not either node's to state alone. Either a graph
converts at a boundary where the two disagree, or the pack must reach every node that touches the buffer.
The model would want the second, because a narrowed weight matrix is read by the projection and by
nothing else, while a hidden state is read by many.

### The verdict: the class is the buffer's

The user's ruling, recorded: **a class belongs to a buffer, not to a node**. A graph value has one class,
the one its producer wrote it at, and a consumer that reads it at another class widens explicitly rather
than reinterpreting. So `input_classes` / `output_classes` name the **buffers'** classes, and a node's
consent is not what decides them.

What that implies, in the order the work would land:

1. **A value's class is derived from its producer.** A graph input's class is the host argument's; a
   node's output's is its own `output_classes` entry. The planner already walks nodes in order, so this
   is a table it can carry.
2. **A consumer reads at the value's class.** A node whose own `input_classes` entry disagrees within the
   real family is re-classed at that slot — the same rewrite `states_the_class` performs, so a projection
   narrowed to `bf16` feeding a `Float` node would have that node read `bf16` and widen.
3. **A crossing between the real and integer families is refused**, because nothing converts and a
   reinterpretation would be a wrong number rather than a different precision.

Until (2) lands, a mismatched edge silently answers a different number; that is the one behaviour the
tree must not keep, whatever else changes.

## Why the language cannot spell it today

Measured, in the current tree:

1. `Buf = T => struct{.native _, .element T}` (`crates/lichen-compute/src/compute.lichen`), so
   the class comes from the **element type**, and the core types are `Num = set(Int, Float)`
   (`crates/lichen-language/src/core.lichen:1`).
2. `ScalarClass` is derived from `LowShape` — `compute.rs:314`, `2908` — and
   `ScalarClass::BFloat16 => LowShape::Float` (`compute.rs:2927`), so the distinction is
   **already lost** one layer above the language.
3. A new scalar type is a language-level change: `crates/lichen-highlevel/src/shape.rs` holds
   the kind-marker table under a codec contract ("an existing tag must NEVER change; a new
   marker takes the next unused tag"), beside `core.lichen`'s `Num`, the checker and the low
   shape.

So "spell a bf16 buffer" is not a mapping somebody forgot; it is a new type.

## The two mechanics the plan turns on

**The enforcement point is one check.** `check_input_classes(&fragment.input_classes, inputs)`
(`crates/lichen-compute/src/compute.rs:5978`) compares the classes a fragment **declared**
against the classes the dispatch **was handed**; a difference is refused. The declared classes
are read off the program's `Buf T` (`compute.rs:2087`), and the host's come from
`RunArgument::Buffer { class, .. }`.

**The pack's class exists, and is a licence rather than an override.** `PlacementPack` carries
`requires: Constraints { class: Option<ScalarClass> }` (`crates/lichen-runtime/src/placement_pack.rs:81`),
and `Constraints::hold_for` returns `ClassMismatch` when the fragment's class is not the one the
pack names — which makes the pack **inert**, and the engine's own choice answers. Nothing
overrides the fragment's class today.

**And a pack can only speak about a node it tiles.** `Decision::Fires` — the only exit a firing
pack has — is produced and consumed entirely inside the fold-tiling path
(`crates/lichen-runtime/src/automatic_tiling.rs`). The pass walks every node, but a node whose
algebra is not a readable fold is **left alone** with a report line; no pack reaches it.

**And the session does not run passes.** `Prepared`'s own invariant
(`crates/lichen-serve/src/lib.rs:147`) says a session *"does not consult a graph pass: a caller
that wants a pass runs it before preparing."* That caller is not possible through the API as it
stands: `Session::prepare` evaluates the program to record the graph **and only then** holds the
`module` a pass is installed into (`install_tiling_with_packs(&module, packs)`), so a pass cannot
be installed before the graph that pass is meant to rewrite is recorded. This is a bridge that
does not exist yet, not a bridge that is merely unused.

## The plan, in the order the dependencies run

1. **The pass applies a pack's class.** When a pack fires for a node it does not tile, the node's
   fragment is landed again with its first input class set to the pack's. Payload compatibility
   is the pack's statement, not a second rule: a pack that crosses shapes is an artifact error.
2. **The session gains an entry that installs a pass before the program is evaluated**, so
   `install_tiling_with_packs` runs against the module the graph is recorded from. Without it,
   piece 1 is unreachable from a service.
3. **Then the model**: the weights are uploaded at their own class (the emitter already stores a
   bf16 buffer as a `uint16` array and widens on every read — `docs/notes/half-precision.md`),
   and a pack per node states it.

## What this is not

It is not a way to run Qwen3.5-2B. Its eighteen linear-attention layers need a matrix-valued
recurrent state and a conv1d along time that no kernel here expresses. A plain transformer —
Qwen3-1.7B, 28 layers, GQA 16/8, RoPE, SwiGLU, tied 151936 vocabulary — is the model whose every
layer is reachable.
