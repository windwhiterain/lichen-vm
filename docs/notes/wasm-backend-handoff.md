# The wasm backend: handoff

> Status: **handoff, with the first migration step landed.** Read this first for
> anything touching wasm code generation.
> It is an index, not a design: the detail lives in
> [wasm-control-flow](wasm-control-flow.md) (the lowering, the four defects, the
> adopted design) and [loop-conversion](loop-conversion.md) §8 (loop conversion's
> own critical path).

## 1. What is on `dev`, and what is in flight

| | where | state |
|---|---|---|
| `Terminator::Jump`; a loop body that computes its state and reaches the backedge | `dev` | **done**, tested |
| `passed_out` contract; `validate()` refuses `passed_out > carried` | `dev` | **done** |
| Block types declared *before* the type section is serialized | `dev` | **done** — a latent bug nothing hit until a loop named a block type |
| `wasm-encoder` 0.248 bump | `feature/waffle-spike` | **verified** — and §3.1 removed its only edit to our code, so it now rides with the new lowering |
| The waffle spike | `feature/waffle-spike` | **passes** — a loop with a carried value, lowered and run through `wasmi` |
| §3.1 the instruction map, on `waffle::Operator`, with §3.3's assembly and §3.4's deletion | `feature/waffle-spike` | **done** — all 62 kernel-execution tests green on the new lowering |
| §3.2 structured control flow (`If`, `Jump`, `While`) | `feature/waffle-spike` | **done** — the three shapes lower, and the loop's own path has no test in the suite |
| The real lowerer, for a loop | `feature/waffle-spike` | **landed in §3.2**, and verifiable only as far as the IR is: nothing in the repository builds a loop body |

**§3.1, §3.3 and §3.4 were one commit, not three.** Replacing the emitter *is*
deleting it: keeping the hand-written `Frame`/`lower_flow`/`WasmState` alive beside
a new lowering meant a second operand-stack discipline in the same crate, which is
the one thing this migration exists to remove.

## 2. What is settled, and by what evidence

Six things were in doubt at the start of this work and are not any more.

1. **A loop body can compute its state and reach the backedge.** The IR could not
   express it — `Flow::Seq`'s terminator is a `Box<Terminator>` and a plain transfer
   was only `Flow::Jump`, a variant of `Flow` — and the consequence was stronger
   than "a reduction is hard": the only body that type-checked passed the header's
   *own* values, so no terminating loop was representable at all.
   [compute-kernel-struct](compute-kernel-struct.md) records the body model.
2. **The exit's values are the header's own** (`passed_out`), which is what gives a
   zero-trip loop a defined result. In the IR's own doc.
3. **waffle can lower the loop we need.** Its IR has **block params**, so a
   loop-carried value is a blockparam and a `BlockTarget`'s `args` — no `OpPhi` to
   place, no locals to allocate. The spike proves it end to end.
4. **The version gap is not a problem.** Our runtime validator is `wasmparser`
   0.228 and waffle links 0.248; `wasmi` validates and runs 0.248's output, and our
   emitter moves to 0.248 with the one change above.
5. **Byte-identity of straight-line output is not asserted anywhere.** It is a doc
   claim in the emitter's own comments and nothing tests it, so a new lowering may
   change the bytes: **the 62 kernel-execution tests are the verification harness**,
   and they run real kernels through `wasmi`.
6. **Whether this repository should own a wasm backend at all** (§5) — raised,
   undecided, and **now decided: yes, through `waffle`.** See §5.

## 3. The migration, in the order it should be done

**Do not attempt the remaining steps together.** That is what went wrong in the
attempt this handoff replaces: a single draft that tried the whole mapping at once
carried two unresolved type questions and had to be thrown away.

### 3.1 The instruction map — **done, on `feature/waffle-spike`**

Lower every body as **one block**, `KernelInstr` → `Operator`. What landed is
`crates/lichen-compute/src/compute/wasm/`:

| file | the question it answers |
|---|---|
| `lower.rs` | what does this body compute — the instruction map, onto `waffle`'s SSA values |
| `assemble.rs` | what does the module look like — the buffer imports, one function per fragment, the `main` export |
| `mixed.rs` | which class is each value, read before any of the above |

