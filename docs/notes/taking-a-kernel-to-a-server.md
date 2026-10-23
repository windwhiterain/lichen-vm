# Taking a kernel to a server

> Status: current — the shapes are as they are; the deployment unit below is **partly unbuilt**,
> and each unbuilt piece is named.
> Points at: `crates/lichen-runtime/src/fused` (the fused artifact and its JSON, since `a1aa17f1`;
> `land_fused_kernel` stayed behind in `lichen-compute` with the recording),
> `crates/lichen-language/src/persist.rs` and `crates/lichen-registry` (the frozen artifact store),
> `crates/lichen-compute-gpu/src/spirv.rs` (what a target emits and refuses),
> [shared-memory-and-barriers](shared-memory-and-barriers.md) §5 and
> [compute-graph-jit](compute-graph-jit.md) (the graph and its policies).

**Two different things in this tree are called an artifact**, and telling them apart is most of the
question. The older one is the **frozen language artifact**: `lichen-registry`'s byte codec under a
`ModuleKey`, the disk `DeviceRegistry`, and `StaticModule` — a package that has been loaded, compiled
and **frozen**, so it "carries values, not spans" ([artifact-cache](artifact-cache.md),
[static-modules](static-modules.md), [packages](packages.md) are its notes, and
`persist::shipping_cache_root()` is its cache root). The newer one is `lichen-runtime`'s
**fused artifact**, `facet_json`'s `Artifact { fragment, claim, plan }`, keyed by `fragment_digest`.
They live at different layers, have different keys, different cache roots, and **neither replaces the
other**.

## The boundary that decides what a server depends on

The language does **not** sit above the compute layer: `lichen-language` is the front end and its
runtime (lexer, parser, preprocessor, checker, VM, the package store) and it names no compute crate.
`lichen-compute` is the **compute sub-language** — the surface, the native operators, the recording,
the passes, the claims and the packs — and it depends on `lichen-highlevel` (the checker) and
`lichen-lowlevel` (the VM), which is why it is the wrong thing for a server to depend on.

Since `a1aa17f1` the artifact half lives in **`lichen-runtime`**, whose only internal dependency is
`lichen-kernel-ir` — `cargo tree -p lichen-runtime` shows **no** `lichen-highlevel`, `lichen-lowlevel`,
`lichen-language`, `lichen-language-parser` or `lichen-preprocess`. So a process that lands and runs
kernels needs **`lichen-runtime` plus a backend**. A **frozen package**, by contrast, *is* the language
layer's state: it is imported and registered by the language runtime, so shipping it puts that whole
side back in the server. **The serving boundary cuts at `lichen-runtime`.** (This note first said
`lichen-compute`, which was wrong for exactly the reason above.)

The seam rule that produced that split, and which decides the next moves:

> **Anything that takes a `Module` belongs to the language-integration side; anything that takes only
> IR types (`Graph`, `KernelFragment`, `Artifact`) belongs to the runtime side.**

**The one edge case the rule produced, and how it settled**: `land_fused_kernel` takes `&mut Graph` —
an IR name — so the rule says it belongs in the runtime, though the first extraction left it behind
because it mutates a **recorded graph**. The next slice moved it (`b6c9aa56`), for a reason the rule
implies and the code forced: the tiling pass calls it, and a `Graph` is IR, so leaving it behind would
have made the runtime depend on the language. Its own comment had claimed the opposite — "this half is
the language side, because a graph is where a recorded dispatch is being rewritten" — and that is a
**design statement, not a seam**. So today **both** reading and landing an artifact are language-free,
and what stays in `lichen-compute` is exactly the `Module`-taking half: the installers, the recording,
and the registry's filling.

**And the split is verified end to end, not asserted.** One slice more (`499ca9fc`) removed the last
edge: the GPU backend's only use of the sub-language was a *test* reaching four runtime items through a
language-side re-export, so the library edge became

```
$ cargo tree -p lichen-compute-gpu --edges normal --prefix none
lichen-compute-gpu v0.1.0
ash v0.38.0+1.3.281
libloading v0.8.9
windows-link v0.2.1
lichen-kernel-ir v0.1.0
lichen-runtime v0.1.0        ← widened by the loader's slice (`d3ddf13a`), and see below
```

