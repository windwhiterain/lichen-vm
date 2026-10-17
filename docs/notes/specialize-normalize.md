# Specialize-then-normalize: a named access should become a tuple index once the type is known

**Status: proposal.** Nothing implements this. It records the shape of the missing
work and why the fix that exists today is a stand-in for it.

## The claim

**A function that has been specialized should have its named accesses normalized
into ordinary tuple indices before it is used.** A read that has become an index
is what every downstream reader already expects, and is what a body-walking reader
is only able to resolve at all.

## What is measured today

`check_named_field` (`crates/lichen-highlevel/src/checker/structs.rs:268-292`)
chooses between a constant subscript and a lazy `TableGet(names, name)` **at check
time**, and commits the choice to the graph. At that moment the kernel body faces
the **generic template**: `.in`'s type is the type variable `T.I`, that node states
no name table, and the only honest answer is the lazy one. Its own comment says why
the constant is the better form — *"a kernel body is walked by operands, and a
`TableGet` is no value to resolve"* — and it is right about the reader while being
unable to produce what it wants.

**Nothing folds the choice afterwards.** Specializing a function (applying it) copies
nodes and fills values; no pass re-walks the copied body. Measured on the `cpu`
path: with `Par = compute.P (compute.KT _)(.I In, .O Out)` and
`In = struct{.g Inner}`, every step of `k.in.g.b` arrives at compute as a
`lazy-TableGet`.

By the time `lichen-compute` compiles the kernel, the type **is** determined —
measured, every level of the walk reads a concrete name table: `[n, in, out]` at
the parameter, `[g]` at `In`, `[b]` at `Inner`. So the information needed to fold
exists; it just never reaches the node.

**The consequence is that the fold is paid by every reader.** `param_path` and
`resolve_steps` (`crates/lichen-compute/src/compute.rs`) re-derive the name table
at each level and reduce the whole read to a positional path `Vec<usize>`. The name
is gone by the time a kernel is emitted, but only for that one reader, and only in
that return value — the graph still says `TableGet`.

## What it buys

- The name stops being load-bearing for any reader; the checker's constant and the
  normalized index become the same thing.
- The false refusals that come from re-deriving the name go with it, rather than
  being repaired in each reader that re-derives it.
- A reader that walks operands — which is what `Lower` is — stops needing a type
  walk at all, because the subscript is already a value.

## Where it would live

**Immediately after the apply clone, before any consumer walks the specialized
body.** That is the only point at which the container type is both determined and
still attached to the node. It is also the point `apply-clone-ownership.md` already
describes as the moment a clone becomes the reader's own tree.

## What it does not settle

The lazy form must stay for a **generic** template: a read whose container is still
undecided has no other answer, and that read has to remain resolvable at the apply.
The proposal normalizes after specialization, not before.

## The stand-in that exists today

`lichen-compute` folds the read itself, at kernel-compile time, and
`scalar_leaf_classes` reads a scalar leaf's class from the type at the leaf's own
path. Both are correct, both are local, and both are what a normalize pass would
make unnecessary for compute specifically — they are a **temporary measure pending
this proposal**, not the shape of the final fix. The two defects it repairs are
recorded in `compute-kernel-struct.md`.

## What implementing it requires, measured

Three facts narrow the work, and the first one changes the proposal's own wording.

**The fold cannot rewrite the node.** `Module`'s operation setter asserts that a node's computation is
defined once (`module.rs:296-300`), and the value slot's invariant forbids replacing a value a slot already
holds. So "normalize the read into an index" is not literally an edit of the `TableGet` node: the fold
**re-points the consumer's key operand** at a freshly minted constant (`alloc_node(block, None,
Some(USize(position)))`, the shape the checker already builds at `structs.rs:290-294`), and the `TableGet`
node is left as an orphan the collector owns. Nothing in the operation invariant is touched.

**The consumer is `slot_read`, not a bare `Index`.** `check_named_field` hands the key to
`slot_read(container_ty, container_value, key, position)` (`structs.rs:311`), so the swap has to find the node
that consumes this key rather than assume its shape.

**The entry decode already has exactly one implementation.** `shape::name_table_index`
(`structs.rs:327-330`, documented as shared with the lowering's reader "so the two cannot decode an entry
differently") is that implementation, and it lives in `lichen-highlevel` while the fold must run in
`lichen-lowlevel` at the clone. So the primitive **sinks into the lowlevel** and `shape::name_table_index`
becomes its caller — one decode, two callers, in the direction the dependency already runs. Writing a second
walk over `TableItem` in the lowlevel would be the duplication that doc comment exists to prevent.

The order of work is therefore: sink the decode, add the fold after `function.rs`'s clone pass and before
anything walks the clones, then verify with a probe that the read's key is a constant and that the three
suites are unchanged (146/13, 140/0, 201/17/2 on today's `dev`).
