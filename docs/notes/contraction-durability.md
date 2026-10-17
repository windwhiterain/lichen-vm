# Is the contraction fix durable?

> Status: current
> Points at: `crates/lichen-compute-gpu/tests/compute_gpu/spliced_transcendentals.rs`,
> `crates/lichen-compute-gpu/src/spirv.rs`, `crates/lichen-compute-gpu/src/dispatch.rs`
> (the pipeline cache key, `:2017`).

The emitter now forbids a driver's contraction wherever the IR says the arithmetic
is exact, and
[the control](contraction-control.md) §9.3 measured that both vendors answer the
definition bit for bit under it. A fix is only worth that measurement if it
**survives the machinery between source and device**, so this note records the one
part of that question that has been measured, the part that is still an argument,
and the part nobody has measured.

## What is measured: a fresh process still answers the definition

`spliced_transcendentals` asserts that a spliced `sin`, `cos` and `ln` answer the
IR definition **bit for bit**, and it dispatches through the shipped emitter. Run
in **three separate processes**, `-j 3 --test-threads 1`:

| run | environment | result |
|---|---|---|
| 1 | plain | **3 passed** (2.58 s) |
| 2 | `__GL_SHADER_DISK_CACHE=0` | **3 passed** (0.77 s) |
| 3 | `__GL_SHADER_DISK_CACHE=1` and a fresh `__GL_SHADER_DISK_CACHE_PATH` | **3 passed** (1.02 s) |

So a **restarted process** reproduces the definition. That is the durability
statement the fix needed, and it holds for the three bodies whose accuracy it
bought.

## The caveat that stops run 3 from being a cold-cache measurement

After run 3, **the cache directory did not exist**: the driver did not honour
`__GL_SHADER_DISK_CACHE_PATH`. Those variables are **GL** controls, and this backend
is Vulkan, so run 2's "disabled" and run 3's "relocated" are both **unverified as
controls** — they did not fail, they simply may have done nothing. The honest
reading is that runs 1-3 measured **three restarts**, not a warm-then-cold pair.

A cold-cache run needs a **Vulkan-level** control — an ICD-private variable or a
relocated `VK_*` cache path — and that has not been found. Until it is, "a cold
driver cache still answers the definition" is **open**.

## What is an argument, not a measurement, and where the boundary is

The decoration is **in the module**: `OpDecorate … NoContraction` on 35 float
results in `sin`, 34 in `cos`, 28 in `exp`. A driver cache keyed on the module's
words therefore cannot return a pipeline built from the undecorated module — the
two modules differ by three words per float instruction (`sin` 578 → 683). That is
an **argument from how such caches are keyed**, not a measurement of this driver's
cache.

The repository's own two cache layers are separate from that, and both were
measured:

- the **compile artifact** is keyed by `ModuleKey`, and
  [the artifact store](artifact-cache.md) records it — a completely different layer,
  above the driver, and it cannot resurrect a pre-decoration module because the
  module it stores **is** the decorated one;
- the **pipeline cache key** in `dispatch.rs:2017` is built from **fragment
  digests**, and `fragment_digest` hashes the body, so `exact_float_arithmetic` is
  inside it **by construction**. [The control](contraction-control.md) §9.2
  measured the digests changing (`sin` `0x8912f05b24d6edf8` → `0xb1c57379da646f46`).

So the two layers the toolchain controls are **measured** to discriminate; the
layer it does not control — the driver's — is **argued**.

## What is measured: a rebuilt emitter emits the same decorations

A probe against the public `spirv::compile` builds `out[i] = operator (in[i])` for
`sin`, `cos` and `ln`, forces a rebuild of `crates/lichen-compute-gpu` (the
`Compiling` line names the worktree), and emits again. `spirv-dis` then counts, in
the second build:

| module | words | `NoContraction` | FMul | FAdd | FSub | FDiv |
|---|---|---|---|---|---|---|
| `sin` | **683** | **35** | 17 | 11 | 7 | 0 |
| `cos` | **664** | **34** | 17 | 11 | 6 | 0 |
| `ln` | **1912** | **110** | 55 | 53 | 1 | **1** |

Every figure [the control](contraction-control.md) §9 recorded reproduces — the word
counts `578 → 683` and `562 → 664`, and the decoration counts 35 and 34 — so the
emitter still produces the decorations after a rebuild, not merely in the slice that
measured them. `ln` is new here and worth its own line: **110 decorations and the
family's only `OpFDiv`**, which is the division `ln` is still 1 ulp wrong about on
the Iris Xe ([the re-verification](operator-reverification.md) §2).

## What is still unmeasured

- **A cold driver cache**, as above: no Vulkan-level control has been found.
- **A third device.** Every number here is one RTX 3060 Laptop and one Intel Iris
  Xe.

## What to do if a number is doubted

Re-run the three tests in a fresh process:

```
cargo test -p lichen-compute-gpu --test compute_gpu -- --test-threads 1 spliced_transcendentals
```

They assert the definition bit for bit, so a duplicate run is a duplicate
measurement. If that ever fails where
[the control](contraction-control.md) §9.3 says it passes, the mechanism has
changed under the measurement — and the **plain** column of
[the mechanism note](transcendental-gap-mechanism.md) is the control that says
whether the environment or the code moved.

## The cold-cache control: there is no application-side cache to warm

The open item above asked for "a Vulkan-level cold-cache control". Measured, the question is **moot for
this path** rather than answered by a control:

- **The application passes `vk::PipelineCache::null()`** (`crates/lichen-compute-gpu/src/dispatch.rs:2048`),
  so it hands the driver **no pipeline cache to reuse**. There is nothing on the toolchain's side whose
  warmth or coldness could change an answer.
- **The driver keeps no visible Vulkan cache on this machine**: `%LOCALAPPDATA%\NVIDIA` holds `DXCache`
  (Direct3D) and `GLCache` (OpenGL) and **no Vulkan directory**, and `%ProgramData%\Khronos\Vulkan\Drivers`
  does not exist — the ICDs come from the registry and the vendored JSON that `VK_DRIVER_FILES` selects.

What is left is a **driver-internal** cache with no observable handle. If one exists, a duplicate run is a
duplicate *measurement* only if the driver is not answering from a stale module — and any module it could
be answering from is keyed on words this emitter no longer produces, since the decoration is in them
(`sin` 578 → 683). So the residual risk is bounded by an **argument**, not by a measurement, and this note
says which rather than claiming a control that has not been found.

A reader who wants the measurement anyway needs a **Vulkan-level** control — a loader or ICD variable that
relocates the driver's shader cache — and none is documented for this ICD.