The map is the table in §3.1 below. Three things the step turned out to be that the
table did not say:

1. **The `Option<WType>` per stack slot is now a plain `Type`.** The handoff asked
   for `Option`, `None` meaning *this lowering cannot name it*, and named the
   cross-kernel call as the one slot that could not be. **`waffle`'s IR has no
   untyped value**, so the `Option` had nowhere to survive — and it does not need
   to: the launch set is in hand, so a callee's result is typed by the callee's own
   `result_classes`, which are exactly the types its wasm signature declares. The
   one unnamed case became a fact.
2. **The comparison's two operators leave one slot, not two.** `I64LtU` then
   `I64ExtendI32U` is a widening, and a slot per operator put the `i32` underneath
   the `i64` where the next instruction read the wrong one.
3. **`select`'s condition is on top**, so its three pops read the condition first
   and hand wasm `[then, else, condition]`; and the result's class is an arm's,
   not the condition's. The hand-written emitter took the top as the result's
   class, which is the condition — and nothing caught it, because the type it
   tracked was never read again.

**Two and three are §2's one cause showing up again**, which is worth stating
plainly: a machine-checked stack is not immune to the operand stack, it only
catches the mistake. The new lowering is not exempt from §2's rule, it is the first
one where a mistake has a name.

> **Since this was written, the operand stack is gone from the kernel IR too, not
> just from the backend.** The body is SSA — every instruction names its operands
> by `ValueId` — so §2's class of bug is now unrepresentable rather than
> merely caught. The map below is what landed at the time; `LocalGet` no longer
> exists and the lowering is one walk of the SSA body rather than a
> stack plus a reorder. See [loop-conversion](loop-conversion.md) §8.5 item 1c.

The map itself, as landed:

| kernel | `waffle` |
|---|---|
| `Const(Int, bits)` | `I64Const { value: bits as u64 }` |
| `Const(Float, bits)` | `F32Const { value: bits as u32 }` — `Operator`'s value is the `f32`'s bits, so the 0.248 `Ieee32` the bump existed for is no longer ours to write |
| `LocalGet(k)` | **gone.** A parameter is read by naming the `ValueId` the entry block's blockparams gave it — the ABI passes each leaf as its own argument, and the body's values are SSA rather than slots |
| `Bin(class, op)` | the class's operator — see the trap below |
| `I32WrapI64` | `I32WrapI64` |
| `Select` | `Select` |
| `Conv { from, to }` | `F32ConvertI64U` / `I64TruncF32U`; `from == to` is a **reclassification** and emits nothing |
| `CallKernel(kid)` | `Call { function_index: base + index[kid] }` |
| `BufferReadCall(class)` | `Call { function_index: read_index[class] }` over `[position, index]` |
| `BufferWriteCall(class)` | `Call { function_index: write_index[class] }` over `[position, index, value]` |

**The trap, and it is the important one.** The kernel's comparison operators yield
an **`i64` `0`/`1` scalar**, while wasm's comparisons yield an **`i32`**. So a
comparison `Bin` is *two* operators (`I64Eq` then `I64ExtendI32U`), and the
converse narrowing (`I32WrapI64`) is what a `CondBr` condition needs. Get this wrong
and the module does not validate. It deserves a named function with its own test,
not a comment — it is the single fact the hand-written emitter tripped over four
times. **Two operators make one value**, and a stack slot per operator puts the
`i32` underneath the `i64`; that is what `lower.rs`'s comparison arm comments.

`lower.rs`'s `Slot` is the discipline the hand-written emitter's `repr` held:
**every slot carries its type**, so a crossing is decided by what the value holds
rather than by what the instruction meant. It is a plain `waffle::Type` and not an
`Option`, for the reason in §3.1's first note.

All 62 kernel-execution tests pass with this lowering in place of the old emitter,
and *that* was the signal to continue.

### 3.2 Structured control flow — **done, on `feature/waffle-spike`**

The three transfers are `compute/wasm/flow.rs`, a file of its own: `lower.rs`
answers *what one instruction computes*, and this answers *where the stack goes*.
The split is the point — the two questions have different state (a stack of values
against a table of labels and joins), and the withdrawn emitter's defects were all
in the second. What landed is:

