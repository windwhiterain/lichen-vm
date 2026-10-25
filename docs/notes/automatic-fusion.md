# Automatic DAG fusion: a heuristic strategy space

This note records the survey behind the strategy space and the plan it selects for lichen.
[graph-fusion](graph-fusion.md) is the design of the machinery (claims, regions, packs); this is
the policy that drives it, and the reason the policy is a **strategy space** rather than an
optimizer.

## The position

Automatic fusion is not an optimization problem lichen claims to solve. It is a **bounded
heuristic** whose job is to **narrow the space a human or an agent searches by hand**: the pass
picks a first plan, the **fusion file** moves it, and neither is asked to be optimal. A measured
greedy pass that is editable beats an expensive search that is not, and the literature is explicit
that greedy region formation leaves performance on the table (Jangda & Bondhugula report up to
4.32× over PolyMage's model and 2.46× over Halide's automatic scheduler; Korch reports up to 1.7×
over rule-based compilers) — which is an argument for an editable plan, not against a greedy pass.

## Automatic first, agentic for the rest

The pass pushes every merge **its own derivation can prove**: a region whose interfaces the
bridge composes and whose shape the fold builds is licensed by that evidence, not by a file
entry. The fusion file exists for what an algorithm **cannot see** — an interface that does not
bridge, a budget the plan wants tighter, numerics a merge would change, a strategy the machine
cannot choose. A file entry for a region the pass can prove is a refinement, never a
requirement.

## The families, and the one lichen adopts

Four families were surveyed, in the order of what they buy:

| family | source | what it does | cost |
|---|---|---|---|
| **H1 dominator-gated** | TVM `FuseOps` | grow a region only toward the producer's immediate post-dominator, so a branch that reconverges fuses and a fan-out that diverges stays shared | needs post-dominators (LCA over reverse topological order) |
| **H2 consumer-side greedy** | IREE | walk consumers first; for each, absorb producers by benefit while the budget holds; refuse a producer with several users unless the recomputation is affordable | cheapest; the shape closest to lichen's existing chain fold |
| **H3 hero priority** | XLA:GPU | best-first over candidate regions, each with a "hero" op that shapes codegen | needs a region-level legality check (cycle avoidance) |
| **H4 sibling merge** | MLIR affine `--mode=sibling` | keep the shared producer materialized and fuse its consumers into one kernel that reads it once | the only family that improves a fan-out without duplicating the producer |

**Lichen adopts H2 + H4.** The case it refuses today — a fan-out whose branches continue as
chains (the norm feeding three projections, each followed by a norm) — is what H4 covers: keep
the shared producer materialized and merge its consumers into one kernel that reads the
producer's output once. H1 is the alternative if post-dominators are ever wanted; H3 is not worth
its bookkeeping here. **What H4 does not cover is the chain behind each consumer**: folding a
branch that begins with a reduction is blocked by the reduction's own limit (measured below), so
the pre-attention's key branch — matvec → squares → norm — stays two kernels and the sibling
region holds the two projections alone.

## Signals, and what lichen can measure today

| signal | precedent | lichen today |
|---|---|---|
| bytes saved | Inductor `score_fusion_memory_threshold` | yes: the region's inputs/outputs and the deleted values' sizes |
| dispatch overhead | Inductor `_REINDEXING_FUSION_LAUNCH_OVERHEAD_NS` | yes: the submission count (`Session::work`) |
| recomputation budget | MLIR `--compute-tolerance=0.30` | yes: count the absorbed reads a fan-out would duplicate |
| peak memory / live range | Inductor `fusion_memory_timeline_peak_memory_increase_gb` | no: needs a live-range model over the graph |
| staging / shared memory | MLIR `--fast-mem-capacity`, the pack's plan | the plan states it; the pass checks it (`check_plan`) |
| occupancy / registers / tiles | Triton `Config(num_warps, num_stages, maxnreg)` | no: the backend does not report them |
| precision | Inductor `keep_addmm_fused_for_half_dtypes`, XLA numerics verification | the pack's class constraints; the device counterfactual |

