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
justified against the schema, so the worst case of any steering —
engine-derived or runner-installed — is that the logic itself runs.

## 2. The three artifacts, and who writes each

| artifact | written by | distributed with the logic? | wrong answer possible? |
|---|---|---|---|
| logical code + interface claims | the logic author, once | yes — it *is* the package | yes — the author's bug, reported as such |
| schema | the compiler, from the logic alone | derived on demand, content-addressed | no — a derivation, not a decision |
| implementation | **the engine, and only the engine**; a **pack** steers it (runner-installed) | **no — never ships with logic** | no, by construction — a steering artifact is *refused*, never wrong |

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
  channel's business*. `(p : tuple{Float, Float}) => p(0) * p(1)` lowers its leaves in
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
3. **Therefore the marked *seed* is never visited, and the `combine` mark's value was the wrong
   claimant.** `Lower::publish_mark` only ever sees nodes the walk lowers, so the `identity` mark on
   the seed (`fold (6, 0.0 ~ "identity")`) never publishes at all — that is the role
   `RoleNotMarked { interface, role }` names. The `combine` mark failed for a **second, different**
   cause, found later and fixed at `cfea0d6b`: it published against the **first claimant among the
   nodes the anchor's walk reaches** rather than the expression it was written on (the paragraph below
   traces it), so in `crates/lichen-language/examples/reduction_algebra.rs` the literal-count `combine`
   bridged to the seed — `0` for the seed `0.0`, **`3.5`** for a distinctive seed. **Corrected by that
   fix**: the literal-count `combine` now bridges to the fold's own `Bin(Float, Add)`, canonical
   `d + d + …`, so the two halves no longer fail for one cause — `combine` reads as `+` and only
   `identity` is still missing (passage 4's scheduling constraint). The two acceptance tests that
   hard-code `Reduction { combine: Add, identity: 0.0 }`
   (`crates/lichen-language/tests/tiled_reduction.rs`, `plan_staging.rs`) were accurate when written
   and are now only *conservative*: the algebra they state is what the program means.

**And a mark on a top-level *routed operator* anchored to the wrong value — measured at `afc116dc`,
fixed at `cfea0d6b`.** The mark channel's premise is that a mark names the value its expression
produced; where the expression is an operator the language **routes** (`compile.rs` sends `a * b` to a
prelude binding), that premise did not hold: in a kernel body a mark written on a top-level operator
recorded the **left operand's value** (`(read * 2.0) ~ "element"` recorded the identity `d`,
`((0.0 + read)) ~ "element"` recorded `0.0`), while a mark on an **application** (`exp (…)`) or one
**inside a `@loop`** recorded the whole expression. **The trace, node by node**: the routing makes the
expression an `Apply`, so `checker/lambda.rs` leaves `val = None`, `term = Some(apply)`, and
`Checker::anchor_marks` stamps the Apply **operation** node — resolution of the anchor was never wrong.
But the definition pass's frozen-apply instantiation attributes **every clone it makes to the
caller-side apply** (`static_module/apply.rs`'s `static_node_apply`), so three distinct walked nodes —
the read, the constant and the `Bin` — all carry that one origin; and `Lower::arithmetic` emits the
left operand, the right operand and *then* the operator, while `publish_mark` de-duplicates on the
stamped node. **First claimant won, and the operands precede the operator.** `exp (…)` is a `Prefix`
that is never routed, which is why it was always right. The fix is emitter-local:
`Lower` carries a `lowering` stack of the anchors currently being defined and `publish_mark` returns
early for a source on it — **a mark is published by the lowering of the node it is anchored at, never
by an operand of it** — with the per-stamped-node de-duplication kept, so an expanded recurrence still
lowers one anchor once per copy.

Every recorded value that moved was **wrong before**: `fusion_numbers`' literal-count producer and
`reduction_algebra`'s three literal-count folds all moved from the fold's **seed literal** to the
`Bin` the body computed (the distinctive-seed case now reads `3.5 + d + …` rather than `3.5`), and
`reduction_interface`'s element kernel moved from `ValueId(0)` — the **first parameter slot**, i.e. the
datum `d` — to the `Bin(Int, Mul)` it computes, which the bridge then refuses **by name**
(`MachineInteger`, the class refusal this note already records for that kernel), so that example's
"5 of 5 projections bridge" is now **4 of 5**: the count fell because it got honest.
`examples/fusion_schema.rs` is byte-identical — all four schema hashes and fragment digests unchanged —
because its marks sit in loops the *emitter* converts rather than in expanded prelude clones. No test
pinned a moved value. The regression test
(`crates/lichen-language/tests/mark_anchor.rs`) compares canonical `Expr` trees and **fails on the
parent commit**, which is what says it catches the defect rather than describing the fix. What is still
wrong on this path: the literal-count `identity` (passage 4), and a mark on a value `define_in`
redirects through a **view** rather than through a frozen apply's residual is unmeasured.

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

**The cheapest fix is one predicate, and its price is a constant-folding optimization.** The
nest is still live in the module — `write_node_value` sets only the apply node's `value`, never
clears its `Apply` operation, and `Module::loop_conversion` re-derives the whole conversion
read-only — so nothing would have to be recorded a second time. A peel that declined to resolve
`Index(pair, 0)` (and `pair_value_half`) off a target that still carries an `Apply` whose callee
is a `looping` function would let `Lower::apply`/`lower_loop`/`loop_state` build the nest, and
the literal case would then read like the runtime one. The cost is not free and is recorded
with it: every literal-bound `@loop` in a kernel would stop being constant-folded into
straight-line code, so every such fragment's **body and digest change** (artifact caches
invalidate, any test pinning an unrolled shape moves) and a kernel folding a literal count emits
a real nest. The benefit is narrow too: a literal count is where tiling has least to offer,
because the whole fold already fits one lane. That is why the reorder above is preferred to the
predicate, and why the practical road to a declared algebra is a **runtime** extent.

