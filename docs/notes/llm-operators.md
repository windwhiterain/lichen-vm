# LLM inference operators, and what the machinery says about them

> Status: **current** — one operator landed, and the friction it produced is the roadmap.
> Points at: `examples/rmsnorm.lichen` (landed at `419aab56`), `crates/lichen-runtime`
> (the passes, the claim schema, `automatic_tiling.rs`), `crates/lichen-fusion-bridge/src/lower.rs`
> (what a marked body can bridge), `crates/lichen-compute/src/compute/graph.rs` (the graph path).
> The machinery itself is [graph-fusion](graph-fusion.md) §5; the plan/pack story is there too.

**Writing an operator is the sharpest test of the machinery**, so each operator is committed as an
`examples/*.lichen` program — an example *is* a test here (it declares its own `output = "…"` and
`crates/lichen-compute/tests/lichen_compute/examples.rs` runs it), which keeps the operator verifiable without a new
test file.

## The ladder, as measured rather than hoped

| step | state |
|---|---|
| **RMSNorm** | **landed** (`419aab56`): a row-wise `Σ x²` reduction plus the rescale that reads it |
| **SiLU / MLP** | **landed** (`3f3693a7`): `silu(gate) × up` as two elementwise kernels. **The floor refuses the honest pair by name** — `InterfacesAreNotProved` — because `Elementwise` has no licence and an elementwise region has no reduction chain to prove; only the pack door is ajar (the schema derives, prints and hashes) |
| **softmax** | **landed** (`f8645274`) and its **textbook three-kernel form merges** (`d364f547`): the normalise step reads both `m` and `s`, with `m` also read by the sum — a **fan-in**, and a fan-in is licensed. It merges because its two producers are **chained** (the sum reads the maximum); a diamond whose producers are **siblings** does not group at all, which is LayerNorm's wall below. A **chain of any length** landed earlier (`3f8007cc`). The licence was restated once, in the two places it was stated: *"a region two edges **arrive at** is that shape exactly when every one of those inputs is named and proven: a `Between` per arrival, each over a slot of its own"* |
| **LayerNorm** | **landed** (`807469f1`) as three kernels in a diamond — `Σx` and `Σx²` as two producers, one normalise reading both. Its region **merges into one node** now (`c9e65140`: a projection may read the fused step's slots **and the invocation**, so `count = int2float k.n` bridges), whereas the grouping alone left it refusing (`f973544a`). **Both of the walls it hit next are gone** (`2fd3971f`: a runtime scalar is a third kind of free value, named `s0` by ordinal, and a producer that declares none takes the consumer's), and the shape it spells — driven through the graph path, since the committed example itself drives three `plrun`s and **records no graph** — merges into one node with `tile ≡ body` **bit for bit** on the device; the merged element's free values are `["d", "n", "r", "r1", "s0"]`. Two limits are honest to keep: the relaxation is **one-sided** (a region where *every* node declares the runtime scalar still refuses, and admitting it needs the pass to compare `Scalar::Constant` bits between nodes), and a merged node is **unregistered**, so its licence is the floor's shape claim rather than a digest-keyed proof about the result. Also, a producer that declares its **own** runtime scalar is still refused, because the merged leaf would carry the **consumer's** number and nothing compares the two nodes' values — that refusal is what keeps a silent wrong constant out. It uses the **one-pass** identity `var = Σx²/n − mean²`, because the textbook two-pass form cannot be a diamond at all: there the mean is read by the variance kernel *and* the normalise kernel, which is `ProducerFeedsTwice` (one value, two slots, no spelling). The answer is exact by hand (`1,2,3,4` ⇒ `Σx = 10`, `Σx² = 30`, `mean = 2.5`, `var = 1.25`, centred `±1.5, ±0.5`) except `sqrt`, which is the language's Newton iteration and is modelled from its own sequence, as SiLU's `exp` is. Measured on the RTX 3060 through `graphrun`: `[-3.0, -2.0, 1.0, 6.0]`, element for element what `cpu` answers, with and without the merge pass. Its `eps` is a **parameter** now (`e24d00fb`), passed as `1.0` on the direct `cpu` path with the declaration unchanged |
| **RoPE** | **landed** (`2f7214af`): `cos`/`sin` added to the language, then the pairwise rotation as three elementwise kernels. They were **hand-assembled definitions** spliced by the lowerer until `82640443` **deleted `sin_cos_definition` and switched them onto a named operator** — the same commit that landed `ln` as one, so a transcendental is now either named in `TRANSCENDENTALS` or nothing (`docs/notes/transcendentals-as-operators.md`). It is **one lane per element** — a rule, not a preference: a `.to` chain can fill both positions of a pair, but its outer link is contended and order is not a contract. Domain `|x| ≤ 8192`, bit-exact against a model; the clamp above that is a **silent `Select`**, so an unbounded angle gets `sin(8192)` |
| **matmul** | **landed, naive and non-square** (`3fdedd3d`, the upload fix `08c15c45`): one lane per element of `C`, the `k` fold inside the lane, with the coordinates **named** by `compute.row`/`compute.col` (`459f3663`). The wall the measurement found was not the one this note predicted — it was the **upload width**: a device run uploaded `count × element` and zeroed the tail, so a read set that was not a prefix of its buffer silently answered `0`, and for `m×K · K×n` the device's two length rules together admitted **only `m = K = n`** (a non-square `2×3 · 3×2` answered `[25, 28, 28, 32]` against the CPU's `[58, 64, 139, 154]`, silently, and a body with **no `@loop`** disagreed identically, which ruled out a lowering bug). **That is now fixed**: a host input is uploaded at its **own length** (the invariant is `[0, len)`, the padding past `len` filled on the device), so `2×3 · 3×2` answers what the CPU answers, element for element, and `examples/matmul.lichen` is committed **non-square** for that reason. What remains, in order: the host still cannot see the body's read set, so a body reaching **past its buffer** while the count is inside is an out-of-bounds device read (the silent-zero region shrank from `[count, padded)` to `[len, padded)` rather than closing), and a host input **shorter than the count** is refused by name; then the **workgroup geometry** (a real two-dimensional workgroup, a row-shaped `Shared` exchange). **An operator's shape can now be a parameter** (`c92b6471`): a recorded config carries the first scalar leaf as the extent and **every other leaf for what it is, at any depth** — and `examples/matmul.lichen` uses it, since its inner dimension is `k.k` rather than a `3` (`97e6ad91`) — and **a `Float` leaf is carried too** (`e24d00fb`): the graph's value table holds a float at its own class, so `eps`, a scale or a temperature can be an argument rather than a literal; the two remaining limits are that a leaf cannot be a value computed *inside* the source function, and that `f64` is inexpressible. **But two leaves are positional, and getting them wrong is silent**: the **first** is the extent, and the **second** is what `compute.row`/`compute.col` divide by — so a new leaf goes *after* `cols`. Putting `k` between them does not refuse; it answers different numbers (measured: `[58.0, 64.0, 31.0, 139.0]` instead of `[58.0, 64.0, 139.0, 154.0]`), which is the same silent-wrong-answer class as the upload width |
| **attention** | **landed, naive** (`576c9229`): two kernels — `scores` (a dot product per query/key pair, one lane each, `query = compute.row k`, `key = compute.col k`) then `attend` (softmax × V, three `@loop` nests with the row maximum carried in the value fold's state) — over `Q = 2 0 1 1 / 0 0 2 1`, `K = 1 1 0 0 / 0 1 1 0`, `V = 1 2 3 4 / 5 6 7 8`. Exact by hand except `exp`, which is modelled from its own sequence; `sqrt(dim) = sqrt(4)` is exact because the Newton definition's reduction is the identity on `4`. On the device `attend` answers the declared line with **six of eight entries differing in the last bit** (this device's `f32` division; the slice that landed this example first reported three, and its own run could not be reproduced because flipping only `attend` to `"gpu"` trips a different rule — `attend`'s lanes are `queries × dim = 8` while the `cpu` `scores` buffer holds 4, so the upload rule refuses it — and the six-of-eight measurement is a whole-example run), and **`scores` was refused outright until `b6dea46c`**: *"a `Select`'s two arms are computed in different classes ([Int, Float])"* — reproducible as `sqrt (int2float k.dim)` failing while `sqrt 4.0` and `sqrt (int2float k.dim + 0.0)` both run, i.e. the `Select` inside `sqrt_body` is left with one arm unclassed when an `int2float` of a config leaf feeds it directly. **The wall the row used to describe is a rule, not a missing feature**: a `@loop` nest **cannot be applied inside another nest's step** (see the finding below), so the one-kernel form — score recomputed per lane — is **not writable**, and this is a demonstration rather than a schedule anyone would ship |
| **KV-cache append** | **landed** (`1f79786d`): one `compute.parallel` kernel over `count = capacity · dim`, one lane per cache element, writing **both** the key and the value row back at `position · dim + column` — a **bijection** (8 lanes → 8 slots), so no lane contends and the write address is genuinely computed rather than the lane's own index (lane 1 writes slot 2, lane 4 slot 1). Exact integers throughout, **nothing modelled**. It rewrites the whole cache because **a write past the count is dropped in silence** — see the first finding below, which is what a one-lane-per-column append actually hits. The graph path **records** it (1 node, the extent plus all three runtime scalars) but **refuses the run** at submit; the direct device leg agrees with `cpu` element for element once the incoming row is padded |
| **GEMV (the decode matmul)** | **landed** (`da584de5`): `y[m] = Σ_k W[m·kdim + k] · x[k]`, one lane per output element, the `k` fold in the lane, leaves `.n` and `.kdim`, declaring `[14.0, 32.0, 50.0, 68.0]` — exact integers, **nothing modelled**. **It chains with the KV-cache operator as it stands**: `K[p][d]` row-major flattened *is* this operator's matrix with `m` = position, so append-then-score-one-query is this kernel unchanged and answers `[3.0, 7.0, 300.0, 0.0]`, element for element the literal-cache run — while the **values** half needs a second address form (the cache is position-major, this operator output-major), and the cache must be the whole `capacity · dim` buffer, which is exactly what the KV-cache operator rewrites. **Both of the walls this row used to name are gone, measured.** The device leg's refusal of a reduction shorter than its output was removed with the length rule, and the same source now answers `[14.0, 32.0, 50.0, 68.0]` on the RTX 3060, 4 of 4 bit-exact. The graph leg **records and runs** the operator once the step's result is collected directly, and the fusion floor now reads it: both stages refuse it **by name** — see the finding below |
| **one decode step, chained** | **landed** (`98ddba9b`): five `compute.parallel` kernels on `cpu` — the KV-cache append, a GEMV scoring one query against the **appended** keys, softmax's row max and denominator, and attention's `acc` nest over the appended values — declaring `[8.888384, 10.773098]`, and **four of the five are a landed kernel's body with nothing but the binder and parameter renamed**. Exact: the append, the four dots `1, 3, 1, 0`, the maximum `3.0`, the offsets `−2, 0, −2, −3`, `exp(0) = 1.0` (the polynomial at zero is `1.0` and seven squarings of `1.0` are `1.0`), and the weighted sum *given* the weights. Modelled: `exp(−2)`, `exp(−3)` and everything downstream, from the language's own sequence re-implemented in `f32` outside the workspace — a reference `eˣ` in `f64` puts the same chain at `[8.8883843, 10.7731007]`, so the degree-10 Taylor costs about half an ulp and three. **Only the last stage had to change, and the change is visible in the answer** — see the finding below |
| **a decode loop** | **landed** (`cbe4feb1`): two decode steps through a **recursive lichen function**, the six-kernel chain appearing **once** (the guard is `s(0) == 2`, the base arm returns the state), and the position **computed inside the recursion** as `s(0) + 2` rather than handed in. It declares a three-part answer — both steps' outputs end to end, the **last step's output alone** (the only one anything outside the recursion can name), and the **trip count** — and the CLI prints all three, `([8.888384, 10.773098, 11.235222, 13.9678135], [11.235222, 13.9678135], 2)`. Exact where the arithmetic is (the appends, the dots, the maxima, the offsets, `exp(0) = 1.0`, the weighted sums given the weights), `exp` modelled from the language's own sequence; **step zero reproduces `decode.lichen`'s declared row and its documented weights exactly**, which is what makes the model trustworthy for step one |
| **greedy sampling (the sampler)** | **landed** (`3af80221`): one lane per row and one `@loop` carrying `(counter, best, position)`, so this is the ladder's first fold whose result is a **selection**. **Nothing is modelled** — integer scores below `2^24`, comparisons and multiplications of integers under `4`, no `exp`, no `sqrt`, no `ln`, no division — and it needs **no transcendental at all**, which is why `ln`'s landing was not what unblocked it. Three surface facts it measured, none of them about the arithmetic. (a) **A `@loop`'s domain has one leaf**: a base arm returning the whole state, or a two-slot tuple, is refused (*`a kernel body's index cannot be placed … with leaves [NodeId(135v1)]`*; `crates/lichen-lowlevel/src/loop_conversion.rs:131` and `crates/lichen-compute/src/compute/body.rs:1195`), so the fold returns the position and **the maximum is unobservable afterwards**. (b) **A `Select` inside a fold's step is refused** — *`a kernel body reached a node with neither a value nor an operation, so there is nothing to emit for it`*, a sentence naming a cause it is not — so the selection is arithmetic on the comparison's `0`/`1`, and **the `1 *` anchor is what makes that fragment lower** (without it: *`instruction 15 mixed an integer and a float in one operation`*); it is **not** the `~` mark, and `examples/attention.lichen`'s `if` is therefore not a precedent, its only `if` being the base test. (c) **Ties are the body's choice, not the operator's**: `>` over the countdown fold answers the **highest** position, `[3.0, 2.0, 3.0, 0.0, 3.0]`, and `>=` the **lowest**, `[1.0, 0.0, 0.0, 0.0, 3.0]`, both measured on the same five rows. It consumes `examples/decode.lichen`'s row **verbatim** (`[8.888384, 10.773098]` answers `[1.0]`) and **agrees with the two-`plrun` spelling only once that one is given a *first writer wins* guard** — `>=` alone answers the lowest position. **Top-k is work, not a wall**: `k = 2` runs as two folds in one lane and two output buffers returned as a tuple in output-field order, the second fold wholly in `Int` (`float2int` on the data, a finite seed standing in for `-inf`, `&` refused twice — by the kernel and by the type checker); **the one-buffer gather, `k` slots per lane, was not reached** |
| **generation** | **the last rung, and its blocker is a budget, not a spelling.** A stop condition **is expressible**: a recursive function's guard may read a top-level binding, an element collected from a `compute.buffer`, or a token a kernel sampled, and a loop that stops on a sampled token answers end to end — measured on `dev@4b9bd7a9`, **host-only** (the CLI installs no backend, so that measurement is a device result for nothing; **the shape itself has since been measured on an RTX 3060 Laptop, bit-exact, by [device-leg](device-leg-of-the-ladder.md) §5**). Three shapes, all run: a guard reading `v[s(0)]` against a threshold, `raw 2: ?a`; the same guard with a top-level `Int` binding in place of the literal, `raw 2: ?a`, which this note used to record as **refused**; and the generation shape itself — `examples/sample.lichen`'s `greedy_raw` sampling a token per row, the loop stepping until the sampled token is the sentinel, `raw 1: ?a` on `1 2 3 4 / 5 9 6 7` where row 1's argmax is position `1`. **What bounds it is `WorkBudget`** (`crates/lichen-highlevel/src/checker.rs:384-389`): an unmarked recursion reaches `apply_depth_limit` = **500** at exactly 500 trip counts (*nested applications exceeded 500 levels*) and the `@loop` marker moves it to `apply_total_limit` = **2 000** (*it applied a function more than 2000 times*), both refusals worded as **non-termination** — a sentence about a loop that never ends rather than one whose stop condition was not met. So the boundary is **a pair of tuned constants**, and the checker's own invariant at `:371-374` says the second *"is what a large trip count asks for, since a loop never spends the nesting bound"* |

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

- **`sqrt` bridges now** (`c5e8e63e`). The reader matches **the whole of `sqrt_body`, instruction for
  instruction**, walking the definition backwards from its own result and anchored on the **definition**
  rather than a copy — so a change to `sqrt_body` moves the reader with it, while a body that is not the
  expansion (≥ 500 instructions, 90 `Select`s, exact constants in known places) differs in some step, the
  match answers `None`, and the walk falls through to its own match and refuses the real `Select`.
  **Measured**: `sqrt (s / int2float 4 + 0.25)` — a **literal** extent — now **fuses**: *"region 0:
  nodes 0 and 1 merged into one node at 1 (the survivor), absorbing input position 2 of node 1, licensed
  by the floor: every node of the region claims Reduction and nothing else"*. A **fixed hidden size
  fuses today**; the runtime extent is the only thing still stopping it.
- **`int2float` bridges only where the walk already reads a real.** Admitted: `Conv{Int→Float}` of an
  integer immediate (read as the real it denotes; above `EXACT_INTEGER_LIMIT` it is refused
  `MachineInteger` rather than answered with a neighbouring number) and of anything else the walk reads
  as a real, with an integer anywhere else refused **by that instruction's own name**. Still refused:
  `Float→Int` (truncation is many-to-one — no inverse, no law) and a scalar fragment's own parameter
  leaf. **`int2float k.n` bridges now** (`c9e65140`): a projection may read the fused step's slots
  **and the invocation** — the entry block's **first and last** leaves, the extent and the lane index —
  while a **runtime scalar leaf between them** is still refused by name (`ReadsARuntimeScalar`). Three
  things this corrected. **The old gloss was wrong**: the sentence said *"the lane index"* while the
  expression that triggered it read **leaf 0, the extent** (the probe printed `parameter block=0
  entry=0 index=0` for `int2float k.n` and `leaf 1 (domain_arity 1)` for `compute.range k.n`, with
  `n_params=2`; `lichen-kernel-ir`'s own invariants already said *"The index is the last leaf"* and
  *"the ABI's first scalar by definition"*), so RMSNorm "refusing verbatim" was RMSNorm refusing the
  **same wrong gloss for the same extent**. **No new symbolic slot was needed**, contrary to what this
  note claimed: `compose.rs` binds the consumer's parameters to the producer's **position for
  position** and requires equal `param_shape`, so the composed body's **last leaf is already its own
  invocation index** and its **first is the extent** — two `Variable`s and one `match`, nothing
  carried. And **the licence is weaker for it**: it now reads *"a projection is one function of the
  fused step's slots **and the invocation**"*, which is what makes a **position-dependent mark** (a
  causal mask, a relative-position term) writable at all. **The measured payoff**: RMSNorm's pair and
  LayerNorm's diamond both merge, and `tile ≡ body` is **bit for bit** on the device for a mark that
  reads the lane index (`+ int2float i` answers `[3.5, 4.0, 4.0, 11.0]` — RMSNorm's documented answer
  plus each lane's own index, so the `i` read is the invocation's and not a renumbering), with the
  `0.25 → 7.25` counterfactual inside the composed body answering a number no recording computes.
  **The risk this buys is worth stating plainly**: the **merge never evaluates the projection** —
  `compose_fan_in` composes bodies and the projection is only checked for bridge-ability and canonical
  form — which is exactly why `tile ≡ body` held, and it means a wrong projection cannot produce a
  wrong merge *here*. The exposure is the **claim**: `RepairFunction::new` still refuses any body
  mentioning anything outside `REPAIR_ARGUMENT | PRODUCER | PRODUCER_NEXT`, so §6's fused **reduction**
  schedule cannot cash an element that reads `n` or `i` (and `tiled.rs` declines any non-datum
  variable), which is the next contract decision rather than a defect. **Unproven**: whether a
  projection reading `i` is sound under that fused-reduction schedule, where the fused step iterates
  over the producer's growing extent and the composed body's own index is not the consumer's original
  one — only the elementwise chain/fan-in path was measured, and it does not transfer by assumption.
  **And it does not transfer in fact either, measured**: under the fused **reduction** derivation the
  same reads are refused *before* any contract question, because the derivation has **no vocabulary for
  them**. A probe over the tiler's `Element::Function` reports, for `i`, `n`, `d + i` and `d + n` alike:
  *"the datum `i` is not a function a kernel body computes: it reads a value the fold has no slot for, or
  an operator…"* (`crates/lichen-fusion-bridge/src/tiled.rs:234`) — the fold's slots are the datum and the
  producers, and the invocation is not among them. `RepairFunction` is narrower still: its free set is
  `REPAIR_ARGUMENT | PRODUCER | PRODUCER_NEXT` (`crates/lichen-fusion/src/repair.rs:59`) and its refusal
  (`:191`) fires for `x - i`, `x - n` **and `x - d`** — so a repair may not read the fold's datum either.
  That makes the next decision bigger than "carry the invocation": the **reduction derivation would need a
  slot for the step's own index**, and that is exactly where the soundness question lives — in this
  schedule the fold runs over the **producer's growing extent**, so the step's index is **not** the
  consumer's original lane, the one thing the elementwise measurement cannot settle.

**A caveat that predates this work and is now load-bearing**: the reader claims "the body computes this
operator", which is a **modelling claim, not a theorem**. `sqrt_body` is a Newton iteration — six steps
from the seed `2.0` do not land exactly on `√` in exact arithmetic — exactly as the `exp` reader already
treats a degree-10 Taylor sum as `eˣ`. It matters here because a fused region is **dispatched** as the
symbolic form; the approximation is the same one the bodies already carry, but it is an assumption
rather than a proof, and nothing verifies it.

**A test-harness hazard, now closed for the stragglers.** At `--test-threads 2` a device test could fail
with *"the \"gpu\" backend declined to return input buffer 0 … not one this device is holding"*, because
a helper calls `install_default()` — which **replaces** the process's backend slot — and a second test
installing one while the first runs leaves the first's buffers on a device that is no longer installed
(`fuse_licence.rs` documents the same thing). The fix is the repository's own idiom, not a new one:
**seven** device-test binaries already held a `static DEVICE: Mutex<()>` for the whole of a test that
runs on a device, and `2a62591e` converted the **three** that had not (`automatic_tiling`,
`fused_region`, `consumer_guard`) — measured 1 failure in 16 runs before, 0 in 16 after, on the binary
that had the defect (`lichen-compute`, not the GPU crate, whose helpers return a context by value and
never touch the slot). **The duplication is gone**: `e766020b` moved the guard into
`crates/lichen-compute/tests/lichen_compute/common.rs` — one `static DEVICE`, `device_slot()` for the lock alone, and
`device()` for the ten-copy helper — so `git grep -c 'static DEVICE' -- crates/lichen-compute/tests` went
**nine files → one**, and ten → two across both crates. Measured after the collapse: **23 binaries ×
8 runs at `--test-threads 2`, 207 runs, zero failures**. What is left, and what it costs:

- **`tests/lichen_compute/combine_fold.rs` was the last unguarded one, and `f818cc57` converted it** — it had justified
  its own `install_default()` by "this case owns its test binary", and the comment went with the
  function it justified. It takes `common::device()`, **not** `device_slot()`: the lock-only spelling
  installs no backend, and since a device is reached through `ParallelBackend` the test would have
  silently stopped exercising a device at all.
- **`tests/lichen_compute/compute.rs` has the only two surviving `install_default()` calls, and both are guarded** —
  each takes `common::device_slot()` on the line above. They are deliberately not routed through
  `device()`: one asserts `installed_backend_name() == None` after an explicit `uninstall()`, and the
  other runs the **CPU** backend *before* installing the GPU one. Both would change behaviour, so the
  duplication of the *call* is what remains, not a hole.
- **Five files have a local `fn device` that takes no lock** — `composed_chain`, `plan_staging`,
  `reduction_chain`, `sparse_input_classes`, `tiled_reduction` — and they are **not** the same hazard:
  each builds a context with `GpuContext::new()` and **never installs the process-global backend**, so
  they do not contend for the slot. What remains is a lookalike shape that can be copied, and one of
  them prints `device: {name}` to stdout where the shared helper is silent.
- **`crates/lichen-compiler/tests/pack_install.rs` keeps its own guard**, and should: a `Mutex` is
  per-process and that file is its own binary, so a shared static could not serialise it anyway, while
  its two device tests do race at `--test-threads 2`.
- **`lichen-compute-gpu/tests/compute_gpu/common.rs`'s `context()` is genuinely different**, not a
  leftover: those tests never call `install_default` or `install`, so there is no process-global
  replacement to guard against.

## The hazard, and the gate that closed the tiling half of it

**Measured, not theorised, and it was a wrong-answer class.** `SchemaNode::reduction()` keeps only the
**top-level operator** of the `combine` projection (`fusion_claim/schema.rs`), and `Folding` rebuilt
the fold from that one operator over the **raw buffer** (`automatic_tiling.rs`). The `x²` of `Σ x²`
lives in the `element` role, which `NODE_LOCAL_ROLES` excludes (`automatic_tiling.rs`). So the two
encodings both lost:

- declare `element` — the honest claim — and the pass **refused** (`BeyondANode`): safe;
- omit `element` — and the pass **tiled**, trusting an algebra that is a strict weakening of the
  body's: the answer changed.

Same program, same device, same data (`x = [1…64]`), only the installed floor differing:

| | `y[0]` on the device | the sum it used |
|---|---|---|
| no pass installed | `0.026747627` | `Σ x² = 89440` |
| floor installed, before the gate | `0.1747408` | `Σ x = 2080` |

Both reproduce bit-for-bit in `f32` from the two closed forms, and the pass **said so itself** while
the run still succeeded: *"node 0: no pack matched its schema …, so the automatic floor tiled: its own
marks declare `+` over 64 lane(s)"*.

**The gate (`6c656f50`)**: `SchemaNode::declares_the_fold()` reads the **canonical** `combine` and
requires `Expr::Binary` over `Add | Mul` with one occurrence of each of `FOLD_SLOTS =
[CONSUMER_STATE, DATUM]` — in either order, since `normalize` sorts the datum before the state — and
nothing else. It sits in `AutomaticTiling::apply` **after `beyond_a_node`, before `tile()`**, so it
fires before any pack is consulted and reports a **node line** (`node N: refused:
CombineIsNotAFold: …`), not a run error. `reduction()`, `element` and `NODE_LOCAL_ROLES` are untouched,
and no existing test moved — the position matters: a gate inside `derive()` would have preempted
`consumer_guard`'s `ReadsAnotherNode` refusal, which pins a different defect. **Measured on the device,
same program and data as the table above**: the pass now refuses and **no number is produced at all**
— nothing lands, the node keeps the program's own body, and the answer is element-for-element the
un-passed one. The wrong value is no longer reachable, and neither is any other tiling of that claim.
A legitimate `Σ x` still tiles byte for byte (its `Shared` slice and barrier still run, and the
seed-raised counterfactual still moves by `65 × 7`).

**The capability half closed the hazard properly** (`9722e681`). The datum is now the combine's **own
non-state operand**, read from the canonical form (`SchemaNode::the_fold() -> DeclaredFold { operator,
datum }`), carried as `Element::Function { position, function }` and compiled **once** into a `Vec<Step>`
over the fold's one buffer read, which `Builder::element_at` replays inside the loop header — so every
refusal happens at compile time and replaying cannot fail. **Schedulable now**: the combines `+ * max
min`, and datums made of `d`, constants and `+ - * / max min` over them. **Still refused, by name**: a
combine of `Sub`/`Div`/a comparison (`CombineIsNotAFold` — and in practice `Sub` never reaches that arm
because `normalize` rewrites `(t − d)` as `(+ t (−d))`, whose negation no body computes), a comparison or
any unary operator as the datum (`DatumIsNotAReadableFunction`), and any free variable but `d`.
**Measured on the RTX 3060**, `x = 1.0 … 192.0`: `Σ x²` now tiles and answers `Σ x² = 2377760` — asserted
element for element against what the **un-passed program's own body** wrote, with `Σ x = 18528` asserted
*not* to be the answer — and `Σ x` still tiles **byte for byte**, pinned by a host test that the two
element kinds build **equal** `KernelBody` values. A `Max` fold tiles too (`… its own marks declare `max`
over 64 lane(s)`) and is **bit-exact**, where `+` is only re-associated.

**The identity landmine is closed, and closing it corrected a premise.** The pass does **not** read a
claim-side literal — `KernelClaim` carries `interface` and `roles` only, and its own invariant says the
projections *"are **not** here: each is a mark on an expression inside the kernel's own body, so a claim
cannot diverge from what the kernel computes."* `automatic_tiling::derive()` already read the body's own
`~ "identity"` mark (`marked_value` → `projection` → `normalize`) and seeded from its canonical form. So
the defect was **never** a lying declaration: it was that **the language could not spell `±∞`**, and
authors therefore wrote a finite seed that was true only for data above it — measured as a `Max` over
`−2e6 …` seeded `-1e6` answering `-1000000` (faithful to the mark, wrong as a maximum), and a `Min`
answering a value no element holds.

`79defe18` gives the spelling, with the one contract restatement the user authorised: **`Div` of a
literal by a zero literal folds to the signed infinity it denotes** (`BinaryOp`'s invariant rewritten;
`infinity_of` reads both signs, so `1.0 / -0.0` is `−∞`) — while a zero or `NaN` numerator and a
non-literal divisor keep their trees exactly as before, so `0.0 / 0.0` is still `NotAnIdentity`. The
landed examples moved to the honest spelling: `examples/softmax.lichen` and `fold_datum.rs`'s max now
seed `((0.0 - 1.0) / 0.0)`, and the pass's own line **names the identity it seeds with**
(`… its own marks declare `max` over 64 lane(s), seeding the fold with `-inf``). `NotAnIdentity` and
`MissingRole` also print their **name** now, not just their sentence.

**What remains true, stated honestly:** a body that marks a *false* seed — `fold (n, (0.0 - 5.0) ~
"identity")` over a `max` — still tiles and answers wrong, because the pass cannot check a seed against
data it never sees; **the report naming the identity is the mitigation, not a gate**. The restatement's
reach is real and visible rather than silent: an element function like `d / (e - e)` that used to keep
its tree now canonicalises to a `±inf` constant. And `tile ≡ body` is **bit-for-bit only for
`max`/`min`** — re-associating `+`/`*` in `f32` is still an assumption, not a proof.
**untouched and still `BeyondANode`**: the datum is read from the combine, and admitting `element` would
need a node-local producer for `r`.

## Smaller findings, each with the thing that would change

- **The loop's bounds, measured — and the audit's own cost line refuted** (`cbe4feb1`). **A launch does
  follow a loop-carried value**: a loop-carried **buffer** as input answers `[1, 2, 1, 2, 0, 0, 0, 0]`
  (positions 0 then 1), the trip count itself as `.t s(0)` answers the same, and `.t (s(0) + 1)` answers
  `[0, 0, 1, 2, 1, 2, 0, 0]` (positions 1 then 2) — *"each launch got its own value; a static read of the
  cell could not produce that"* — so `decode.lichen`'s literal `2` is a **choice, not a wall**. And the
  audit's cost line ("nothing else in the program can see the trip count") is **wrong**: the count **is**
  the declared third entry. What is true instead is narrower and cuts the other way — the count is
  **decided by a literal in the recursion's own body**, and replacing that literal with a top-level `Int`
  binding is **refused** (*"table lookup missed — no entry for this key"*) while the literal-only control
  runs: **the loop can be asked how far it got; it cannot be told what to do.** **Both halves of that
  sentence were wrong, and are corrected here** (measured on `dev@4b9bd7a9`, host-only — the CLI installs
  no backend). **The control runs**: a four-line program whose only difference from `decode.lichen`'s
  guard is that `2` is spelled as a top-level `stop_at = 2` answers `raw 2: ?a` against this note's
  *refused*. **The quoted error cannot arise from such a program at all** — `TableMiss` is pushed only
  from `TableGet` (`crates/lichen-lowlevel/src/evaluation.rs:363`, `:389`, `:474`), which is the
  `t <== k` read of `examples/table.lichen`, so the sentence quoted a diagnostic belonging to a different
  feature. What is true is the other way round, and it is the sentence this note should have carried: **a
  recursive function's guard may be any value the language computes — a top-level binding, an element
  collected from a buffer, or a token a kernel sampled — and the loop stops when it says so.** The
  refusal that *does* exist is not about the guard; it is the work budget the next bullet measures.
- **Three bounds on the recursion, two of them silent.** (1) **One level removed, it stops.** A helper whose
  parameter is a buffer and whose **every leaf is a literal** collects `raw none` — no error, no refusal, no
  report line (with a scalar leaf it is refused **loudly**: *"input buffer 0 holds Float elements, but this
  kernel reads that position as Int"*), which is why the loop's chain was deliberately **not** factored
  into a named `step` function: *"the worst of the three, since nothing says so."* (2) ~~**The extent leaf
  never follows the recursion**: `.n (s(0) * 8)` collects an **empty buffer, silently** — every other leaf
  follows, and the one that decides how many lanes run does not.~~ **Corrected by measurement: the extent
  leaf follows the recursion like every other leaf** — the `extent` slice, `dev@81e45151`, host-only, and
  the programs and their answers are in the finding *"the launch extent follows the state"* below. **The
  struck sentence was a real measurement at `dev@01da5a49` and it no longer reproduces**: the same
  recursion with `.n (s(0) * 2 + 2)` and a body that writes its own lane index answers `[0.0, 1.0, 2.0,
  3.0]` — four lanes, the extent the recursion asked for at its last step. (3) **A `@gc` block inside the
  recursion
  panics** (`invalid SlotMap key used`, `crates/lichen-lowlevel/src/equality.rs:823`) — measured, worked
  around, not diagnosed.
- **The launch extent follows the state, measured, and the wall that said otherwise was the spelling**
  (`extent` slice, `dev@81e45151`; **every verdict below is host-only**, because the CLI installs no
  compute backend, so `cpu` answers nothing about a device). **Why this was worth re-measuring**: eight
  claims of this shape have already fallen or narrowed to a spelling when they were measured in this line
  of work, so the sentence was treated as a hypothesis rather than read as a contract.
  - **The control.** `dbl_raw` over `struct{.n Int, .in In, .out Out}`, writing `a[i] · 2`, launched at
    `.n 4` over `[1.0, 2.0, 3.0, 4.0]`, answers **`[2.0, 4.0, 6.0, 8.0]`**. Every row below is this
    program with the extent written differently, and where a row says so, a body that makes the extent
    visible.
  - **Shape A — any value the language computes, not only a literal.** The control with `half = 2` above
    and `.n (half * 2)` answers **`[2.0, 4.0, 6.0, 8.0]`** — the literal's own answer, because the
    expression's value *is* `4`, which is the claim in itself. **The row that separates the two** is
    `n = (x => x + 1) 2` with `.n n`, which answers **`[2.0, 4.0, 6.0]`**: three lanes, an answer no
    spelling of a literal `4` produces. **So the leaf is read at its own value, not at its source text** —
    which is the same sentence `scalar_leaf_classes` already states about a leaf's *class*, one step
    further along.
  - **Shape B — a value the recursion computes (the crux).** The body writes its own lane index, so the
    answer's **length is the extent**. `drive = s => if s(0) == 2 then s else { got = (compute.plrun idx
    ((Arg)(.n (s(0) * 2 + 2), .cap 1, .t 1, .I In(.a src))) : Out)  drive (s(0) + 1, got) }`, seeded
    `drive (0, Out(.z src))`, answers **`[0.0, 1.0, 2.0, 3.0]`**: step zero asked for 2 lanes and step one
    for 4, and the state the loop returns is step one's. **This is the direct refutation of the wall**,
    and it is one leaf: the identical program with `.n 4` spelled as a literal and `.t (s(0) + 1)` read
    from the state answers **`[2.0, 4.0, 2.0, 4.0]`** — so *both* leaves follow, and neither is a
    special case of the other.

    ```text
      idx_raw = (k : Par) => {
        i = compute.range k.n
        compute.write ((compute.Write _)(.to k.out.z, .at i, .value (int2float i ~ "element")))
      }
      src = (compute.buffer ([0.0, 0.0, 0.0, 0.0] : array{Float, 4}) : (compute.Buf Float))
      drive = s => if s(0) == 2 then s else {
        got = (compute.plrun idx ((Arg)(.n (s(0) * 2 + 2), .cap 1, .t 1, .I In(.a src))) : Out)
        drive (s(0) + 1, got) }
      final = drive (0, Out(.z src))
      compute.collect final(1).z
    ```
  - **Shape C — a scalar read back out of a buffer, which is the device-shaped spelling.** A one-lane
    kernel writes `3.0` into a buffer, the loop reads it back and converts:
    `n = float2int (compute.collect stated.g : array{Float, 1})[0]`, and the score launched at `.n n`
    over `[1, 2, 3, 4, 0, 0, 0, 0]` answers **`[3.0, 7.0, 0.0]`** — three lanes. **The same read with
    no loop around it** — a one-lane copy kernel, `float2int (compute.collect stated.z :
    array{Float, 1})[0]`, and `dbl` launched at `.n n` — answers **`[2.0, 4.0, 6.0]`**, so the buffer
    spelling is not a recursion artefact either. **The price is the round trip**, not the capability: on
    a device this is a download per step, and that is the real argument for the masked spelling rather
    than an inability to write this one.
  - **The two spellings, side by side, on the growing cache itself.** One program, two recursions over
    the same cache (`K` of capacity 6 positions × `dim = 2`, the loop asking for 1, 2 and 3 lanes over
    three steps). The
    **extent-follows** recursion launches the score at `.n s(1)` and answers **`[3.0, 15.0, 15.0]`**; the
    **masked** recursion launches it at `.n 6` with a `.used s(1)` leaf and `mine = if live then a + b else
    held`, and answers **`[3.0, 15.0, 15.0, 0.0, 0.0, 0.0]`**. **The first three elements agree**, so the
    comparison is like for like: the masked spelling runs **6 lanes where 3 are wanted**, and over the
    loop **18 where 6 are** — three times the lanes at this fill, a factor of `capacity / filled` in
    general. **Two more costs, both about what a reader must write**: the mask needs a third config leaf
    (`.used`) and a `Select` per element, and because the output buffer is as long as the launch the
    `collect` hands back **`capacity` elements**, not `filled` — so the masking is not only paid in lanes
    but has to be undone at the boundary.

    ```text
      score_raw = (k : ParS) => {
        i = compute.range k.n
        a = compute.read ((compute.Read _)(.from k.in.k, .at (i * k.dim)))
        b = compute.read ((compute.Read _)(.from k.in.k, .at (i * k.dim + 1)))
        compute.write ((compute.Write _)(.to k.out.s, .at i, .value ((a + b) ~ "element")))
      }
      masked_score_raw = (k : ParM) => {                       # ParM adds a third leaf, .used
        i = compute.range k.n
        a = compute.read ((compute.Read _)(.from k.in.k, .at (i * k.dim)))
        b = compute.read ((compute.Read _)(.from k.in.k, .at (i * k.dim + 1)))
        held = 0.0
        live = i < k.used
        mine = if live then (a + b) else held
        compute.write ((compute.Write _)(.to k.out.s, .at i, .value (mine ~ "element")))
      }
      following = s => if s(0) == 3 then s else {             # .n s(1): the extent follows
        grown = (compute.plrun append ((ArgK)(.n 12, .dim 2, .t s(1), .I KVIn(.k s(2), .nk nk))) : KVOut)
        scored = (compute.plrun score ((ArgS)(.n s(1), .dim 2, .I SIn(.k grown.k))) : SOut)
        following (s(0) + 1, s(1) + 1, grown.k, scored.s) }
      masked = s => if s(0) == 3 then s else {                 # .n 6: fixed, and the tail masked
        grown = (compute.plrun append ((ArgK)(.n 12, .dim 2, .t s(1), .I KVIn(.k s(2), .nk nk))) : KVOut)
        scored = (compute.plrun masked_score ((ArgM)(.n 6, .dim 2, .used s(1), .I MIn(.k grown.k))) : MOut)
        masked (s(0) + 1, s(1) + 1, grown.k, scored.s) }
    ```
  - **Where it genuinely cannot be done: the recorded leg, and the rule is named.** `record_launch`
    (`crates/lichen-compute/src/compute.rs:4885-4892`) reads the extent on its own and accepts **a
    literal or a value already in the graph's table**: *"a dispatch's count is …, and a count has to be
    a literal or one of this function's arguments: a graph's extent is either known while it is built or
    read from its own value table at run time"*. **Measured**: inside `compute.graph`, a dispatch at
    `.n a.n` — the graph function's own argument — records and the program prints `raw ?: ?a`; the same
    dispatch at `.n (a.n + 1)` does not record, and the surface reports the graph's own failure rather
    than the count: *"this function's return is not a value a graph can hand back, so there is no graph to
    record: a graph's value table holds what its dispatches produced and the arguments it was run with, and
    this body's own value is not one of those"*. **So a growing loop is a `plrun` loop and not a
    `graphrun` one**, and ~~the sentence above is quoted from the source rather than captured from a run:
    the CLI's graph body shapes that reach the count all fail on the graph's return value first, and no
    probe of mine surfaced the count's own sentence.~~ **What is written now**, measured at
    `dev@dd13f417` ([recorded-extent-count](recorded-extent-count.md)): **the count's own sentence is
    reachable from the CLI, in three spellings**, captured from a run rather than quoted —
    *"error: compute.graph: a dispatch's count is Str("five"), and a count has to be a literal or one of
    this function's arguments: a graph's extent is either known while it is built or read from its own
    value table at run time"*. **So the first half of this paragraph stands and the second does not.** A
    count computed *inside* the recording is still intercepted by the return-value rule first, which is
    what `.n (a.n + 1)` measured here and what the slice confirmed in every shape it tried; what the
    slice adds is *why* the rule is there — **principled**: the graph's value table holds a caller's
    arguments and a dispatch's outputs and **no node kind produces a number**, so a count the host
    computed while recording has nothing replay could re-read it from. **And two counts that do
    record**, which this paragraph did not separate: the count **as the graph's own argument** (as
    `.n a.n` measured here), and a count **computed on the host** before recording. What is left of
    the old sentence is the *diagnostic*, not the rule — `{found}` is a `Debug` render, so an
    array-valued count prints an arena address.
    **Both diagnostics are now worked** ([recorded-extent-count](recorded-extent-count.md) §"Fixed",
    `feature/rcd`): `{found}` is named through `graph::describe_low_value` — *"a dispatch's count is a
    tuple or a struct value"*, not an arena address — and the return-value sentence now says it is a
    symptom when a dispatch inside the body refused first (*"… is not one of those: a dispatch in this
    body refused before that, and this is only what it left behind"*). **The rule is unchanged.** What
    the paragraph above still says holds: a count computed *inside* the recording is still intercepted
    by the return-value rule first, because that dispatch's operator is **never demanded** — the count
    cell is undecided — so the count refusal is never produced to travel. That is now measured rather
    than inferred, and closing it needs a second reading of the count outside the VM's demand path.
  - **Two bounds that are rules rather than walls.** The extent is still read as an **`Int`**, and a
    `Float` first leaf is refused by name (`scalar_leaf_classes`, `compute.rs:1850`), so *"the extent
    follows the state"* does not mean any value; and a count past **`MAX_PARALLEL_ELEMENTS` = 2^20** is
    refused before allocation (`compute.rs:5866`), which is the same ceiling every other launch has.
  - **What this does not measure.** **The device leg.** Every answer above is `cpu`, and a device run was
    not attempted: the CLI installs no backend. `run_on_installed_backend` takes the extent as the run's
    own `count` and the remaining leaves as push-constant scalars (`compute.rs:5798-5804`), which is the
    same split the host path uses, so the shape is expected to carry over — **expected, not measured.**
- **What a real decode loop still lacks.** **The sampler is no longer a wall** (`examples/sample.lichen`):
  `ln` is a named operator (`82640443`) and **greedy sampling needs no transcendental at all** — a row of
  integers, a comparison and a position. Three things it measured instead, all of them about the
  surface and not about `ln`: **a `@loop` fold's result is one scalar** (the conversion's domain has one
  leaf), so a fold that carries a pair returns the pair's position and drops the maximum; **a `Select`
  inside a fold's step is refused**, with a diagnostic that names a cause it is not, so the selection is
  written as arithmetic on the comparison's `0`/`1`; and **ties are the body's choice, not the operator's**
  — `>` over a countdown fold answers the highest position and `>=` answers the lowest, both measured. **A
  stop condition is a wall** (item 1 above): a loop that cannot be told
  when to stop can **unroll but not generate**. **Corrected by measurement: the stop condition is not a
  wall at all, and the quote above was wrong about it** (see the `generation` row and the correction in
  item 1). A recursive function **does** stop on a runtime value: the guard reads a value a kernel
  sampled and the loop steps until that value is the sentinel, which is the generation shape verbatim,
  and it answers. **What generation hits is the checker's `WorkBudget` and nothing else** — an unmarked
  recursion reaches the nesting bound at exactly **500** trip counts and a `@loop`-marked one at
  **2 000**, both refusals worded as non-termination. **A growing cache is mostly work** with one wall
  inside it — an **extent that follows the state** is not writable, so a fixed large extent plus masking is
  the only spelling and it costs the lanes. **Per-token rows are work** (both steps append the same row,
  because `nk`, `nv` and `q` are top-level constants). And **`wontfix:D9`'s depth is the obvious thing to
  hit**: the recursion here is depth 2, and a loop of hundreds of steps is unmeasured. **That last
  sentence is measured now** — a buffers-derived stop condition runs to **500** steps unmarked and
  **2 000** marked, so `wontfix:D9`'s stack paths are not what a generated loop meets; the budget is, and
  it is a pair of constants rather than a language rule.
- **A growing cache is work, and the wall inside it was the spelling, not the capability** — measured, and
  the sentence below is what it displaced (`extent` slice, `dev@81e45151`, **host-only**: the CLI installs
  no compute backend, so every verdict here is a `cpu` answer). **What was written**: ~~**A growing cache
  is mostly work** with one wall inside it — an **extent that follows the state** is not writable, so a
  fixed large extent plus masking is the only spelling and it costs the lanes.~~ **What is written now:**
  the extent **does** follow the state on the direct `compute.plrun` path, in three spellings and their
  controls, and masking is a **choice** rather than the only one. **What survives is the cost, and the
  recorded leg.** The full programs, their answers and the one rule that still refuses are in the finding
  below.
- **A second note's table is stale where its own correction is right, and this measurement is the
  evidence** (`stop` slice, `dev@4b9bd7a9`). `docs/notes/loop-conversion.md:149-154` still carries the
  earlier reading — *"| `loop` | decidable | **expanded** — the default still wins when it works" |* —
  while §8.1 at `:422-425` corrected it to *"`@loop` means 'this is a loop', full stop"*. **Measured**: a
  buffers-derived stop condition at **600** trip counts **answers** when the recursion is marked and is
  **refused** when it is not, which is §8.1's column and not §1.1's. **So the marker, not the guard, is
  what a generated loop needs**, and the table a reader would consult first is the one that is wrong.
  Reported rather than fixed here — `loop-conversion.md` is a second document and this slice corrected
  one — but it should not be left to be found again.
- **A chain is possible, and its cost is one division's placement.** Four of `decode.lichen`'s five stages
  are a landed kernel byte-for-byte (softmax's two) or with only the binder and parameter renamed (the
  append and the score). **Only the last stage changed**, and it is not free: hoisting the softmax out of
  `attend` gives `[8.888384, 10.773098]`, while `attend` **verbatim** over the same scores and the same
  values gives `[8.888383, 10.773099]` — **a last bit in both elements**, because hoisted each weight is
  divided once and the sum is exact, while inline one division divides an accumulated sum. A chain has to
  **choose**, and the choice is visible in the answer.
- **The transpose between a position-major cache and an output-major operator is the caller's, and getting
  it wrong is silent.** The score stage reads `p · dim + d` as GEMV's own `i · kdim + (s(0) − 1)` (output =
  position, fold = head dimension) while the weighting stage reads the same buffer as
  `(s(0) − 1) · dim + column` (output = column, fold = position). Written at the score stage's form, the
  weighting stage answers `[6.0754085, 33.36727]` — wrong in both elements, **no error, no refusal, no
  report line** — and **there is no spelling that transposes a buffer**.
- **A chain hides a wrong extent worse than a single kernel does.** With `.kdim` first, the launch runs two
  lanes with `n = 4` as the stride, two slots read past the buffer and answer `0`, and the chain still
  answers `[7.425879, 8.748409]` — plausible, with no refusal, *because the wrong scores are still a
  softmax*. The last stage cannot show it at all: its extent and its divisor are both `2`, because a
  one-query step's output extent **is** the head dimension.
- **`dim` is carried as three unrelated literals** (`.dim`, `.kdim`, `.dim`) and `.kdim` is the **stride**,
  so a mismatch reads the cache at the wrong pitch, silently.
- **Where the loop wall actually bites — not where this note first said it did**
  (`wall-claims-audit.md` §3, §4). Carried state is expressible inside one program, and an `N`-step loop
  **is written once**: a lichen function recurses and may dispatch a kernel on each step, so this file's
  shape is not repeated `N` times. What is missing is narrower and it is about **silence**: a
  **loop-carried** value cannot advance a launch, and the failure is **silent for a recursion state** and
  loud for a parameter. `dim` is still carried as three unrelated literals (`.dim`, `.kdim`, `.dim`) and
  `.kdim` is the **stride**, so a mismatch reads the cache at the wrong pitch, silently.
- **A hypothesis worth testing, and it is now measured and it is false** (`device-leg-of-the-ladder.md`,
  RTX 3060 Laptop, `feature/dleg` off `dev@0ec49dbf`). The original text: *the weighting stage is **not**
  the GEMV refusal — its reduction is *longer* than its output — so **a chained step may reach a device
  before a lone GEMV does**. Unverified: the chain's device and graph legs are inferred from the KV-cache
  and GEMV measurements, because the CLI installs no compute backend.* **Measured, it does not.** The rule is
  one — `InputShorterThanCount`, which compares an input's **length** against the **launch count** rather
  than against the addresses the body reads — and the chain's **first** kernel is the append, so
  `decode_loop.lichen` refuses on the append's own sentence (*"input buffer 2 holds 2 element(s) but the
  run covers 8 index/indices"*) and never reaches the score stage. A lone `score_raw` refuses on the same
  rule with the same two-over-four numbers. **The weighting stage is still not the refusal — it is simply
  never asked.** The half of the hypothesis that survives is the reason the whole thing reads as a
  *spelling* gap: padding the append's incoming row to the count makes it dispatch and agree with `cpu`
  element for element, so the two-input shape is not the wall. What the ladder met instead, on the
  operator this file's ladder ends with, ~~was a **driver that takes the process down** (`0xc0000005`)
  on a module the emitter produced without complaint — see that note's §1 for the bisect.~~
  **That was repaired** ([dispatch-crash](dispatch-crash.md) §7, the missing `OpConstant`), and
  **the ladder's last operator now runs on a device bit-exact**: `examples/generate.lichen` answers
  `([1.0, 2.0, 2.0, 0.0, 3.0], 3, [-1.0, -4.0, -7.0, 3.0])` on both backends with no ulp distance
  — see [device-leg](device-leg-of-the-ladder.md) §5.
- **The length rule below has since been removed** (the owner's decision; the readings are kept as
  the record of the rule as it stood). A host input shorter than the run's count is **not refused**
  any more: the device uploads it at its own length and a lane above it reads the cleared padding as
  `0`, which is the host path's own zero-fill, so the two backends answer one answer. `RaggedInputs`
  was removed with it, because its premise — *a dispatch reads every input to `count`* — was never the
  body's rule. **The four operators that rule refused now run on a device**: `gemv`, `kvcache`,
  `decode` and `decode_loop`, measured element for element against `cpu` in
  [device-operator-sweep](device-operator-sweep.md)'s re-measured table.
- **The rule's own shape, corrected, and it is narrower than the sentence above** (`device-leg-of-the-ladder.md`
  §5, RTX 3060 Laptop). `InputShorterThanCount` compares a **host** input's **length** against the
  **launch count** — the word *host* is load-bearing and was left out above. `staged_bytes` is
  written over `BatchInput::Host` alone (`crates/lichen-compute-gpu/src/dispatch.rs:965-987`), and
  a launch output is `BatchInput::Produced`, which stages nothing and so has no length to check.
  **Measured consequence: a one-element kernel output feeding a five-index launch is not what
  refuses.** `examples/generate.lichen` samples one token into one element and feeds it to the
  append unchanged, and the refusal moved to a different input when that input was made a host
  buffer instead.
- **Not a trap, and the parenthesised form is what the example already uses** (`025d2e17`). The report that
  `.at (s(0) - 1)` fails to parse **was wrong**: the grammar of record
  (`docs/language-spec.md:96-97`, `farg := '.' name expr | expr`, implemented at `parse.rs:1153-1168` with
  the value read at full-expression level) accepts a parenthesised value in **every** argument position
  measured — instantiation fields, indexes, type access, native calls, arrays, one-operand keywords and slot
  reads — and `examples/decode.lichen:248` already uses the parenthesised spelling. What the slice hit was
  **one missing `)`**. The measured facts about that: an unclosed delimiter is reported **where parsing
  stops, never where it was opened** (general, not specific to `.at`), and a deep cascade can put a *later*
  line first because diagnostics are emitted in **chumsky's order** (`parse.rs:164`, loop at `:171-176`) —
  the first error at `236:9` is genuinely the furthest failure, so sorting would move it *away* from the
  edit. `max`/`min`'s asymmetry (a two-operand keyword takes atoms) is their own stated rule
  (`parse.rs:493-496`). **Two decisions this leaves open**: an **unclosed-delimiter diagnostic** — one new
  message class on every unbalanced input, on a surface `docs/notes/parse-clean-probe.md:33-38` reserves, so
  it was not taken; and **four stale parser expectations on `dev`**
  (`broken_statements_are_recovered`, `index_errors_carry_spans`, `parse_errors_carry_spans`,
  `statement_errors_carry_spans` expect a label set without `'sin', 'cos'` while the grammar registers both),
  pre-existing and left in the owner's hands.
- **Leaf one is positional even with no coordinates; leaf two only matters where coordinates are used.**
  Measured on GEMV: `.n, .kdim, .spare` is **identical** to `.n, .spare, .kdim`, while `.kdim` **first**
  answers `[14.0, 38.0, 62.0]` instead of `[14.0, 32.0, 50.0, 68.0]` — **with no refusal at all**: the
  launch runs over `kdim = 3`, `n = 4` becomes the stride, and `x`'s fourth element reads past its own
  three and answers `0` (`compute.rs:6234`). The **first** leaf governs through `scalar_leaf_classes`
  (`compute.rs:1849`), and the second is reached **only from `coordinate`** (`body.rs:788`) — so "two leaves
  are positional" is exact only for a body that calls `compute.row`/`compute.col`.
- **The graph path could not carry a GEMV, and the check that refused it is gone.** `RaggedInputs`
  (`crates/lichen-graph-ir/src/run.rs`, with `ragged_host`) refused the run at submit: *"node 0's inputs
  are not all the same length: 48 and 12"* — it compared **payload bytes** — and a GEMV's two inputs
  (`M · kdim` against `kdim`) are **never** the same length except at `M = 1`, so **the check's own
  sentence was false of a legitimate body**. It was removed with the length rule (the correction above),
  and **what a graph step needs in its place is a spelling, not a device feature**: the return must be
  annotated. Measured on the RTX 3060 with `crates/lichen-compute/examples/graph_leg_probe.rs`:

  | graph argument | reading |
  |---|---|
  | a scalar alone | **runs** — `compute.collect (compute.graphrun built 3)` answers `[20, 22, 24]` |
  | one buffer, read | *"expected ?, found ?"* |
  | one buffer, **never read** (all-`Int`, no arithmetic on it) | *"expected ?, found ?"* — the same |
  | GEMV's two buffers, read, unequal (12 and 3) | *"a `compute.collect` reads every element of one buffer, and its buffer position holds nothing (the empty value of a failed read)"* |
  | non-square matmul's two, unequal (6 and 9) | *"expected ?, found ?"* |

  **The `expected ?, found ?` rows are answer collisions, and the collision is intended.** A
  correspondence evaluated twice mints a cell each, `write_node_answer`
  (`lichen-lowlevel/src/equality.rs`) unifies the new answer with the one already propagated into the
  class, the two element placeholders are `GraphValue(1)` against `GraphValue(0)`, and the engine
  reports the disagreement — the maintainer's ruling is *"答案相撞就 unify error 啊，有什么问题"*
  ([answer-collision](answer-collision.md), which also records the two diagnostic defects fixed on the
  way: the missing span and the dropped `value_a`/`value_b`). **And what decides which rows collide is
  the step's return, not its argument: annotating it is the spelling.** The same one-buffer program that
  collides as above **runs** when the step's last expression carries its type —
  `compute.plrun k2 (…) : Out2` answers `[2, 4, 6]`, the argument buffer `[1, 2, 3]` doubled twice — and
  a two-dispatch step that reads the argument buffer answers `[4, 8, 12]`. A `~ "claim:…"` mark does
  **not** substitute for the annotation (the claimed variant still collides). So the collision is the
  recording's re-drive minting a second answer **when the return is not nameable in the first
  evaluation**, which is the compute side's decision, and **an argument buffer is not a blocker on the
  graph leg**: the probe `crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs` runs
  both of those shapes on the device. The table's `expected ?, found ?` rows are the unannotated
  spelling, and the device path is not implicated either — the type system's owner reproduced the same
  collision through the CLI with **no compute backend installed**.

  **The suite now says the same thing, and the owner has ruled it the contract**: five of its graph-leg
  programs — `automatic_tiling.rs`, `combine_fold.rs`, `fold_datum.rs`, `identity_from_body.rs` and
  `pack_placement.rs` — wrote the step's `plrun` without that annotation, and **thirteen of their
  assertions were red on exactly this collision** (`ComputeValue(GraphValue(1))` against
  `ComputeValue(GraphValue(0))` at the step's application). One `: Out` per program, five lines in all,
  turned all thirteen green: the suite went from `206 passed / 15 failed / 2 ignored` to
  `219 passed / 2 failed / 2 ignored`, the two remaining failures being different causes. So an
  **unannotated step return is not a spelling this suite accepts**, and a graph leg written without the
  annotation will collide rather than run — the re-drive is deliberate and the collision is its
  consequence, not a bug to be found in the body.

  **Two failures are left, and neither is the annotation.** Measured on `dev@8b3cbfa9`, the suite is
  `219 passed / 2 failed / 2 ignored`:
  - `graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches` compares the stub backend's log
    for a plain run of the step against the same step recorded by `compute.graph`. Its two readings are
    now `left = ["over [0, 3) with 0 input(s) of length 0", "over [0, 3) with 1 input(s) of length 3"]`
    and `right = []`: **the graph dispatches twice and the plain run dispatches not at all**. Annotating
    the step's last `plrun` (which the re-drive otherwise doubles) halves the graph's trace from four
    lines to those two and does not fix the assertion, so the open question is the *plain* leg — a
    program whose answer is never collected performs no dispatch, and whether that is the intended
    laziness or a lost forcing is **not diagnosed**. The test's own premise (`plain.len() == 2`) says
    the author measured two, so something changed on the forcing path and not in the graph.
    **Two hypotheses were then tested and excluded**: collecting the plain answer
    (`out = compute.collect r.w` instead of returning the step's struct) leaves `plain` empty, and so
    does deciding every class in the shared `KERNELS` (`compute.Buf Int`, `compute.KT Int`) — with both,
    the plain leg still logs nothing while the graph logs its two lines, and the plain run's root comes
    back as an array with **no diagnostics**: a `compute.collect` that answers without a dispatch, which
    is the reading to start from rather than a wrong answer to the test's question. The two edits were
    reverted, so the test still spells the program it was written with.
  - `graph_structure::a_parameter_is_bound_by_the_position_the_source_names_and_not_by_read_order`
    refuses to check at all: *"compute.parallel: no output of this parallel index function carries a
    decided scalar class: its codomain names 1 output(s) and none of them is a `compute.write` of a
    decided scalar … An undecided class is not an `Int` (`docs/notes/compile-order-class.md` §3)"* at
    `span: Some((14, 30))`. That is the compile-order class report the type system's owner landed, so
    the program is spelling a `compute.Buf _` whose element class nothing decides — **the report is
    right and the test's program is the lenient spelling**, but whether the fix is a decided class in
    the program or a normalize on the classing path is the owner's call and is not made here.

  **What is still open is GEMV's own program**, and it is not the collision: it refuses at stage `Check`
  with the collect's *"empty value of a failed read"* (span `None`) and *"this value is not a container"*
  at the `collect` line, and it does the same with `x` padded to the count — so neither the short input
  nor the argument binding is what refuses there. **The fold is excluded, with the program fixed**: the straight-line variant as first written put a line-leading `+` where the grammar wants a separator and refused at `Parse` (the earlier reading was retracted for that reason); rewritten as one line it parses and refuses the same way -- `span: Some((18, 17))`, the collect sentence, stage `Check` -- so the fold is not what refuses GEMV. **Nor is it two buffers, or `Float`**: a two-buffer
  annotated step reads both argument buffers and answers `[11, 22, 33]` in `Int` and `[11.0, 22.0,
  33.0]` in `Float` (`two_buffers_annotated`, `two_buffers_float`). **The computed read address is not
  it either**: the same GEMV body with both reads at the lane index, `Int`/`Float` aside, refuses the
  same way (`gemv_at_i`, run once and not yet a committed probe case). **What separates GEMV's probe from
  every shape that runs is the collect, not the body or the type**: the identical two-buffer program
  *runs* when the graph run is collected directly — `compute.collect (compute.graphrun built (3, p, q))` —
  and *refuses* when the same value is bound and its field collected (`out = (compute.graphrun built
  (3, p, q) : Out2)` then `compute.collect out.w`). GEMV binds and collects a field, and every refusing
  variant in this probe does the same; the spelling that works is to collect the graph run directly. The graph leg is still the only path that consults the fusion floor, and it now carries
  two-input steps whose returns are annotated.
- **The fusion floor now reads real operators, and it refuses both shapes by name.**
  `crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs` drives the two committed
  kernels — `examples/gemv.lichen`'s body and `examples/kvcache.lichen`'s — through `compute.graph` with
  `install_floor_for_run(&[])` and the step's result collected directly. Every answer is the recording's
  answer, which is the floor's own contract: a refused region changes nothing.
  - **GEMV, one node.** `region 0 (nodes 0): refused: RegionIsNotAPair: the region holds 1 instance(s),
    and a merge is one producer and one consumer`, then `node 0: refused: BeyondANode: the instance at 0
    claims the role(s) element beyond combine and identity, whose code reads the value another node
    produced, and this reader names no producer`. **The second sentence does not describe GEMV**: its
    `element` code reads `.w` and `.x`, both graph arguments, and the node has no producer at all. So a
    kernel whose surface-forced claim carries `element` — `Reduction(element)` is "a small lie the surface
    forces" — is refused by the tiling pass with a cause it does not have. The answer stays
    `[14.0, 32.0, 50.0, 68.0]` with and without the floor.
  - **The decode pair, two nodes** (append, then scorer). `region 0 (nodes 0, 1): refused:
    ProjectionNotBridged: the instance at 0's element projection did not bridge: the projection selects
    between two values, and the symbolic language has no conditional`, the same sentence for the other
    instance, then `BeyondANode` again. The cause is the append's `if fresh then … else …`: a `Select` in
    the very expression marked `element`, so this refusal is the **append's spelling** — copy-or-replace is
    what a cache write is — and not the scorer's. Both stages refuse, so nothing merges and no answer
    changes. The tiling pass's **node lines name the instances in the opposite order from the region
    line's** (`node 0 … the instance at 1`, `node 1 … the instance at 0`), and the instance id is the
    process-wide registry id, so a reader cannot tell which node is meant.
  - **The two-kernel decode step runs on the device through the graph leg.** At `capacity = 4, dim = 2` it
    answers `[3.0, 100.0, 11.0, 15.0]`, the hand arithmetic (`K' = [1, 2, 100, 0, 5, 6, 7, 8]`, the fresh
    row's second column reading past `nk` and answering `0`, times `q = [1, 1]`), identical with and
    without the floor and identical to the direct chain. So the decode step's first two kernels reach the
    device through `compute.graph`; it is the floor that declines to fuse them and the position that
    nothing advances.
  - **And the append loses one cache slot on the device at `capacity = 8, dim = 1`** — a silent
    device/host divergence this probe found by accident, and the reason the pair above is pinned at
    `(4, 2)`. The device answers `[1.0, 100.0, 3.0, 4.0, 5.0, 0.0, 7.0, 8.0]` where `cpu` answers
    `[1.0, 100.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]`, in both cache halves, with no refusal and no report
    line. The graph leg reproduces the direct chain exactly, so the graph is not implicated. **The
    mechanism is the tail-lane obligation, not this operator**, and it is now **fixed**
    (`zz_append_slot_probe.rs`, and `lichen-compute-gpu.md` §"A computed write address is not protected
    by the padding"): a surplus lane ran the whole body unguarded, its fabricated `(column, position)`
    put its write at `position · dim + column`, which is **inside `[0, count)`**, and when its
    `position` equalled `t` it took the fresh arm and wrote `nk[column]` there. A `nk` with eight
    elements makes it visible as the wrong value: the device answered `47 = nk[4]` where the host
    answers `60`. The zeroes were the same write with a one-element `nk` (the padded input answered `0`),
    and the outcome was a **race**: three identical runs clobbered `K`'s slot 5 every time and `V`'s
    once out of three. So `(4, 2)` agreeing was the race lost, **not** a safe configuration, and the
    committed KV-cache example's "agrees element for element" held of the runs taken and was not a
    guarantee. **How it is fixed**: the entry point's body is wrapped in `if (index < extent)`, where
    `extent` is push-constant member `0` — the `count` the run dispatches over — so a surplus lane
    executes no instruction of the body and writes nothing; the write address is never computed at
    all. The probe now answers `[10, 100, 30, 40, 50, 60, 70, 80]` on **both** backends in the same
    run, `60` in slot `5` for the eight-element `nk`, and the three repeats agree with each other.
- **A real operator is a second witness to the compile-order class defect.** With the two inputs
  equalised, submit is refused by the **class** arm: *"instruction 0 mixed an integer and a float in one
  operation … this is a malformed fragment rather than an unsupported shape"* — the same
  `unwrap_or(ScalarClass::Int)` chain as attention's `attend` (`compute.rs:2424-2430`), and now reached by
  an operator nobody had to hand-build. It refuses **earlier** than the KV-cache slice's eight-byte
  payload count; those are different bugs and are not conflated. **The type system's author has since ruled
  on it** (`44190cc1`, recorded in `docs/notes/compile-order-class.md` §8): a class that is *determinable*
  is meant to be decided by checking or by a normalize, and a later phase **reads** it rather than creating
  it; a graph step recorded while the parameter reads `Undecided` must **report** it, and that is the
  **record** path's job alone — refusing inside `walk_role` instead fixes the pinned assertions but breaks
  four targets and 38 tests, because that site's `Undecided` is the ordinary "annotation is still a cell"
  case that must stay lazy (`compute.rs:1178-1195`); the JIT may not write back into the type system; and
  **carrying more does not reach this cell**, because the frozen template's own node read is `None` — there
  is nothing on the template side to carry. So the gap is the **order in which the recording reads it**.
- **What stands between here and a decode step**, measured and then re-measured: the two walls this bullet
  used to name — the graph leg refusing the two-input shape, and the device leg refusing a reduction shorter
  than its output — are **both gone**. The append-then-score pair runs on the device through `compute.graph`
  and answers `[3.0, 100.0, 11.0, 15.0]`, element for element the direct chain. What remains is **the
  position**, which no source-level state can advance, and **the floor**, which refuses both real shapes by
  name (the finding below).
- **The write side has three measured rules, and the first is silent on both backends.** (1) **A write past
  the count is dropped in silence**: the host output is sized at exactly `count` (`compute.rs:5979`) and a
  write outside the span is dropped (`compute.rs:6255-6263`, whose own sentence is *"a write outside the
  span is dropped rather than aliasing another worker's slots"*). Measured with the kernel a decode loop
  actually wants — one lane per **column**, writing at `t · dim + d` = 4 and 5 over a count of 2 — the
  `cpu` path answers `[0.0, 0.0]` **and so does the RTX 3060**: no error, no refusal, no line. That is why
  the committed KV-cache operator rewrites the whole cache instead of one row. (2) **Capacity against
  filled prefix is no longer asymmetric** (the device check was removed — see the correction above):
  the device used to refuse a host input shorter than the count while the **host path silently
  zero-fills the tail** (`compute.rs:6234`); both zero-fill now, so a cache holding only its filled
  prefix `[1, 2, 3, 4]` answers `[1.0, 2.0, 3.0, 4.0, 100.0, 200.0, 0.0, 0.0]` on **both** backends.
  (3) **A written-but-never-read
  region is not observable**: `collect` reads every element of a buffer exactly `count` long
  (`compute.rs:1550-1621`) and on the device the output is allocated `padded` = 64 (`dispatch.rs:1052`)
  while `collect` still reads `count`, so the surplus lanes' slots are written and never handed back.
  **The audit re-measured all three** (`wall-claims-audit.md` §6): the **drop is a spelling gap** — raising
  the count so the write lands inside it works, which is why the committed operator rewrites the whole
  buffer rather than one row — and **a third silent no-op sits beside it**: aiming the write at the
  **input** buffer, the only in-place spelling, **runs and does nothing**, collecting the cache unchanged
  with no error and no report line. **Corrected later by measurement (`4891068f`): that sentence is true of
  what a reader sees and false of the mechanism.** The refusal for that spelling **already exists** —
  `compute.rs:2465`, the `None` arm beside the per-output `write_value_node` gate, pinned by
  `crates/lichen-compute/tests/lichen_compute/compute.rs:2191` — and it **fires** as soon as the same program collects the
  kernel's *output* instead of its input. What used to hide it was a second, unrelated rule: `render_build`
  (`crates/lichen-language/src/run.rs:152`) gated the report on `produced_nothing &&`, by an `# Invariant`
  saying *"a runtime refusal explains a value that never arrived, so it is reported only when evaluation
  produced nothing"*. Here the root collects the **input** cache — a value that arrives whatever the kernel
  did — so the refusal was computed and **withheld**. That gate has since been **deleted, on the user's
  decision**, so every runtime refusal reports and this spelling now refuses by name where it once answered.
  The other two shapes are not gaps: an in-place write bound but unused never reaches the graph, and one read
  back is refused by the read gate. So the finding was not a missing refusal but **a refusal a language-wide
  gate withheld**. The reproduction is committed as
  `crates/lichen-compute/tests/lichen_compute/inplace_write_refusal.lichen`.
  `code-audit.md`'s `wontfix:D17` is a contract about what a write **is** — *the value of an output
  position and not an effect* — and a write to an input is exactly the spelling that sentence declines.
- **The graph path refuses this operator's run, and the arithmetic looks like eight-byte elements**
  (measured, not diagnosed). The recorded graph is one node holding the extent plus **all three** runtime
  scalars. At submit it refuses *"input buffer 0 holds 4 element(s) but the run covers 8"*; with 12-element
  inputs *"holds 6"*; with 16-element inputs there is **no refusal** — 32 → 4, 48 → 6, 64 → 8, i.e.
  **payload bytes counted in eight-byte elements while every buffer is `Float`**. For contrast, the same
  crate's one-buffer `Float` graph **with no runtime scalar** dispatches and answers correctly, so the leg
  works and the refusal is this operator's. `ragged_host` (`graph-ir/src/run.rs:520`) compares payload
  bytes and its own `# Invariant` **admits a width-ratio blind spot**, but the counting site was not
  proven. **Both readers have since moved**, so this records the graph leg as it stood: the class defect
  was fixed (`7eedb125`, [compile-order-class](compile-order-class.md)) and the length check was removed
  (the correction above).
- **What a decode loop still needs, measured — and corrected** (`wall-claims-audit.md` §3, §4). Carried state
  across calls: **yes** — a `plrun` output is a buffer a later `plrun` takes as an input, and the committed
  operator's output is shaped to be fed straight back. A second kernel reading what the first wrote:
  **yes**. **A loop: yes, and the claim that there is none was wrong** — a lichen function **recurses**, and
  a recursive function may dispatch a kernel on each step, so an `N`-step decode loop is written **once**
  instead of `N` times (measured end to end). The honest cost: the trip count is the **function's own
  state**, so nothing else in the program can see it, and every launch inside it inherits the finding
  below. **Advancing the position: a top-level binding already does it** — a binding becomes a runtime
  scalar and the kernel follows it; what is absent is a **loop-carried** value, whose failure is **silent
  for a recursion state and loud for a parameter**.
- **The `examples` binary cannot currently give a verdict at all**: it panics with *"invalid SlotMap key
  used"* (`crates/lichen-lowlevel/src/evaluation.rs:517`) **on `dev` itself, with the new example removed**
  — a pre-existing failure that **masks** the four drift names the runner otherwise reports.

- **A class had two sources of truth, and the emitter's was the wrong one** (fixed at `b6dea46c`). The
  GPU emitter's `value_class` derived a **result's** class from **its operand 0**
  (`crates/lichen-compute-gpu/src/spirv.rs:2118` before the fix) — correct where the subject really is an
  operand, wrong where it was asked to answer for the result — while `KernelInstr::own_class`
  (`crates/lichen-kernel-ir/src/lib.rs:918-922`) had **stated the right answer all along**
  (`Conv { to, .. } => Some(*to)`), and the emitter's own slot for a crossing's result was already inserted
  as `to` (`spirv.rs:1656`). So **the classing disagreed with the emission it was guiding**, and nothing in
  the GPU crate ever read `own_class`. The chain is exact: `sqrt (int2float k.dim)` inlines `sqrt_body`
  with the definition's parameter **renamed to the crossing's value id** (`body.rs:452`), so the `Select`
  in `unary.rs:212-217` holds the `Conv` itself as one arm; `value_class` read that arm through
  `operand_class(Conv, 0)` → `from` → **`Int`**, the other arm is a `Float` constant, and `select_class`
  refused `[Int, Float]`. `int2float k.dim + 0.0` dodged it because a `Bin` states its own class and the
  crossing stops being a value anyone asks about. The fix asks `own_class` first; **no check was widened**
  (two genuinely disagreeing arms still refuse by name, and a test pins that), no `Invariant` changed —
  `value_class`'s own doc already said *"a value's class is its **definition's**"* — so the code now
  matches the sentence that was already there.
- **The check this argues for, and it is cheap**: for **every** `KernelInstr`, the class the GPU emitter
  reports for a value it produces must equal `own_class`'s answer for the same instruction — exhaustive
  over the variant list, needs no device, and it would have failed the day `Conv` was added. As the slice
  that fixed it put it: *"neither check is about `int2float`; both are about **a class having two sources of
  truth**."* A second, weaker one: the class the emitter gives a value's slot must equal the class
  `value_class` reports for it. Either would catch the next instruction of the same shape.
  **That guard exists now** (`f3124d42`): ten probes over five variants (`Const`, `Bin`, `BufferReadCall`,
  `SliceReadCall`, and `Conv` in both directions), with **no new API** — `select_class` already speaks the
  class aloud, so the instrument is a `Select` whose arms are the probed value and a constant of the class
  `own_class` states — and a **compile-enforced ledger**: `every_variant()`'s exhaustive `match` over
  `KernelInstr` means a new variant must be classified, and the eight it does not probe (`Exp`, `Sqrt`,
  `Floor`, the two write calls, `I32WrapI64`, `Select`, `CallKernel`) each carry their reason in the test.
  **Reverting the one-line fix makes it name four disagreements, not one** — `BufferReadCall`,
  `SliceReadCall` and `Conv` — because the removed line answered `Int` for **any** read at `Float`
  (`operand_class(op, 0)` is the access chain's index), so **three of the four were latent and invisible**.
  **And it found the next one of the same shape, which is now fixed — with the emission, not the classing, as
  the truth** (`3f3706a1`). `CallKernel` is the one variant where `own_class` states `None`, so `value_class`
  fell back to `operand_class(op, 0)` — which for a call reads the callee's **first parameter leaf**
  (`spirv.rs:2200-2203`) while the emission gives the call's result slot the callee's **result** class. Four
  reasons settled it: **both emitters already type a call by its callee's `result_classes`** — the GPU one at
  `spirv.rs:1901`/`:1920`, and the wasm backend at `lichen-compute/src/compute/wasm/lower.rs:308-316`, whose
  own `# Invariant` says *"a cross-kernel call is typed by its callee's own `result_classes`"* — so **only
  `value_class` dissented**; `operand_class`'s own contract is *"an operator's operands are the class it
  names"*, a fact about the value in the position asked of a **different** value; `own_class` returning
  `None` for a call is **correct, not a gap** (`arity()` returns `None` for the same reason,
  `kernel-ir/lib.rs:896-909` — the class is the callee's, so the instruction has none of its own); and `Conv`
  is the same rule one level down (`b6dea46c`: a `Conv`'s value is `to`, not `from`). So the `b6dea46c`
  shape did **not** transfer. The fix reads the callee's result list and **drops the `operand_class(op, 0)`
  fallback** — reachable only for `CallKernel`, and *"leaving it open is exactly the trap the guard exists to
  close"*. The two handed rows **swap**: the honest spelling is emitted and the genuinely disagreeing one is
  refused by the sound refusal, and `MixedClasses` became **unreachable for an honest body rather than
  relaxed**. **The guard grew the right thing for it**: its oracle for a call is now the callee's declared
  `result_classes` — *"an input to the compile, not an answer it gives"*, the same move one level down from
  the module's buffer classes — and reverting just the `value_class` arm makes it fail in **both**
  directions. The general lesson, in its author's words: the guard is *"exhaustive over the **variant list**,
  not over the **class sources**"*, and `own_class` being `None` was read as "no oracle exists" when **the
  oracle existed one call away in the callee** — *"making the oracle a per-probe declared source, rather than
  a hardcoded `own_class`, is what closes the class of holes, not the one variant."* Two residuals it states
  plainly: the probe **cannot separate** "read operand 0" from "fell back to the module" (two classes, and
  the probe needs three distinct values, so both wrong answers coincide at the same wrong class); and **no
  `.lichen` fixture exercises the shape** — both rows were hand-built, so whether the frontend can emit a
  `Float`→`Int` call feeding a `Select` with an `Int` module is unmeasured.
  **What the guard cannot catch**, in its author's words: `Exp`/`Sqrt`/`Floor` (whose `own_class` is `Float`
  and whose `operand_class` is `None`, so a regression to `module` would pass unnoticed, and closing that
  needs a `pub(crate)` accessor — more machinery than the guard is worth); the slot-versus-`value_class`
  check, whose absence the `CallKernel` finding proves is a real gap; anything surfacing at a **phi edge**
  rather than a `Select` arm; a bug **inside** `select_class` itself, since the instrument *is* the
  `Select`; and future variants — the match forces *classification*, but nothing forces a *probe*.

- **A `@loop` nest cannot be applied inside another nest's step, and that is attention's real wall** (a rule,
  not a missing feature). Nesting lowers and runs: a `nest3`-shaped program answers correctly, triple
  nesting too. What refuses is a nest *inside a step*, and the mechanism is an ordering one —
  `lower_loop_steps` sets the step's terminator **after** its operands
  (`crates/lichen-compute/src/compute/body.rs:1200`), overwriting the inner nest's entry terminator, while
  demand runs from the codomain outwards (`:222`), so the enclosing step's own value is computed in the
  enclosing block *after* the inner nest returns. Verbatim: *"`Bin(Float, Div)` reads `ValueId(32)`, which
  block 7 receives as a parameter, and block 7 does not dominate block 2"*. That is why the ladder's
  one-kernel attention is unwritable and why `examples/attention.lichen` splits into `scores` then
  `attend`.
- **For a fold-heavy operator, grouping is no longer the wall — the claim is.** Attention's two-kernel
  split **groups** as a weakly-connected component (`region 0 (nodes 0, 1)`) and then the merge refuses by
  name: `ElementWithoutProducer: the instance at 1 reads the one at 0 and its element `d` does not read the
  producer's value`, with both nodes also `BeyondANode`. The reason is structural: here `element` **is** the
  whole fold, so no element expression is left for a producer's value to flow into. The floor can now
  reach a region it cannot cash — which is exactly the gap `docs/notes/two-dimensional-workgroup.md` and the
  licence's own limits describe from the other side.
- **Tiled attention needs a two-index `Shared` exchange, and a silent class must be gated before it.**
  A `Shared` slice is a one-length `OpTypeArray` and `slice_pointer` a one-index `OpAccessChain`
  (`docs/notes/two-dimensional-workgroup.md` §1.1). The rectangle is *not* the wall — that note's §4
  measures `(16, 4, 1)` compiling, validating and running — but the same §4 measures a **staged exchange at
  `count = 64` under a rectangle answering wrong and writing nothing past index 15, with no error, no
  refusal and no report line**. That is the class a tile's partial sums would pass through, so it is the
  first thing to gate, not the last. Two honest caveats from the attention slice itself: its floor
  measurement was taken over a **hand-built** two-node graph (the `fused_region.rs` idiom), and **both of
  the reasons it gave for that have since been corrected by measurement**. First, *"`compute.graph` refuses
  to record a two-`plrun` chain"* is **not what `dev` does**: a chain of two recorded calls was never
  refused — `graph_argument.rs` records 2 nodes, `chain_merge.rs` 3, and the attention chain itself records
  **2** (node 0 `inputs [4, 5]` / count `Value(0)` / scalars `[Value(2), Value(3)]`, node 1 `inputs [7, 6]` /
  count `Value(1)` / scalars `[Value(3), Value(2)]`, 7 inputs, 9 values, `Some([8])` — exactly the wiring
  the calls imply). Second, running the pass over that **recorded** graph refuses it **differently and
  earlier**: `ExtentsDiffer: the producer dispatches over Value(0) and the consumer over Value(1), and a
  merged node has one extent` — because attention's two kernels **genuinely** run over different extents
  (queries × keys = 4, queries × dim = 8), so the hand-built `ElementWithoutProducer`/`BeyondANode` refusal
  was partly an artifact of the hand-built shape. **What actually keeps the recorded graph from dispatching
  is a compile-order dependence in the class inference**: `compute.parallel` compiles `attend`'s fragment
  with `output_classes [Int]` / `input_classes [Float, Int]` where the program declares `Float`, and it
  depends only on which kernel compiled first — a program that *declares* the step compiles `attend`
  **first** and gets `[Float, Int] → [Int]`, while the same kernels without the step compile `scores` first
  and get `[Float, Float] → [Float]`. `graphrun` then refused *"input buffer 1 holds 4 element(s) but the
  run covers 8 index/indices, so a lane would read past it"* (`InputShorterThanCount`, removed since —
  see the correction at the top of this list): the wrong class meant the wrong element width. The chain is
  `compute.rs:2013-2017` — `walk_role` reads a `Buf T`'s element class through
  `low_type_of_slot(module, element).filter(|shape| shape.is_known())` — into **lazily-filled cells**
  (`lichen-highlevel/src/shape.rs:858` → `low_type_of` at `:704`) that **no pass evaluates a type term
  for**; 4738 `refine_class_low_type` calls in the failing program never refine that slot, so
  `compile_parallel_fragment` reads the role table at `:2392` **before** the two passes that could fill it
  (`:2409`, `:2415`), and `None` falls through `body.rs:1821-1827` to `compute.rs:2424-2430`'s
  **`unwrap_or(ScalarClass::Int)`**. Four fixes were attempted and **rejected on measurement** — a
  placeholder class from `roles.input_classes` (no effect: the fragment is compiled at `compute.parallel`,
  `recording = false`), the class route through `class_value`/`class_low_type` (both empty), the term's
  *type half* (which made it `[Int, Int]`, worse), and forcing the slot with `evaluate_node` (no effect) —
  so **no source change shipped**; the defect is pinned instead by
  `crates/lichen-compute/tests/lichen_compute/compile_order_class.rs`, **ignored by default** (the repo's precedent in
  `graph_structure.rs`) so the suite stays green while `--ignored` shows both assertions red. **Still
  open**: which write fills that element cell in the passing program — proved *not*
  `seed_template_term_low_types`, *not* `infer_template_low_types`, and *not* any of those 4738 calls.
  And the `scores` kernel **does** run on the device now: the `sqrt` emitter bug that kept it off was fixed
  at `b6dea46c`.

- **A matmul's claim earns two refusals, and one of them names a mark that is there.** With the floor
  installed, the recorded matmul reports `region 0 (nodes 0): refused: RoleNotMarked: the instance at 0
  claims a `identity` role and its body carries no mark for it` — and again for the node — while the body
  **does** carry `0.0 ~ "identity"`. Nothing merges and nothing tiles, which is right (a naive matmul has
  no row reduction to fuse, since `k` lives inside one lane), so the licence is irrelevant here; what is
  not explained is the sentence. Two facts worth carrying: the **instance id is not stable across runs**
  of the same program (1, 4, 5, 8, 9 were seen) because the kernel registry is process-wide and
  append-only, and `code-audit.md`'s **P1-39** (`compute.call` refused in a parallel body) stands as an
  independent wall for a matmul that wants to call out to a helper.

- **A `compute.write` is the *value* of an output position, not an effect** — so a second write naming
  the same buffer leaves the first a **dead expression**, by decision (`code-audit.md` P1-42,
  `wontfix:D17`) rather than by accident. Measured: two independent writes to one output answer only the
  later one (`raw[raw 0, raw 100, raw 0, raw 101]` at four lanes, uniformly at sixteen), and two writes
  to the *same* position answer the later one alone. The authoring consequence is what matters for a
  pairwise operator, and it is narrower than "a lane cannot write two positions": a **`.to` chain**
  *can* fill both positions of a pair — the second write's target is the first write's **value** — and at
  eight lanes it answers **element for element** what the independent one-lane-per-slot spelling answers.
  What does not hold is contention: a chain's **outer** link has two lanes landing on one slot, the last
  writer wins, and order is not a contract (`tests/lichen_compute/compute.rs` says so in its own comment). **So one lane
  per element is the only spelling whose answer does not depend on a schedule** — which is why
  `examples/rope.lichen` is written that way, as a rule rather than a preference.
- **A refused kernel can be swallowed by a tuple** — a hole that is *not* in `code-audit.md`. A body that
  writes two outputs in the wrong order refuses on a bare `compute.collect outs.c` (naming itself: *"a
  write fills the output its `.to` names, and the codomain's order is not that name"*), but the same
  program ending in **`(compute.collect outs.c, compute.collect outs.s)`** answers **`(raw none, raw
  none)`** — a **loud refusal turned silent** by the runner's laziness (likewise for a tuple of `read`s
  and for three collects; binding the pair to a name and then reading refuses again). That is the class
  this project treats as worst, and it is a candidate slice.

- **The canonicaliser rewrites arithmetic, so a region hash can name what the body does not compute.**
  `exp(d − r)` becomes `exp(d) * exp(−r)` (`lichen-fusion/src/normalize.rs`, `exponent_product`), and the
  hashed projection is the rewritten form — while as Taylor sums the two are **not numerically equal**.
  A digest therefore identifies an algebra, not the body's own expression tree.
- **A `Max`/`Min` fold can now be scheduled** (`9722e681`, the capability half of the `combine` gate — the
  datum-carrying slice, not a loosened gate). A `Max` combine over a describable datum tiles behind the
  same proof and is **bit-exact**, so softmax's numerically stable subtraction can finally take a
  shared-memory reduction and a tile. The seed it starts from is the **body's own `~ "identity"` mark**,
  so `tile ≡ body` holds by construction; what it cannot know is whether that mark is *true* of the data,
  and the pass's own line naming the identity is the mitigation (`79defe18`).
- **Non-uniform softmax is never exact in `f32` here.** Scanning 2394 four-element integer rows, **none**
  reproduce the true `f32` softmax (best gap 7 × 10⁻⁷), because this language's `exp` is a degree-10
  Taylor sum: the true `f32` answer and the program's answer differ in **every** element. So the
  example's declared output is the **program-faithful** value, derived independently by modelling
  `exp_body` and the program's own fold order in `f32` — it matches the printed bits exactly. `m = 4.0`,
  the offsets `−3 … 0` and `exp(0) = 1.0` are the parts that are exact.
- **The device agreed bit for bit.** The two-kernel softmax ran through `compute.graph`/`graphrun` on the
  RTX 3060 and answered exactly the CPU result, which is the claim the floating-point note makes about
  the shared `exp` body; with the tiling pass installed the same run reported the `CombineIsNotAFold`
  line through the run report's own channel.
- **What attention needs, in order.** (1) **A launch that is not one row** — **the capability is there
  and the plumbing has landed** (`f4a4e470`): a two-extent config records and runs on the device through
  `graphrun`, answering the direct run element for element, and the rule is stated as *the extent, then at
  most one runtime scalar, then the buffers*. What is left is the language's **spelling** and the
  **workgroup geometry**; the proposal, its sites and its plan are
  [two-dimensional-launch](two-dimensional-launch.md). **This is the only wall left before attention.**
  (2) **Fan-in** — **the licence is done** (`d364f547`), with chains (`3f8007cc`), and **the grouping
  now reaches it** (`f973544a`). The reach used to be narrower than the licence read: `region_positions`
  grouped by a **forward-only adjacency scan**, so two **independent** producers were **siblings** — the
  second was fed by nothing already in the region, started a new region, and the licence was never
  reached (`RegionIsNotAPair: the region holds 1 instance(s)`). Softmax's diamond grouped only because
  its producers are **chained** (the sum reads the maximum), and `chain_merge.rs`'s fan-in was
  **hand-wired**, so a sibling diamond had never been exercised on a real path. A region is now a
  **weakly-connected component** of the producer/consumer relation — an unclaimed node is still a
  boundary, and inside a maximal run of claimed nodes one backwards sweep through a value→producer map
  with union-find (earlier root winning, so a component is named by its earliest node) is **linear in the
  reads**. **A third wall sat between the grouping and the merge**, and `region_merge.rs`'s `shape()`
  owned it: with the grouping installed the region held three instances and `shape()` still refused
  `RegionIsNotAPair`, so the licence was reached and then dropped. It now accepts a **deliberately
  narrow** predicate — every node but the last feeds the last, and the last reads two or more — so a
  **deeper tree still refuses rather than merging weakly**, and `chain`/`fan_out` are still decided
  first. Measured on the device: a sibling diamond's recorded and merged answers are **bit for bit**
  equal, and the `+1.0 → +10.0` counterfactual inside the composed body answered `(x[i] − 10)/40` — a
  number no recording computes — so the merged node is what dispatched.
  **What still blocks LayerNorm** (`807469f1`) is therefore *not* the grouping: its region now holds all
  three nodes and refuses `ProjectionNotBridged`, because `count = int2float k.n` puts the launch extent
  inside the marked `element` and the bridge refuses a domain leaf there
  (`crates/lichen-fusion-bridge/src/refusal.rs:105-106`). RMSNorm's own pair refuses it **verbatim**, so
  it is a bridge question about naming the lane index in a projection, not an operator's. The two-pass
  spelling stays `ProducerFeedsTwice`.
  The licence was restated **once, in the two places it was stated** (`licence()` and
  `RegionSchema::off_chain_node`): a region two edges *arrive at* is served "exactly when **every one of
  those inputs is named and proven**: a `Between` per arrival, each over a slot of its own". Per input
  there is now one named slot — `Variable::producer(ordinal)` over `r, r1, r2, …`, where **ordinal 0 stays
  `PRODUCER`** so that no existing pack and no region hash moves — and `region_edges` refuses
  `ProducerFeedsTwice` (one value, two slots, no spelling) and `InputsAreNotNamed` (wider than
  `FAN_IN_INPUTS = 8`), while a consumer whose `element` does not contain an absorbed slot refuses
  `ElementWithoutProducer`. **Measured on the device**: three kernels against one merged node, the same
  eight values **bit for bit**, with a counterfactual edit inside the composed body (`+100` → `+10000`)
  answering a number no recording computes — so the merged node is what dispatched.
  **Three honest limits.** A fan-in's licence is **structural** — "`compose_fan_in` can serve this" —
  and **not** the reduction-chain algebra the symbolic engine derives for a chain, so a fan-in has a
  schema and a body but no fused-loop derivation. The composed body **recomputes** each producer's cone
  per read rather than storing and reloading it, so value-identity is evidenced at one device shape, not
  proved for arbitrary width. And the body grows **O(n²)** in size — fine at three, not what a
  thirty-kernel fan-in wants.
  (3) **A `Max`/`Min` tile** — **done** (`9722e681`), with the identity caveat above; so what remains is
  (1) and (2).
- **Two costs the chain support carries.** `compose`'s composition is a **modelling claim, not a
  theorem** — the merged body **recomputes the producer's cone per read** rather than storing and
  reloading it, so each step was proven to obey every condition `compose` states, but the whole chain
  being value-identical is evidenced by the device measurement (n = 3) rather than proved for arbitrary
  n. And the fold is **O(n²) in body size** (`compose` clones the accumulated body per link): fine at
  three, not what a thirty-kernel chain wants, where a single-pass generalisation would be a different
  mechanism. A tree that is neither a chain nor a fan-out still refuses with the pair sentence.

- **A transcendental bridges anywhere, not only at a marked expression's root.** The reader is consulted
  from `Walk::instruction` (`fusion-bridge/src/lower.rs`), which the walk calls for **every value it
  reaches** — so `exp` (and `sqrt`, since `c5e8e63e`) is recognised wherever it appears, including
  nested under arithmetic inside a reduction's own marked `element`. An earlier version of this note
  claimed it had to stay outside the marked expression; that was wrong, and SiLU measured it.
- **An independent derivation of a transcendental operator must model *this* `exp`, not `Math::Exp`.**
  `exp_body` is a clamped, halved, Horner-evaluated degree-10 Taylor sum squared back up, and the two
  disagree in the 6th–7th significant digit (relative error 1.5 × 10⁻⁷ … 3.0 × 10⁻⁶). SiLU's declared
  output is therefore the value the VM's own `exp` gives — derived independently by re-implementing
  `exp_body` outside the workspace — and the example does **not** claim exactness. Every later operator
  with a transcendental inherits this discipline, and it is the same modelling caveat as the `sqrt`
  reader's: the symbolic operator is not a theorem about the body.
- **The language has no unary minus.** SiLU's negative branch must be written `(0.0 - 1.0)`; `-1.0`
  fails to parse (`expected a separator, … found '-'`). For an operator whose defining feature is its
  negative branch, that is a real ergonomic hit.

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
- **A graph of exactly one claimed kernel** reports **whichever check comes first, and the interface
  check precedes the arity check**: an `Elementwise` single-instance region is refused
  `InterfacesAreNotProved`, while a `Reduction`-claiming one reaches the arity check and reports
  `RegionIsNotAPair` ("the region holds 1 instance(s)"), both as stage lines.
- **The kernel registry is process-wide and append-only**, so a long-lived host that runs one program
  twice under different claims meets `AmbiguousClaim` in the region merge — while the tiling pass
  deliberately collapses the same collision by digest. The merge refuses where the tiling pass picks.
- **`eps` and `n` cannot be kernel parameters on the graph path**, so RMSNorm's `eps` is a literal in
  the body: correct for this example and wrong for anything else.
- **`BeyondANode`'s message claims a fact it never inspected** ("whose code reads the value another node
  produced") about a node that has no producer. The check fires on the role *name* alone.



