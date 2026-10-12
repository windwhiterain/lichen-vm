# LLM inference operators, and what the machinery says about them

> Status: **current** — one operator landed, and the friction it produced is the roadmap.
> Points at: `examples/rmsnorm.lichen` (landed at `419aab56`), `crates/lichen-runtime`
> (the passes, the claim schema, `automatic_tiling.rs`), `crates/lichen-fusion-bridge/src/lower.rs`
> (what a marked body can bridge), `crates/lichen-compute/src/compute/graph.rs` (the graph path).
> The machinery itself is [graph-fusion](graph-fusion.md) §5; the plan/pack story is there too.

**Writing an operator is the sharpest test of the machinery**, so each operator is committed as an
`examples/*.lichen` program — an example *is* a test here (it declares its own `output = "…"` and
`crates/lichen-compute/tests/examples.rs` runs it), which keeps the operator verifiable without a new
test file.

## The ladder, as measured rather than hoped

| step | state |
|---|---|
| **RMSNorm** | **landed** (`419aab56`): a row-wise `Σ x²` reduction plus the rescale that reads it |
| **SiLU / MLP** | **ready** — `claim:Elementwise(element)` over `exp` and `int2float`, no reduction and no edge. `exp` bridges (there is an `inlined_exp_operand` reader), but it must stay outside the marked expression, and a consumer on a reduction's tail still needs the reduction role (below) |
| **softmax** | **two walls**: the hazard below, and `exp` *inside* a marked `element`, which is the same invisible-transcendental problem as `sqrt` |
| **RoPE** | **language wall first**: `cos`/`sin` do not exist in the language at all |
| **matmul** | **not close**: nothing expresses a two-dimensional launch — `compute.range` is one dimension and a parallel fragment's shape is `(config, index)` |
| **attention** | behind both the softmax and the matmul walls |

## What RMSNorm measured

Its two kernels claim a reduction and its consumer, and the floor is **willing**: varying only the
denominator, `y = x / s * w` merges **with no pack** —

```
region 0: nodes 0 and 1 merged into one node at 1 (the survivor), absorbing input position 2 of
node 1, licensed by the floor: every node of the region claims Reduction and nothing else; the
merged fragment is unregistered, so it carries no claim and a later digest-keyed pass cannot name it
```

— while `s / int2float k.n` is refused (`Conversion`) and `sqrt (s / int2float k.n + 0.25)` is refused
(`Select`). **Two ordinary pieces of RMSNorm arithmetic stand between the operator and its own
fusion**, and neither is negotiable in the current surface:

- **`sqrt` is not `sqrt` to the bridge.** `KernelInstr::Sqrt` is replaced by its IR definition before a
  kernel is registered (`KernelBody::expand_transcendentals`, `lichen-kernel-ir/src/body.rs`, inlining
  `sqrt_body` — a Newton iteration full of `Select`s). The bridge reads the inlined **`exp`**
  (`unary.rs`'s `inlined_exp_operand`, used in `fusion-bridge/src/lower.rs`) and has **no `sqrt`
  reader**, although `lichen_fusion::UnaryOp::Sqrt` exists and `normalize` folds it. The walk meets the
  expansion's first `Select` and reports `BridgeRefusal::Select`. **An operator author must know that
  `sqrt` is not `sqrt` here, and nothing in the surface says so.** The fix mirrors the `exp` reader.
- **`int2float k.n` refuses the region** (`BridgeRefusal::Conversion`: "the symbolic language is one
  arithmetic class"). A row reduction divides by its extent and the extent is an `Int`; the symbol
  `Float(4.0)` already exists, so admitting `Conv{Int→Float}` for a scalar source is the fix.

## The hazard: the tiling pass can silently replace the operator's algebra

**Measured, not theorised, and it is a wrong-answer class.** `SchemaNode::reduction()` keeps only the
**top-level operator** of the `combine` projection (`fusion_claim/schema.rs`), and `Folding` rebuilds
the fold from that one operator over the **raw buffer** (`automatic_tiling.rs`). The `x²` of `Σ x²`
lives in the `element` role, which `NODE_LOCAL_ROLES` excludes (`automatic_tiling.rs`). So the two
encodings both lose:

- declare `element` — the honest claim — and the pass **refuses** (`BeyondANode`): safe;
- omit `element` — and the pass **tiles**, trusting an algebra that is a strict weakening of the
  body's: the answer changes.

Same program, same device, same data (`x = [1…64]`), only the installed floor differing:

| | `y[0]` on the device | the sum it used |
|---|---|---|
| no pass installed | `0.026747627` | `Σ x² = 89440` |
| floor installed | `0.1747408` | `Σ x = 2080` |

Both reproduce bit-for-bit in `f32` from the two closed forms, and the pass **said so itself** while
the run still succeeded: *"node 0: no pack matched its schema …, so the automatic floor tiled: its own
marks declare `+` over 64 lane(s)"*. **There is currently no claim an author can write for `Σ x²` that
is both truthful and safe.** Two ways out, and they are a real choice: carry the datum function into
`Folding` (which already has an `Element` enum), or refuse to tile a `combine` whose canonical form is
not exactly `Add(acc, datum)` / `Mul(acc, datum)`.

## Smaller findings, each with the thing that would change

- **A graph cannot run on the host at all** (`compute/graph.rs`: "this graph was compiled for the
  \"cpu\" backend, and a graph runs through the `ParallelBackend` contract — which the cpu path is
  not"), and the only `ParallelBackend` in the workspace is the GPU one. So `compute.graph`/`graphrun`
  — **the only path that consults the floor** — is device-only, and **no example can exercise the
  fusion machinery**, which is what these operators exist to probe. The floor's lines are reachable
  only from a host program with a device.
- **The compiler CLI installs no backend**, so every pass line it can print with `--pack` comes from a
  path it cannot execute.
- **`Space::Warp` has never been seen from the surface**: every slice in every measurement was
  `space: Shared`, and the `Shared` was the floor's own choice (`Decision::Unmatched` takes
  `(Space::Shared, Plan::default(), Vec::new())`), not a plan's. The warp exchange is reachable from a
  hand-built fragment, not (yet) from lichen.
- **A merged node never lands anything on a device**: the merge reports its fragment *"unregistered"*,
  so the tiling pass refuses it (`node 0: refused: no claim is registered for the fragment at 0`). Only
  a **standalone** reduction reaches a staged body today — and the hazard above governs whether that
  one is correct.
- **The `element` role is a `Reduction` role**, so a consumer that is not a reduction must claim
  `Reduction(element)` — "a small lie the surface forces" — or bring a pack for the floor's licence.
- **A graph of exactly one claimed kernel cannot run with the floor installed**: `RegionIsNotAPair`
  ("the region holds 1 instance(s)") comes back as a run **error**, not a stage line.
- **The kernel registry is process-wide and append-only**, so a long-lived host that runs one program
  twice under different claims meets `AmbiguousClaim` in the region merge — while the tiling pass
  deliberately collapses the same collision by digest. The merge refuses where the tiling pass picks.
- **`eps` and `n` cannot be kernel parameters on the graph path**, so RMSNorm's `eps` is a literal in
  the body: correct for this example and wrong for anything else.
- **`BeyondANode`'s message claims a fact it never inspected** ("whose code reads the value another node
  produced") about a node that has no producer. The check fires on the role *name* alone.