The first four are enough for a first pass; the last three are deliberately out of scope until
the backend reports them.

## The fusion file

The closest precedent is XLA's `--xla_force_config` / `--xla_candidate_configs_file`: a file that
**overrides the automatic pass** for named instructions. Lichen's file is the same shape, with
the region as the named unit:

```json
{
  "strategy": "sibling",
  "budget": {
    "max_region_nodes": 32,
    "max_region_depth": 8,
    "compute_tolerance": 0.30,
    "staging_bytes": 49152,
    "passes": 2
  },
  "refuse": {
    "shared_producer_duplication": false,
    "opaque_is_barrier": true
  },
  "patterns": [
    { "name": "norm_projections", "nodes": ["scale", "matvec"], "priority": 10 }
  ],
  "overrides": [
    { "region": "sha256:5b73…", "action": "force_merge", "reason": "measured 1.4x" },
    { "region": "sha256:02bd…", "action": "barrier", "reason": "numerics" }
  ]
}
```

The same file is what the pass **emits** (with the plan filled in), so a human or an agent edits
a plan rather than a bag of knobs. The file's overrides are installed as **packs**: the region
hash is the pack's `schema`, the strategy names what the pass would do, and the existing
`fuse`/`class` directives carry the rest. The rule that must not move: **a pack decides which
regions are attempted, never what a merge must satisfy** ([graph-fusion](graph-fusion.md) §5).

## Pitfalls the pass must respect

- **Do not duplicate a shared producer into every consumer.** XLA's emitters generated a
  multi-user node twice and the code size grew exponentially with the number of splits; IREE
  refuses a producer with more than one use unless cloning is cheap. The existing
  `compose_fan_out` *does* copy the producer's cone per consumer, which is why H4 (sibling
  merge) is the shape to prefer over widening the fan-out that duplicates.
- **Bound recomputation with a number**, not an argument: MLIR's `--compute-tolerance` is the
  precedent; the budget above uses it.
- **Bound live range and peak memory** before it is measured: a merge that keeps a value alive
  for the whole region can raise the graph's peak. Inductor refused a fusion over exactly this.
- **Keep the boundary values the region does not consume**: a deleted value read outside the
  region is the existing `ProducedValueIsReadElsewhere` refusal, and the pass must keep refusing
  it.
- **Precision changes are real**: a merge can remove a materialization that rounded, so the
  region's classes and the device counterfactual are the two checks, never an assumption.

## Plan for lichen, in order

