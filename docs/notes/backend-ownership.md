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

## What each crate keeps

| crate | verifies | starts a backend |
| --- | --- | --- |
| `lichen-compute` | the IR: fragments, classes, plans, graph shape, refusals | no |
| `lichen-compute-gpu` | the device: what a dispatched kernel answers | yes |
| `lichen-serve` | the serving path: scheduling, refusals, tokenizer, sampler | no |
| `lichen-compiler` | the compiler: what it installs, what it refuses | no |

## The costs, stated

- The end-to-end path — a lichen program compiled, landed on the device, and
  answering a number — is no longer covered by a test. It is covered by the
  device crate's own IR-level acceptance and by the serve crate's examples.
- A suite that no longer runs a kernel cannot report a number, so the
  assertions that read one are gone rather than rewritten.
- The serve crate's examples are run by hand, not by CI. The CI runner has no
  device, so a test there would skip anyway.
