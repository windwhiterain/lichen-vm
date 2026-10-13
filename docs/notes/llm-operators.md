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
| **softmax** | **landed** (`f8645274`): a `max` fold plus the normalise that reads it, in the RMSNorm two-kernel shape — and that pair **merges**. What still refuses is **arity**: a **chain of any length** now merges (`3f8007cc`, same floor licence, no pack — the floor was already granting it, only `merge_region` refused), but the textbook three-kernel form is **irreducibly a diamond** — the normalise step needs both `m` and `s`, and `yᵢ = exp(dᵢ − m)/s` needs `m` twice, so it is *not* expressible as a linear chain either. Fan-in is what refuses (`NotAReductionChain`, one incoming edge per consumer) |
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
  leaf. **`int2float k.n` therefore still stops**, now with the accurate sentence: *"the projection reads
  parameter leaf 0 of the kernel's own domain — the lane index — and a projection is one function of the
  fused step's slots"* — the walk goes *through* the whole Newton expansion to stop on the `Conv` inside
  it. **The measured reason it cannot move**: every `Conv{Int→Float}` that survives the lowering has
  `ValueId(0)` = `BlockParam { block: 0, index: 0 }`, the **launch extent**, as its source (the lowering
  folds `int2float` of a *literal* before a kernel is registered, so the literal variant holds **zero**
  conversions). Reading the extent is excluded by `lower.rs::parameter`'s invariant and unsound as a
  constant — `canonical_form` excludes a constant extent's value, and a graph's count may be
  `Count::Value` — so it needs a new symbolic slot rather than a shortcut.

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
- **What attention needs, in order.** (1) **A launch that is not one row** — `compute.range` is one
  dimension and a parallel fragment's shape is `(config, index)`, so matmul and attention both need a
  two-dimensional workgroup, and matmul is nowhere near. (2) **Fan-in — a region that is not a chain**
  — **and chains themselves are done** (`3f8007cc`: any length, same licence, no pack; the earlier
  `RegionIsNotAPair` on a linear three-kernel chain was a pure capability gap, since `licence()`'s
  `off_chain_node()` already answered `None` for a chain and only `merge_region` refused afterwards).
  What remains is the **diamond**: `region_edges` refuses a node two instances feed
  (`NotAReductionChain`) and `region_schema` gives each consumer **one** `ReadRoles` from the **first**
  edge found. Concretely it needs a symbolic variable **per input ordinal** (`r0`, `r1`) in
  `lichen_fusion::Variable`, a per-input `ReadRoles` in the bridge, a `Between` per input, and a
  `compose` that inlines **two** producers into one body — plus a restatement of `licence()`'s own rule
  ("two arrive at … is not that chain"), which is why it was not taken. Attention sits behind it.
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