**And the wall an integer reduction meets is the class, not an operator.** The bridge refuses a
projection that computes in the machine-integer class at all — `Const(Int, _)`, `Bin(Int, _)` and
`BufferReadCall(Int)` each earn `MachineInteger`
(`crates/lichen-fusion-bridge/src/lower.rs`) — on the reason that "`Int` wraps and `f64` does not
hold every value" (`crates/lichen-fusion-bridge/src/refusal.rs`). Measured, that wall is the
**only** thing between an integer reduction and a readable algebra when its count is a runtime
value: both marks publish there, and the class refuses both halves. Neither repair is cheap:

- **Narrowing the refusal by operator** is sound only where the class's arithmetic agrees with the
  reals: `Max`/`Min` bridge cleanly, because the `f64` conversion is monotone and
  `max(f64 a, f64 b)` is `f64(max a b)`, and an exactly-representable `Const` does too. It buys
  almost nothing, though: the integer operation worth reading is a **sum**, and a sum is precisely
  the wrapping case, so `Add`/`Sub`/`Mul` stay unreadable and the refusal stands where it matters.
- **Giving the symbolic language an integer class** is the honest repair: wrapping `i64` addition
  is associative with identity `0` — it is Z/2⁶⁴ — so an integer sum's algebra is real, and
  `BinaryOp::associative` would be a truthful licence rather than one borrowed from the reals. Its
  price is the whole symbolic path, because `Expr`, `normalize` and the repair-function derivation
  would each have to say which class they are in.

So an integer reduction does not fuse today, and that is a limit of the symbolic language rather
than a hole in the mark channel.

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
  can see. **There are two granularities of that one text** (`e302ade`): the
  region's, and — since a placement happens inside **one** kernel — a
  **node**'s, which is the region's per-node lines without the position, so one
  pack serves a node wherever a region puts it. The region's form writes the
  position and then the node's own text, so the two cannot drift and no region
  hash moved when the node form was factored out. In it: the instance count;
  each instance's position, interface and,
  per role in name order, the **canonical** projection in a fully parenthesized
  prefix form (`(max t (+ d (- r)))`, which no precedence or rendering rule can
  make ambiguous); each `between`'s producer, consumer and input ordinal with its
  canonical element; and each port's kind, region-local node and ordinal, and
  element class. Out of it: kernel ids, graph value numbers, a constant extent's
  number, the backend, the raw fragments and their digests, the marks' body
  positions, and the **memory plan** (§5) — where a value is kept and when the
  lanes synchronise is *how* the region is implemented, so two packs with one
  hash and different plans are two implementations of it rather than two
  schemas. The exclusions are what make two spellings one schema — the
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

## 5. Optimization packs: guidance the runner installs

A pack is the **guidance** half of the system, and it is guidance rather than an
implementation because the computation has exactly one source. What a pack supplies is what
the engine cannot derive and what only *this* machine knows: the **algebra** a region's
fusion needs, the **placement** of the values the fusion stages, and the **directives** that
decide which of the region's operations run in one kernel. It never supplies the arithmetic
— an artifact that carried a body would be a second source of truth for what the program
computes, and *faster, never wrong* would then rest on a differential harness instead of on
construction.

A pack is an ordinary artifact installed on the runner's machine (via the package
manager into Lichen Home, beside the toolchain), and it binds by schema, not
by name and not by the logic author's say-so. **What has landed is the binding
and one field of it** — a **placement pack** matched to a node's schema, whose
plan steers the tiling (`crates/lichen-language/src/placement_pack.rs`, at
`e302ade`); nothing installs one yet, so a host hands its packs in as read data,
and the installation path is still a `missing` row below. The rest of the block
is the interface the other fields are owed in:

```lichen
--- inside a pack, schematically ---
schema     = "sha256:…"                      -- landed: the NODE schema's hash; **optional** since `fuse`
name       = "sum-in-shared"                 -- landed: what the report calls it
requires   = { class = f32 }                 -- landed for `class` only; `axis` has no surface yet
reference  = <the algebra this claims>       -- owed: no interface verification exists
fuse       = { region = "sha256:…" }         -- landed: the REGION schema's hash; `as = one_kernel` is its only form
plan       = { staging = …, sync = … }       -- landed for `staging`; `sync` is owed
```

**A pack must steer something.** `schema` became optional at `78131b5a` because a pack that only asks
for a fusion names a **region** and has no node to place; a pack that neither stages a value nor names a
region is refused (`EmptyPlan`), a `fuse` with no region steers no fusion, and the emitter's placement
gate is "names this node **and** stages a value", so a fusion-only pack can never fire there. A region's
`requires` is read off its first input **port**'s class — one rule with the node case, not a second one.

The landed pack's plan is **the fused artifact's own spelling** — one `Plan`
type, not a second one — so a pack and an artifact cannot disagree about what a
staging entry is.

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
  command: *print the schema hash this pack must target* — and **which schema
  depends on what the pack steers**: a placement pack names the **node**
  schema, because an intra-kernel staging happens inside one kernel, while a
  pack that steers a *fusion* names the region's. The two forms are one rule
  (§4), and a run report's `no pack matched its schema sha256:…` line is that
  command for the node case.
- **Losing a pack is loud and safe.** A pack that does not match changes
  nothing the program computes — the logic runs, possibly engine-fused — and
  the run report says which packs were consulted and which fired. A logic
  change that drifts the hash reports `pack "flash-attention" no longer
  matches (schema 9f2c… → 41b0…)`. So an installed pack can make a program
  **faster, never wrong**: the fallback is the reference implementation
  itself.
