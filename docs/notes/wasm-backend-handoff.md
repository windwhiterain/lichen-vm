# The wasm backend: handoff

> Status: **handoff.** Read this first for anything touching wasm code generation.
> It is an index, not a design: the detail lives in
> [wasm-control-flow](wasm-control-flow.md) (the lowering, the four defects, the
> adopted design) and [loop-conversion](loop-conversion.md) §8 (loop conversion's
> own critical path).

## 1. What is on `dev`, and what is parked

| | where | state |
|---|---|---|
| `Terminator::Jump`; a loop body that computes its state and reaches the backedge | `dev` | **done**, tested |
| `passed_out` contract; `validate()` refuses `passed_out > carried` | `dev` | **done** |
| Block types declared *before* the type section is serialized | `dev` | **done** — a latent bug nothing hit until a loop named a block type |
| The wasm loop emitter | `dev` | **refused by name.** The hand-written attempt is withdrawn; the `While` arm refuses rather than half-emitting |
| `wasm-encoder` 0.248 bump | `feature/waffle-spike` | **verified**, not merged — one line, 62 kernel-execution tests green |
| The waffle spike | `feature/waffle-spike` | **passes** — a loop with a carried value, lowered and run through `wasmi` |
| The real lowerer | nowhere | **not started.** Read §3 before writing it |

**The `wasm-encoder` bump is independent of waffle and should be merged on its own
first.** It is `Instruction::F32Const(f32::from_bits(n as u32))` becoming
`...into()`, because 0.248's `F32Const` takes an `Ieee32`. Nothing else changed.

## 2. What is settled, and by what evidence

Five things were in doubt at the start of this work and are not any more.

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

## 3. The migration, in the order it should be done

**Do not attempt 3 and 4 together.** That is what went wrong in the attempt this
handoff replaces: a single draft that tried the whole mapping at once carried two
unresolved type questions and had to be thrown away.

### 3.1 Port the instruction map, with no control flow at all

Lower every body as **one block**. The mapping is `KernelInstr` → `Operator`:

| kernel | `waffle` |
|---|---|
| `Const(Int, bits)` | `I64Const { value: bits }` |
| `Const(Float, bits)` | `F32Const { value: f32::from_bits(bits as u32).into() }` |
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
times.

Because the IR is untyped, **each stack slot must carry its type** — `Option<WType>`,
where `None` means *this lowering cannot name it* (a cross-kernel call's result is
its callee's domain's) and is **never a guess**. This is the discipline the
hand-written emitter's `repr` held; keep it.

When this step is done, all 62 kernel-execution tests should pass with the new
lowering in place of the old emitter, and *that* is the signal to continue. It is
also worth landing it at that point.

### 3.2 Structured control flow

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

### 3.3 Module assembly onto waffle's `Module`

This **replaces** the current `assemble_module` rather than sitting beside it. What
must be reproduced exactly, because the host's linker resolves by name and the
cross-kernel call indices are computed from it:

- imports `env.read_i64`, `env.write_i64` (and the `f32` pair) — **each class's
  `read` first, then its `write`** — so `base = 2 × buffered_classes.len()`;
- one function per fragment, in the `ordered` BFS order, with `main` (the root)
  exported;
- `CallKernel`'s index is `base + index[kid]`.

One shape still needs a decision, and it is the same one that bit the hand-written
emitter: a `Flow::Block` whose `entry` names a label is **both** a loop header and
the block holding the loop. Decide it explicitly here — do not let it fall out.

### 3.4 Delete the hand-written structured emitter

`Frame`, `lower_flow`, `lower_terminator`'s `If`/`While` arms, `block_types`,
`WasmState` and the block-type bookkeeping. This is a **deletion**, and it is the
point of the exercise: those are the parts that kept breaking.

## 4. What not to do

- **Do not hand-write the stack.** Four defects came from one cause: operand-stack
  height tracked by a hand-maintained integer. That is
  [wasm-control-flow](wasm-control-flow.md) §2, and it is why the loop emitter was
  withdrawn.
- **Do not reason about emitted bytes by reading them.** `wasm-tools print` and
  `wasm-tools validate` are installed and settle in seconds what hand-decoding got
  wrong repeatedly. `cargo install wasm-tools` if the machine is fresh.
- **Do not treat `UnsupportedFeature` from waffle as a wall** — check it. `waffle`
  can reducify irreducible flow (`Reducifier`), so its limits are worth reading
  before working around them.

## 5. The one open question above all of this

**Why does this repository hand-write a wasm backend at all?** The CPU path exists
so kernels run without a device. Lowering to native code through Cranelift, or
interpreting `KernelBody` in the existing VM, are both alternatives to owning a wasm
code generator — and owning one has now cost a session. That question was raised and
not decided; it is worth deciding before the migration above, not after.