1. **Membership** — a claimed consumer may join a claimed producer across an unclaimed node;
   claim the elementwise kernel (the norm's scale) so a norm chain is complete. (In flight.)
2. **Shape** — the topological fold over the region's DAG: each node composed into the fragment
   its absorbed inputs built, in evaluation order; chain, fan-out and fan-in become what a fold
   of one or two steps produces. The child shapes the fold composes with:
   `compose`, `compose_fan_out`, `compose_fan_in`.
3. **Policy** — H2 + H4 with the budget above, the plan emitted as the fusion file, overrides
   installed as packs.
4. **Verification** — `composed_probe` for the region lines and the device counterfactual;
   the resident token (`resident_token`) for the dispatch count and wall time; the serve suite
   and the comments/fmt checks for the tree.

## What landed

The first slice of the strategy space is a **file the runner installs**, a **plan it answers**
and a **gate the merge pass reads**. None of it decides what a merge must satisfy: the gate
declines a region, and it never accepts one.

### The file

`crates/lichen-runtime/src/fusion_file.rs` reads the manifest with `facet_json`, and
`read_fusion_file` is its only constructor. The schema it accepts is the one this note sketches:

| field | what it says | what spends it |
|---|---|---|
| `strategy` | the family the pass runs under | `sibling` derives the sibling regions; `component` folds the whole component into one body |
| `budget.max_region_nodes` | the most nodes a region may hold and still be attempted | the gate |
| `budget.max_region_depth` | the deepest a region's own DAG may be | the gate |
| `budget.compute_tolerance` | the recomputation the plan tolerates | carried |
| `budget.staging_bytes` | the staging the plan allows | carried |
| `budget.passes` | how many merge passes the plan asks for | carried |
| `refuse.shared_producer_duplication` | whether a shared producer may be duplicated | carried |
| `refuse.opaque_is_barrier` | whether an unproved interface bars the merge | carried |
| `patterns` | the nodes a human matched by hand | carried |
| `overrides` | one action per region | the plan |

**A field the file omits takes the floor's own answer**: a budget field left out bounds
nothing, and the refusals the engine already makes stand whatever the file says. The refusals
are named and quote the field they met (`UnknownStrategy`, `BadRegionHash`,
`RegionIsOverriddenTwice`, `PackRefused`), in the pack's own style.

### The plan

`FusionFile::plan` answers a `FusionPlan { strategy, budget, overrides, barriers }`. A
**force-merge** override becomes a `fuse` pack naming the region and asking for `one_kernel`,
read with the pack's own reader (`read_placement_pack`), so a pack the file spells is a pack
the engine already knows. A **barrier** names the region and asks for no merge — the pack
vocabulary cannot spell that, because `fuse` is the only directive that names a region and it
asks for a merge — so the plan carries the barred regions beside the packs.

### The emitted file

`emit_plan` is the other direction: a recorded report and a `FusionPlan` answer a **file text**,
and the text is the schema `read_fusion_file` parses — so `read_fusion_file(emit_plan(report,
plan))` reads back. The strategy and the budget are the plan's own, and every override is one
region line the report holds: a region that **merged** becomes a `force_merge`, a region that was
**refused** becomes a `barrier` with the check's own name as its reason, and a region line that
names no `sha256:` hash is left out. The parse is deliberately conservative — only `region N …`
lines are read, and only the two facts such a line carries — so a report the emitter cannot read
yields a file that states less, never a file that states something else. `fusion_file_probe`
prints the file and reads it back, and `fusion_file_emit` prints the report it came from.

### The gate

The merge pass reads the plan's policy before any legality check: a barred region is refused
by name (`RegionIsBarrier`), and a region over the budget is left alone by name
(`RegionOverBudget`, carrying the region's own node count and depth beside the budget's two
bounds). The strategy a plan named is reported once, as `strategy "sibling"` or
`strategy "component"`; a pass no plan installed names no strategy, because the floor's own
answer is the one it runs under.

The pre-attention region is **9 nodes and 5 deep** (measured by `fusion_file_probe`), which is
why the sketch budget above admits it; the sibling region beside it is **2 nodes and 1 deep**.

**The rule that did not move**: the gate may only *decline*. A region the gate lets through is
still refused by every legality check that stands under it — the probe shows the sibling region
refused by `RegionOverBudget` under a tighter budget of the same plan, and barred by name under
the plan's barrier.

### The scalar union

The composition now admits a producer whose domain declares runtime scalars beside the extent and
the index. **The merged parameter list is the union of the bodies' own scalar leaves**: the
extent, then each body's runtime scalars in the order the bodies are composed, then the index —
and **each body binds its own leaves**, so a body never reads another body's number. The merged
graph node binds the same union, in the merged fragment's own order, so the launch supplies every
body's own numbers.

The refusals are named: `ShapesDiffer` when the two bodies cannot share one parameter list (a
domain that names no extent and index, or a body whose entry block does not receive its own
domain's leaves), `ParameterCountsDiffer` for the same fact on a consumer, and
`ScalarArityDiffers` when a node binds another number of runtime scalars than its fragment
declares.

**The ordinary region's pre-attention still does not merge, for a different reason.** It is now
refused by the licence's own extent condition, `ExtentsDiffer`: the composite dispatches over 2048
lanes and over 1024, and one kernel dispatches over one extent. (Before the licence was widened it
refused at `NotAReductionChain`, the shape's own name, and the composition's
`ForwardedValueIsLaneDependent` was never reached.) That limit is the reduction's own, not the
domains'.

### The sibling merge (H4)

The `sibling` strategy derives a second shape of region: the **sibling region**. Where the
ordinary rule groups the claimed nodes into components, the sibling rule looks inside a component
for a **shared producer** — a claimed node with several claimed consumers — and makes those
consumers one region. The producer keeps a region of its own, so it stays **materialized** and
its value is read once by the merged kernel instead of being copied into every consumer.

**The region is the consumers themselves**, as MLIR's `--mode=sibling` spells it, and **the
consumers are grouped by the extent they dispatch over**, because one kernel dispatches over one
extent: the pre-attention composite's three consumers are two over 1024 lanes and one over 2048,
so the sibling region is the two that can share a kernel and the one that cannot keeps the
ordinary rule.

The fold is two steps: **each consumer composes on its own** (the chain fold, H2), and the
consumer fragments **merge into one fragment** (H4). The second step needed a new primitive:
`compose_siblings` in `lichen-kernel-ir`. Neither existing primitive can say it —
`compose_fan_out` copies the producer's cone into every consumer (the duplication the survey
warns about) and `compose_fan_in` chains the bodies, so each reads the one before it. The sibling
merge wants **independent bodies that share one input**: no branch reads another branch's output,
and the value they share is one port of the merged fragment however many of them read it.

The primitive is small and sound under the existing invariants: the branches run in the list's
order, the merged inputs are the branches' own ports less the shared one, the merged outputs are
the branches' sinks, the merged parameter list is the union of the branches' scalar leaves, and
each body binds its own leaves. The refusals are named: `NoBranchToInline`,
`OneBranchIsNotASiblingSet`, and the shared input's own (`AbsorbedIsOutOfRange`,
`AbsorbedIsNotRead`).

