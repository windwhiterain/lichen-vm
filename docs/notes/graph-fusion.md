# Graph fusion: arbitrary logic, declared interfaces, runner-installed implementations

> Status: **proposal.** Revised after review: the fusion substrate is **not** a
> tensor-operator vocabulary (the first draft's `ml.map`/`ml.reduce` layer,
> rejected) but an **interface layer over arbitrary logic** — Flash Attention
> itself is the evidence for the rejection, because production attention
> always carries custom logic (bias, masking, ALiBi, clipping) that no fixed
> operator set expresses. The schema is still automatic; matching is exact
> hash plus explicit constraints; and optimization files are **installed on
> the runner's machine**, never distributed with the logic.
>
> The graph JIT this sits on is current
> ([compute-graph-jit](compute-graph-jit.md)); the fusion mathematics is the
> worked example in [reduction fusion](../reference/reduction%20fusion.typ)
> (Neptune, PLDI'26); the mark primitive that carries an interface claim is
> current (`~`; `crates/lichen-highlevel/src/marks.rs`), and the attribute
> machinery it deliberately bypasses is current
> ([attributes](attributes.md)).
>
> Companions: [compute-graph-jit](compute-graph-jit.md) (the recorded graph this
> rewrites), [lichen-compute](lichen-compute.md) (the fragment IR the engine
> reads), [loop-conversion](loop-conversion.md) (the dynamic loop a fused
> reduction body needs), [attributes](attributes.md) (what a mark is not),
> [artifact-cache](artifact-cache.md) (the content-addressed store schema
> artifacts and packs ride on).

## 1. The separation, stated as one rule

**Fusion is an interface layer over arbitrary logic. Nothing the optimizer
knows is inferred from arbitrary code, and nothing is restricted to a
vocabulary: the logic itself declares the few facts code cannot carry — the
algebraic laws — as interface instances, and the schema is the automatic
summary of those declarations plus the analyzable code between them.**

The rejected alternative deserves one paragraph, because it is every existing
ML system's answer. Torch-style stacks build the graph out of *tensor
operators* (`matmul`, `softmax`, `layer_norm`) and fuse by pattern-matching
that operator graph. It works until it matters: Flash Attention exists
precisely because the interesting optimisations live where the operator
boundaries dissolve, and every production attention is a *custom* attention —
a bias here, a mask there, an ALiBi slope, a clip — at which point the
pattern stops matching and the fusion silently stops firing. The failure is
not that the patterns are incomplete; it is that **the pattern is matched
against the wrong thing**: against which operators the program named, rather
than against which algebra the program satisfies.

The interface layer inverts that. A reduction is not an operator you call but
a **property a piece of logic declares about itself**: "this loop is a
reduction whose combine is associative with this identity." The declaration
is an interface instance; the loop stays arbitrary code; and a custom bias
inside attention changes nothing the fusion reads, because the fusion never
read operator names in the first place.

The property this buys, stated once because everything below is a
consequence: **the logical code is the reference implementation, and it is
always runnable.** Interfaces are additive metadata; a program with none runs
exactly as written, unfused, correctly. Every optimisation is a substitution
justified against the schema, so the worst case of any implementation —
engine-derived or hand-written — is that the logic itself runs.

## 2. The three artifacts, and who writes each

| artifact | written by | distributed with the logic? | wrong answer possible? |
|---|---|---|---|
| logical code + interface claims | the logic author, once | yes — it *is* the package | yes — the author's bug, reported as such |
| schema | the compiler, from the logic alone | derived on demand, content-addressed | no — a derivation, not a decision |
| implementation | the engine (built in), or an **optimization pack** (runner-installed) | **no — never ships with logic** | engine: no (by construction); pack: caught by the differential obligation |

The third row is a distribution rule, not a packaging detail: **a logic
package's artifact contains logic and nothing else; optimisation is a
property of the machine that runs it.** The model is the GPU driver, not the
Python wheel — the machine that will execute the graph installs the
implementations its owner trusts (a flash-attention pack, a vendor BLAS
pack), and the same logic package runs fused on a machine that has them and
unfused on a machine that does not, bit-identical in result either way. That
is what "separating logic from optimisation" means when it is taken all the
way down to the distribution channel.

## 3. The interface layer: declaring what code cannot carry

An **interface** is a named algebraic contract. The load-bearing one for
reduction fusion:

```
Reduction { combine, identity, element }   -- combine: associative, identity: its unit
```

**The claim rides on the kernel, and the projections ride the kernel's own body.**
Fusion operates on kernels, so the interface is declared at kernel granularity: the
structural claim is a **mark** on the expression that produces the kernel, and every
role the interface requires is a **mark** on the expression inside the kernel's body
that *is* that projection. A mark is written `e ~ "payload"`, where the payload is a
string literal. Spike spelling:

```lichen
-- the claim: which interface this kernel implements, and the roles it requires
producer_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1)
    else fold (s(0) - 1, (s(1) + compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (6, 0.0 ~ "identity")))
}
producer_raw ~ "claim:Reduction(combine,identity)"
```

The kernel carries **a list of interface instances** — each an interface name and the
roles it requires — and a body mark names the role it is the projection of. What is
load-bearing is that the projections are *inside the body the claim is about*, so **a
claim cannot diverge from the code it describes** — nothing is restated, and a body
edit that changes the algebra changes the projection the mark points at.

### A mark is a compile-time primitive, not an attribute

A mark **rides an expression**: it is a side table beside the IR tree, keyed by
`ExprId`, and the expression lowers to exactly what it would lower to without it. There
is no `ExprKind` for it, so every `match` on expression kinds is unchanged and a reader
that does not want a mark never meets one. Three facts make this the right carrier, and
each is a way the attribute slot was the wrong one:

- **A mark never unifies.** An attribute slot is padded, propagated and
  meet-combined; a mark must stay pinned to one expression, so it has no slot to
  combine over. The checked form is the type, not a convention: the mark's payload is
  **text**, and a table of text has no path to the pair layout at all.
- **The interface check is structural, not a lattice.** Whether a claim's roles are
  covered is a question about *which expressions the body marks*, answered by table
  lookups over the IR — never a compare-and-combine over values.
- **A label's pair is unreachable from its own value.** The checker makes an
  annotated expression's *value node* the pair's element 0, so no graph edge points at
  the pair and a reader holding the value cannot find the pair. That is why the
  attribute channel needed a side table of its own (`Module::stamp_label`, the
  clone-travel in `node_apply`, the GC retention) — all of which is gone with it.

The one thing that *does* cross from the highlevel into the lowlevel is a mark's
**position**, and it crosses in exactly one place: the anchor (`Checker::anchor_marks`
in `crates/lichen-highlevel/src/checker.rs`). The anchor is *the node a reader walks* —
the expression's own value node for a value expression, and a function template's
parameter node for a claim, which is the one node of a function that an instantiation
still names. A body the dispatcher compiles is a **clone**, so the clone carries
nothing and `Module::marks_of` resolves it through its origin chain to the template's
anchor; the claim is read at kernel-compile time, from the function being compiled, and
stored in the kernel registry beside the fragment. The runtime kernel value carries
nothing.

Three facts make this minimal rather than merely small:

- **Only the laws are declared; everything else is read.** The engine needs
  `combine` and `identity` because no analysis recovers "this loop is a sum" from a
  loop body reliably — that is the fact code cannot carry. But the *element function*
  `g` between two reductions is ordinary straight-line code, and a kernel body
  already lowers to a `KernelFragment` — an expression tree the engine can read,
  invert and distribute over. The interface surface is exactly the gap between what
  code says and what fusion needs.
- **The claim is attached to the expression, not to a name.** A mark rides the
  expression's own IR id, so it survives inlining, renaming and reformatting, and it
  points at the dataflow, not at a function somebody might rename. Renaming the
  function cannot detach the claim; changing the algebra can (§4).
- **A claim is checked, and a claim that cannot be checked is refused, not
  believed.** Where the body normalizes symbolically to the declared combine, the
  checker proves the instance; where it cannot, fusion of that instance is refused by
  name and the logic runs unfused. The refusal list (§6) is the interface layer's
  teeth: a wrong `combine` is a wrong answer, so the engine would rather fuse nothing.

`matmul` needs no operator: its inner contraction is a loop marked
`~ "claim:Reduction(combine,identity)"`. That is what makes attention's *third*
reduction the same kind of citizen as softmax's two, without anyone having written the
word `matmul` in an operator registry.

### What the mark channel cost, measured, and what closed

The first spike built the channel over attributes and measured where it stopped. The
five findings, and what this one did with each:

- **A label's pair is reachable from nothing** — *closed, by not riding a pair at
  all*. A mark's payload is text, so there is no slot node for the unreachable-pair
  problem to exist in; the side table, the `node_apply` clone-travel and the GC
  retention that worked around it are deleted.
- **The kernel emitter reads the marks** — *kept, and it is now the only reader*.
  `Lower::value` asks `Module::marks_at` for every value it lowers and publishes what
  it finds as `KernelFragment::marks`. No backend reads the field (a mark says what a
  value means, not what the kernel computes) and `fragment_digest` hashes it, so two
  fragments differing only in their marks never share an id.
- **A per-call clone loses the mark** — *closed, by the origin hop*. The dispatcher
  compiles a **clone**, so the clone carries nothing; `Module::marks_at` walks the
  clone's origin chain to the template node the checker anchored, which is the one
  lookup both paths share. The lesson recorded from the first attempt: the anchor must
  be **the node the reader walks**, never a node synthesized to read it. Anchoring on
  `Checker::value_of` — which mints a fresh `Index(pair, 0)` — published marks on a
  node no emitter ever lowers, and every body mark was silently lost. The anchor is
  the expression's own value node, or its term for the kinds whose value is decided
  only at the apply, and a **function** template's parameter node for a claim.
- **A body cannot carry three marks** — *closed, by a fresh production*. `~` is its
  own sigil at the annotation level with a string-literal payload, so `e ~ "a" ~ "b"`
  is two marks and a `@loop` recurrence carries three (`combine`, `accumulator`,
  `identity` in the spike's example) with no interaction with the `if`-sugar. The
  payload is still a string rather than a record: text is what keeps the mark out of
  the pair layout, so a record payload is a later decision, not a smaller one.
- **A scalar element kernel's class is its body's** — *unchanged, and not the mark
  channel's business*. `(p : <Float, Float>) => p(0) * p(1)` lowers its leaves in
  `Int`, because the body states no concrete float operand, so the bridge refuses the
  projection by name (see [floating-point](floating-point.md) §5.1). An element
  function that must be `Float` has to carry a float operand of its own.

**One measurement is still open for a literal count, and closed for a runtime one.**
A mark reaches the fragment only where the emitter *lowers* its anchor: on the dispatched
path the kernel compiler walks the value the checker's definition pass already decided, so
an expression whose value that pass decided is never lowered and its mark never publishes.
In the spike's program that is the reduction's `identity`
(`fold (6, 0.0 ~ "identity")`). The refusal is named
(`RoleNotMarked { interface, role }`).

The ordering constraint the measurement establishes, with the code path:

1. **The definition pass runs a convertible `@loop` to decide it.** `Checker::check_term`
   does not evaluate an `Apply` (`state[e].val` stays `None`,
   `crates/lichen-highlevel/src/checker/lambda.rs` `check_app`), so at check time the
   apply node is live and the nest would be reachable. The decision happens in the deep
   pass: `Module::function_apply` sees `function_is_looping(function)` and a successful
   `Module::loop_conversion`, and calls `apply_loop`
   (`crates/lichen-lowlevel/src/function.rs`, `loop_run.rs`), which **writes the value
   the host loop answered onto the apply node** (`write_node_value`). The apply therefore
   carries both its `Apply` operation and a concrete `[value, type]` pair.
2. **The emitter's value resolution prefers the pair.** Walking the write's `.value`
   reaches the recorder through the peel `Index(pair, 0)`; `value_of_node` and
   `pair_value_half` (`crates/lichen-compute/src/compute.rs`) resolve that to the pair's
   element 0 — the exit value the host loop accumulated — instead of the `Apply`
   operation. The walk therefore descends the *unrolled* chain of state and arithmetic
   nodes. `Lower::apply` is never entered, so `Lower::lower_loop`
   (`crates/lichen-compute/src/compute/body.rs`) never runs, the entering call's state
   is never read, and the seed literal is never lowered.
3. **Therefore the anchor is never visited.** `Lower::publish_mark` only ever sees nodes
   the walk lowers, and the marked seed is not one of them. The same expression's
   `combine` mark survives by luck alone: its anchor sits on the add the unrolled chain
   still lowers.

**The `lower_loop` path is reachable, and the discriminator is measured.** It fires when
the checker leaves the loop *undecided* — when the entering state is a run-time value, so
`loop_iteration` reports `Undecided` and `apply_loop` writes no value, leaving
`value_of_node` nothing to peel. Instrumenting `Lower::apply` and `Lower::lower_loop` and
running both programs: `a_kernel_loop_nest_carries_a_runtime_state`
(`crates/lichen-language/tests/compute.rs`, whose seed is a buffer read) reaches each
**twice**, while `reduction_interface` (whose seed was the literal `0.0` when this was
measured — it is the runtime extent now, and reaches each twice for that reason) reached
each **zero times**. So the constraint is exactly: *the emitter converts a kernel body's
`@loop` only if the definition pass has not already run it.*

**Why the obvious patch is the wrong one.** Preferring the operation over the pair in
`value_of_node` does not reach the emitter's `apply`: the write's `.value` is not the
apply node, it is a node the unrolled chain derived, whose class already holds the
decided scalar. Publishing the mark from the deciding pass instead — reaching back from
the loop's result to the marked anchor — would make the mark a property of a *search*
rather than of the node the reader walks, which is the model §3 rejected when it deleted
the first spike's side table. The honest fix is to reorder *when the definition pass
evaluates a recurrence inside a kernel body*: the kernel must be compiled while the nest
is still unconverted, so the emitter reaches `Lower::apply` and the marked argument
lowers. That is a change to the evaluator's scheduling — a kernel closure's body would
have to be compiled before the deep pass runs its applies, which needs a check-time
compilation hook the `Ctx` boundary does not expose today (the plugin's
`NativeOp::build` has the argument's *value node*, not `&mut Module`) — so it is recorded
here rather than attempted. Until it lands, the fix is in the loop conversion and
*not* in the mark channel, exactly as this subsection first said.

**And the discriminator is enough to get out of it from the program side, which is what
`reduction_interface` now does.** A fold whose count is a **runtime** value — the buffer's
own length, `fold (k.n, 0.0 ~ "identity")` — is exactly the "the definition pass leaves
the loop undecided" case this subsection measured: the entering state is a runtime scalar,
so `loop_iteration` reports `Undecided`, `apply_loop` writes no value, the emitter reaches
`Lower::apply`, `lower_loop` runs, and the marked seed is lowered. The printed run shows
`identity` published for both the producer and the consumer and the producer's declared
reduction as `combine + identity 0`, where the literal count published `combine` alone and
the two-slot reduction was refused. `fusion_numbers`'s `report_marks` producer keeps the
literal seed and still shows the refusal, so the two halves are visible side by side in
the examples. What is *not* closed is the literal case: the scheduling change above is
still what would close it, and nothing here patches around it.

The chain the spike *does* demonstrate end to end: a kernel's claim is read from the
registry the compile filled and its roles from the fragment beside it
(`crates/lichen-language/src/fusion_claim`), the fragment is the one the **dispatcher**
compiled, and a marked subexpression is bridged to `lichen_fusion::Expr` over
`Variable::DATUM`, `Variable::PRODUCER` and `Variable::CONSUMER_STATE` by
`crates/lichen-fusion-bridge` (`cargo run -p lichen-language --example
reduction_interface` prints each step and each refusal).

## 4. The schema: derived from the interface graph, stable under everything else

A schema is computed per **fusible region** of the recorded graph: a maximal
connected run of kernel nodes whose registry entries carry claims. A node with no
claim is a boundary. It is plain data, content-hashed, storable in the artifact
store. The types are `crates/lichen-language/src/fusion_claim/schema.rs`, and
`derive_regions` is the entry point:

```rust
derive_regions(graph: &Graph, instances: &[Instance]) -> Vec<Result<RegionSchema, SchemaRefusal>>

Instance { kernel: KernelId, claim: KernelClaim, fragment: KernelFragment }

RegionSchema {
    nodes: Vec<SchemaNode>,   // the interface instances, in evaluation order
    between: Vec<Between>,    // the code connecting adjacent instances (the g's)
    ports: Vec<Port>,         // region inputs/outputs: role, class
    hash: SchemaHash,         // sha256 over the canonical form of all of the above
}

SchemaNode {
    position: usize,                    // the instance's place in the region
    kernel: KernelId,                   // the registry id, so a report can name it
    interface: String,                  // which contract (e.g. Reduction)
    projections: Vec<RoleProjection>,   // one per role the claim requires, in name order
}

RoleProjection {
    role: String,       // combine, identity, element
    expression: Expr,   // the marked subexpression as the logic reads it
    canonical: Expr,    // the same tree, canonicalized
}

Between {
    producer: usize, consumer: usize, input: usize,   // the edge, in region-local positions
    element: Expr, canonical: Expr,                   // the consumer's `element` role
}

Port {
    kind: PortKind,             // Input, Output, Extent
    node: usize, position: usize,
    class: Option<ScalarClass>,
    value: Option<ValueId>,     // one run's numbering: reported, never hashed
    count: Option<usize>,       // a constant extent
}
```

Three things the sketch did not survive contact with the real graph, and each is
a decision:

- **The input is the graph *plus* the registry.** A recorded node holds a cloned
  `KernelFragment` and no `KernelId`, and the claim lives in the registry keyed
  by id (`compute.graph` reads `ComputeValue::ParKernel(id, _)` while recording
  and stores only `entry.fragment`). So a derivation takes the `Graph` and the
  `Instance`s a program dispatched and joins them on the fragment itself — a
  digest bucket and an exact comparison. Two of a program's kernels that share a
  body and claim different interfaces leave a node's instance ambiguous, and
  that is `AmbiguousClaim`, not a guess.
- **A projection is stored in two forms.** §6's abstraction must read the element
  function *as the logic wrote it*: taken canonical, `exp(d - r)` is
  `exp(d) * exp(-r)` and the outermost producer-free subexpression is `d`, not
  the datum a `g` of `r` is written over — the two instances in
  `examples/fusion_schema.rs` derive only from `expression`. §4's stability is a
  property of the canonical form, so `canonical` is `expression` normalized once
  at derivation and the engine reads `expression`.
- **`LawSet` is not a field, because nothing proves one yet.** The schema records
  the interface and its projections; the laws §3 requires (`associative`, with
  the identity as its unit) are what §9's missing *interface verification* would
  establish, and until it lands a claim is read as declared.

The schema is deliberately a function of **the interface graph plus the
fragments between instances**, and of nothing else:

- **Stable under everything except declared semantics.** Rename the function,
  reformat, reorder independent statements, rewrite the loop's uninteresting
  plumbing — same schema, because none of that is in the interface graph.
  Change the algebra — a different combine, a different `g` — and the hash
  changes. This stability is a *requirement*, not a nicety: §5's packs match
  by hash, so a schema that wobbled under refactoring would make every
  installed pack silently useless.
- **What the hash covers, and what it excludes.**
  `RegionSchema::canonical_form` is the text `sha256` is taken over, and it is
  also what a report prints, so a pack author recomputes the hash from what they
  can see. In it: the instance count; each instance's position, interface and,
  per role in name order, the **canonical** projection in a fully parenthesized
  prefix form (`(max t (+ d (- r)))`, which no precedence or rendering rule can
  make ambiguous); each `between`'s producer, consumer and input ordinal with its
  canonical element; and each port's kind, region-local node and ordinal, and
  element class. Out of it: kernel ids, graph value numbers, a constant extent's
  number, the backend, the raw fragments and their digests, and the marks' body
  positions. The exclusions are what make two spellings one schema — the
  re-spelled chain in `examples/fusion_schema.rs` changes both kernels' fragment
  digests and derives the same hash.
- **Canonicalisation is `lichen_fusion::normalize`**, applied to every projection
  exactly once at derivation: commutative operand order, constant folding
  (`0.0 - 1000000.0` is one literal), flattening and cancellation, a product
  distributed over a sum, and `exp(a + b)` into `exp(a) * exp(b)`.
  **Alpha-equivalence is free**: the bridge reads a body into the three named
  slots (`d`, `r`, `t`), so renaming a source binder cannot reach a projection.
- **It is the pattern language for both implementation sources.** The engine
  matches structural shape (a `Reduction` feeding analyzable code whose `g`
  mentions the reduction's running value, feeding another `Reduction`); a
  pack matches the exact skeleton hash plus explicit constraints on the
  fragments. One contract, two consumers, neither reads source text.
- **Custom logic lands in `between`, and that is the design working.** An
  ALiBi slope inside softmax is not an obstacle the schema must be taught —
  it is fragment content the engine's inversion step (§6) either handles or
  refuses by name. A tensor-operator pattern would not have matched at all.

**Where the schema meets the algebra.** A region assembles into one
producer→code→consumer chain or is refused by name: a node fed by two instances
of its region, or feeding two, is `NotAReductionChain`; a consumer that declares
no `element` role is `NoElementRole`; and the element must read
`Variable::PRODUCER` or the edge that made the producer its input is not the
algebra the engine fuses (`ElementWithoutProducer`). A claimed role with no mark
(`RoleNotMarked`) and a projection that does not bridge (`ProjectionNotBridged`)
are refusals too. A region is never silently skipped, and
`examples/fusion_schema.rs` prints each schema, its canonical form and its hash.

**One anchoring fact the example measures.** A projection prints the computation
its mark rides — a `combine` prints as the add or max it marks, an `identity` as
the literal, an `element` as the subexpression, never a buffer read and never a
neighbour's value. The one reader this needed was the transcendental:
`compile_fragment` expands `exp` into the IR's one primitive sequence *before*
the fragment is registered, so the marked value's instruction is the expanded
polynomial and the walk meets its `Select`.
`lichen_kernel_ir::inlined_exp_operand` reads that definition back, and is the
same definition's other half. What is *not* claimed: the expansion rewrites the
marked value in place, so a mark on a transcendental is the one body mark whose
instruction lowering does not preserve.

## 5. Optimization packs: implementations the runner installs

A pack is the hand-written half of the system — the place for register-level
tiling, shared-memory choreography, instruction scheduling, everything the
engine provably derives *correctly* and therefore conservatively. A pack is
an ordinary artifact installed on the runner's machine (via the package
manager into Lichen Home, beside the toolchain), and it binds by schema, not
by name and not by the logic author's say-so:

```lichen
--- inside a pack, schematically ---
implements = schema "sha256:9f2c…"          -- the region schema's skeleton hash
requires   = { class = f32, axis = "last" }  -- explicit constraints on fragments/ports
reference  = <the region's derivation>       -- what this claims to equal
kernel     = cfg => { …the hand-written fused body… } "gpu"
```

The rules, each a decision:

- **The logic never names a pack, and its package never ships one.** The
  logic author writes attention once; whether *this* machine runs it fused is
  between the runner and its installed packs. A logic package that could name
  its optimisation would be a coupling channel — the thing this design exists
  to remove — and a package that *shipped* one would make every consumer run
  code only the author vetted, which is the driver model inverted.
- **Matching is exact hash plus explicit constraints.** On a hit, the
  constraints are checked against the region's actual fragments and ports; on
  any miss the pack is simply inert. Fuzzy structural matching is rejected:
  a pack that fires on algebra it was not written against is a wrong answer
  with good benchmarks, and "faster, never wrong" (below) dies there. The
  cost is real — two genuinely different spellings of one attention are two
  hashes — and schema canonicalisation (§4) is what keeps the cost small
  without admitting semantic distance. Tooling owes the pack author one
  command: *print the schema hash of this region* so a pack can target it.
- **Losing a pack is loud and safe.** A pack that does not match changes
  nothing the program computes — the logic runs, possibly engine-fused — and
  the run report says which packs were consulted and which fired. A logic
  change that drifts the hash reports `pack "flash-attention" no longer
  matches (schema 9f2c… → 41b0…)`. So an installed pack can make a program
  **faster, never wrong**: the fallback is the reference implementation
  itself.
- **Equivalence is an obligation with teeth.** A pack ships a differential
  harness — its output against the reference region's output over generated
  inputs — run where the pack is authored and published, the way the graph
  suite compares against hand-derived closed forms rather than a second run
  of the same code. At install and at run time the system checks hash and
  constraints only, the same way a cached artifact is trusted. A pack that
  cannot name its `reference` region is refused at install: an implementation
  of nothing is nothing. A pack that hands the engine its *repair function*
  (§6) has one more thing that harness has to cover: the per-element identity,
  which the engine checks symbolically only for a function it derived itself.
- **Pack beats engine.** When both cover a region, the pack applies and the
  report says so — a derived fusion is the floor, not the ceiling, and a
  human who wrote a pack knew something the engine did not.

## 6. The engine: reduction fusion by repair function

The mathematics is worked through in
[reduction fusion](../reference/reduction%20fusion.typ); this section is what
that mathematics *is* inside the system — the engine's one transform, stated
as the obligations it checks and the kernel it emits. Where the paper has
"the producer reduction's operator", the engine has the `Reduction`
instance's declared `combine`; where it has `g`, the engine has the fragment
it lifted from the code between the two instances.

**The shape it matches.** A producer reduction `r` (declared combine `op`),
analyzable element-wise code `g(d_i, r_i, r_N)` depending on the producer's
*final* value `r_N`, and a consumer reduction `t` (declared associative
combine `f`):

```
r   = … ~ "claim:Reduction(combine,identity)"
out = … ~ "claim:Reduction(combine,identity)"     over  i => g(d_i, r_i, r)
```

The `r_N` inside `g` is what blocks naive fusion: the consumer's step needs
the producer's answer before the producer has finished. The engine's move,
per the reference, is to index the consumer by the *growing* producer extent
`N` and ask for a **repair function** `h`:

```
h(g(d, r, r_N), r_N, r_{N+1}) = g(d, r, r_{N+1})        -- repairs one element
h(f(x, y), ·, ·) = f(h(x, ·, ·), h(y, ·, ·))            -- distributes over f
```

and then the fused rolling update `t'_{N+1} = f(h(t'_N, r_N, r_{N+1}),
g(d_{N+1}, r_{N+1}, r_{N+1}))` computes the consumer in the same pass as the
producer, with no intermediate ever stored.

**What the engine does, mechanically, on the schema:**

1. **Inline** all analyzable code between the two instances into `g` (the
   reference notes this is always safe).
2. **Find `h` by the inverse-function method.** Rewrite `g` as
   `g'(r_N)(c)` with `c` the outermost `r_N`-free subexpression — the
   reference's rule, *"the outermost expression independent of the parameter
   is the one to abstract"*, which is exactly the choice that maximizes
   invertibility — then `h(x, r_N, r_{N+1}) = g'(r_{N+1})(g'(r_N)⁻¹(x))`.
   Inversion is symbolic rewriting on the fragment's expression tree: each
   operator either names its inverse (`exp`/`ln`, `+c`/`−c`, `*c`/`÷c` for
   `c ≠ 0`) or the rewrite fails.

   The candidate is taken **whole and unconditionally**: nothing inside `c` is
   ever inverted, and nothing inside it has to be. The per-element identity is
   `g'(r_{N+1})(g'(r_N)⁻¹(g'(r_N)(c))) = g'(r_{N+1})(c)`, a composition that
   holds for any `c` whatsoever, and the rolling update recomputes
   `g(d, r_{N+1})` from the datum rather than from `c`. So `relu²`, a `max`, an
   `exp` or a slope term inside `c` is custom logic the engine carries and never
   reads; a refusal can only come from the chain *above* `c` — `g = relu(d − r_N)`
   abstracts `c = d` and then names `relu` as the operator it could not undo.
3. **Check `h` distributes over `f`**, structurally — the two sides of the
   distributivity equation are small expression trees, and the check is a
   normalisation and comparison, not a search.
4. **Emit the rolling update** as one kernel body: the state tuple `(r, t')`
   carried across the reduction loop ([loop-conversion](loop-conversion.md)
   supplies the loop), `op`, `h`, `g` inlined. Several producers concatenate
   into one state tuple — the reference's multi-producer case is free here.
5. **Split-K** for parallelism: local partials plus a global combine whose
   update applies `h` to repair each partial — the reference's two-level
   equations, mapped onto two dispatches (which exist today) or one workgroup
   kernel (once roadmap axis C lands).

**Equivalence is by construction, and construction refuses.** Each step is a
check that either holds symbolically or stops the fusion with a named cause:

| refusal | when |
|---|---|
| `NotAReductionChain` | the region's instances and fragments are not producer→code→consumer |
| `UnverifiableInstance { interface }` | a declared `combine`/`identity` the body cannot be checked against (§3) |
| `UnanalyzableBetween` | the code between instances does not lower to a readable fragment |
| `ElementFunctionNotInvertible { at }` | the chain above `c` does not invert (`g = relu(d − r_N)` names `relu`) |
| `NoRepairFunction { g }` | inversion succeeded but the composite is not closed over the known operators |
| `NotDistributive { h, f }` | `h` does not distribute over the consumer's declared combine |

Refusal is the correct outcome often enough that it must be cheap and named;
a region the engine refuses is precisely a region where a pack is the answer.

**Where steps 2–4 stand.** `crates/lichen-fusion` implements them symbolically
over expression trees of its own, with `FusionRefusal` naming the causes the
table lists — `NotInvertible { at, cause }` and `NotDistributive { repair,
combine }`, plus the budget that canonicalization is held to — and
`examples/softmax.rs` deriving §7's online softmax through a per-element bias,
through a rectified square, and from a hand-written `h`, checking each against
the two-pass form over random inputs. The bridge is in
(`crates/lichen-fusion-bridge`), and §4's schema now feeds this function its
three arguments directly: `examples/fusion_schema.rs` reads a producer's
`combine`/`identity` and a consumer's `element` out of a derived schema and
prints the rolling update — §7's `g = exp(d - r)` between a max and a sum comes
out as `r' = max(d, r)`, `t' = t * exp(r - r') + exp(d - r')`, the online softmax,
with no `exp` written anywhere in the engine. What the schema cannot yet supply
is the *check*: `UnverifiableInstance` is still §9's missing row, so a claimed
combine is believed. Split-K (step 5) is untouched.

**A pack may hand the engine its `h`.** Derivation is the floor, not the only
door: an optimization file can supply the repair function directly
(`derive_with_repair`), and the engine then skips the abstraction and the
inversion entirely and keeps the two obligations that do not depend on where `h`
came from — it must read nothing but the value being repaired and the producer's
two values, and it must distribute over `f`. The per-element identity
`h(g(d, r_N), r_N, r_{N+1}) = g(d, r_{N+1})` is then the **pack's** differential
obligation rather than a symbolic check: the engine has none it can discharge on
an expression it did not derive, and `x · exp(r' − r_N)` — a rescale with its sign
flipped — passes every check the engine does run.

## 7. Flash Attention v1, with custom logic, is the acceptance case

Walk attention *as production code writes it* through the engine — with a
bias and a slope no operator vocabulary would carry:

```lichen
scores = <contraction loop over k>   ~ "claim:Reduction(combine,identity)"
m      = <row max loop>              ~ "claim:Reduction(combine,identity)"
-- custom element-wise: exp(x + alibi·i − m), bias folded in — read, not declared
s      = <sum loop over that>        ~ "claim:Reduction(combine,identity)"
o      = <second contraction>        ~ "claim:Reduction(combine,identity)"
```

- The engine matches `m → g → s`. Inversion on `g = exp((x + alibi·i) − m)`:
  the outermost `m`-free subexpression is `c = x + alibi·i` (the custom logic
  lands *inside* `c`, unread and unopposed), `g'(m)(c) = exp(c − m)`,
  inverse `ln`, giving `h(x, m_old, m_new) = x · exp(m_old − m_new)` — which
  distributes over `+` trivially. The emitted rolling state `(m, s)` with
  `m' = max(m, x)`, `s' = s · exp(m − m') + exp(x + alibi·i − m')` **is Flash
  Attention v1's online softmax, derived** — and derived *through* the custom
  slope, the case that kills every operator-graph pattern matcher.
- The second contraction chains the same way against the rescale, extending
  the state to `(m, s, acc)`. Split-K is the paper's tiling: local partials
  per tile, a global repair-combine across tiles.

What the engine deliberately does not derive is the rest of the paper — SRAM
budgeting, register blocking, the `exp2` trick. That remainder is a pack's
job, installed per machine, matched on the schema of the *canonical* region.
The division of labour this design predicts, stated as the test it is judged
by: **the correctness of fused attention comes from the engine, through
whatever custom logic the program carries; the last factor of speed comes
from a pack; neither is written into the logic, and neither ships with it.**

The win is also a different quantity from the one the graph JIT measured.
There, fusion removed *dispatch overhead* (65–81% of a 16-link chain at
≤ 65 536 elements, ~10% at a million). Here it removes **stored
intermediates**: unfused attention materializes the `N×N` score matrix twice,
fused attention materializes neither — O(N²) memory to O(N), which matters at
every dispatch-overhead regime and grows exactly where the dispatch win
shrinks.

## 8. Orchestration: the rewrite sits between recording and running

The pass ordering inside `compute.graph` / `compute.graphrun`:

```
record (today)  →  collect interface instances, cut fusible regions  →  per region: schema
                →  per schema: installed packs, then engine
                →  rewrite the node list  →  run under the chosen policy
```

Five properties, each chosen to cost the existing design nothing:

- **The graph IR does not change.** A fused region is one node — still "a
  kernel fragment over an index range", the one kind
  [compute-graph-jit](compute-graph-jit.md) argues for. Fusion changes *which
  fragment a node holds and how many nodes there are*, not what a node is.
  Split-K in two-dispatch form is two nodes with one edge. The whole feature
  is a *rewriter over* the IR, not a revision *of* it — the strongest sign
  the IR's one-kind-of-node decision was right.
- **Rewriting merges; it never reorders.** The recorded order is the
  program's own evaluation order; the engine consumes a connected region and
  emits one node at the region's last position, with every crossing edge
  routed through the new node's declared ports. `EdgeBeforeItsProducer`
  remains the only cycle check, unamended.
- **The unoptimized transcript is kept.** A run under `--no-fusion` — or
  after any refusal — executes the recorded graph verbatim: the reference
  implementation, one flag away. Any suspected mis-fusion is bisected by
  naming the pack or the region, and the differential harness runs
  region-local, not program-global.
- **The cost policy is boring on purpose.** The measured lesson of the chain
  experiments is *no minimum-count gate*. Reduction fusion's economics are
  simpler than chain fusion's: it removes a materialized intermediate, which
  is never a pessimisation in this model (the emitted body is still the
  buffer-map ABI; pressure tuning is the pack author's department). So:
  **fuse every region the engine can prove, apply every pack that matches,
  report both.** The report extends the graph note's formula — submissions =
  closure calls + segments — with the fusion facts: regions found, interface
  instances read, engine firings, packs consulted and fired, and every
  refusal by name. "Your attention fused via pack `flash-attention`; the
  second reduction chain was refused (`ElementFunctionNotInvertible`)" is the
  actionable sentence this owes the user.
- **A pack's effect is local and explainable.** Because packs bind by schema
  and report by name, the answer to "why is this machine faster than that
  one" is a diff of two run reports, not a diff of two environments.

Liveness gets strictly better: every fused region is intermediates that never
reach the value table, so the fan-out memory inversion the graph note warns
about shrinks exactly where fusion fires.

## 9. What exists, what is missing, and the order to build in

| dependency | status | what this design needs from it |
|---|---|---|
| recorded graph, value table, policies | **in** ([compute-graph-jit](compute-graph-jit.md)) | the rewrite substrate, unchanged |
| marks as a compile-time primitive | **in** (`~`; `crates/lichen-highlevel/src/marks.rs`, `crates/lichen-lowlevel`'s anchor table) | the interface claim's carrier — a side table beside the IR, never a value |
| dynamic loop in a kernel body | **in** ([loop-conversion](loop-conversion.md)) | the fused reduction's inner loop |
| fragment IR with expression trees | **in** ([lichen-compute](lichen-compute.md)) | what `g`, `combine`, `h` are made of, and where an in-body mark is published (`KernelFragment::marks`) |
| symbolic repair-function derivation | **in flight** (`crates/lichen-fusion`) | §6 steps 2–4 over expression trees of its own; `crates/lichen-fusion-bridge` carries a *marked subexpression* of a `KernelFragment` to it, for the arithmetic subset and the three slots a projection reads |
| **region schema derivation** | **in** (`crates/lichen-language/src/fusion_claim/schema.rs`) | §4's per-region `RegionSchema`: the instances and their bridged projections, the between code, the boundary ports and the `sha256` content hash, with a refusal naming every way a region fails to assemble. The input is the recorded graph **plus** the registry instances, joined on the fragment, because a graph node holds no kernel id. `crates/lichen-language/examples/fusion_schema.rs` derives the reference sum/max chain and §7's max → `exp(d − r)` → sum chain, prints both canonical forms and hashes, shows a re-spelling keeping the hash and a changed `g` moving it, and hands the schema's own projections to `derive` |
| reading a marked transcendental back | **in** (`lichen_kernel_ir::inlined_exp_operand`, `crates/lichen-fusion-bridge`) | `compile_fragment` expands `exp` into the IR's primitive sequence **before** the fragment is registered, so a mark on one anchors the expanded polynomial and the walk meets its `Select`: the definition's other half reads that sequence back, so §7's `g = exp(d − r)` bridges. The expansion still rewrites the marked value in place, which is the one body mark lowering does not preserve |
| a **numeric** validation of the engine over real data | **in** (`crates/lichen-language/examples/fusion_numbers.rs`) | the reference instance (producer `sum`, `g = d - r`, consumer `max`) derives, and the emitted rolling update agrees with the two-pass form over fixed seeds and lengths; the per-element identity is checked numerically beside it |
| buffer ingress for the fused chain (roadmap axis A) | **in** (`compute.buffer`) | a program array as a kernel input, so a chain has data the device did not itself compute |
| a **kernel** column for the numeric comparison | **in** (`crates/lichen-language/examples/fusion_numbers.rs`) | the reference instance's consumer is a maximum, and the language can write one now: `max a b` lowers to `KernelBin::Max` in both backends and the fusion bridge maps it to `BinaryOp::Max`. The example writes the producer (a sum) and the consumer (a `max` over `d - r`) as `compute.parallel` kernels over `compute.buffer` inputs, runs them unfused on the cpu backend, and prints all three columns — the two host forms and the kernels — with a class-appropriate tolerance. Two emission defects had to fall first: a value's class was filed under the wrong node, and a buffer's element class came from the body rather than from the parameter ([operators](operators.md) §11, [compute-buffer-wrapper](compute-buffer-wrapper.md)) |
| the identity's mark on a `@loop` seed | **in** (`crates/lichen-language/examples/reduction_interface.rs`) | §3's open measurement, **resolved**: a fold that seeds from a *literal* count is decided by the definition pass before the emitter reaches the loop, so the seed's mark is never lowered; a fold that seeds from the buffer's **runtime** length (`fold (k.n, …)`, with the input arriving through `compute.buffer`) stays undecided through that pass, the emitter reaches the loop, and the marked seed is lowered — `reduction_interface` prints `identity` published for both kernels and `combine + identity 0` for the producer's declared reduction |
| content-addressed artifact store | **in** ([artifact-cache](artifact-cache.md)) | schema artifacts and pack bindings |
| float | **in flight** (ten worktrees, [floating-point](floating-point.md)) | softmax/attention are float algorithms; the engine is class-agnostic but its acceptance case is not |
| interface verification (symbolic combine check) | **missing** | §3's teeth; without it claims are trusted, which this design refuses to do |
| a mark whose anchor the dispatcher never lowers | **missing** | §3's open measurement, and now the *literal*-count half of it only: the definition pass runs a convertible `@loop` to a decision before the kernel emitter walks the body, so the emitter peels the decided pair instead of entering `Lower::apply` and the marked seed is never lowered. A recurrence whose count is a runtime value stays undecided, so its seed is lowered and its mark publishes (row above) |
| shared memory + workgroup | **missing** (roadmap axis C) | one-dispatch split-K; until then split-K runs as two dispatches, correct and merely slower |
| pack install path in the package manager | **missing** | the runner-side channel of §5 |

Staging, each stage independently useful:

1. **Interface claims + schema derivation, no fusion.** Programs annotate,
  the compiler derives and prints schemas, the run report exists. Packs can
  already be authored against printed hashes.
2. **The engine, split-K as two dispatches.** The full repair-function
  pipeline emitting local+global node pairs. This stage is done when it
  derives online softmax from §7's annotated code, custom slope included.
3. **One-dispatch split-K**, after axis C lands — an emitter change behind
  the same engine, invisible to logic and packs alike.

## 10. Decisions taken in review, and what stays open

Settled by the superior's review of the first draft:

- **No tensor-operator vocabulary.** Fusion is an interface layer over
  arbitrary logic (§1); `Reduction` is an interface, not an operator (§3).
- **Matching is exact skeleton hash plus explicit constraints** (§5); fuzzy
  matching is rejected on correctness grounds.
- **Logic never names or ships optimisations; packs are installed by the
  runner** (§2, §5) — the driver model, not the wheel.
- **The claim is declared at kernel granularity, and its projections are marks in the
  kernel's body** (§3). The rejected alternative was an *expression-level* claim: a
  label on the reduction's value carrying the projections as lambdas in its payload.
  It was rejected because the payload then restates the algebra beside the code that
  performs it — a claim free to diverge from the body — and because fusion operates on
  kernels, so a claim about a kernel should ride the kernel.
- **A mark is a primitive, not an attribute** (§3). The carrier was the attribute slot
  until it was measured: slots unify and the interface check is structural, and the
  slot's pair is unreachable from the value it annotates. `~` is a side table beside
  the expression tree, and the one thing that crosses into the lowlevel is its
  position — the anchor, written once.

Still open, cheapest to settle before the interface syntax is written:

- **How strong is instance verification in v1?** Symbolic-only (refuse what
  cannot be proved — simplest, most conservative, and some honest reductions
  will refuse) versus symbolic-plus-differential (a claim backed by a
  property harness is believed — more coverage, and the harness is one more
  artifact to define). This note is written symbolic-only; §3's
  `UnverifiableInstance` is where a second tier would slot in.
- **The mark payload's richness** — the spike's payload is one string literal, because
  text is exactly what keeps a mark out of the pair layout. A record payload (a claim
  naming its interface and roles as fields rather than as `claim:Name(role,…)` text)
  is the natural next step, and the mark channel is already positioned for it: the
  fragment's payload is a *list* of fields per mark, so a reader that wants "the mark's
  own text" reads the unnamed field rather than a second variant.
- **Pack trust beyond correctness** — signing/provenance of installed packs
  is the package manager's roadmap item, deliberately not this design's.

## 11. Non-goals

- **Inferring algebra from code.** The engine reads interface claims and
  analyzable fragments, nothing else. Unannotated logic fuses exactly as much
  as it does today and is never pattern-matched into a reduction it did not
  declare.
- **Autotuning.** Tile sizes, workgroup widths, vectorisation factors: the
  pack author's and roadmap axis C's business. The engine emits one correct
  fused form; searching the space of faster ones is a later note.
- **A general rewrite engine.** One transform (repair-function fusion) plus
  substitution (packs); not an equality-saturation framework. A second
  transform enters the same way: a schema-level pattern, a proof obligation,
  a named refusal.
- **Reordering, parallelism invention, or any change to what a graph runs.**
  The graph remains a transcript; this design only changes how much of the
  transcript one node computes.

## Related

- [compute-graph-jit.md](compute-graph-jit.md) — the substrate: nodes, values,
  policies, and the measured fusion economics this extends from dispatch
  overhead to memory footprint.
- [attributes.md](attributes.md) — what a mark deliberately is **not**; fusion is the
  consumer the schema tail was waiting for, and the mark channel is its replacement.
- [gpu-algorithm-roadmap.md](gpu-algorithm-roadmap.md) — axes B and C, the
  capabilities the fused kernel body needs.
- [loop-conversion.md](loop-conversion.md) — the dynamic loop, landed, that a
  fused reduction body lowers through.
- [reduction fusion](../reference/reduction%20fusion.typ) — the mathematics
  §6 operationalizes.
- [artifact-cache.md](artifact-cache.md) — the content-addressed store schema
  artifacts and pack bindings ride on.
