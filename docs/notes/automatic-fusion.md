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

## The families, and the one lichen adopts

Four families were surveyed, in the order of what they buy:

| family | source | what it does | cost |
|---|---|---|---|
| **H1 dominator-gated** | TVM `FuseOps` | grow a region only toward the producer's immediate post-dominator, so a branch that reconverges fuses and a fan-out that diverges stays shared | needs post-dominators (LCA over reverse topological order) |
| **H2 consumer-side greedy** | IREE | walk consumers first; for each, absorb producers by benefit while the budget holds; refuse a producer with several users unless the recomputation is affordable | cheapest; the shape closest to lichen's existing chain fold |
| **H3 hero priority** | XLA:GPU | best-first over candidate regions, each with a "hero" op that shapes codegen | needs a region-level legality check (cycle avoidance) |
| **H4 sibling merge** | MLIR affine `--mode=sibling` | keep the shared producer materialized and fuse its consumers into one kernel that reads it once | the only family that improves a fan-out without duplicating the producer |

**Lichen adopts H2 + H4.** The case it refuses today — a fan-out whose branches continue as
chains (the norm feeding three projections, each followed by a norm) — is exactly what H2 + H4
cover: fold each branch as a chain (H2), then merge the branch heads as siblings (H4). H1 is the
alternative if post-dominators are ever wanted; H3 is not worth its bookkeeping here.

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
  "strategy": "greedy_sibling",
  "budget": {
    "max_region_nodes": 32,
    "max_region_depth": 4,
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
