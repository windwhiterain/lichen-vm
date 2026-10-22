# Who may start a backend, and what a crate may verify

> Status: current
> Ruled by: the superior's verdicts of 2026-10-21, recorded here so the rules
> outlive the session that heard them.
> Points at: `crates/lichen-compute-gpu` (the only crate that starts a
> backend), `crates/lichen-compute` (the sub-language and the fusion), and
> `crates/lichen-serve` (the serving path).

## The rules

1. **Only `lichen-compute-gpu` may start a backend.** A backend is where a
   compiled parallel kernel runs: the device (Vulkan) or the CPU (the wasm
   interpreter in `lichen-runtime`). Every other crate asserts on the IR it
   produced, never on the result of running it.
2. **`lichen-compute` verifies the IR, not the execution.** A test that
   compiles a program and reads what the program *computed* is testing a
   backend, and a backend is not this crate's.
3. **No dev-dependencies in the workspace, except the test harness.** Every
   edge a crate declares is a normal dependency, so a crate's tests reach the
   same public API a consumer does. `lichen-testkit` is the one exception: it
   **uses** the crates it serves, so a normal edge back to `lichen-highlevel`
   would be a cycle, and a harness the served crates depended on would have the
   arrow the wrong way round.
4. **A test that verifies what belongs to another crate is deleted, not
   moved.** The verification belongs to the crate that owns the behaviour, and
   that crate writes it once.
5. **The serve crate's instances are examples.** A program that stands up a
   session is a demonstration, not a shipped binary.
6. **A crate generates the IR; the backend turns it into a program.** The wasm
   assembler and the engine that runs what it assembles are the runtime's, behind
   its `wasm` feature, because the runtime's own rule says so: a thing that
   takes only IR types — a `KernelFragment`, an `Artifact`, a `KernelId` — belongs
   there, and a thing that takes a `Module` belongs to the language-integration
   side. The assembler took a `KernelFragment` and named a `Module` nowhere, so
   it was in the wrong crate, and `lichen-compute` now names no assembler at
   all: it lowers a body to an IR and asks a backend to run it.

## Why the rules are one rule

The device is one device: a process may hold it, and holding two at once is
a race. Before these rules, four crates took it in the same process tree —
`lichen-compute` (63 tests), `lichen-serve` (69), `lichen-compiler` (4) and
`lichen-compute-gpu` (52) — and the cost was not only contention. Every
device-backed test **skips** rather than fails when no device opens, so on a
machine without one (the CI runner, `ubuntu-latest`) all 188 of them were
`SKIPPED`: the suite was green and had verified nothing.

Rule 1 makes the ownership single, and rules 2 and 4 make each suite's
subject its own. A crate that asserts only its own subject runs the same
everywhere — with a device or without — which is what makes the suite mean
something on a machine that has no GPU.

## The recording path, and why it is a separate entry point

A program whose tail is `compute.graph step` is **recorded**, not run: the
recording intercepts a launch before it reaches a backend. So a caller that wants
the record — a graph's shape, the lines the passes wrote, a node's schema — needs
no device at all.

`lichen-serve` used to reach that only through a session, because `prepare` was a
method on a session and a session owns a device. The recording half is now
`lichen_serve::record(source, packs)`, a free function, and `Session::prepare`
goes through it: one recording path, two callers. A `Recorded` holds the graph
and the report, and offers no `step`.

**A record is not a run, and the difference is the extent.** The automatic
floor is asked over the **recorded** graph, and it names each node's schema
there — so a schema is learnable without a device, which is what the serve
suite's schema probe now does. What a record cannot say is the **tiling**: the
floor tiles on the extent, and the extent of a runtime argument is only known at
a dispatch. The measured difference: the same probe, recorded with a placeholder
pack installed, names the schema; recorded with no pack, it answered `a
recorded graph prints its node's schema: []` in 0.25 s — no device, and nothing
to read, because no pass was there to report. The product's own path
(`src/narrowed.rs`) still prepares, which runs.

## What each crate keeps

| crate | verifies | starts a backend |
| --- | --- | --- |
| `lichen-compute` | the IR: fragments, classes, plans, graph shape, refusals | no |
| `lichen-runtime` | the wasm backend: the assembler and the engine, behind its `wasm` feature | the wasm one, when the feature is on |
| `lichen-compute-gpu` | the device: what a dispatched kernel answers | yes |
| `lichen-serve` | the serving path: scheduling, refusals, tokenizer, sampler | no |
| `lichen-compiler` | the compiler: what it installs, what it refuses | no |

## The costs, stated

- The end-to-end path — a lichen program compiled, landed on the device, and
  answering a number — is no longer covered by a test. It is covered by the
  device crate's own IR-level acceptance and by the serve crate's examples.
- **A recorded fact is not always data-independent.** The automatic floor and
  the fusion licence decide on the data, so a stub that answers with its own
  value makes them decide differently: the floor tiles where the test expected a
  refusal. A test whose subject is such a decision needs a real backend, and it
  is not a test of the record.
- A suite that no longer runs a kernel cannot report a number, so the
  assertions that read one are gone rather than rewritten.
- The serve crate's examples are run by hand, not by CI. The CI runner has no
  device, so a test there would skip anyway.
