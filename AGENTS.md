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

## Verify

- `cargo check` for compilation pass.
- `cargo tests` for behaviour correctness.
- `cargo fix --allow-dirty`, `cargo fmt` for final commit, no tests need after them.
