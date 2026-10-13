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
a linear scan over `build.state`, hand-written in `crates/lichen-language/src/lib.rs`
where a node had to answer back for a diagnostic's span. That scan is **gone**: the
attribution now goes through `Build`'s own lookup (§"Who reads it"), which is O(1)
and returns strictly more.

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
- **`Checker::node_edges`** — the **twelve** sites that register an attribution
  explicitly: an index read's result, a table read's key, a struct's key and
  element, an assert condition, an annotation tail slot. These are nodes the slot
  layout does not own, and they carry a path a unify extended past the slot it
  refused on. Measured over the 30-example corpus: **93.8% of what they register
  the slot layout does not own at all**, so the two sources are **complementary,
  not overlapping** — which is why `node_edges` keeps winning where it is present.
- **`Module::node_origin`** — a clone an apply materialized. `lowering_of`
  resolves through it first, so a per-call clone answers for the template it
  instantiates: the same anchor handoff `Module::marks_of` makes for a mark.

A node outside all three names no expression, and the map says so rather than
guessing. An installed kind marker is the honest example: it is built before any
expression exists.

## Who reads it

`Build::node_loc` answers for a runtime diagnostic, and it resolves in a **stated
order**, because the order is load-bearing rather than incidental:

1. the node's own entry — the map is `node_edges` **then** the slots, so a
   deliberately registered caret (`indexing.rs` puts a type-access element at the
   read's own type slot) structurally wins over the slot's default;
2. the node's `origin`'s `node_edges` entry;
3. `apply_edges[origin]` — the caller's argument, for a clone of a **frozen**
   template, whose origin is the apply that materialized it;
4. the origin's slot entry.

Step 3 must outrank step 4. A frozen-template clone's origin **is** an apply node,
and an apply node is `state[call_expr].term`, so the wrong order moves the caret
from the argument `(2,5)` to the call `(3,1)`.

The `ApplyError` path deliberately does **not** go through this: it already reads
`apply_edges` itself, and routing it moves thirteen spans and drops the unify's own
`tag_descent` path.

## The other direction

`Build::state[expr]` was already the forward map and needed nothing. What the
test harness adds is that a node can answer *back*, which is what makes a
failure readable: see [test-harness](test-harness.md).