and a later slice widened it further: `impl DeviceFactsSource for GpuContext` cannot live in either
crate without one of them naming the other (the orphan rule), so **`lichen-runtime` became a normal
dependency of the backend**, bringing `facet`, `facet-json` and **`wasmi`** in transitively. The
language-free property is unaffected — neither the plain tree nor `--all-targets` names
`lichen-highlevel`, `lichen-lowlevel`, `lichen-language*`, `lichen-preprocess`, `lichen-perspective` or
`lichen-registry` (all seven were present before, under dev-dependencies through `lichen-compute`,
which also made that a dev-dependency *cycle*). **`wasmi` was then made optional** (`d5e3217f`), and
that is the pattern a serving image wants: `lichen-runtime` carries a **default-on `wasm` feature**
(`wasmi = { workspace = true, optional = true }`, `wasm = ["dep:wasmi"]`), everything naming the
engine lives in one `#[cfg(feature = "wasm")] mod cache`, and the GPU crate takes the runtime with
**`default-features = false`** — stated by *path*, because cargo refuses `default-features = false` on
a `workspace = true` dependency. Measured: `cargo tree -p lichen-compute-gpu --edges normal` no longer
names `wasmi`, `wasmparser`, `wat`, `wast` or `spin`, both feature sets check with **zero warnings**,
and the device suite is unchanged at **70 passed**. What `--no-default-features` removes is
`cached_module` (it hands back a `wasmi::Engine`/`Module` expr, so it cannot exist without an engine);
`module_cache_misses()` stays and is permanently `0`, which is the honest reading rather than a broken
one. **The caveat is cargo's, not the split's**: features unify across a workspace build, so
`--workspace` re-enables `wasm` because `lichen-compute` needs it — a genuinely GPU-only image must be
built with the GPU crate as its root, which is exactly what `cargo tree -p lichen-compute-gpu`
measures.

## The loader, and the facts it refuses on

`lichen_runtime::load_fused_kernel(text, &mut Graph, node, &impl DeviceFactsSource) -> Result<LoadedKernel, LoadRefusal>`
(`load.rs`) is the one call the previous paragraph asked for, and it is **gate-ordered so that a
refusal touches nothing**: probe the **format version** → `read_fused_kernel` (parse, convert, the body
gate, the fragment gate, `check_plan`) → `DeviceFacts::check(&fragment, &plan)` → `replace_kernel`. A
refused load leaves the graph **byte-identical**, and the version refusal fires **before the rest of
the text is parsed** — the price is parsing the text twice, which its invariant records. `LoadRefusal`
names five gates (`Read`, `VersionMissing`, `VersionMismatch { found, wanted }`, `Device(…)`,
`Graph(…)`) and `DeviceRefusal` names seven.

**`DeviceFacts`** is the model `TargetFacts` states, extended to the thing the fragment will actually
run on: `int_bits`, `max_workgroup_lanes` (a *ceiling*, never rounded down — rounding runs a different
program), `subgroup_width: Option<u32>`, `max_shared_bytes`, `max_storage_buffers`. The middle one is
why the loader exists: **a module cannot pin its subgroup width** (`OpExecutionMode SubgroupSize` needs
Vulkan 1.3 while this target emits SPIR-V 1.3), so a `Warp` slice on a narrower device answers
*undefined* in a way `spirv-val` cannot see — and `check_plan` only ever compared that width against the
**workgroup**, which is why `DeviceFacts::check` walks the plan too. Two facts the design considered and
left out, with reasons worth keeping: **indirect dispatch is not a capability** (`vkCmdDispatchIndirect`
is core in Vulkan 1.0; the real limit belongs to a run's extent, and "a fact no gate reads is a number
nobody asked for"), and **push-constant bytes belong to the emitter** (re-deriving that rule in the
runtime would be the second copy of a rule this repository refuses). The probe reads
`shaderInt64`, `maxComputeWorkGroupInvocations ∧ maxComputeWorkGroupSize[0]`, `subgroupSize` through
`vkGetPhysicalDeviceProperties2` — and since ash's missing-function stub *panics*, a 1.0 loader yields
`None` (`SubgroupWidthNotReported`) rather than a guess. It deliberately does **not** read
`spirv::LOCAL_SIZE_X`/`SUBGROUP_WIDTH` as device facts: a loader that compared an artifact against them
would be checking the compiler against itself.

**The version is an explicit required `version: u32`** on `schema::Artifact` (`FORMAT_VERSION = 3`),
not a wrapper — `deny_unknown_fields` already refuses a later format's extra fields, so a wrapper would
only add a second name for one artifact. A missing version is refused by that same denial (measured:
deleting `"version": 3` fails all five `fused_landing` tests, naming the path `version`), and the
loader adds what the field cannot: a **named** refusal before a full parse. Measured on the device
(RTX 3060, `int_bits 64`, `max_workgroup_lanes 1024`, `max_workgroup_lanes_y 1024`, `subgroup_width
Some(32)`, `max_shared_bytes 49152`): the one-call load answers what the direct path answers, a
`"version": 4` text is refused with `submissions()`/`fence_waits()`/the node's digest all unchanged,
and a hand-made `subgroup_width: 16` refuses a 32-element `Warp` slice by name. **The bump to 2**,
`Target`'s gaining `lane_height`, measured the gate in the other direction: a hand-written **version-1**
text names `VersionMismatch` — *"the text is artifact format 1 and this build reads format 2, so the
artifact is refused before any of it is parsed"* — where the strict read alone would have answered
`missing field 'lane_height' in type 'Target' (at 'fragment.target') (bytes 814..815)`. Two mutation
checks make those tests discriminate: setting the wrong format constant or the matching subgroup
width makes them fail.

**The bump to 3 is a second instance of the same gate**, by the same mechanism: `KernelBody` gained
`exact_float_arithmetic`, and a fragment reaching the GPU backend through a fused artifact carries it
as text like every other body field. The field is **required rather than defaulted** on purpose —
`#[facet(default)]` would let a version-2 text read as `false` and the kernel would quietly lose its
rounding points — so a stored artifact from the previous build is **refused by name**, which is what
the gate above was built to do. [contraction-control](contraction-control.md) §9.5.