**The merge of a sibling region stands at the region's first position**, not at its last: a
sibling region's sinks are read by nodes that may sit inside the region's own span (the pre-
attention's rope reads the key projection's output), so the merged node stands before them. Two
refusals guard that placement — `InputIsProducedAfterTheMerge` and `OutputIsReadBeforeTheMerge`.

**The licence still applies**: a sibling region is a fan-out of the shared producer, and the
floor's own proof holds it because the shared producer stays **outside** the region — its value is
read at one port, and the region's own branches are shapes the fold builds. A pack whose `fuse`
names the region's schema is what licenses it when the classes have moved its hash.

**What the pre-attention composite shows.** The sibling strategy derives five regions: the sum of
squares alone, the norm alone (the shared producer), the **sibling region** (the key projection
and the value projection — the two consumers over 1024 lanes), and the two chains beside it (the
key branch's squares → norm, and the whole query chain). The sibling region's schema is
`sha256:820a9163de05d5657e7e011c994c6cfb8812f71144850b41de8e9e180f1cd41f`, the fusion file's
override names it, and **the merge fires**: the pre-attention graph goes from **11 nodes to 10**,
one kernel reading the norm's output once in place of two.

**Extending the region over the consumers' own chains is blocked, and the block is measured.**
A sibling region that holds the consumers *and their descendant chains* — the key projection
chain (2,3,4) and the value projection (6) — derives and is licensed, but the merge then
refuses at `ForwardedValueIsLaneDependent`: the branch chain is a matvec feeding a squares
feeding a norm, and the matvec's accumulator is state a loop carried forward, so the chain cannot
be rebuilt at the norm's address. **That is the reduction's own limit, not the domains'** — the
same limit the pre-attention region has always had, now inside a branch instead of at the shared
producer. H4 moves the shared producer out of the merge; it does not lift the reduction's limit
behind a consumer.

### The default derivation is the sibling one

**The floor's own answer is the sibling derivation, and no file is needed for it.** The merge
pass with no plan installed derives its regions with `sibling_regions`: a shared producer whose
consumers share an extent becomes a **sibling region**, and the ordinary component rule holds
everywhere else. The file's `strategy` names the derivation instead: `sibling` asks for the
floor's own answer, and **`component` asks for the plain component derivation** — the whole
claimed component as one region, in which a producer two consumers read has its cone **copied
into each of them**, the duplication the survey warns about. The name the component derivation
had before — `greedy_sibling` — still reads, and still means the component derivation, so a file
written before the rename answers the plan it answered then.

**Measured** (`crates/lichen-serve/examples/fuse_probe.rs`, the pre-attention composite, the class
pack alone and no file at all):

| | before | now |
|---|---|---|
| the class pack alone | 11 nodes, the ordinary region refused by `ExtentsDiffer` | **10 nodes**, the sibling region merged under the floor (the ordinary rule now cuts that region at its extent changes, so the class pack alone reports three regions) |
| the class pack and the fuse pack | 11 nodes, the fuse pack naming no region | 10 nodes, the same merge, the pack still inert |

The region lines are the sibling one: `region 2: nodes 6, 2 merged into one node at 2 (the
survivor), the branches of node 1 reading its value at one port, licensed by the floor: every
node of the region claims Reduction and nothing else`.

**The ordinary fan-out is no longer a region under the default.** The three-node diamond the
compute suite records (`fan_out_merge`) is now two regions — the producer alone, refused by
`RegionIsNotAPair`, and the **sibling region** holding the two consumers — so the graph answers
**2 nodes instead of 1**, with the producer still materialized. A pack that names the old
three-node region's hash names no region at all, and the merge is licensed by the floor instead.
**That is the point**: the duplicated cone is gone, and what is left reads the producer's value
once.

**What the sibling derivation cannot split.** A fan-out whose consumers do **not** share an
extent is not a sibling set: one kernel dispatches over one extent, so the consumers are grouped
by the extent they dispatch over and a group of one keeps the ordinary rule. And the branches
are the **direct consumers** only — a consumer's own chain is not pulled into the sibling region,
because the reduction's own limit refuses it there (`ForwardedValueIsLaneDependent`, above).

### The class pass, and the classes a merged node inherits

**The class application is a pass of its own, and it runs first.** It was in the tiling pass,
which runs after the merge — so a merged node's ports never took the classes its branches were
given, and the fused step read the bf16 residents as floats. The pass is
`crates/lichen-runtime/src/class_pass.rs` (the compute-side `crates/lichen-compute/src/class_pass.rs`
is the language wrapper), and the floor's chain is now **[the classes, then the region merge, then
the tiling]**. A caller with no class pack sees no change: the pass lands nothing and answers no
graph.