- **Equivalence is an obligation with teeth, and it lands on the algebra rather than on a
  body.** Because a pack hands over no arithmetic, the placement half cannot be *wrong*: the
  worst a placement directive can do is be **refused** — by the three facts (§5's second
  half), by a target that cannot honour a space, or by an interface that never declared the
  name the directive used. What remains is the algebra, and that is where the harness
  belongs: a differential run of the region's derivation against the reference region's over
  generated inputs, the way the graph suite compares against hand-derived closed forms rather
  than a second run of the same code. Part of it the engine checks symbolically and refuses
  **by name**, which is stronger than a harness: a `combine` that is not associative is
  refused rather than tiled (`BinaryOp::associative`), and the per-element identity of a
  repair function the engine derived itself it checks for free. A pack that cannot name its
  `reference` is refused at install: a claim about nothing is nothing.
- **Pack beats engine.** When both cover a region, the pack applies and the
  report says so — a derived fusion is the floor, not the ceiling, and a
  human who wrote a pack knew something the engine did not.

**The one directive that is neither algebra nor placement, and the two gaps under it.** A
pack also needs to say *which of the region's operations are one kernel*, and that is a
directive rather than an implementation, because computing a region in one kernel is an
inlining the engine can perform — it needs no arithmetic from anyone. Two gaps stand between
that sentence and the tree, and both are `missing` rather than merely unimplemented:

- **the engine cannot fuse on request.** It derives fusion for a
  producer→code→consumer chain and refuses a region that is not one (below), so for a
  fan-out body there is nothing for a directive to drive — and a fan-out body is the shape
  this subsection's staging is about.
- **the domain correspondence has no surface.** Inlining one operation into another is legal
  only where the two domains line up: which element of `B` a given element of `A` feeds.
  Where both run over one `compute.range k.n` the engine reads it off, but a non-trivial
  mapping has nowhere to be stated. The paper's `partition`/`claim` are that surface and
  lichen does not have it, which is the same absence
  [shared-memory-and-barriers](shared-memory-and-barriers.md) §5 records.

**Measured, and the first gap is wider than it reads** (at `ebae69bc`): *nothing* in `crates/` fuses
two kernels today. `fusion_claim::derive_regions` has no caller outside
`examples/fusion_schema.rs`; `lichen_fusion::derive`/`derive_with_repair` derive `Expr`-level
assignments and **no emitter turns them into a `KernelBody`**; and the only transform reachable from
production is the intra-kernel tiling of one reduction (`automatic_tiling.rs`). So the first gap is not
"it cannot fuse on request yet" — there is no fusion *to* request.

The graph side, by contrast, is already there, and that is the measurement's most useful correction: a
**graph pass can return a two-nodes-into-one graph today** (`GraphPass::apply` answers a whole `Graph`,
and `with_inputs` + `push` + `returning` build a legal one that the runner accepts — measured on the
host stub), so [compute-graph-jit](compute-graph-jit.md)'s "general builder" is not what blocks a
merge. What blocks it is:

1. **no fused body** — nothing composes two fragments, and the engine's own derivation stops at `Expr`
   (§6's repair-function fusion included);
2. **the landing path** — `land_fused_kernel` replaces a node through `Graph::replace_kernel`, whose
   gate is `FragmentShape` equality, so a merged fragment is refused (`ShapeDisagrees`) even when the
   merge is legal: a merge *is* a shape change, which is what "not a linear chain" means; and
3. **renumbering, forced rather than optional** — `push` allocates ids consecutively from
   `input_count()`, so a merge that is not at the tail shifts every later value, every edge and count
   naming it, and `returns()`.

**The first slice, ranked, and the measurement owed before any code.** Smallest honest step: the
**graph-level merge machinery alone** — a rebuild/renumber plus a landing path that is not
`replace_kernel`, with the replacement fragment supplied as an ordinary `fused/` artifact (which a human
can write), because that needs no arithmetic. Then the pass that inlines a consumer into a producer,
which additionally needs a **body composition** function and the legality conditions: the absorbed
output is read exactly once and is not also returned; the two extents correspond; the read's address is
a function of the index the survivor can compute; the classes are equal; and the merged ports are the
union minus the absorbed buffer. The `fuse` directive comes **after** both, because it would drive a
region engine that does not run, add a field to `Pack`, and force the node-schema/region-schema question
into the matcher.

**That measurement has been taken, and the plan survives with two corrections.** The splice is
expressible: a hand-composed fragment of `examples/fusion_schema.rs`'s chain passes
`KernelBody::validate` and `KernelFragment::validate`, and once the value table is **compacted** and the
consumer's **entry block** is inlined into the producer's return block, the merged body **runs on a
device** and answers the two-pass reference element for element (8, 100 and 1024 elements, each against
its closed form). So the missing piece is a **function** — a body-level splice plus a value-table
compaction — and not an IR capability. The two corrections:

- **The landing blocker is narrower than this section said.** A merged fragment's shape (inputs 1,
  outputs 1, arity 2) *equals* the producer node's, so `replace_kernel` accepts it: the merge lands on
  the producer's node today, and what remains is the **graph rebuild** — drop the consumer, repoint
  `returns`, renumber. A merged fragment is refused only where its ports differ from the target node's,
  and the union of ports is exactly what the two fragments *cannot* state: they carry ordinals, and
  `derive_regions`' `Port`s are the only place the union is computed.
- **A straight block splice is refused by the backend, and the reason is a class the IR does not
  state.** The literal splice leaves the consumer's former entry parameter read in no block of its own,
  so the SPIR-V emitter's class inference falls back to the module class and refuses the integer edge
  (`MixedClasses`). Inlining the entry block removes the cause; a **carried value stating its own class**
  in the kernel IR would remove the need for the workaround — the same repair §4 records as an option.
  Compaction is not optional either, and one gate is why: the artifact reader re-derives a block
  parameter's index, so an orphan parameter passes `KernelBody::validate` and is refused only when the
  artifact is read.

