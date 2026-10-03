# Lowering structured control flow to WebAssembly

> Status: **design, and a withdrawn attempt.** A loop emitter for the wasm backend
> was written, found to need four more fixes before it would validate, and
> **withdrawn** in favour of the slot-based emitter §4 describes. What is recorded
> here is what the attempt cost and why, because every one of the four defects came
> from the same source: **reasoning about the operand stack by hand.**
>
> Read this before writing a loop emitter for wasm. §1 is the ground truth about how
> a loop is written at all, §2 is the four defects, §3 is the ordering bug that was
> hiding under them, §4 is the shape to build instead.

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

There is **no library that does this for us**: wasm has no IR at the level of MLIR's
`scf`/`cf`, and no crate lowers a structured CFG onto the stack machine. `wasm-encoder`
is an instruction emitter, `wat`/`wast` parse text, and Binaryen is C++. The mapping is
everyone's own work; what is not everyone's own work is doing it slot-first.

### 4.1 A cheaper variant, if the full slot-based pass is too much

Keep the stack-based emitter, but make the **stack shape a checked model rather than
an integer**: a small `Stack` type that every helper must go through (`push`, `pop`,
`set_local`, `get_local`, `open_frame`, `close_frame`), with a debug assertion at each
construct that the model agrees with the construct's declared type. That catches §2's
defects 2 and 3 at emission and turns the withdrawn attempt into something reviewable.
It does not make 1 and 4 unrepresentable, which is the argument for the full version.

### 4.2 What the existing machinery already is

`crates/lichen-compute/src/compute.rs` still holds the loop emitter's prerequisites:
`Frame::Header` with its locals range, `WasmState::{set_locals, get_locals,
loop_locals}`, `count_carried`, and the `carried_locals` reservation. They are marked
`#[allow(dead_code)]` with a pointer here, and they are the *slot* model's skeleton —
a slot-based emitter subsumes them rather than discarding them. `lower_terminator`'s
`While` arm **refuses a loop by name** until that emitter lands, so no body with a loop
is half-emitted.
