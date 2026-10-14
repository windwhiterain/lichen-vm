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
  `try_value_of` (an undecided value is a finding, not a failure), `try_node_of` (an
  apply's `state.val` is empty by construction, so its answer lives in its pair's own
  value slot), and `class_of` (a class's members each keep their own slot value, which
  can differ from the class's answer).
- **A probe is a test file that prints and asserts nothing** — `cargo test -p
  lichen-highlevel --test <probe> -- --nocapture` — and it stays in the tree while the
  investigation it serves is open.
- **The language front end is the faithful runner for a runtime value**: an IR probe
  carries no core prelude, so a probe and the CLI can disagree about a program's answer.
  Use the TestKit for the shape a question is about, and the language tests or the CLI
  for what a program finally answers.

## Verify

- `cargo check` for compilation pass.
- `cargo tests` for behaviour correctness.
- `cargo fix --allow-dirty`, `cargo fmt` for final commit, no tests need after them.
