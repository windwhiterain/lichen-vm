# lichen-lowlevel: optional static shape for direct bytecode emission

**Status: current** — describes the shipped `LowShape` marker, how it is stored
(nothing in the side table; it lives *with* the private `Node::value`), and how
the compute JIT consumes it to emit multi-arg kernels.  The shape is an
**analysis result**, not a checker stamp.

The gap this closes: a lowlevel [`Node`](crates/lichen-lowlevel) carries a
`value` (possibly still `Parameterized`) and an `operator`, but *no
compile-time notion of what value shape it produces*.  A bytecode backend
therefore had to either (a) read the already-evaluated value, or (b) walk the
*type half* of the `[value, type]` pair.  Both are brittle: (a) cannot emit
for a still-lazy node, (b) couples the backend to the checker's type encoding.

## The model: shape is determined by partial evaluation, not by the checker

The checker **cannot** set a node's shape at lowering time — types resolve
lazily through unification, so most nodes' shapes are only known after the
graph is partly resolved.  Instead, shape is a **partial evaluation of the
lowlevel value graph**, computed by the layer that consumes it (a backend), on
the already-checked graph:

- a concrete `USize` leaf → `USize`
- `Add`/`Sub`/`Leq`/`Eq` → `USize` (lichen comparisons yield `0`/`1`)
- `Index(container, k)` → the container's element shape
- `Apply(f, x)` → `f`'s codomain shape
- an `Array`/`Table`/`Function` value → the corresponding shape from structure
- anything still undecidable (lazy) → **absent** (no traced shape)

Shape generation is therefore **optional and per-node**: a node with no shape
is either *type-check-only* scaffolding the backend never reaches, or a node
*materialized before the backend runs* (so it sees a concrete leaf).  Only the
traceable value-graph spine gets a shape.

### The one node the value graph cannot decide: the parameter

The value cell of a function's parameter is symbolic (bound at apply time), so
the value graph alone does not say "scalar or 2-tuple or 3-tuple."  Its shape
(the kernel *domain*) is read from the parameter's **type cell** — a lowlevel
node in the same graph, so this is still partial evaluation, not a separate
type system.  This is also the only choice that keeps the wasm arity
consistent with `launch`, which passes **exactly the declared domain arity**
(`compute(1) k (5, 3, 2)` → 3 args → wasm must take 3 params).  A body-usage
inference would under-read in the annotated-but-unused-element case and
mismatch the launch arg count.

## The shape vocabulary and where it is stored

`LowShape` is a closed enum of value shapes:

```
USize
Tuple(Vec<LowShape>)            // arity = len; a kernel domain is a Tuple
Array(Box<LowShape>, usize)     // homogeneous element shape + length
Function(Box<LowShape>, Box<LowShape>)
Table(Box<LowShape>, Box<LowShape>)
```

It is **never a lichen value** — host metadata, sibling to
[`ArrayItem::shallow`](crates/lichen-lowlevel) and `Node::evaluated_deep` — so
"a type is just a value" (`Type : Type`) is untouched.

A node's shape is stored **with its private value**: a private
`low_shape: Option<LowShape>` field on `Node` (and `StaticNode`), read/written
only through the controlled [`Module::node_shape`]/[`Module::set_node_shape`],
mirroring the `value` field's gate.  It is carried through the apply-clone
pass, copied by the static freeze, and serialized by the artifact codec (see
`crates/lichen-language/src/persist.rs`).

## What the compute JIT now does

`crates/lichen-language/src/compute.rs`:

- `kernel_param_shape` reads the parameter's type cell → the domain shape
  (`USize` for a scalar, `Tuple([USize; n])` for a tuple), and stores it on
  the parameter's value cell via `set_node_shape`.
- `codegen_function` derives the wasm arity from that shape and emits
  `(param i64)* (result i64)`.
- `emit_node` reads the stored parameter shape / detects the parameter's value
  node to distinguish a scalar parameter read (`local.get 0`) from a
  tuple-element read (`local.get k`).
- `run_kernel` uses the dynamic `wasmi::Func::call` over an `&[i64]` argument
  vector, so an arity-N kernel launches with N arguments; `Launch` unpacks a
  scalar `USize` or an `Array` of `USize`.

## Out of scope (next steps on the same foundation)

- Shapes for the whole traceable spine of a general backend (arrays/tables as
  first-class kernel values, `Apply`, higher-order kernels) — the value-graph
  rules above extend naturally.
- Static/imported kernel functions (compute v1 rejects them; the freeze/persist
  plumbing is already in place for when they are supported).
- SPIRV / GPU backend — `wasm-encoder` + `wasmi` only today.
