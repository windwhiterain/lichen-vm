# Per-value class semantics in a kernel body

> Status: current.
>
> What this note is: how a kernel's `Int`/`Float` **classes** are decided
> per-value, why a body may compute in one class and cross to the other, and
> what each target pays for it. It is the contract a kernel body's arithmetic
> obeys, on both backends.
>
> Points at: `crates/lichen-compute/src/compute/body.rs` (the lowering that
> assigns `KernelInstr` classes), `crates/lichen-compute/src/compute/wasm/mixed.rs`
> (the shared class check), `crates/lichen-compute-gpu/src/spirv.rs` (the SPIR-V
> module contract), and `crates/lichen-language/tests/compute.rs` (the
> end-to-end pins).

## The class of a value is a fact of the value

The lowered kernel IR is **untyped**: every instruction declares the class it
produces, and a class is read off *values*, never off the checker's nodes. Each
backend supplies the types, and the two must agree — so the mix refusal's wording
and its rule are shared:

- **A `Bin`'s class is its operands', not its result's.** The lowering emits the
  two operands first and reads the class they produced; the node's own class is
  only the fallback when an operand emitted no class at all. Trusting the node
  was a real defect: a comparison's result is the language's `0`/`1` scalar
  whatever the operands are, so `x > 1.0` over two `Float`s declared
  `Bin(Int, Gt)` and the class check refused a mix — correctly, and for the wrong
  reason. Arithmetic happens to agree either way; a comparison does not.
- **A comparison yields the `0`/`1` scalar, which is `1`/`0` in either class**,
  so a condition is never "an integer" that a float position has to refuse. All
  three uses — a `select` condition, a branch's condition, a stored predicate —
  go through that one fact, and each backend materialises the scalar in the
  class the position wants.
- **A block parameter's class comes from the branches that reach it**, and a
  cross-kernel call's results are *unclassifiable* rather than the other class.
  Unclassifiable is the absence of an answer; a check that read it as one would
  refuse a fragment the IR says nothing about, so it is carried and never judged.

## A body may compute in one class and cross

**A body may compute in one class and cross to the other**, and the crossing is
the explicit `int2float`/`float2int` (`KernelInstr::Conv`). `int2float (x + 1)`
over `x : Int` is legal and runs; it answers `6.0: Float`. There is no
fragment-wide "one representation" rule — a body may hold both classes at once.

What stays refused, and the refusal names the instruction position:

- **A genuine mix in one operation** (`x + 0.5` with no crossing): `mixed_classes`
  refuses it *where the two classes actually meet*, which is the operation, not
  the arithmetic that produced its operand. `Int` and `Float` do not convert in
  either direction, so a value of one class in a position of the other is a
  malformed fragment rather than an unsupported shape.
- **A float `%` or bitwise operator**, refused at the operand where the class is
  still visible: a kernel's float operators are `+ - * /` and the four order
  comparisons.
- **A conversion whose operand is not the class it converts from** — `int2float
  1.0` is refused before the body is lowered.

`from == to` in a `Conv` is a **reclassification, not a no-op**: it is what the
lowering writes for a value that already holds its result's representation.

**Why the instruction carries the pair rather than a single class.** Which of the
two crossing directions a `Conv` is *cannot* be read off its operand: `Int → Float`
and `Float → Int` are the same shape to an operand, and the direction is the
language's decision rather than a target's. A backend that re-derived it would be
guessing, and the two could guess differently — the exact failure
[floating-point §5.1](floating-point.md#51-the-permission-and-what-it-still-refuses)
records the wasm backend producing. It is the same reason every other instruction
names its own class rather than leaving one inferred: a fact the language decided
is stated, not reconstructed. And a conversion is the **one** instruction whose
operand and result are different classes, so naming "the class" would name one of
them and leave the other guessed.

**What the pair does not say is the representation.** It says what the language
asked for, not what the value's representation is, and the two differ at the ABI: a
float fragment's index and count arrive in `f32` locals (the fragment's class is
what the parameter list is typed by) while the language's number is an `Int`, so
`int2float` of the index is a conversion whose operand already holds its result's
representation. That is a fact about the target and the ABI rather than about the
program, so it is not in the instruction and no backend may assume it: each tracks
the representation it is actually building and lowers a crossing between a class
and itself to nothing.

