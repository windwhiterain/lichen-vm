# The lowering map: which expression a node came from

> Status: current
> Points at: `crates/lichen-highlevel/src/lowering.rs` — `Build::lowerings`,
> `Build::lowering_of`, `Build::lowering_slot`,
> `crates/lichen-highlevel/src/checker.rs` (`Build::lowerings`, and
> `check_ann`'s attribute stamp), `crates/lichen-highlevel/src/checker/diagnostics.rs`
> (the `loc(e, slot)` the edges are recorded with),
> `crates/lichen-highlevel/tests/lowering.rs`.

A lowlevel `NodeId` is dense, positional and meaningless: `node 41` says nothing
about the expression that produced it. The question "which of my expressions is
this?" had no O(1) answer anywhere in the workspace — the only general route was
the linear scan `crates/lichen-language/src/lib.rs` performs over
`build.state`, hand-written at each of its two call sites.

## What the map is

`Build::lowerings() -> &HashMap<NodeId, Loc>`: every node this build compiled,
mapped to the expression it lowered from and the position inside that
expression's `[value, type, attrs…]` pair. `Loc` is the type the crate already
uses for this — an expression plus a recursive path through its spine — so the
map introduces no vocabulary of its own and a diagnostic's `Loc` and a node's
answer are the same thing.

The map is **total over the compiled slots**: for every expression, the pair
node, the value node and the type node are all present, whether or not a check
failed. It comes from data the checker already records (`ExprState`), so it is
*derived* rather than recorded, and it is held in a `OnceLock`: a build pays
nothing for a map only a failing diagnostic or a test reads.

## Three sources, one table

The attribution is assembled from what each site knows, and `node_edges` stays
the authority wherever it knows more:

- **`ExprState`** — an expression's pair, value and type. Total, and exact about
  the slot. Its **attribute** slot is the one entry it cannot place, because
  `ExprState::attr` records one node without its index in the merged tail;
  `check_ann` stamps that node itself where it does know the index.
- **`Checker::node_edges`** — the ten sites that register an attribution
  explicitly: an index read's result, a table read's key, a struct's key and
  element. These are nodes the slot layout does not own, and they carry a path
  a unify extended past the slot it refused on.
- **`Module::node_origin`** — a clone an apply materialized. `lowering_of`
  resolves through it first, so a per-call clone answers for the template it
  instantiates: the same anchor handoff `Module::marks_of` makes for a mark.

A node outside all three names no expression, and the map says so rather than
guessing. An installed kind marker is the honest example: it is built before any
expression exists.

## The other direction

`Build::state[expr]` was already the forward map and needed nothing. What the
test harness adds is that a node can answer *back*, which is what makes a
failure readable: see [test-harness](test-harness.md).
