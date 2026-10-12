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
language-side re-export, so the library edge is now

```
$ cargo tree -p lichen-compute-gpu --edges normal --prefix none
lichen-compute-gpu v0.1.0
ash v0.38.0+1.3.281
libloading v0.8.9
windows-link v0.2.1
lichen-kernel-ir v0.1.0
```

— nothing else, and neither the plain tree nor `--all-targets` names `lichen-highlevel`,
`lichen-lowlevel`, `lichen-language*`, `lichen-preprocess`, `lichen-perspective` or `lichen-registry`
(all seven were present before, under dev-dependencies through `lichen-compute`, which also made that a
dev-dependency *cycle*). So the closed, language-free path a server composes is

```
read_fused_kernel → land_fused_kernel (into a Graph) → GpuContext::new()
  → install(context) or install_default() → Runner::new(&context, policy).run(graph, inputs)
```

with no `Module`, no checker and no sub-language reachable from it. **What is missing there is an API
shape rather than a dependency**: the backend's entry points take a `&Graph`, so read-and-land is
three lines the *host* writes instead of one call that says "run this artifact". A loader that owns
that, plus the version and device-fact gates below, is the slice that makes this note's unit real.

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
   the body states its own slice write/read pairs (`KernelBody::barriers`, and `barrier_scan` reads
   them from the body), so **the server needs no plan and no marks**. `claim: null` is already
   allowed, and a server that runs no passes needs no claim either.
2. **What is missing is small**: `schema::Fragment` is already `Facet`, so "serialize the fragment
   alone" is expressible today — there is just **no versioned `read_fragment`/`write_fragment` pair**,
   and no refusal for a version that does not match. That pair is the clean form: one blob, one
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