- **`If`** is a `CondBr` to **two arm blocks of its own**, each started on the
  stack the branch inherited — the selector is consumed by the branch, so what an
  arm starts from is what is below it — and both arriving at a **join block whose
  parameters are the `passes` values**. An absent arm branches straight to the join
  with the stack as it stood, which is what makes a one-armed branch expressible
  without inventing a value.
- **`Jump`** is a `Br` whose `args` are the top `passes` values.
- **`While`** is the spike's shape exactly: the carried tuple's blockparams are
  reserved on a header block *first*, the header's instructions run with those
  blockparams as their starting stack, and the test is a `CondBr` whose true edge is
  the exit — handed the top `passed_out` of the header's own values — and whose
  false edge is the body, whose last act is a `Br` back to the header with the next
  state.

**The header's contract is asserted, and it is a height check.** The stack at the
`CondBr` is `state(carried), condition` and nothing else, so a header that has
consumed its own tuple, or left anything else behind, is refused by name rather
than emitted as a branch whose exit reads the wrong slots.

**The shape §3.1 left open is served as §3.1 said it would be.** A `Flow::Block`
whose `entry` names a label **is** the loop's header: the fragment's entry block
stays the preheader, the header is a block of the loop's own, and the carried tuple
is its blockparams. The preheader hands the header one value per carried slot, and
that is where the initial state comes from — which is the concrete reading of
"a loop's state may not live in the entry block":
`FunctionBody::new` fixes the entry block's blockparams to the function's
parameters, so the tuple, which is a block's *parameters*, has nowhere else to be.

**The join's parameters are typed by the values that arrive**, never by a count.
`materialize_join` is the one place that creates a selection's join, and it takes
each blockparam's type from the slot the branch is handing over; a loop's header
and exit are the only labels whose params are made without an arriving branch, and
both are `i64` (below).

Four decisions the note did not state:

1. **A carried value is typed `i64`.** The IR states a `carried` **count**, never a
   type (`lichen_kernel_ir::body`), and a header's blockparams have to be typed
   *when the header is created*, before any instruction has run — so the type
   cannot be read off the stack the way every other type in this lowering is.
   `Int` is what every scalar kernel's state is and what the acceptance case (a
   reduction with a counter) carries; a loop that hands its exit a value that is
   not `i64` is refused by name rather than typed wrong.
2. **A one-armed branch passes zero values.** `passes > 0` with `on_zero == None`
   is refused: the absent side arrives with the stack as it stood, so a join with
   parameters would have no source on that path.