**The landed kernel is registered under the claim it was compiled for.** A node's fragment is the
only thing a graph holds — no kernel id — so a class that changes it is a *new* kernel, and the
merge's join (the registry) would not find the node without it. The class pass interns the landed
fragment with the node's own claim, so the merge composes the branches it was given and the merged
fragment inherits their classes. A pack never answers a legality check: the class pass rewrites
classes by name and the merge's checks run after, as they always did.

**Measured** (`crates/lichen-serve/examples/fuse_class_probe.rs`, the pre-attention composite, the
class pack naming nodes 2, 6, 7 at slot 1, the sibling strategy):

- the merged node at position 2 reads **`[Float, BFloat16, BFloat16]`** — the norm's output as a
  float and **both** weight buffers as brain floats, which is what the branches were given;
- the fused step runs and its **three answers equal** the same step prepared without the merge
  (1024, 1024 and 2048 elements), and both differ from the same step with no class pack at all —
  the brain-float truncation of the weights is visible in the numbers.

**The unmerged leg of that probe asks for the `component` strategy**, because the floor's own
answer is now the sibling one: with no file at all the step merges, so the step the merged one is
compared against is the one the plain component derivation leaves alone.

**The wall this slice met was the symbolic bridge, and it is now past.** A class pack narrows a
buffer, so the branch's body reads a `BFloat16` — and the bridge refused a brain-float read by name
(`UnsupportedClass`), so the region's schema could not be derived and every region holding a
class-landed node was refused (`ProjectionNotBridged`). The bridge now reads a brain-float buffer as
the real it holds, which is what the emitter has always done with one (it widens on the way in and
narrows on the way out, so no body holds a half). **A bf16 *computation* is still refused** — SPIR-V
has no brain-float arithmetic, and the emitter refuses it too.

