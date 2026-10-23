# One test harness: write IR by name, ask the module about it by name

> Status: current
> Points at: `crates/lichen-testkit/src/writer.rs` (`Writer`, `Program`),
> `crates/lichen-testkit/src/probe.rs` (`Probe`),
> `crates/lichen-highlevel/tests/highlevel/` (every IR a test writes),
> [lowering-map](lowering-map.md) (what `explain` reads).

Three crates each rolled their own way of building an IR — `lichen-highlevel`'s
tests carried 240 lines of `int(&mut ir, 5)`-shaped helpers, `lichen-language`
had a second runner, and thirteen of its test files rolled a third instead of
using the shared one. A failing test could only say `ExprId(17)`, and reading
that back means re-deriving which expression the writer meant.

`lichen-testkit` is the one harness. It is a **workspace member and a
dev-dependency** of both crates, so no library half of the workspace links it.

## The name is the contract

`Writer`'s every builder **requires** the name its result is written under, so
a test cannot build an unnamed expression:

```rust
let mut w = Writer::new();
let x = w.param("x");
let id = w.lam("id", x, x);
let five = w.int("five", 5);
let call = w.app("call", id, five);
```

`Probe` takes that name back. `probe.usize_of("call")`, `probe.node_of("five")`,
`probe.diagnostic_of(DiagKind::Guard)` and `probe.explain(node)` are the four
readings a test makes, and each reports in the program's words: an unknown name
panics listing the names the program *does* have.

**For an investigation, three readings report instead of panicking**, because the
state they describe is the finding rather than a failure:

- `try_value_of` / `try_node_of` — an undecided value is a finding, and an apply's
  `state.val` is empty by construction, so its answer lives in its expr's own value
  slot rather than in the apply's value;
- `class_of(name)` — a class's members each keep their own slot value, which can
  differ from the class's answer, so a probe can read the members rather than the
  verdict (the name also appears in the compute and GPU crates, where
  `scalar_class_of` and `buffer_class_of` are unrelated symbols);
- `Module::class_members_with_values(node)` in `lichen-lowlevel` — the same reading
  at the lowlevel, for a node that no name reaches.

The prose for a running investigation is
[nested-apply-none](nested-apply-none.md), which is where these three were added and
what they were added for.

The builders take `&self` over a `RefCell`, not `&mut self`. That is what lets
`w.array("key", &[w.int("one", 1), w.int("two", 2)])` compile — the way IR is
actually written. With `&mut self` every sub-expression has to be hoisted into a
`let` first, which is the harness being writer-*hostile*.

## What names are not

**The names are the testkit's, not the IR's.** `IR::expr` is still
`Expr { kind }` and `ExprId` still a dense index: the production frontend pays
nothing for them, and a name can never reach the expr layout. The *lowering*
half is not the testkit's — the node→expression attribution is
`Build`'s, because that is a fact about a build and not about a test. See
[lowering-map](lowering-map.md).

## What moved, and what did not

A test moves up only when its **subject** is the checker or the type system. A
test whose subject is the frontend stays, whatever it looks like:

| Moved | Subject |
| --- | --- |
| `table.rs` | the table literal and `TableGet` read — the checker's indexing pass |
| `loop_run.rs` | `@loop` — a field on the `Function` node the checker consumes |

| Stayed | Why |
| --- | --- |
| `float_literal.rs` | the **lexer** decides `1.5` is a float; `writer.float` would make the assertion true by construction |
| `field_read_kinds.rs` | 13 of 15 assert a rendered message string or a source span — a rendering test in a checker's clothes |
| `gc_block.rs`, `merge_carry.rs` | their subject is reachable only through the frontend's **block** lowering, not through an `ExprKind` |
| `consumer_guard.rs` | the tiling/fusion pass, and it needs a live device |
| `frozen_function_type.rs`, `registry.rs`, `persist.rs` | the package store and artifact codec |

The rule that decides each is worth stating once: **if the fact under test can
only be produced by turning source text into IR, it belongs to the frontend's
crate**, and a hand-built equivalent would test the harness rather than the
program.

## The two invariants this keeps

- **A test asserts a semantic fact** — a value, a node's structure, a
  diagnostic's *kind* — never a rendered string. See
  [tests-do-not-render](tests-do-not-render.md); the moved tests keep that rule,
  which is why `table.rs`'s ten became `DiagKind` assertions rather than the
  message text they used to match on.
- **A refusal is still a build.** `Probe::build` refuses a failing program
  outright; `Probe::build_refusing` returns it, because for most tests the
  refusal is the subject.