**And the path a server composes is now one call, with nothing else reachable from it**:

```
load_fused_kernel (version → parse → body/fragment gates → check_plan → DeviceFacts::check)
  → GpuContext::new() → install(context) or install_default()
  → Runner::new(&context, policy).run(graph, inputs)
```

— no `Module`, no checker, no sub-language, and **no dispatch before every gate has passed**.


## What the fused artifact carries, layer by layer

| field | what it holds | which layer it is |
|---|---|---|
| `fragment` | `param_shape`, `roles`, `body`, inputs/outputs and their classes, `target`, `slices`, `marks` | the IR — with `target` a machine fact and `marks` the lichen `~ "…"` annotations |
| `claim: Option<Claim { interface, roles }>` | the fusion interface and its role names | a **language-level** concept, already allowed to be `null` |
| `plan: Plan` | each staged value's **name** and its `space` | the machine's decision |

So the fused artifact is a **build-time interchange**: the claim is what a *pass* reads to decide
whether a region fuses, and a plan's `Staging.name` names a value by the body's own **mark** — which
is the coupling to know about, because **a plan cannot be applied without the marks**.

## The deployment unit: a runtime fragment, and what is missing

The honest unit for a server is therefore **not** the fused artifact but the fragment alone, with the
build-time half already resolved at home:

1. **The plan is materialised before shipping.** Staging is what a plan *creates*; once it is applied,
   the body states its own slice write/read exprs (`KernelBody::barriers`, and `barrier_scan` reads
   them from the body), so **the server needs no plan and no marks**. `claim: null` is already
   allowed, and a server that runs no passes needs no claim either.
2. **What is missing is small**: `schema::Fragment` is already `Facet`, so "serialize the fragment
   alone" is expressible today — there is just **no versioned `read_fragment`/`write_fragment` expr**,
   and no refusal for a version that does not match. That expr is the clean form: one blob, one
   digest, one named refusal when it cannot be honoured.
3. **The emitted module is not persisted.** `MODULES` is a process-wide `ModuleCache` with a miss
   counter (`lichen-runtime/src/module_cache.rs` since `a1aa17f1`), so every restart re-emits SPIR-V or wasm. Persisting the
   emitted module under its digest is the second missing piece, and it is what removes the emitter
   from a serving image altogether.

## What must not ship, and what must be checked at load

- **Not the pack.** Placement is *that machine's* decision: a pack is named on the run-time command
  line, a pack that does not parse is a **named failure that stops the run**, and an inert pack is
  silent ([graph-fusion](graph-fusion.md) §5). Shipping a pack with a model would bake one machine's
  placement into the model.
- **The device's own facts must be probed and gated, by name.** The artifact cannot carry them all:
  the **subgroup width cannot be pinned** by the module (`OpExecutionMode SubgroupSize` needs Vulkan
  1.3, and this target emits SPIR-V 1.3), so a `Space::Warp` artifact on a device whose subgroup is
  narrower answers **undefined** in a way `spirv-val` cannot see. A loader therefore queries
  `subgroupSize`, shared-memory capacity, indirect-dispatch support and the local size, and **refuses
  by name** rather than mis-dispatching.
- **The order that matters**: an extent written by one dispatch and read by the next is ordered
  `TRANSFER_WRITE → DRAW_INDIRECT | INDIRECT_COMMAND_READ`; the plausible `COMPUTE_SHADER / SHADER_READ`
  barrier validates clean and **dispatches zero** workgroups
  ([lichen-compute-gpu](lichen-compute-gpu.md)'s invariants). This is why the emitted module and the
  runtime that records it must be versioned **together**.

## One submission, and the shape a serving loop wants

A graph runs under one of three policies ([compute-graph-jit](compute-graph-jit.md)): `Serial`,
`Async`, and `Batch`, which since `09bbeee7` is the backend's own answer — the GPU records a whole
graph in **one** submission and one fence wait (measured: `(1,1)` against `Serial`'s `(2,2)` on the
same graph and device), and a backend without the capability refuses by name. A serving loop's
per-step graph is exactly a `Batch`. Where the extent itself is computed on the device — a count one
kernel wrote and the next dispatches over — the run is **one submission with no stall**
(`427f768b`), which is the shape dynamic work needs; **`Async` refuses a device-held extent by name
today** (`ExtentIsNotInFlight`), so dynamic work and host overlap are not yet available together.

## What is not decided, and what would decide it

- **Can the deployment control its driver and Vulkan version?** If yes, the probe-and-gate story can
  be lighter; if no, it is the safety-critical part.
- **Latency first, or reproducibility first?** The latency features are the ones that lean hardest on
  device facts (the warp exchange, indirect dispatch, one-submission batching); a
  reproducibility-first deployment would rather use shared memory and a workgroup barrier, which the
  IR already defines bit-for-bit on both wasm and SPIR-V, so a CPU reference path can be diffed
  against the GPU.