**A class pack moves the region's hash, because the region's ports' classes are in its canonical
form.** The pre-attention's sibling region is `sha256:820a91…` before the classes land and
`sha256:ff1d97…` after them, so a `fuse` pack that named the region names the *pre-class* region
and no longer names it. The merge still fires — under the floor's own proof, which a sibling region
holds — and the report says so. **The floor's licence line names no hash** (its wording is pinned
by the compute tests), so the emitted file no longer carries that region's override: a human who
wants it named has to learn the hash another way.

**The module-scoped join does not see a class-landed kernel.** The floor installed on a module
that has already run joins the graph's nodes against the kernels *that module names*, and a landed
kernel is one the module does not name. The registry-scoped paths — the compiler's `--pack` and the
serve's `install_floor_for_run` — read the registry and do see it.

### The widened floor licence

**The floor's licence is the proof the engine makes of the region, and it no longer asks for a
reduction chain.** It asks three things, each with its own refusal:

| the licence's condition | what it decides | the refusal when it does not hold |
|---|---|---|
| every node of the region claims an interface the bridge composes — `Reduction` or `Elementwise` today | the algebra the merge reads | `InterfacesAreNotProved` |
| the region's shape is one the fold builds — a chain, a sibling, a fan-out or a fan-in | the composition the merge makes | `NotAReductionChain` |
| the region dispatches over one extent | the launch the merged node makes | `ExtentsDiffer` |

**The four shapes are the four compositions** `lichen-kernel-ir` has: `compose` (a chain),
`compose_siblings` (a sibling), `compose_fan_out` (a fan-out) and `compose_fan_in` (a fan-in). A
region whose edges are none of them is refused by the shape's own name; a region whose edges are
one of them is the floor's to merge.

**The pack's door is unchanged.** A pack whose `fuse` names the region's schema still licenses what
the floor cannot prove, and every legality check still stands under it: a pack decides which
regions are attempted, never what a merge must satisfy.

**Measured** (`crates/lichen-serve/examples/fusion_file_probe.rs`, the pre-attention composite, the
sibling strategy, the plan of `examples/fusion_file.json`):

| region | before | now |
|---|---|---|
| 0 (node 0) | `RegionIsNotAPair` | `RegionIsNotAPair` |
| 1 (node 1) | `InterfacesAreNotProved` | `RegionIsNotAPair` — the licence holds, and the arity check answers |
| 2 (nodes 6, 2) | merged, licensed by the floor | merged, licensed by the floor |
| 3 (nodes 3, 4) — the knorm's squares and scale | `InterfacesAreNotProved` | **licensed by the floor**, and refused by the merge at `StagedReadIsNotOrdered` — the merged node keeps the producer's store, and its 1 024 lanes are 16 groups of 64, so the barrier that separates them reaches no read |
| 4 (nodes 7, 8, 9) — the whole query chain | `InterfacesAreNotProved` | **licensed by the floor**, and refused by the composition at `MergedBodyIsRefused` — the staged fan-in's barrier would sit in the reduction's own loop |

