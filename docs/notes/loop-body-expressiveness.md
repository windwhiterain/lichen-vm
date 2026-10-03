# A loop body cannot compute its state and jump back

> Status: **finding, unresolved.** Found while settling §8.5 step 1 of
> [loop-conversion](loop-conversion.md) (the `passed_out` contract) and step 2 (the
> wasm `While`). It is the third shape of the same defect that
> `ca2a2a2`/`ca372a2` found twice: the IR cannot express the thing the feature
> exists to run. **The `passed_out` contract itself is settled** — see §3 — and the
> wasm emitter is fixed against it; what is blocked is the next step, because the
> acceptance case has no representation to emit.

## 1. The fact

**`Terminator` has no plain-jump variant.**

```rust
pub enum Terminator {
    Return,
    If { on_one, on_zero, join, passes },
    While { header, body, exit, carried, passed_out },
}
```

A plain transfer is `Flow::Jump { target, passes }` — a variant of **`Flow`**, not
of `Terminator`. `Flow::Seq`'s own field is a `Box<Terminator>`:

```rust
Seq { instrs: Vec<KernelInstr>, terminator: Box<Terminator> }
```

So the one shape `Flow::Seq` exists to express — *"run these instructions, then
perform this **plain** transfer"*, its own documentation — **cannot be built**.
A `Seq` can only end in `Return`, `If` or `While`.

## 2. Why that blocks the acceptance case

The reduction of [loop-conversion §8.5](loop-conversion.md) is a loop body that
**computes** `[counter - 1, accumulator + read(counter - 1)]` and **jumps back to
the header** with it. Every route the current IR offers is refused or wrong:

| the body, as built | what happens |
|---|---|
| `Flow::Jump` | Not a `Terminator`, so it does not type-check as a `Seq` body. |
| `Flow::Seq` ending in `Terminator::If` | `validate_flow` requires the `If`'s `join` to **be** the header, and requires both arms to arrive there. That expresses "the merge *is* the loop head", not "hand the header a new state"; and it forces both arms to the header, so a body that exits cannot be written either. |
| `Flow::Seq` ending in `Terminator::While` | Refused: "a loop body nests another loop". |
| `Flow::Block { entry: Some(header), .. }` | Refused: the block's own entry **is** the header, so the loop's header and its body would be one block — and the label would then be defined twice. |

### 2.1 The consequence, which is stronger than "a reduction is blocked"

**The current IR cannot express a *terminating* loop at all**, and the argument is
short. The only body that type-checks is a bare `Flow::Jump` back to the header
(which the SPIR-V emitter refuses, having no block for `OpLoopMerge`'s continue
target), and a `Jump` passes the header's **own** values — `ca372a2`'s message says
exactly this: "a bare `Jump` back to the header, which passes the header's own
values and nothing else". Two things then follow:

- the state never changes, so a pure condition over it evaluates the **same way on
  every iteration** — an infinite loop or a zero-trip one, and nothing between;
- no route leaves for the `exit`, so no body can branch out of the loop either.

The wasm emitter's terminating path — `br` to the `loop` label, which is
`local.set` the next state and `br` — is therefore **unreachable today**, and the
SPIR-V emitter says the same from the other end by refusing the one body that
exists. The gap `Seq` was added to fill is not "a reduction is hard"; it is that
**no loop the IR can build does anything**.

## 3. What was settled on the way, and is not in question

The **`passed_out` contract** ([loop-conversion §8.5](loop-conversion.md) step 1,
and `crates/lichen-kernel-ir/src/body.rs`): the loop's whole state is the tuple of
`carried` values the header's instructions start from, both counts are read off
*that* stack after the condition is popped, and **the exit receives the header's own
top `passed_out` values**. The exit's values are the header's and not the body's,
which is what gives a zero-trip loop a defined result: a trip count of zero never
runs the body, so a `passed_out` the body had to compute would have no source on
that path. `passed_out ≤ carried` therefore holds, and `validate()` now refuses a
loop that breaks it by name.

The wasm `While` was then reordered against that contract: the `Block`/`Loop` pair
now opens **before** the header's instructions, the state tuple is re-read with
`local.get` on every entry, and the false edge falls out of the `block` with the
top `passed_out` of that tuple. That is §8.3 item 1 closed, and it has an execution
test (a hand-built `While` fragment run through `wasmi`).

## 4. The fix, and what it costs

**Two things are missing, and both are needed before the acceptance case can be
built.**

1. **`Flow::Seq`'s terminator should be the plain transfer it documents** — that
   is, `Seq { instrs, terminator: Box<Flow> }`, so a `Seq` can end in `Flow::Jump`
   (and, as today, in an `If`). This is the smallest change that makes the
   documented shape real, and it **adds no new concept**: `Flow::Jump` already
   exists, is already handled by both emitters' `Flow` walks, and is already the
   thing `Seq`'s own doc names.
2. **`validate_flow` must let a loop body's `If` leave for the loop's `exit`** as
   well as for its header. Today the only accepted body `If` is one whose `join`
   *is* the header, so "compute the state on one path and leave on the other" — the
   reducer's actual shape — stays inexpressible even after (1). This is why (1)
   alone is not enough, and it is worth saying plainly: a loop that can only ever
   continue is a loop with one exit, and that exit is the test.

Alternatives to (1), and why they are worse:

- **`Terminator::Jump`** — reaches the same place through a type whose whole
  meaning is "where control goes **after** a sequence", which would then carry two
  ways to say a plain jump (`Flow::Jump` and `Terminator::Jump`). The duplication is
  the cost; nothing about the IR needs `Terminator` to grow.
- **Leaving the IR alone and lowering a reduction into the *other* shapes** — there
  is no such shape (§2), so this is not an option at all.

**What the fix touches**, and this is why it is worth stating before it is made:
`body.rs`'s label/exit/entry walks and `validate_flow`; the wasm emitter's
`lower_flow`/`lower_terminator`; the SPIR-V emitter's, once
`feature/spirv-loop-emitter` is rebased; and the class-checking walk that refuses a
mixed `Int`/`Float` body. Each is a `match` that gains or loses an arm rather than a
change of meaning.

**Do not start this before deciding it.** It is an IR change under a feature whose
whole remaining critical path sits on top of it, and the last two times this seam
was crossed (`a3e4713`, `ca372a2`) the shape was discovered by trying to build the
acceptance case against it.

## 5. What this changes about [loop-conversion §8.5](loop-conversion.md)

The order there stands, but step 1 grew a second half and step 3 depends on it:

| §8.5 step | now |
|---|---|
| 1. Settle `passed_out` | **Done** — §3 above, and it is in the IR's own doc. |
| 1b. Make the body expressible | **New, and blocking.** §4 above. Nothing can produce a loop until it lands. |
| 2. Fix the wasm `While` | **Done**, against the settled contract, with an execution test. |
| 3. Rebase the SPIR-V emitter for `Seq` | Unchanged, and it now also inherits the `Terminator::While` refusal's replacement — the `Jump` arm it wrote can become the real backedge. |
| 4. Delete `value_decided`, make the evaluator record a loop | Unchanged, and it cannot be tested until 1b lands. |

