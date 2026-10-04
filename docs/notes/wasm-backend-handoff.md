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
| §3.2 structured control flow (`If`, `Jump`, `While`) | `feature/waffle-spike` | **not started.** A body whose transfer is not a `Return` is refused by name |
| The real lowerer, for a loop | nowhere yet | **§3.2.** Read §3 before writing it |

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
   [loop-body-expressiveness](loop-body-expressiveness.md) §2.
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

The map itself, as landed:

| kernel | `waffle` |
|---|---|
| `Const(Int, bits)` | `I64Const { value: bits as u64 }` |
| `Const(Float, bits)` | `F32Const { value: bits as u32 }` — `Operator`'s value is the `f32`'s bits, so the 0.248 `Ieee32` the bump existed for is no longer ours to write |
| `LocalGet(k)` | the `k`th blockparam of the entry block — **the ABI passes each leaf as its own argument** |
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

### 3.2 Structured control flow — **next**

- **`If`** → `CondBr` to two arm blocks, both ending at a **join block whose
  parameters are the `passes` values**. A one-armed `If` has its absent side branch
  straight to the join with the stack as it stood — that is what makes a one-armed
  branch expressible without inventing a value.
- **`Jump`** → `Br` with the top `passes` values as `args`.
- **`While`** → exactly the spike's shape: reserve the carried tuple's blockparams
  on a header block *first*, then lower the test with those blockparams as the
  starting stack, then `CondBr { if_true: exit-with-the-top-`passed_out`, if_false:
  body }` where the body's last act is `Br` back to the header with the next state.

  **The header must leave the tuple and then the condition** (the IR's own contract)
  — that is what makes the exit's values read the right slots, and the old emitter
  asserted it at emission. Keep asserting it.

**The one shape §3.1 left open is decided, and it belongs here.** A `Flow::Block`
whose `entry` names a label is **both** a loop header and the block holding the
loop, and §3.1 refuses it by name rather than picking a meaning for it. **The
meaning is the header's**: the fragment's entry block stays the preheader, and the
labelled block becomes a block of its own holding the carried tuple as its
blockparams — which is the same shape the spike proved and the same shape
`FunctionBody::new` forces, since the entry block's blockparams *are* the function's
parameters and cannot be added to. A loop whose labelled block is the fragment's
entry has no representation, because the carried values would have nowhere to live.

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