**The licence widened; the reduction's own limit did not.** The two chains the licence now holds
are led by a reduction whose fold is **lane-dependent**: the `squares` reduction sums one row of
the hidden state, and the `scale` that reads it asks for the row of *another* lane — so the
producer's cone cannot be rebuilt at the read's address. The store-keeping path is the answer to
that limit, and it has **two** of its own: the merged node must be narrower than one lane group
(`StagedReadIsNotOrdered`), and the staged read must sit in control flow every lane reaches
(`MergedBodyIsRefused` — the qnorm fan-in's barrier would sit in the reduction's own loop). The
path and its measurements are in [graph-fusion](graph-fusion.md) §5; the fan-in's own store-keeping
composition is `compose_fan_in_staged`, beside the pinned `compose_fan_in`. **The node count is
unchanged** (11 → 10): the sibling region was the only merge the pre-attention had before, and it
still is.

**An elementwise chain now merges under the floor, and the floor's own line says so.** The
`lichen-compute` elementwise chain (`[elementwise, elementwise]`) and the `[elementwise,
reduction]` chain both merge with no pack installed, and the report reads `licensed by the floor:
every node of the region claims Elementwise and nothing else`. The line names the **interfaces the
region's own nodes claim**, so a region's licence is readable from the report alone.

**A fan-out is licensed now.** A producer two consumers read was refused by the shape's own name
(`NotAReductionChain`); it is one of the four shapes the fold builds, so the floor holds it. The
`serving-performance-boundary` note's ordinary pre-attention region is refused by the **extent**
instead, because the composite dispatches over 2048 lanes and over 1024.


- **The file is emitted, but not the regions a floor licence merged.** `emit_plan` writes the overrides
  the report names by hash; a region merged under the floor's own proof leaves a line with no hash
  (its wording is pinned), so a human adds that override by naming the post-class hash. **A region
  refused by a check other than the licence's names no hash either** — `RegionIsNotAPair` and
  `ForwardedValueIsLaneDependent` carry none — so the pre-attention's emitted file now carries **no**
  override at all. Naming a refused region by hash is the emitter's own business.
- **`passes` is parsed and the floor runs one merge.** The chain is one class pass, one region merge
  and one tiling pass, and the plan's pass count is carried, not spent.
- **`compute_tolerance` and `staging_bytes` are carried.** No recomputation is counted and no
  staging is checked against the budget.
- **The strategy steers the regions' shape and nothing else.** `sibling` derives the sibling
  regions beside the ordinary ones, and it is the floor's own answer; `component` asks for the
  plain component derivation, in which a shared producer's cone is copied into each consumer.
  The regions are still attempted in the graph's own order.
- **The file's `refuse` booleans are carried.** The engine's own refusals — `NotAReductionChain`
  for a shape the fold does not build, `InterfacesAreNotProved` for an interface the bridge does not
  compose — stand whatever the file says.
- **`patterns` are carried.** Nothing consults them yet.

## References

- TVM operator fusion (`FuseOps`, post-dominators): <https://github.com/apache/tvm/blob/main/docs/arch/fusion.rst>
- IREE `DispatchCreation` (`hasOneUse`, sibling/producer cloning, bounded iterations): <https://github.com/iree-org/iree/blob/main/compiler/src/iree/compiler/DispatchCreation/Passes.td>
- MLIR affine fusion (`--mode`, `--compute-tolerance`): <https://github.com/llvm/llvm-project/blob/main/mlir/include/mlir/Dialect/Affine/Transforms/Passes.td>
- XLA `DebugOptions` (force config, fusion flags): <https://github.com/openxla/xla/blob/main/xla/xla.proto>
- TorchInductor config and scheduler: <https://github.com/pytorch/pytorch/blob/main/torch/_inductor/config.py>
- Triton autotuner (config space, pruning): <https://github.com/triton-lang/triton/blob/main/python/triton/runtime/autotuner.py>
- Korch (graph partitioning + BLP): <https://arxiv.org/abs/2406.09465>
- DNNFusion (operator classification): <https://arxiv.org/abs/2108.13342>
- Jangda & Bondhugula (DP fusion groups): <https://doi.org/10.1145/3404846>
