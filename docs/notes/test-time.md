# Test time: where it goes, and what moves it

> Status: current
> Points at: `crates/lichen-compute/tests/lichen_compute/common.rs` (the shared
> store), the workspace `Cargo.toml` (the profile), and
> [test-binary-layout](test-binary-layout.md) (the empty harnesses).

## The measurement, and how it was taken

Every number below is the same tree — `dev` at `725a4799` — on one 20-core
Windows machine, each profile in its own private target directory. Two
instruments are honest here and one is not:

- **Honest: run the test binary directly**, several times, and take the median.
  A whole-workspace `cargo test` mixes the run with cargo's own scheduling, and
  on this machine that is tens of seconds.
- **Honest: compare one suite against itself with libtest's `--skip`**, which
  costs nothing and answers "what does this group of tests pay".
- **Not honest: time one test *area* at a time.** Each invocation is a fresh
  process, so every area re-registers the embedded compute package, and the
  per-area totals come to more than the suite they came from.

## The whole workspace, unoptimized

| | |
|---|---|
| build from an empty target (`cargo test --workspace --tests --no-run -j 3`) | **183 s** |
| run (`--tests --no-fail-fast`) | **106.6 s** |
| the sum of the 51 suites' own times | 82.5 s |
| what the run adds on top of that | ~24 s of cargo and process startup |

The ten heaviest suites, in seconds:

| seconds | tests | suite |
|---|---|---|
| 55.71 | 226 | `lichen_compute` |
| 8.85 | 94 | `compute_gpu` |
| 7.47 | 2 | `tools` |
| 3.02 | 18 | `language_server` |
| 2.50 | 164 | `basic` |
| 2.47 | 1 | `pack_install` |
| 1.09 | 136 | `highlevel` |
| 0.38 | 11 | `serve` |
| 0.36 | 275 | `language` |

So the compute crate is **two thirds** of the workspace's test time, and
`compute_gpu` — a device suite, running Vulkan — is the largest block that no
profile change touches.

## What moves it, measured

| lever | reading | cost |
|---|---|---|
| **the 24 `zz_*` probes** | `--skip zz_` takes the compute suite from **54.8 s to 24.8 s** | deleting settled probes |
| **nine probes, deleted** | the suite's own line goes **55.7 s → 35.7 s** for four of them, at the same profile | the note rows that named them |
| **`opt-level = 2` in the workspace profile** | compute suite **55.7 s → 4.2 s**; whole workspace **106.6 s → 43.2 s** | build **183 s → 872 s** |
| **one store per thread** | the suite's wall clock **27.1 s → 23.8 s** | none; already landed |
| **17 empty lib harnesses** | 51 test executables become **34**; ~90 ms of startup each | a manifest change per crate |

All of them together, on the same tree: the compute suite is **218 tests in 2.70 s**
where it was 226 in 55.7 s, and the whole workspace's warm run is **12.6 s** where
it was 106.6 s.

### The profile trade, in the units that decide it

`opt-level = 2` costs **+689 s of build** and saves **63 s of run**. That is a
**net loss of about ten minutes on a cold build**, and a 2.5× win whenever the
target directory is already there. So the question is not whether the profile
helps the run — it is whether the run is what you are paying for. The same
numbers are why `opt-level = 3` is not the answer: it buys 0.8 s over
`opt-level = 2` on the run and costs much more again on the build.

### The floor, whatever the profile

`[lib] test = false` on the crates with no inline tests removes 17 of the 51 test
executables, at ~90 ms of startup each, and the remaining suites still pay their
own startup: cargo's own scheduling over the binaries is ~24 s of a 106 s run.
Even after all of that, the warm whole-workspace run is 12.6 s, of which the
device suite is 4.3 s and cargo's own overhead is a large share of the rest.

### Which probes, and on what grounds

The rule is that a probe stays while its investigation is open, and the notes
are the record. Auditing the twenty-four against `docs/notes/` gives sixteen
**keep** (each with a passage saying the question is still open), five
**delete-candidates** (each with a passage recording the answer), and three the
notes do not reach at all. Of the five, two are hedged by their own notes —
`lichen-compute-gpu.md` says its pre-guard readings "are kept as the
evidence", and `operator-gaps.md` still lists `zz_fold_buffer_probe.rs` in a
command line — so the landed set is the three unhedged ones plus the one that
cannot run:

| probe | grounds |
|---|---|
| `zz_dispatch_timing_probe` | `physics-operators.md` records its answer and names no next step |
| `zz_clamp_grouping_probe` | `writing-a-solver.md` §"What may sit where, measured" records the three readings |
| `zz_plain_leg_probe` | `llm-operators.md` — "It is now diagnosed, and the premise is stale rather than the behaviour wrong" |
| `zz_phase_timing_probe` | **broken**: its program names the `"gpu"` backend, which the check refuses with no backend installed, so it prints nothing |

Two more were deleted on the owner's word, against the reading that a note's
hedge should keep them: `zz_append_slot_probe` (the note said its pre-guard
readings "are kept as the evidence" — the readings are kept, in the note that
records them) and `zz_fold_buffer_probe` (still listed in a command line, with no
row behind it). And the three the notes never named: `zz_conv2d_probe` and
`zz_pool_probe`, whose readings the examples `examples/convolution2d.lichen` and
`examples/pooling.lichen` carry — both run bit-exact on the device — and
`zz_phase_timing_probe`, whose question this note answers.

The three the notes do not name — `zz_conv2d_probe`, `zz_pool_probe`,
`zz_phase_timing_probe` — need one sentence from the owner, not a re-run: their
examples (`examples/convolution2d.lichen`, `examples/pooling.lichen`) already
run bit-exact on the device, so the question is whether the example carries the
reading or the probe does.

## What a single program costs

Measured with the suite's own helpers, one program of the shape the tests use
(a parallel kernel, a launch and a read), with the store already registered:

| phase | cost |
|---|---|
| registering the embedded package in a fresh store | **32 ms** |
| parsing the program | 9.2 ms |
| checking and lowering it | **3.3 ms** |
| compiling *and* running it (emission and the engine) | ~40 ms |
| reading the recorded fragment back out | 3.1 ms |

Two of those readings are worth keeping because they invert an intuition. The
**check is cheap** (3.3 ms) — a 30 ms reading of the same phase is a check that
*emitted* the kernel, which only happens with a backend installed, and emission
is the backend's business. And **the parse is not what a source test pays for**:
building the same program as IR with the harness's writer saved 1.1 ms of 31 ms,
because the hand-built program is checked just as expensively. A test that
writes its program as source is not slow because the parser is slow.

## The three traps in measuring this

1. **The profile lives in `Cargo.toml`.** Reverting that file silently rebuilds a
   directory that was measured *with* it, and the "optimized" numbers become
   the unoptimized ones wearing the same path.
2. **Timing an area alone inflates it.** A fresh process pays the 32 ms package
   registration again, on every thread.
3. **The shared store makes the suite's own numbers depend on its thread count.**
   One store per thread means the package registers once per thread, so
   `--test-threads 1` and `--test-threads 20` are different measurements.