Neither opcode is named in the IR, because the two targets hold the same number in
genuinely different places:

- wasm's locals are typed per class, so the conversion is the opcode that crosses
  them (`f32.convert_i64_u`, `i64.trunc_f32_u`) — and **nothing at all** when the
  value on the stack already holds a `to`.
- SPIR-V's index is the invocation id — a 32-bit integer in every module, float
  included — so `Int → Float` there is always `OpConvertUToF`, and a module that
  has not declared the other class's type refuses the direction by name rather than
  declaring a type it did not need.

`Float → Int` **truncates toward zero in neither backend's promise**: the
interpreter refuses what it cannot represent ([operators](operators.md)), wasm
traps, and SPIR-V is undefined. A kernel is the intersection of what the backends
compute *the same way*, and this conversion is in it only for the values both
answer identically.

## Every SPIR-V module declares both element types

The price of the crossing is that a module's arithmetic class and a buffer's
element class are separate facts, and **both element types exist in every
module**:

- `OpTypeFloat 32` and the float `0.0`/`1.0` constants are declared **in every
  module**, not only in float ones: `Float32` is core SPIR-V and costs no
  capability, and an int module that materialises a comparison into a float
  position needs them. An unused module-scope constant is legal.
- **Only the 64-bit integer is conditional.** An integer module declares
  `OpTypeInt 64 0` and the `Int64` capability; a float module's integer is
  32-bit, so it needs no device with `shaderInt64` (`needs_int64` is what a
  caller checks before building a pipeline).
- So **a crossing always has the type it needs**: `Int → Float` is
  `OpConvertUToF` and `Float → Int` is `OpConvertFToU`, in both directions of
  module class. The unsigned reading is deliberate — the language's `Int` is a
  machine-sized unsigned integer, so `OpConvertSToF` would read the same bits and
  answer a different number above the signed range.
- **A module's arithmetic has one class; each buffer its own.** The module's
  arithmetic class is baked in (it selects the opcodes a `Bin` reaches and is the
  fallback for a slot the classes do not reach), while a buffer's element type,
  array stride, block struct and variable type are read per buffer.

The measured price, which is a documented truncation and not a defect:

- **In a float fragment, `Int` data is `u32` on the GPU target and `i64` on the
  wasm target**, so the two diverge past 2³². It is the same kind of recorded
  price as kernels computing `f32` while the interpreter computes `f64`
  ([floating-point](floating-point.md)).
- **In an int fragment, `Float` data is `f32` on both targets.**

## Where a class is fixed in the shape

A fragment's `param_shape` leaf carries a `ScalarClass`, and a
`KernelFragment`'s `input_classes`/`output_classes` hold one class per buffer
position or write ordinal. The buffers' classes cannot live on the shape, because
a parallel fragment's shape is the parameter's scalar leaves followed by the
index, however many buffers the body reads. All of them are in
`fragment_digest`, so two fragments differing only in a class cannot intern to one
id.

A **decided** leaf's class is its parameter field's: `scalar_leaf_classes` reads
each parallel parameter leaf's class from the field the role walk names, so a
runtime `Float` scalar reaches the body's own argument. Two parallel leaves stay
`Int` whatever the parameter says, because neither is data: the **index** is a
lane number, and the **first scalar** is the launch extent — a `Float` one there
is refused by name. See
[compute-runtime-scalars](compute-runtime-scalars.md).

## What is still open

- **A parallel run's result class reaches the type graph through the read
  argument's shape rather than through the result's own type cell.** A `plrun`
  result's element type is undecided until the host annotates the binding
  ([compute-buffer-wrapper](compute-buffer-wrapper.md) records the recipe), and
  the honest fix — re-establishing the class on the result's own cell — is the
  design question of [class-channel](class-channel.md) §2/§3.
