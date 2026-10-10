# A loop body cannot compute its state and jump back

> Status: **closed.** Found while settling §8.5 step 1 of
> [loop-conversion](loop-conversion.md) (the `passed_out` contract) and step 2 (the
> wasm `While`), as the third shape of a defect `ca372a2` had already found twice:
> the IR could not express the thing the feature exists to run. **Both halves are
> now fixed** — §4 — and the tests that pin them are in
> `crates/lichen-compute/src/compute.rs` and `crates/lichen-kernel-ir/src/body.rs`.
>
> One thing found on the way is *not* closed and is not about this IR at all: the
> wasm emitter cannot serve a loop, and the attempt to make it is withdrawn. That is
> [wasm-control-flow](wasm-control-flow.md).

## 1. The fact

**`Terminator` had no plain-jump variant.**

```rust
pub enum Terminator {
    Return,
    If { on_one, on_zero, join, passes },
    While { header, body, exit, carried, passed_out },
}
```

A plain transfer was `Flow::Jump { target, passes }` — a variant of **`Flow`**, not
of `Terminator` — while `Flow::Seq`'s own field is a `Box<Terminator>`:

```rust
Seq { instrs: Vec<KernelInstr>, terminator: Box<Terminator> }
```

So the one shape `Flow::Seq` exists to express — *"run these instructions, then
perform this **plain** transfer"*, its own documentation — **could not be built**.
A `Seq` could only end in `Return`, `If` or `While`.

## 2. Why that blocked the acceptance case

The reduction of [loop-conversion §8.5](loop-conversion.md) is a loop body that
**computes** a next state and **jumps back to the header** with it. Every route the
old IR offered was refused or wrong:

| the body, as built | what happened |
|---|---|
| `Flow::Jump` | Not a `Terminator`, so it did not type-check as a `Seq` body. |
| `Flow::Seq` ending in `Terminator::If` | `validate_flow` required the `If`'s `join` to **be** the header and both arms to arrive there, so a body that leaves the loop could not be written. |
| `Flow::Seq` ending in `Terminator::While` | Refused: "a loop body nests another loop". |
| `Flow::Block { entry: Some(header), .. }` | The block's own entry **is** the header, so the label would be defined twice. |

### 2.1 The consequence, which is stronger than "a reduction is blocked"

**The old IR could not express a *terminating* loop at all.** The only body that
type-checked was a bare `Flow::Jump` back to the header, and a `Jump` passes the
header's **own** values — `ca372a2`'s message says exactly this: "a bare `Jump` back
to the header, which passes the header's own values and nothing else". Two things
followed:

- the state never changed, so a pure condition over it evaluated the **same way on
  every iteration** — an infinite loop or a zero-trip one, and nothing between;
- no route left for the `exit`, so no body could branch out either.

The gap `Seq` was added to fill is not "a reduction is hard"; it is that **no loop
the IR could build did anything**.

## 3. What was settled on the way, and is not in question

The **`passed_out` contract** ([loop-conversion §8.5](loop-conversion.md) step 1,
and `crates/lichen-kernel-ir/src/body.rs`): the loop's whole state is the tuple of
`carried` values the header's instructions start from, both counts are read off
*that* stack after the condition is popped, and **the exit receives the header's own
top `passed_out` values**. The exit's values are the header's and not the body's,
which is what gives a zero-trip loop a defined result: a trip count of zero never
runs the body, so a `passed_out` the body had to compute would have no source on
that path. `passed_out ≤ carried` therefore holds, and `validate()` refuses a loop
that breaks it by name.

## 4. The fix, as it landed

**Two changes, and both were needed — and both fix the *transfer*.** §2.1's
consequence has **two** halves, and only one of them is closed:

- a body that cannot *reach* the header, and
- a body that cannot *read* the state it would carry back.

**This section closed the first.** What follows is both changes.

1. **`Terminator::Jump` exists**, so a `Seq` can end in the plain transfer its own
   documentation names. §2's table had a row for `Terminator::If` "joining the
   header", which was the reading that made this look unnecessary: a selection's
   *join* is where arms meet, and a body that leaves the loop does not want its arms
   to meet anywhere. With `Terminator::Jump` a body reaches the header directly.
2. **`validate_flow` lets a loop body name the loop's landmarks.** A `While` body's
   transfer may arrive at the loop's `header` (the backedge) or its `exit` (leaving),
   and a body's `If` may join at either — both are the loop's own control flow, and
   any other target is refused by name. The obligation is carried as
   `FlagEnd::{Return, Loop { header, exit }}` rather than as "everything must reach
   the header", which is what the old rule said and what made a loop with a body that
   decides between continuing and leaving inexpressible.

### 4.1 Closed: the carried tuple is a block parameter, not an instruction

**This section was written when it was the open half of §2.1, and it is no longer
open. `KernelInstr::LocalGet` has been deleted.**

The body is **SSA** now. A block declares the values it receives in
`BasicBlock::params`, an instruction names its operands by `ValueId`, and a
`Return` hands out a **list**. So:

- a header's carried values **are** its block parameters;
- reading one is an ordinary read of a named value, with no instruction to
  invent;
- a backedge's arguments **are** the next iteration's state, because
  `Br { target, args }` hands the target exactly its `params` — which
  `KernelBody::validate` checks, so a phi cannot be built incompletely.

**And the entry block's parameters are the fragment's own domain leaves**, in
flattening order. That is the whole of why one rule serves both: "read the
function's argument" and "read the loop's carried state" are the same operation,
because a parameter *is* a value the block received.

§2.1's sentence — a condition over a state that cannot change is evaluated the
same way on every iteration — no longer describes the IR. What is still true is
that **no source program reaches a loop yet**: the conversion that builds the nest
from a `@loop`-marked recursion is not written, so the capability exists and
nothing uses it.

The note that settles why it dissolved rather than becoming a new instruction is
[loop-conversion](loop-conversion.md) §8.5 item 1c.

`Terminator::Jump` does duplicate `Flow::Jump`, and that is deliberate: the two enums
split transfers by **where they can appear**, not by what they mean. An `If` arm is a
`Flow` (it may be a block or a bare jump, and has no instructions of its own, so a
jump *is* its whole content); a `Seq`'s own transfer is a `Terminator`. Merging them
would mean making `If` a `Flow` variant, which is a larger change to both emitters for
no expressive gain.

## 5. What this changes about [loop-conversion §8.5](loop-conversion.md)

| §8.5 step | now |
|---|---|
| 1. Settle `passed_out` | **Done** — §3 above, and it is in the IR's own doc. |
| 1b. Make the body expressible | **Half** — the *transfer* is done (§4 above); the *carried read* is not (§4.1), and it is what keeps §2.1 true. |
| 2. Fix the wasm `While` | **Done** — the hand-written reorder was withdrawn and the backend lowers through `waffle` ([wasm-control-flow](wasm-control-flow.md) §5, [wasm-backend-handoff](wasm-backend-handoff.md) §3.2). |
| 3. Rebase the SPIR-V emitter for `Seq` | Unchanged, and it inherits the `Jump` arm it wrote as the real backedge. |
| 4. Delete `value_decided` and make the evaluator record a loop | Unchanged, and it is now the largest thing between the IR and a running loop. The emitters are no longer the blocker on either side. |
