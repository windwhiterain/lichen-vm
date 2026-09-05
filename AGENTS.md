## Memory
REAMD.md, /docs, commends.
- new feature should whether go along with a memory or create new one in /docs/notes.
- keep relative docs synced.
- always avoid replication, if exists, merge or reference.

## Approval
- always present a plan and ask for user approval unless the instruction is direct.
- in the mid-turn of implementation, any design point is meet, stop and ask user.
- when coming to subtle place, stop and ask user's idea.

## Worktree
make sure your are on your assigned worktree (usually you should create one for new feature). 

## Verify
- `cargo check` for compilation pass.
- `cargo tests` for behaviour correctness.
- `cargo fix --allow-dirty`, `cargo fmt` for final commit.
