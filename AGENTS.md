## Memory

REAMD.md, /docs, commends.
lichen code examples: examples/

- new feature should whether go along with a memory or create new one in /docs/notes.
- keep relative docs synced.
- always avoid replication, if exists, merge or reference.

## Comments

A comment states an invariant or a safety condition, and nothing else. The rule
and its budgets are in `comments-test.toml`, enforced in CI by `comments-check`;
[the note](docs/notes/comment-policy.md) is the prose form of the same thing.

## TestKit

One harness for writing IR and asking the module about it: `crates/lichen-testkit`,
a workspace member and a dev-dependency of `lichen-highlevel`.
[The note](docs/notes/test-harness.md) is the prose form.

- **The name is the contract**: every `Writer` builder requires the name its
  expression is written under (`w.lam("id", x, x)`, `w.app("call", f, three)`), and
  `Probe` takes that name back (`probe.usize_of("call")`, `probe.explain(node)`), so a
  failure never has to be re-derived from the writer to be read.
- **The readings**: `value_of`/`usize_of`/`array_of` for a value, `pair_of`/`node_of`/
  `type_of` for a slot, `explain` for a node's origin in the program's words,
  `diagnostics`/`diagnostic_of` for what the check recorded.
- **For an investigation**, prefer the readings that report rather than panic:
  `try_value_of` (an undecided value is a finding, not a failure) and `try_node_of` (an
  apply's `state.val` is empty by construction, so its answer lives in its pair's own
  value slot).
- **A probe is a test file that prints and asserts nothing** — `cargo test -p
  lichen-highlevel --test <probe> -- --nocapture` — and it stays in the tree while the
  investigation it serves is open.
- **The language front end is the faithful runner for a runtime value**: an IR probe
  carries no core prelude, so a probe and the CLI can disagree about a program's answer.
  Use the TestKit for the shape a question is about, and the language tests or the CLI
  for what a program finally answers.

## Tests

A crate carries **at most two** test binaries: the **lib** binary holding the inline
`#[cfg(test)]` unit tests, and **one** external binary holding every integration
area. An area is a module under `tests/<name>/`, never a second top-level file —
Cargo links a binary per `tests/*.rs` and per `tests/<dir>/main.rs`, and each one
statically links the whole stack.
[The note](docs/notes/test-binary-layout.md) is the prose form, with the census.

- **The bound is a check, not a reading**: `test-layout-check --config
  test-layout.toml` reads `cargo metadata --no-deps` and fails on any package
  above one target of kind `test`, naming the crate and the binaries it carries.
  The checker is a crate of its own
  ([test-layout](https://github.com/windwhiterain/test-layout)), installed the
  way `comments-check` is, and CI runs it.
- **Run one test with an argument, not with a second file**: libtest's own free
  filter is what selects it (`cargo test -p <crate> --test <name> <module>`,
  `-- --exact <module>::<test>`, `-- --list`). An external binary's names carry no
  crate prefix; a lib binary's do.
- The inline half cannot be folded into the external one — a unit test reaches the
  crate's private items — which is why two is the floor and not a concession.
- **One binary means one thread pool over every test in the crate**: `cargo test`
  gave 54 sequential processes, and now gives 77 tests at `--test-threads = cpus`.
  On a suite that loads a model, cap it.
- **A `mod` declaration rustfmt wants to move is one rustfmt drops**: `mod common;`
  belongs in alphabetical position in `tests/<name>/mod.rs`, or `cargo fmt` removes
  it and the binary loses a module.

## Dependencies

- **No dev-dependencies, except the test harness.** Every edge a crate declares
  is a normal dependency, so a crate's tests reach the same public API a
  consumer does. The one exception is `lichen-testkit`, which **uses** the
  crates it serves: a normal edge back to `lichen-highlevel` would be a cycle,
  and a harness that the crates it serves depend on would have the arrow the
  wrong way round.
- A crate gains a dependency when it needs it at run time. A test that needs
  another crate's API is that crate's test, or the dependency is normal.

## Backends

- **Only `lichen-compute-gpu` may start a backend.** A backend is where a
  compiled parallel kernel runs: the device (Vulkan) or the CPU (the wasm
  interpreter). Every other crate asserts on the IR it produced, never on the
  result of running it.
- **A crate verifies what belongs to it.** A test that asserts what another
  crate's behaviour is — a program's computed value, a device's answer — is
  deleted, not moved: the owning crate writes that verification once.
- The serve crate's **instances are examples**: a program that stands up a
  session is a demonstration, not a shipped binary.
- [The note](docs/notes/backend-ownership.md) is the prose form, with the
  census and the costs.

## Land

A branch lands **in the checkout that owns the branch**: `git -C <checkout> merge
--ff-only <branch>`, which moves the ref **and** that checkout's index and worktree.

- **Never move `dev` with `git update-ref` while a checkout is live on it.** The
  ref moves; the checkout's index does not. The next commit from that checkout is
  built from the stale index, so every path that landed since its last sync
  appears **reverted** — files re-added at their old paths, new files "deleted" —
  and the session that made them is blameless. This was measured here: four
  reverts of a test-layout fold, each commit in the other session's name and none
  of its doing, every one of them carrying an index last synced 59 minutes
  earlier.
- The tell is a commit whose message is about one thing and whose diff touches
  something else entirely: a commit about attributes that "deletes" a note, or
  "adds" a file that has existed for hours. Read the diff before believing the
  message.
- `git log --oneline <commit> -- <path>` names the commits that touched a path,
  across the whole history, so the stale-index commits are findable after the fact.

## Verify

- `cargo check` for compilation pass.
- `cargo tests` for behaviour correctness.
- `cargo fix --allow-dirty`, `cargo fmt` for final commit, no tests need after them.
- A filtered run can read as a pass: `--exact` needs the **full** path
  (`<crate>::<module>::<test>`), because a bare name filters to **0 tests** and
  prints `ok`. Cite the tool's own `test result:` line, never a count you derived.
- A parsed table is not the output it parsed: PowerShell's `-match 'FAILED'` is
  **case-insensitive**, so it matched the `0 failed` of a *passing* run and
  reported nine passes as failures. Both traps were measured twice each while the
  rounds in [graph-recording-rounds](docs/notes/graph-recording-rounds.md) were
  being verified; that note records the readings, this list records the rules.