3. **A loop with no backedge is refused.** When the body is lowered, whether it
   arrived back at the header is carried out of the walk as a `Backedge` fact —
   it is the one thing a stack cannot say, because the header and the exit are
   both reachable from the body's level. A body that never arrives at its header
   has no `Br` to emit, and this is the check the IR's own `validate` intends but
   cannot complete (see §3.2's last paragraph).
4. **A selection whose arms both leave the loop has no join.** This is the one IR
   shape the lowering does not serve: `validate` lets an `If` inside a loop name
   the header or the exit as its join, so an arm may leave and never arrive; both
   arms doing so is refused rather than emitted against a block nothing created.

**What the suite does and does not exercise.** All 62 kernel-execution tests pass.
**No test in the repository produces a loop body**, so the loop's own path is
verified by structure and by a probe, not by the suite: a throwaway body — deleted
before the commit, to respect this repository's rule that agents do not add tests —
lowered to

```wat
(func $kernel0 (param i64) (result i64)
  (local i64 i32)
  local.get 0
  local.set 1
  loop ;; label = @1
    local.get 0
    i64.const 0
    i64.eq
    i64.extend_i32_u
    i32.wrap_i64
    local.set 2
    local.get 2
    if ;; label = @2
      local.get 1
      local.set 1
      local.get 1
      return
    else
      local.get 1
      local.set 1
      br 1 (;@1;)
    end
  end
  unreachable)
```

(the header's address is `local 1`, its carried blockparam, on both edges — which
is what the header's height check pins down), which is the spike's `countdown`
shape, and the zero-trip path ran under `wasmi` and returned the initial state.
**The backedge is unexercised by any test.**

**And the IR cannot yet build a loop that terminates.** This is the finding the
probe produced, and it is not a backend defect: `KernelInstr::LocalGet` names a
*parameter leaf*, and no instruction names a carried tuple element, so a body
cannot compute a state smaller than the one the preheader supplied. Every loop a
body can express today either runs zero trips or runs forever — the header's test
is a function of the function's own arguments, which do not change. That is the
shape the body model in [compute-kernel-struct](compute-kernel-struct.md)
leaves, and
it survives §3.2: a loop now *lowers* and is *expressible* as structure, but no
program that reaches it can terminate. **What is still missing is an instruction
that reads the carried tuple into the body** — the same gap that leaves the
acceptance case ([loop-conversion](loop-conversion.md) §8.5 item 5) unrun.

### 3.3 Module assembly onto waffle's `Module` — **done, with §3.1**

This **replaced** the emitter's own section-by-section assembly rather than sitting
beside it. What had to be reproduced exactly, because the host's linker resolves by
name and the cross-kernel call indices are computed from it:

- imports `env.read_i64`, `env.write_i64` (and the `f32` pair) — **each class's
  `read` first, then its `write`** — so `base = 2 × buffered_classes.len()`;
- one function per fragment, in the `ordered` BFS order, with `main` (the root)
  exported;
- `CallKernel`'s index is `base + index[kid]`.

`waffle`'s `to_wasm_bytes` writes every section from the module's index spaces, so
what `assemble.rs` has to get right is not *how* a section is serialized but **what
order the entities are pushed in** — the order is the index space. One shape the old
emitter had to compute by hand falls out of it: **`base` is `module.funcs.len()` at
the moment the imports are declared**, because no defined function exists yet.

### 3.4 Delete the hand-written structured emitter — **done, with §3.1**

`Frame`, `lower_flow`, `lower_terminator`, `emit_jump`, `block_arities`,
`count_carried`, `block_types`, `WasmState` and `lower_instrs` are gone. This was a
**deletion**, and it is the point of the exercise: those are the parts that kept
breaking.

**It also carried the class-mixing check out of `compute.rs`.** `OperandClass`,
`operand_class`, `as_operand_class`, `check_flow`/`check_instr`/`check_terminator`
and `mixed_classes` are `wasm/mixed.rs` now, next to the lowering that consumes
them. They are still a **read of the IR before any entity exists**, and their
wording is still the SPIR-V emitter's, byte for byte — that sentence is a contract
between two crates, not a constant one of them owns.

## 4. What not to do

- **Do not hand-write the stack.** Four defects came from one cause: operand-stack
  height tracked by a hand-maintained integer. That is
  [wasm-control-flow](wasm-control-flow.md) §2, and it is why the loop emitter was
  withdrawn. **The new lowering is not exempt from the cause, only from the
  silence**: two of §3.1's own bugs were the operand stack's order and count, and
  the 62 tests named both in one run.
- **Do not reason about emitted bytes by reading them.** `wasm-tools print` and
  `wasm-tools validate` are installed and settle in seconds what hand-decoding got
  wrong repeatedly. `cargo install wasm-tools` if the machine is fresh.
- **Do not treat `UnsupportedFeature` from waffle as a wall** — check it. `waffle`
  can reducify irreducible flow (`Reducifier`), so its limits are worth reading
  before working around them.

## 5. The question that was open above all of this — **decided**

The question was: **why does this repository hand-write a wasm backend at all?** The
CPU path exists so kernels run without a device; lowering to native code through
Cranelift, or interpreting `KernelBody` in the existing VM, are both alternatives to
owning a wasm code generator — and owning one had cost a session.

**Decision: the backend stays, and it lowers through `waffle`.** The cost was in
owning a *code generator*, not in emitting wasm: `waffle` is that generator, and
§3.1 is what is left of the work once it is. The alternatives were not priced
against that, and §3.2 is the test of whether this decision was right — if the loop
lands and the emitter is then short, the answer was yes; if §3.2 turns into another
session, reopen it.

**What the decision commits to**, which it did not before: `waffle` is a dependency
of the wasm path, and the 62 kernel-execution tests are the harness that says the
migration is working. There is no second backend to keep in step with it.
