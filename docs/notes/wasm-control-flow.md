# Lowering structured control flow to WebAssembly

> Status: **design, adopted.** A loop emitter for the wasm backend was written, found
> to need four more fixes before it would validate, and **withdrawn**. What replaced
> it is **`waffle`** (§5), which owns the whole slot-first pipeline, and a spike has
> proved it end to end. **For anything actionable, read
> [wasm-backend-handoff](wasm-backend-handoff.md) first**; this note is the analysis
> behind it.
>
> §1 is the ground truth about how a loop is written at all, §2 the four defects of
> the withdrawn attempt and their one cause, §3 the ordering bug that was hiding
> under them, §4 the shape that replaced the hand-written emitter, and §5 the
> adoption and its evidence.

## 1. How a loop is written in wasm, exactly

A loop needs a **`loop`** (the header, which a backedge re-enters) wrapped in a
**`block`** (the exit, which a branch leaves), and four rules do all the work:

1. **A `br` to a `loop` label carries no operands.** The `loop` frame exists to be
   re-entered; there is nowhere for operands to go. So **the loop's own type is
   `[] -> []`** — with a result type, the backedge would be required to hand the
   result over, which no backedge can do. A branch *out* of the loop is a branch to
   the enclosing `block`, and that is the frame whose type carries the exit's values.
2. **The loop's test has to run before the loop opens as well as at the `loop`
   label.** A `While` that is emitted where the terminator sits computes its
   condition once, outside the loop. The condition is the loop's own code, so the
   `block`/`loop` pair opens *before* it and the backedge re-enters it.
3. **A frame's base height is fixed when it opens.** A `block`'s results are its
   *produced* values; anything the stack held below the frame is unwound away by a
   branch that exits it. So a loop has to be reached with its state as the **whole**
   operand stack, and values the surrounding code holds cannot survive across it.
4. **The state has to be in locals before the pair opens**, because the test runs
   before the loop and again at the label — and rule 1 means the backedge cannot
   carry the next state back as an operand.

Rule 4 is the one that spills into everything else: the state tuple needs locals, the
preheader and the test each need locals of their own, and **which locals those are has
to be decided before any of them is emitted**.

## 2. The four defects, and the one cause

Each of these was written, shipped to a failing test, and found by reading the
emitted bytes:

1. **The condition was computed once, outside the loop** — the `block`/`loop` pair
   opened after the header's instructions had already been emitted. Nothing needs
   `Flow::Block`'s own terminator-vs-body distinction more than this.
2. **The recorded stack height drifted.** `set_locals`/`get_locals` changed the
   operand stack but not the `depth` field that a later branch consulted, so a
   height assertion passed while the stack was in a different place.
3. **The loop's locals collided with the code that feeds it.** The tuple's locals were
   handed out when the loop opened, *after* the preheader and test had been compiled —
   and their instructions name locals too. Two live values landed in one slot, so a
   `local.get` read the wrong one. (The module still *validated* in some shapes; the
   numbers were simply wrong.)
4. **The loop's frame was given the exit's type**, so the backedge was required to
   supply results it could not carry — §1 rule 1.

Four defects, one cause: **all four are properties of the operand stack, and the
emitter was tracking that stack in a hand-maintained integer.** A machine-checked
model would have caught 2 and 3 at emission; 1 and 4 are shape errors that a typed
lowering makes unrepresentable.

**What actually found them** was not reading the code — it was `wasm-tools print` and
`wasm-tools validate` on the emitted bytes. Both crates were already in the tree
(`wasmi` depends on them); the CLI is a `cargo install`. Any future wasm work should
reach for them before reasoning about bytes.

## 3. An ordering bug that was hiding under it

**Every block type was declared after the type section had been serialized.**

```rust
wasm.section(&types);          // the section is written here
// ... and then, later:
let id = types.len();
types.ty().function(vec![], vec![value_type(class); arity]);
block_types.insert((class, arity), id);
```

`wasm.section` serializes, so anything added to `types` afterwards is in no section at
all — the index a body names then refers to a type the module does not have, which
wasmi reports as `unknown type: type index out of bounds`. Nothing noticed while no
body named a block type; the first loop did. **Fixed** (the pairs are collected and
declared before the section is written), and it is the one part of this work that is
independently correct and landed.

## 4. The shape to build instead: slot-based, stack later

**Every value of a structured body gets a local.** The emitter never asks what is on
the operand stack, because the answer is always *nothing*: an instruction reads its
operands from locals and writes its result to a local. A loop then needs no special
discipline at all — the backedge writes the state's slots, and §1's rules 1, 3 and 4
stop being the emitter's problem, because nothing is ever on the stack to survive a
frame.

