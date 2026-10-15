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
| **SiLU / MLP** | **landed** (`3f3693a7`): `silu(gate) × up` as two elementwise kernels. **The floor refuses the honest pair by name** — `InterfacesAreNotProved` — because `Elementwise` has no licence and an elementwise region has no reduction chain to prove; only the pack door is ajar (the schema derives, prints and hashes) |
| **softmax** | **landed** (`f8645274`) and its **textbook three-kernel form merges** (`d364f547`): the normalise step reads both `m` and `s`, with `m` also read by the sum — a **fan-in**, and a fan-in is licensed. It merges because its two producers are **chained** (the sum reads the maximum); a diamond whose producers are **siblings** does not group at all, which is LayerNorm's wall below. A **chain of any length** landed earlier (`3f8007cc`). The licence was restated once, in the two places it was stated: *"a region two edges **arrive at** is that shape exactly when every one of those inputs is named and proven: a `Between` per arrival, each over a slot of its own"* |
| **LayerNorm** | **landed** (`807469f1`) as three kernels in a diamond — `Σx` and `Σx²` as two producers, one normalise reading both. Its region **merges into one node** now (`c9e65140`: a projection may read the fused step's slots **and the invocation**, so `count = int2float k.n` bridges), whereas the grouping alone left it refusing (`f973544a`). **Both of the walls it hit next are gone** (`2fd3971f`: a runtime scalar is a third kind of free value, named `s0` by ordinal, and a producer that declares none takes the consumer's), and the shape it spells — driven through the graph path, since the committed example itself drives three `plrun`s and **records no graph** — merges into one node with `tile ≡ body` **bit for bit** on the device; the merged element's free values are `["d", "n", "r", "r1", "s0"]`. Two limits are honest to keep: the relaxation is **one-sided** (a region where *every* node declares the runtime scalar still refuses, and admitting it needs the pass to compare `Scalar::Constant` bits between nodes), and a merged node is **unregistered**, so its licence is the floor's shape claim rather than a digest-keyed proof about the result. Also, a producer that declares its **own** runtime scalar is still refused, because the merged leaf would carry the **consumer's** number and nothing compares the two nodes' values — that refusal is what keeps a silent wrong constant out. It uses the **one-pass** identity `var = Σx²/n − mean²`, because the textbook two-pass form cannot be a diamond at all: there the mean is read by the variance kernel *and* the normalise kernel, which is `ProducerFeedsTwice` (one value, two slots, no spelling). The answer is exact by hand (`1,2,3,4` ⇒ `Σx = 10`, `Σx² = 30`, `mean = 2.5`, `var = 1.25`, centred `±1.5, ±0.5`) except `sqrt`, which is the language's Newton iteration and is modelled from its own sequence, as SiLU's `exp` is. Measured on the RTX 3060 through `graphrun`: `[-3.0, -2.0, 1.0, 6.0]`, element for element what `cpu` answers, with and without the merge pass. Its `eps` is a **parameter** now (`e24d00fb`), passed as `1.0` on the direct `cpu` path with the declaration unchanged |
| **RoPE** | **landed** (`2f7214af`): `cos`/`sin` added to the language (definitions spliced by the lowerer, read by the bridge like `exp`/`sqrt`), then the pairwise rotation as three elementwise kernels. It is **one lane per element** — a rule, not a preference: a `.to` chain can fill both positions of a pair, but its outer link is contended and order is not a contract. Domain `|x| ≤ 8192`, bit-exact against a model; the clamp above that is a **silent `Select`**, so an unbounded angle gets `sin(8192)` |
| **matmul** | **landed, naive and non-square** (`3fdedd3d`, the upload fix `08c15c45`): one lane per element of `C`, the `k` fold inside the lane, with the coordinates **named** by `compute.row`/`compute.col` (`459f3663`). The wall the measurement found was not the one this note predicted — it was the **upload width**: a device run uploaded `count × element` and zeroed the tail, so a read set that was not a prefix of its buffer silently answered `0`, and for `m×K · K×n` the device's two length rules together admitted **only `m = K = n`** (a non-square `2×3 · 3×2` answered `[25, 28, 28, 32]` against the CPU's `[58, 64, 139, 154]`, silently, and a body with **no `@loop`** disagreed identically, which ruled out a lowering bug). **That is now fixed**: a host input is uploaded at its **own length** (the invariant is `[0, len)`, the padding past `len` filled on the device), so `2×3 · 3×2` answers what the CPU answers, element for element, and `examples/matmul.lichen` is committed **non-square** for that reason. What remains, in order: the host still cannot see the body's read set, so a body reaching **past its buffer** while the count is inside is an out-of-bounds device read (the silent-zero region shrank from `[count, padded)` to `[len, padded)` rather than closing), and a host input **shorter than the count** is refused by name; then the **workgroup geometry** (a real two-dimensional workgroup, a row-shaped `Shared` exchange). **An operator's shape can now be a parameter** (`c92b6471`): a recorded config carries the first scalar leaf as the extent and **every other leaf for what it is, at any depth** — and `examples/matmul.lichen` uses it, since its inner dimension is `k.k` rather than a `3` (`97e6ad91`) — and **a `Float` leaf is carried too** (`e24d00fb`): the graph's value table holds a float at its own class, so `eps`, a scale or a temperature can be an argument rather than a literal; the two remaining limits are that a leaf cannot be a value computed *inside* the source function, and that `f64` is inexpressible. **But two leaves are positional, and getting them wrong is silent**: the **first** is the extent, and the **second** is what `compute.row`/`compute.col` divide by — so a new leaf goes *after* `cols`. Putting `k` between them does not refuse; it answers different numbers (measured: `[58.0, 64.0, 31.0, 139.0]` instead of `[58.0, 64.0, 139.0, 154.0]`), which is the same silent-wrong-answer class as the upload width |
| **attention** | **landed, naive** (`576c9229`): two kernels — `scores` (a dot product per query/key pair, one lane each, `query = compute.row k`, `key = compute.col k`) then `attend` (softmax × V, three `@loop` nests with the row maximum carried in the value fold's state) — over `Q = 2 0 1 1 / 0 0 2 1`, `K = 1 1 0 0 / 0 1 1 0`, `V = 1 2 3 4 / 5 6 7 8`. Exact by hand except `exp`, which is modelled from its own sequence; `sqrt(dim) = sqrt(4)` is exact because the Newton definition's reduction is the identity on `4`. On the device `attend` answers the declared line with **six of eight entries differing in the last bit** (this device's `f32` division; the slice that landed this example first reported three, and its own run could not be reproduced because flipping only `attend` to `"gpu"` trips a different rule — `attend`'s lanes are `queries × dim = 8` while the `cpu` `scores` buffer holds 4, so the upload rule refuses it — and the six-of-eight measurement is a whole-example run), and **`scores` was refused outright until `b6dea46c`**: *"a `Select`'s two arms are computed in different classes ([Int, Float])"* — reproducible as `sqrt (int2float k.dim)` failing while `sqrt 4.0` and `sqrt (int2float k.dim + 0.0)` both run, i.e. the `Select` inside `sqrt_body` is left with one arm unclassed when an `int2float` of a config leaf feeds it directly. **The wall the row used to describe is a rule, not a missing feature**: a `@loop` nest **cannot be applied inside another nest's step** (see the finding below), so the one-kernel form — score recomputed per lane — is **not writable**, and this is a demonstration rather than a schedule anyone would ship |
| **KV-cache append** | **landed** (`1f79786d`): one `compute.parallel` kernel over `count = capacity · dim`, one lane per cache element, writing **both** the key and the value row back at `position · dim + column` — a **bijection** (8 lanes → 8 slots), so no lane contends and the write address is genuinely computed rather than the lane's own index (lane 1 writes slot 2, lane 4 slot 1). Exact integers throughout, **nothing modelled**. It rewrites the whole cache because **a write past the count is dropped in silence** — see the first finding below, which is what a one-lane-per-column append actually hits. The graph path **records** it (1 node, the extent plus all three runtime scalars) but **refuses the run** at submit; the direct device leg agrees with `cpu` element for element once the incoming row is padded |
| **GEMV (the decode matmul)** | **landed** (`da584de5`): `y[m] = Σ_k W[m·kdim + k] · x[k]`, one lane per output element, the `k` fold in the lane, leaves `.n` and `.kdim`, declaring `[14.0, 32.0, 50.0, 68.0]` — exact integers, **nothing modelled**. **It chains with the KV-cache operator as it stands**: `K[p][d]` row-major flattened *is* this operator's matrix with `m` = position, so append-then-score-one-query is this kernel unchanged and answers `[3.0, 7.0, 300.0, 0.0]`, element for element the literal-cache run — while the **values** half needs a second address form (the cache is position-major, this operator output-major), and the cache must be the whole `capacity · dim` buffer, which is exactly what the KV-cache operator rewrites. **Two of the three remaining walls are measured here**: the device leg refuses a GEMV whose reduction is shorter than its output (*"input buffer 1 holds 3 element(s) but the run covers 4 index/indices"*, `dispatch.rs:219`), and the graph leg refuses it **by construction** — see the finding below |

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
`crates/lichen-compute/tests/common/mod.rs` — one `static DEVICE`, `device_slot()` for the lock alone, and
`device()` for the ten-copy helper — so `git grep -c 'static DEVICE' -- crates/lichen-compute/tests` went
**nine files → one**, and ten → two across both crates. Measured after the collapse: **23 binaries ×
8 runs at `--test-threads 2`, 207 runs, zero failures**. What is left, and what it costs:

- **`tests/combine_fold.rs` was the last unguarded one, and `f818cc57` converted it** — it had justified
  its own `install_default()` by "this case owns its test binary", and the comment went with the
  function it justified. It takes `common::device()`, **not** `device_slot()`: the lock-only spelling
  installs no backend, and since a device is reached through `ParallelBackend` the test would have
  silently stopped exercising a device at all.
- **`tests/compute.rs` has the only two surviving `install_default()` calls, and both are guarded** —
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

- **Leaf one is positional even with no coordinates; leaf two only matters where coordinates are used.**
  Measured on GEMV: `.n, .kdim, .spare` is **identical** to `.n, .spare, .kdim`, while `.kdim` **first**
  answers `[14.0, 38.0, 62.0]` instead of `[14.0, 32.0, 50.0, 68.0]` — **with no refusal at all**: the
  launch runs over `kdim = 3`, `n = 4` becomes the stride, and `x`'s fourth element reads past its own
  three and answers `0` (`compute.rs:6234`). The **first** leaf governs through `scalar_leaf_classes`
  (`compute.rs:1849`), and the second is reached **only from `coordinate`** (`body.rs:788`) — so "two leaves
  are positional" is exact only for a body that calls `compute.row`/`compute.col`.
- **The graph path cannot carry a GEMV at all, by construction.** `RaggedInputs`
  (`crates/lichen-graph-ir/src/run.rs:273`, with `ragged_host` at `:520-529`) refuses the run at submit:
  *"node 0's inputs are not all the same length: 48 and 12"* — it compares **payload bytes** — and a GEMV's
  two inputs (`M · kdim` against `kdim`) are **never** the same length except at `M = 1`, so **the check's
  own sentence is false of a legitimate body**. Since the graph leg is the only path that consults the
  fusion floor, nothing here reaches the floor.
- **A real operator is a second witness to the compile-order class defect.** With the two inputs
  equalised, submit is refused by the **class** arm: *"instruction 0 mixed an integer and a float in one
  operation … this is a malformed fragment rather than an unsupported shape"* — the same
  `unwrap_or(ScalarClass::Int)` chain as attention's `attend` (`compute.rs:2424-2430`), and now reached by
  an operator nobody had to hand-build. It refuses **earlier** than the KV-cache slice's eight-byte
  payload count; those are different bugs and are not conflated.
- **What stands between here and a decode step**, measured: the graph leg refuses the two-input shape, the
  device leg refuses a reduction shorter than its output, and nothing can advance a position. Beyond those
  three, arithmetic, layout and chaining are expressible today.
- **The write side has three measured rules, and the first is silent on both backends.** (1) **A write past
  the count is dropped in silence**: the host output is sized at exactly `count` (`compute.rs:5979`) and a
  write outside the span is dropped (`compute.rs:6255-6263`, whose own sentence is *"a write outside the
  span is dropped rather than aliasing another worker's slots"*). Measured with the kernel a decode loop
  actually wants — one lane per **column**, writing at `t · dim + d` = 4 and 5 over a count of 2 — the
  `cpu` path answers `[0.0, 0.0]` **and so does the RTX 3060**: no error, no refusal, no line. That is why
  the committed KV-cache operator rewrites the whole cache instead of one row. (2) **Capacity against
  filled prefix is asymmetric**: a host input **shorter than the count** is *refused on the device by name*
  (`dispatch.rs:219`, `InputShorterThanCount`, because an input is uploaded at its own length) while the
  **host path silently zero-fills the tail** (`compute.rs:6234`) — a cache holding only its filled prefix
  `[1, 2, 3, 4]` is refused on the device (*"input buffer 0 holds 4 element(s) but the run covers 8"*) and
  answers `[1.0, 2.0, 3.0, 4.0, 100.0, 200.0, 0.0, 0.0]` on `cpu` with nothing said. (3) **A written-but-never-read
  region is not observable**: `collect` reads every element of a buffer exactly `count` long
  (`compute.rs:1550-1621`) and on the device the output is allocated `padded` = 64 (`dispatch.rs:1052`)
  while `collect` still reads `count`, so the surplus lanes' slots are written and never handed back.
- **The graph path refuses this operator's run, and the arithmetic looks like eight-byte elements**
  (measured, not diagnosed). The recorded graph is one node holding the extent plus **all three** runtime
  scalars. At submit it refuses *"input buffer 0 holds 4 element(s) but the run covers 8"*; with 12-element
  inputs *"holds 6"*; with 16-element inputs there is **no refusal** — 32 → 4, 48 → 6, 64 → 8, i.e.
  **payload bytes counted in eight-byte elements while every buffer is `Float`**. For contrast, the same
  crate's one-buffer `Float` graph **with no runtime scalar** dispatches and answers correctly, so the leg
  works and the refusal is this operator's. `ragged_host` (`graph-ir/src/run.rs:520`) compares payload
  bytes and its own `# Invariant` **admits a width-ratio blind spot**, but the counting site was not
  proven.
- **What a decode loop still needs, measured.** Carried state across calls: **yes** — a `plrun` output is a
  buffer a later `plrun` takes as an input, and the committed operator's output is shaped to be fed
  straight back. A second kernel reading what the first wrote: **yes**, the same two steps. **A loop: no,
  and that is a wall** — no `for`, `while` or loop keyword appears in any of the 34 examples; `@loop` is a
  fold inside one lane; and `compute.call` inside a parallel body is refused with a `NodeId`
  (`code-audit.md` P1-39, `todo`). **And there is no way to advance the position**: `t` is a parameter the
  host hands in and the language holds no mutable state between calls.
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
  and get `[Float, Float] → [Float]`. `graphrun` then refuses *"input buffer 1 holds 4 element(s) but the
  run covers 8 index/indices, so a lane would read past it"* (`InputShorterThanCount`,
  `lichen-compute-gpu/src/dispatch.rs:219`): the wrong class means the wrong element width. The chain is
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
  `crates/lichen-compute/tests/compile_order_class.rs`, **ignored by default** (the repo's precedent in
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
  writer wins, and order is not a contract (`tests/compute.rs` says so in its own comment). **So one lane
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