**Four decisions the splice cannot make for itself**, and they are the policy layer's rather than a
body function's: **which input is absorbed** (a graph edge, in neither fragment), **whether the
producer's value may be bound in-lane** (here every lane computes the same fold, so binding it locally
is exact — but lane-invariance is stated nowhere in a fragment), **what a duplicated mark means** (a
merge carries `combine` twice: the kernel IR's reader refuses it as `AmbiguousStagedValue` while the
language's `marked_value` silently takes the first, so the two must be made to agree), and **whether
two slices are one** (two bodies staging into ordinal `0` collide, and unifying them claims one value).

**The composition landed** (`027447ab`): `lichen_kernel_ir::compose(producer, consumer, absorbed) ->
Result<KernelFragment, ComposeRefusal>`, in the crate that depends on nothing, because the splice is
pure `KernelFragment` data. It renumbers the consumer's values and blocks by the producer's counts,
binds the value the producer **writes** where the consumer read the absorbed position (a write produces
nothing, so the forwarded value is the write's value operand), **inlines the consumer's entry block**
into the producer's return block, compacts the value table so no `BlockParam` survives whose block does
not list it, and validates its own answer. Three things it settles by **contract** rather than by a
caller's decision: the producer's **store** goes (its outputs are not the merged fragment's, and a store
that survives is a write the program never made — benign on the measured chain only because both writes
land on the same lane index), its **marks** go (they name roles of an interface that no longer exists,
so the merged fragment speaks the consumer's), and `roles` is **empty** (a merged fragment has no single
parameter struct for a path to name). Measured on `examples/fusion_schema.rs`'s chain: 38 values, 9
blocks, one write, one input, three marks, and the three-way agreement — merged, the two-pass reference
and the program's own recorded graph, element for element at 8, 100 and 1024 elements against
`n − n(n+1)/2` — plus a round trip through the fused artifact.

The four decisions are **named refusals**, not guesses: `ForwardedValueIsLaneDependent` (the forwarded
value is reachable from the producer's invocation index, so binding it in-lane would read another
lane's — a slice and a barrier rather than an inlining), `DuplicatedMark` (two of the **surviving**
marks carry one text, which dropping a side cannot resolve), `SlicesCollide`, and a shape, target or
class disagreement. Its preconditions are the caller's, and they are stated rather than checked
infallibly: equal `param_shape`, `target` and entry-parameter counts with the entry parameters
corresponding position for position; the producer's output ordinals and the consumer's surviving input
ordinals naming the same buffers in the same order; exactly one producer write and one return block; a
consumer entry block that is no branch target; and constant read positions. **`compose` never sees the
graph** — the region between a composed fragment and the graph rebuild that repoints the edges is the
next slice's. Unproven and carried: no emitter-level test reproduces the `MixedClasses` refusal a
straight block splice earns, so the entry-block inlining is evidenced only by the merged body running;
two of the refusals are the function's own gates rather than decisions; and the compaction path that
would drop an unread `SliceWriteCall`'s version is unreached, because two slice lists are refused.

**And the composition got its caller** (`98761ae4`): `crates/lichen-language/src/region_merge.rs` is a
graph pass that **derives the regions** (`derive_regions`, its first production caller), checks what it
may merge, composes, and **rebuilds the graph** — the region becomes one node at the region's last
position, the absorbed node is gone, later values are renumbered and `returns()` repointed. Measured on
`examples/fusion_schema.rs`'s chain: two nodes, four values and `returns [3]` become one node, three
values and `returns [2]`; the answer is element for element the answer the same program gives without
the pass; and a **counterfactual** closes the question the equal answer cannot — swapping the operands
inside the composed body moves the answer from `−28` to `35`, so the composed node is what the device
dispatched, rather than a rewrite nobody ran. Legality is refused by name: a region that is not a pair,
an edge that is not one input, a produced value the graph **returns** or reads elsewhere (deleting a
returned value is a wrong answer, not a refusal), extents that do not correspond, a read address the
survivor cannot compute, disagreeing classes, and **ports that do not correspond** — that last one is
load-bearing and was not on the list until the caller needed it: `compose` never sees the graph, so the
position-for-position correspondence of the producer's reads with the consumer's surviving inputs is a
caller's precondition whose violation would be a wrong answer, and `compose`'s own doc now says so and
names the check.

**What the merge can reach today is narrower than "shape-changing fusion", and the reason is a
classification `compose` does not yet make — not staging.** `compose` refuses a forwarded value that is
reachable from the producer's invocation index, and an **elementwise** producer's value always is — so
the only region that merges today is one whose producer is **lane-invariant**: a fold over the whole
extent, written at position `0`. Measured since, and the first reading of this was wrong: **lane
invariance is strictly stronger than what a merge needs.** An **index-local** producer — one whose
output at an address is a pure function of that address — needs **no exchange at all**, because the
consumer can recompute it at whatever address it reads; the checkable predicate is the same
`depends_on` walk with the write's address substituted for the lane, plus the soundness clauses that
the cone contains no `SliceReadCall`, no `CallKernel` and no non-entry block parameter (loop-carried
state). So `lane-invariant ⊊ index-local`, and the first elementwise case (`e[i] = d[i] * 2` feeding
`z[i] = e[i]`) is closed by a **lane-conditioned** form of the refusal — refuse only when an absorbed
read addresses neither `0` nor the survivor's own invocation index, which is the address test the merge
pass already carries. The general case is **read-site recomputation**: one copy of the producer's cone
per read, with the address rebound (`KernelBody`'s transcendental inlining is the same machinery), which
is what `e[i] + e[i + 1]` needs and what in-lane binding cannot do.

**No shape in this tree needs cross-lane staging**, and that matters for this section's second half:
the only staging that exists is intra-kernel (`Folding::tiled` — one reduction folded across a
workgroup, with no consumer node and no graph edge), no producer→consumer region measured needs a slice
or a barrier, and there is no third class (neither lane-invariant nor index-local) in the tree. The
staging slice is therefore speculative for now, and the plan's two fact checks have nothing to bite on
until a shape needs one; see the refusals below for the state those two are actually in.

**The decision to fuse is the machine's, not the engine's, and not the logic's.** Inlining is
*logically* safe wherever those predicates hold — that is what makes this half tempting to do
automatically — but whether it pays is a machine fact: it changes register pressure, occupancy and
instruction count, and an extreme optimization has to weigh all of it. So a merge is **claim-gated**:
the logic's claim states a *fact* about its code (a reduction's algebra, an elementwise kernel's
function), and a **pack** states the machine's decision — **`fuse` landed at `78131b5a`**, naming the
**region** schema it licenses, with the pass reporting which of the two licensed each firing (the pack
by name, or the floor's own proof). The rule that keeps the two apart is the pass's own invariant: **a
pack changes which regions are attempted, never what must hold** — every legality condition below still
refuses by name under a pack. The floor itself merges only what a declaration lets it *prove*: today, a
reduction chain whose algebra licenses the merge. **And the first elementwise merge runs**: `compose`
refused *any* lane-dependent forwarded value, and measured that was too broad — where the producer's
write and every absorbed read address the same entry parameter, binding it in-lane is exact and needs
no slice and no barrier. That is now exactly what it accepts (and nothing more; the check compares the
bodies' own statements, so a write at `i + 1` read at `i + 1` is still refused, conservatively). The
device proves it end to end: `e[i] = d[i]·2` feeding `z[i] = e[i] + 1` merges under a pack, answers the
un-merged program element for element, and a **counterfactual** — `+ 1.0` moved to `+ 10.0` inside the
composed body — moves the answer from `[3, 5, …]` to `[12, 14, …]`, which is what says the composed
node is the one dispatched rather than a rewrite nobody ran.
Two more facts a reader should carry: `returns() == None` forbids **every**
merge (the runner then takes the whole value table, so a deleted value changes the answer, and the pass
refuses rather than merging), and the surviving node is **anonymous** — a composed fragment is never
registered, so it carries no claim and a later digest-keyed pass walks past it, which the merged report
line now says for itself. Registering it with the consumer's claim is declined for now: a composed
fragment is not the consumer (it computes a fused reduction that is neither interface's algebra, and
`compose` empties `roles`), and the registry is process-global and append-only, so a ghost row would be
visible to every later join in the process.

### The memory plan: the second thing a pack hands over

A fused body is a DAG, not a chain. Once a kernel's read and write are a value —
[code-audit D17](code-audit.md)'s "a `compute.write` is not an effect, it is the
value of an output position" — a body that carries a value from one lane to
another needs somewhere to keep it between the write and the read. There are
three spaces, and the choice is not free:

| space | who sees it | what it costs |
|---|---|---|
| **registers** | the lane that wrote it | nothing to synchronise — a **value** no other lane reads; a **slice** placed here is a per-lane array, which the GPU target refuses by name today |
| **shared** | the workgroup | a barrier between a cross-lane write and its read |
| **global** | every workgroup | a second graph node — the graph's business, not a kernel's |

Which one a value *may* use is decided by who reads it and how uniform it is over
the lanes; the second half is what `# p` ([attributes](attributes.md)) certifies.
The mechanism — the buffer type's `DP`, the three facts that decide a barrier, the
scan that inserts them — is
[shared-memory-and-barriers](shared-memory-and-barriers.md)'s and is not restated
here. What this note owes is the **interface**: what a pack may say about the
plan, and what the engine does with it.

**The interface is four seams, and they are the whole of this subsection:**

- **The plan is pack-side data, never schema data.** §4's exclusions say it: the
  region's hash covers what it *computes*, and a placement or a synchronisation
  point is *how*. A plan that could move the hash would turn a cost decision into
  a semantic one, and would make two packs that compute the same thing at
  different speeds mutually inert.
- **A declared plan is checked against the same facts the automatic one is.** The
  three facts of [shared-memory-and-barriers](shared-memory-and-barriers.md) §4 —
  whether the addresses may overlap, whether writer and reader are provably the
  same lane, and whether a barrier already separates them — are the only
  authority. A pack that declares a barrier where none is needed wastes a
  synchronisation; a pack that **omits** one the facts require is
  `PlanContradicted` and refused by name. A plan is a proposal the engine
  validates, and that is *all* a plan can be: a placement directive the facts refuse is
  **refused**, which is what makes §5's *faster, never wrong* true of a runner-installed
  pack by construction rather than by a harness. **And the check is a comparison rather
  than a second
  derivation**, because the automatic scan and a declared `sync` produce the *same
  artifact*: both are a set of `(write, read)` pairs, named by mark in a plan and by
  `ValueId` in a fragment, and the fragment resolves one to the other (below).
- **The automatic plan is the fallback, and it must be derivable with no pack at
  all.** For a region no pack covers, the emitter places a value in the cheapest
  space its own analysis proves: registers where the definition cone shows every
  reader is the writing lane, shared plus the scan's barriers where a reader is
  another lane of the group, and otherwise **the region does not fuse at all** — a
  reader in another workgroup is a graph edge, which is the engine declining rather
  than a third placement. That walk, in three steps with the width entering two of
  them, is [shared-memory-and-barriers](shared-memory-and-barriers.md) §5's and is
  not restated here. A pack **replaces** the placement and the barrier
  positions; it never replaces the obligation to have them.
- **The uniformity certificate decides which placements are legal, and it is the
  perspective slot.** A value certified uniform over the group's width needs no
  cross-lane staging at all — every lane computes the same value, so there is
  nothing to exchange; a value certified uniform over `n < group` needs staging
  **only where a reader sits outside the writer's own `n`-subgroup**, because inside
  it those lanes already agree; and an uncertified value that crosses lanes is
  either staged with the scan's barrier or refused. Where the lattice admits a width
  no device has (`6` against a 64-lane group), a placement must refuse or state its
  rounding and never round silently — that open question stays
  [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md) §4.2's.
- **A plan names values by mark, and that is forced rather than chosen.** §4's
  canonical form excludes "a run's value numbering: reported, never hashed", so a
  plan cannot name a value by number and stay valid across sessions. It names the
  **mark** the body published — §3's channel, and alpha-equivalence free — while a
  plan naming a value no mark published is refused by name, **and so is a mark text
  that two values carry**, because that names an ambiguity rather than a value. The
  mark channel was built for declaring what code cannot carry; this is the second
  thing it carries.
- **The resolution runs one way, and the IR already has both halves.** A plan holds
  the text, and the fragment holds the value: `KernelMark` is "the body value it was
  attached to, and the payload it carried", so the fragment resolves a payload text to
  its own `ValueId` at compile time while the text is what survives to be installed and
  re-matched. That is the same split §4 makes for the hash, and it is why a plan needs
  no value numbering to be durable.
- **The plan is checked where the width is, so it has to be carried there.**
  Matching is schema-level — hash plus constraints — and says nothing about
  placement; the check needs the fragment's lane-group width and the three facts, so
  a plan travels from a match to the emit as data on the matched region. Which width
  that is, is the fragment's to declare, on `int_width`'s template: a target fact as
  data, in `fragment_digest`, refused by name when a plan asks for a width the target
  does not have.
- **Two hashes, and the plan is named in neither.** §4 excludes the plan from the
  *schema* hash, so two packs that compute one region differently are two
  implementations of one schema and neither is inert; the plan's **effect** — the
  slots it stages and the barriers it places — is in the *fragment digest*, because
  two fragments differing in staging are two modules, and a module cache that
  conflated them would serve one kernel's shader for another's. The first rule keeps
  a cost decision from becoming a semantic one; the second keeps a cache from lying.

- **The artifact is text, and it carries the plan — which is what gives a placement a route to
  the emit.** A fused kernel crosses as an **artifact** whose format `lichen-compute` defines and
  reads back (`crates/lichen-compute/src/compute/fused/`, **landed** at `fb0574c`): pretty JSON
  through `facet`, holding the fragment, the claim it is interned with, and the plan it was
  checked against. `read_fused_kernel` parses, converts, runs **both** IR gates —
  `KernelBody::validate` and `KernelFragment::validate`, each of which sees a fact the other
  cannot — and then `check_plan`; it is **pure**, interning nothing and consulting no global, so
  the same text reads back as the same kernel in any process. Carrying the plan is the point
  rather than bookkeeping: `KernelFragment` has no plan field, `LaunchSet` has none, and
  `ParallelBackend::run` takes none, so before this artifact a placement had **no route to the
  emit at all**. `land_fused_kernel` closes the loop: it reads the text (every check runs
  before anything is replaced) and puts the fragment into the **recorded graph node the runner
  reads**, so the fused body is what runs. The text is spelled for a person — `ValueId` is a
  bare number, every enum is a lower-case string, and a value-table entry's position *is* its
  identity — because a fused kernel is something a human tunes by hand, and a refusal names the
  field's path and the byte range rather than only that the text failed. Two things are
  recorded rather than hidden: a stray key inside an enum **variant** is ignored
  (`deny_unknown_fields` does not reach variant structs in `facet` 0.45), and the dependency
  brings `facet-reflect`'s `smallvec` **alpha** with it.

**What a pack author writes, schematically.** The spelling is the implementing
session's to settle; these seams are the interface:

```lichen
schema     = "sha256:…"     -- landed: the node schema's hash (see the block above)
requires   = { class = f32 } -- landed for `class`; `axis` has no surface
reference  = <the region's derivation>   -- owed
plan       = {
  -- `name` is what the pack calls the value it stages, and what the landed
  -- fragment publishes as its mark: the plan's only stable way to name one.
  -- `space` is a plan value — `Register | Shared` — and names no length.
  -- `group_width` is a **constraint on this placement** — the group width the facts
  -- are checked against — and not the origin of a length: a slice's element count is
  -- the body's, and a plan's width is the plan's own group width.
  -- **A text, for now**: naming a value by the *path* the body published is the
  -- deferred decision §3's open item records, so a name here is still a mark's
  -- payload text rather than a path.
  staging = [ { name = "tile", space = Shared, group_width = 64 } ]
  -- `auto` is the scan. An explicit sync names the two marks it separates, never a
  -- position: a position drifts under re-spelling. Owed: the IR's `Plan` has no
  -- `sync` field yet.
  sync    = auto        -- or { between = { write = "tile", read = "tile_read" } }
}
```

**This block is the only place above the IR that names a space** — the buffer's **type** names
the `DP`, and a space is what the plan puts inside it — and the pack names no body either: the
body is the engine's, from the region's own dataflow, so *a slice is a thing and a space is
where it lives*. That is the rule the mechanism is built on rather than a convention of this
note:
[shared-memory-and-barriers](shared-memory-and-barriers.md) §5 states it with the layer table,
and with the three quantities — a slice's element count, a staging group's width, a device's
lane width — that keep being read as one.

The refusals this interface owes, each naming its cause — **and the state of two of them is not what
this paragraph first claimed**, measured at `9616085b`: `UncertifiedCrossLaneRead` **exists only in
these notes** (nothing in `crates/` names it), and the code's `PlanContradicted` means something
narrower than "the three facts refute a plan": it is *the plan's `space` against the slice's declared
`space`* (`crates/lichen-kernel-ir/src/plan.rs`). So the fact-checking half is unwritten, and it stays
that way until a shape needs a staging to check. The two that do exist:
`StagingNotOnThisBackend` (a target that cannot honour a space refuses it **by
name** — the host path refuses any staged body by the memory note's §7 decision,
and the GPU target refuses a `Register`-placed slice — so a plan that
needs either learns it and which),
and `StagingWidthNotAWidth` (the divisibility lattice against a device's
lane-group width). Naming them is this note's; **implementing each is the
mechanism's**, and [shared-memory-and-barriers](shared-memory-and-barriers.md) §9
records which of its steps carries which.

**And this is where the non-chain region finally bites.** §4 refuses a region
that is not a producer→code→consumer chain, so a fan-out body — the shape whose
staging this subsection is about — is not fusable today at all
([compute-graph-jit](compute-graph-jit.md)'s missing general builder). The
interface is written now, ahead of that, because a pack that steers a region
must hand over a *plan* with it, and the engine owes a stated contract to check
the plan against rather than a silent acceptance.

**The mechanism is not this note's to build**, and the implementation is
delegated: the wrapper, the spine, the scan, the barrier instruction and the
staging table are [shared-memory-and-barriers](shared-memory-and-barriers.md)'s
staging (§9), and the perspective work they ride on is
[attributes](attributes.md)'s.

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
   The template is substituted **after** the inverse is taken: the inverse is the
   parameter solved at the step's own `r_N`, which is what the template is written
   at, and substituting the producer first would solve at the next value instead —
   a different repair function.
   Inversion is symbolic rewriting on the fragment's expression tree: each
   operator either names its inverse (`exp`/`ln`, `+c`/`−c`, `*c`/`÷c` for
   `c ≠ 0`) or the rewrite fails. **The symbolic `ln` is the only `ln` that
   exists**: the kernel vocabulary has `exp`, `sqrt` and `floor` as prefix
   operators but no `ln`, because its range reduction is a loop and the IR's
   definitions are one-block straight-line
   ([floating-point](floating-point.md) §7). Nothing about this rewrite changes
   with that: it is over the engine's own expression trees, and the bridge from a
   kernel fragment refuses a projection through `sqrt` or `floor` by name — after
   its expansion, a body that held one holds only primitives, so a marked
   projection through either is refused as an unreadable shape rather than read as
   something else ([operators](operators.md) §11).

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
Two of those three are a **memory plan** (§5): the rolling state `(m, s, acc)` is
register residency and the tiling is what goes to shared memory, so a pack
replaces the engine's conservative placement with the paper's. The division of
labour this design predicts, stated as the test it is judged by: **the
correctness of fused attention comes from the engine, through whatever custom
logic the program carries; the last factor of speed comes from a pack; neither
is written into the logic, and neither ships with it.**

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

**The seam landed** (`d3f984f`): the rewrite is a **graph pass** a host installs
(`crates/lichen-compute/src/compute/graph_pass.rs`), consulted **once per run, immediately before the
dispatcher**, on the graph that will run, with its lines going to the run report's own channel so a
refusal is visible rather than swallowed. `lichen-compute` names no fusion crate: the host that reads
a claim installs the pass (`crates/lichen-language/src/automatic_tiling.rs`), which is the direction
the bridge's manifest already stated. Two properties are the seam's own:

- **A rewrite takes an entry of its own.** The graph store is keyed by a digest of its nodes' bodies,
  so writing the pass's answer over the recorded entry would leave the index naming bytes that are no
  longer there — and a later run with no pass installed would execute the rewrite. The rewritten
  graph is interned under **its own** digest instead: the recording stays what the program wrote, and
  clearing the pass makes the program's own body answer again.
- **A declined rewrite changes nothing.** A pass decides before it lands — a node whose launch the
  staging cannot serve keeps the body the program wrote rather than a half-built one — and the land
  itself goes through the artifact reader, which runs both IR gates and `check_plan` before anything
  is replaced.
- **A node the pass may not read is left alone.** The automatic floor derives a node's algebra from its
  own marks (`ReadRoles::DATUM_ONLY`), and that is only *the node's* algebra when the node reads
  nothing another node produced. So the tiling pass refuses on two rules, both named, neither a guess:
  a claim declaring a role outside `combine`/`identity` is refused from the **claim shape** alone,
  before any graph exists (`BeyondANode`), and a node any of whose `kernel.inputs` is a value another
  node produced is refused against the **graph edge** (`beyond_a_node` — a graph numbers its arguments
  from zero and its node outputs after them, so that comparison is what separates its own datum from a
  producer's). Both were measured as live wrong answers before they were refused: an `element`-role
  consumer landed a body whose reads were `[0]` where the recording read `[0, 1]`, and a
  `combine`-only consumer answered `[12.0, …]` where the program's own answer is `[24.0, …]`.
  **The rules are narrower than the region's own notion of an edge in one direction worth naming**: a
  node whose **extent** is another node's output is a consumer to the region derivation
  (`feeds`/`extent_of`) and is not examined here, because a count is not a buffer the algebra's datum
  read maps to `Element::Buffer`.

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
| the **memory plan** interface | **the carrier, the check, the landing and the match landed; the fact checks and `sync` remain** | **Landed** at `77fafaf`: a plan is data (`lichen_kernel_ir::{Plan, Staging}`) and `KernelFragment::check_plan` refuses one the fragment refutes rather than believing it — a name no mark published (`UnknownStagedValue`), a name two values carry (`AmbiguousStagedValue`), a value that is not a staging (`NotAStagedValue`), a group width the target has not (`StagingWidthNotAWidth`), and a space the slice does not declare (`PlanContradicted`). A tiled fold publishes the value it staged under the **logic's own mark** (`Tiled::staged`, `Tiled::marks`), and `crates/lichen-language/tests/plan_staging.rs` drives a declared plan on a device. The **artifact that carries a fused kernel and its plan** landed at `fb0574c`, and the **landing** followed: `land_fused_kernel` reads the text (so every check runs before anything is replaced) and puts the fragment into the **recorded graph node the runner reads**, refusing by name a shape that would renumber the graph's values or mis-bind its edges. **Matching landed** at `e302ade`: a **placement pack** (`crates/lichen-language/src/placement_pack.rs`, text through `facet`) is bound to a **node schema**'s hash with explicit constraints, its plan's space and group width steer the tiling, the plan rides the artifact so `check_plan` verifies the fragment honours it, and a miss is inert — the engine's own choice stays the floor. What remains: the plan checked against the **three facts** (`PlanContradicted` as the facts refute a plan, `UncertifiedCrossLaneRead`), its `sync`, and the pack **installation path** (a host hands packs in as data today). The interface is §5; [shared-memory-and-barriers](shared-memory-and-barriers.md) §9 step 7 records the rest |
| pack install path in the package manager | **missing** | the runner-side channel of §5 |
| **an integer** reduction, at any count | **missing** | §3's machine-integer wall: the symbolic language is over the reals, so the bridge refuses the `Int` class outright — a runtime-count integer fold publishes **both** marks and the class refuses both. The narrow repair (bridge `Max`/`Min`/exact constants) cannot reach a sum, which is the case one wants; the honest one is an integer class in `Expr` |
| the engine's **fusion on request** — a directive that says *these operations are one kernel* | **a merged region runs, a pack can ask for one, and the general shapes are what remain** | §5 measured at `ebae69bc`: **no code path in `crates/` fused two kernels** — `derive_regions` had no caller outside an example, `lichen_fusion::derive` has no `KernelBody` emitter, and the only transform in production was the intra-kernel tiling of one reduction. **Both halves then landed**: the body composition at `027447ab` (`lichen_kernel_ir::compose`, its four decisions as named refusals) and its caller at `98761ae4` (`region_merge.rs`, a graph pass that derives the region, checks, composes and rebuilds the graph — a device counterfactual shows the composed node is what dispatched). What remains: the **general index-local shapes** (the accepted check is parameter-only, so a computed address — a write at `i + 1` read at `i + 1` — is still refused, and read-site recomputation is unwritten), an elementwise consumer that reads **only** the absorbed buffer (`PortsDoNotCorrespond`), and the merged node's **anonymity** (unregistered, so no later digest-keyed pass can name it) |
| a **domain correspondence** surface (`partition`/`claim`) | **missing** | §5: inlining one operation into another is legal only where the two domains line up, and a non-trivial mapping has nowhere to be stated |

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
- **The memory plan is a pack's, and the engine validates it** (§5). A body that is
  not a chain needs staging — registers, shared, or global — and while the
  conservative plan is derivable, the last factor of performance in a real operator
  is a **placement decision only the machine knows**, so the plan rides the pack. It is checked against
  [shared-memory-and-barriers](shared-memory-and-barriers.md) §4's three facts, never
  believed, and it does not enter the schema hash. **The mechanism is another
  session's to implement**; this design fixes only the interface it implements
  against.

Still open, cheapest to settle before the interface syntax is written:

- **How strong is instance verification in v1?** Symbolic-only (refuse what
  cannot be proved — simplest, most conservative, and some honest reductions
  will refuse) versus symbolic-plus-differential (a claim backed by a
  property harness is believed — more coverage, and the harness is one more
  artifact to define). This note is written symbolic-only; §3's
  `UnverifiableInstance` is where a second tier would slot in.
- **The mark payload's richness, and with it mark *paths* — measured, and deferred.** The
  spike's payload is one string literal, because text is exactly what keeps a mark out of the
  pair layout, and the channel is positioned for a record: a mark's payload is a *list* of
  fields, so a reader that wants "the mark's own text" reads the unnamed field rather than a
  second variant. Keying a mark by the **path** of the marks enclosing it, rooted at the
  fragment's interface, is what would let a plan name a value durably — and it is blocked by a
  surface rather than by a design:
  - **No surface produces a named field.** `mark_payload`
    (`crates/lichen-language-parser/src/parse.rs`) takes a string literal and nothing else, the
    AST, the IR and the module carry one `Vec<String>` per mark, `MarkField`'s only producer
    writes `name: None` (`crates/lichen-compute/src/compute/body.rs`), and
    `KernelMark::field(name)` has no caller. So a claim's interface and its roles either gain a
    grammar or are parsed back out of `claim:Name(role,…)` — the mini-language a record payload
    exists to end.
  - **The names a plan may use have to be declared somewhere, and §4 is why.** The canonical
    form commits to the interface and to each required role's canonical projection, and to
    nothing else: not a mark's anchor, not a mark that is not a role. A plan naming one of
    *those* would be matched by a hash that does not fix what the name points at, so two kernels
    with one algebra and that mark on two different expressions would take one plan. A claim's
    declaration is therefore load-bearing rather than a restatement of the interface.
  - The three spellings are a **named mark** (`~ interface "Reduction" ~ role "combine"`), a
    **registry** keyed by interface in the IR, and **nothing declared** at all. Only the first
    keeps the declaration in the program — and so inside the hash — while ending the
    mini-language; only the first two keep `RoleNotMarked` and the undeclared-name refusal
    reachable.
  - **Two families of existing marks are what any of the three has to answer for.** The
    `accumulator` marks (`crates/lichen-language/examples/reduction_interface.rs`,
    `crates/lichen-language/examples/fusion_numbers.rs`) are not roles of `Reduction` and are
    read today as roles no claim requires; and `element` in all four chains of
    `crates/lichen-language/examples/fusion_schema.rs` sits *inside* the `combine` mark, because
    a consumer's per-element function is used inside its own combine, so a path reads
    `Reduction/combine/element` where the interface declares `element`. Resolving a role by the
    mark's own segment rather than by its whole path is the reading that keeps that example as
    written.
  - **A path cannot be completed by ancestry alone.** Every example's claim mark rides the *use
    site* (`compute.parallel (producer_raw ~ "claim:…")`), whose anchor is the function
    template's parameter node rather than an `ExprId` ancestor of the body, so a checker can
    compute only the relative chain among body marks and the root has to be applied where the
    interface is known — the kernel compile, or the lowering that publishes them.

  **Deferred deliberately**, because its only consumer is the plan: §5's functional half — a plan
  that is data, checked against the three facts, and refused by name — is the thing to build
  first.
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