A later pass collapses slots back onto the stack where that pays. **That collapse is
where height reasoning belongs**, in one place, over a finished function, instead of
distributed across every construct that emits a branch. This is what LLVM's, Cranelift's
and Binaryen's wasm backends all do — Binaryen's pass for it is literally called
`Stackify` — and it is why none of them track a height by hand.

There is **no library that lowers an arbitrary CFG onto wasm's stack machine** in the
way MLIR's `scf`/`cf` dialects structure the problem for SPIR-V: `wasm-encoder` is an
instruction emitter, `wat`/`wast` parse text, and Binaryen (which has the pass, called
`Stackify`) is C++. **What does exist is `waffle`, which owns the whole slot-first
pipeline** — see §5, which is where this work ended up rather than hand-writing §4.

### 4.1 A cheaper variant, if the full slot-based pass is too much

Keep the stack-based emitter, but make the **stack shape a checked model rather than
an integer**: a small `Stack` type that every helper must go through (`push`, `pop`,
`set_local`, `get_local`, `open_frame`, `close_frame`), with a debug assertion at each
construct that the model agrees with the construct's declared type. That catches §2's
defects 2 and 3 at emission and turns the withdrawn attempt into something reviewable.
It does not make 1 and 4 unrepresentable, which is the argument for the full version.
**This variant was not taken**: §5's spike settled the question in favour of the
library, and hand-writing either version is now off the table.

## 5. Adopting `waffle`, and the spike that settles it

**Decision: the wasm backend lowers through [`waffle`](https://github.com/bytecodealliance/waffle)
(a Bytecode Alliance crate, Apache-2.0 WITH LLVM-exception, v0.3.2).** It defines
an SSA IR with **block params**, and its backend is a table of contents for exactly
the work §2 lists as gone wrong:

```
waffle/backend/
  reducify.rs   // makes the CFG reducible (so an irreducible input is not a wall)
  treeify.rs    // trees
  stackify.rs   // → WasmBlock::{Block, Loop, If, Br, Select, BlockParams, …}
  localify.rs   // which values need locals
```

`StackifyContext::compute()` is the CFG→structured-wasm mapping; `Localify` is the
"which value survives a backedge" question; `lower_value`/`lower_set_value` are the
`local.get`/`local.set` discipline. **Block params are the loop-carried value**, so
`Terminator::While`'s `carried`/`passed_out` map onto `FunctionBody::add_blockparam`
and a `BlockTarget`'s `args` — no `OpPhi` to place and no locals to allocate by hand.

### 5.1 The spike, and what it proved

A spike (`crates/lichen-compute/src/waffle_spike.rs`) built the countdown loop as a
CFG — `entry(n) → head(c) → {done(c) | body(c)}, body(c) → head(c-1)` — with the
carried value as `head`'s blockparam, compiled it, and ran it through `wasmi`. It
passes for `n ∈ {0, 1, 4, 64, 1000}`, and `waffle` emitted:

```wat
(func $countdown (param i64) (result i64)
  (local i64 i64 i32)
  local.get 0
  local.set 2
  loop                       ;; no block type: a backedge carries no operands
    local.get 2
    i64.eqz
    local.set 3
    local.get 3
    if
      local.get 2
      local.set 1
      local.get 1
      return
    else
      local.get 2
      local.set 2
      local.get 2
      i64.const 1
      i64.sub
      local.set 1
      local.get 1
      local.set 2
      br 1
    end
  end
  unreachable)
```

That is §1's four rules already obeyed, and §4's slot-based strategy already
implemented — with more `local.set` traffic than is ideal, which is what a later
peeling pass (`basic_opt`, or our own) is for. **Correctness first.** Note also that
`i64.eqz` produces an `i32`, so a `CondBr` condition needs the narrowing the kernel
IR spells `KernelInstr::I32WrapI64`; the type checker states that fact here, where
the hand-written emitter had to remember it.

### 5.2 The two facts that had to be checked first, both settled

1. **A loop's state may not live in the entry block.** `FunctionBody::new` builds the
   entry block's blockparams *from the function signature*, and a `CondBr`'s two
   targets must pass the same number of args — so a loop needs a block of its own,
   entered through a preheader. **That is the kernel IR's own shape** (`Flow::While`'s
   header is a block inside the fragment, and the entry block is the preheader), so
   the two agree rather than one bending to the other.
2. **The version gap between `waffle`'s `wasm-encoder`/`wasmparser` 0.248 and the
   `wasmparser` 0.228 that `wasmi` 2.0.0 validates with is not a problem.** The spike
   proves it end to end: `wasmi`'s own validator accepts and runs 0.248's output. And
   our existing emitter moves to 0.248 with **one mechanical change**
   (`Instruction::F32Const(..)` now takes an `Ieee32`, so `..into()`), after which the
   whole kernel-execution suite — 62 tests that assemble and run real kernels through
   `wasmi` — passes.

