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

## What is still unmeasured

- **A cold driver cache**, as above: no Vulkan-level control has been found.
- **The decoration count after a rebuild.** The counts above are from the modules
  the slices emitted; re-emitting after a clean rebuild and counting the
  decorations with `spirv-dis` would show the emitter still produces them, and that
  needs a probe against `spirv::compile` — [the control](contraction-control.md)
  §9 already does this for 26 modules, so the gap is narrow rather than wide.
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
