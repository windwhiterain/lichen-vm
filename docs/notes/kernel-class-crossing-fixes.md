# Plan: finish per-value class semantics — the three conversion fixes

> Status: **landed.** All three work items are implemented and the three tests
> are green, both backends included; §9 records what was measured and the one
> addition the plan did not name.
> Worktree: `.worktrees/kernel-param-struct`, branch `feature/kernel-param-struct`.
> Supersedes the diagnosis (not the accessor part) of
> [type-query-api-proposal](type-query-api-proposal.md) — see §6.

## 0. What is broken, measured

Three tests in `crates/lichen-language/tests/compute.rs` fail on this branch.
They have **three distinct causes**, and none of them is a missing type-query
API:

```bash
cargo test -p lichen-language --test compute -- \
  a_jit_kernel_crosses_the_two_classes_both_ways \
  a_conversion_the_body_cannot_hold_is_refused_by_name \
  a_varying_float_element_is_seeded_from_the_index
```

1. **`a_jit_kernel_crosses_the_two_classes_both_ways`** — `emit_node`
   (`crates/lichen-compute/src/compute.rs:4314`) binds
   `let class = node_class(module, node)` — the class of the node's **result**
   — and at `:4586` emits `KernelInstr::Bin(class, bin)`. But a `Bin`'s class
   field is the **operands'** class: the validator
   (`check_instr`, `compute.rs:2965`) constrains both operands against it, and
   the wasm emitter picks the opcode from it. For arithmetic result == operand
   class, but a **comparison's** result is the language's `Int` `0`/`1` whatever
   the operands are — so `x > 1.0` over two `Float`s emits `Bin(Int, Gt)` and
   the validator refuses a mix. Observed: `instruction 2 mixed an integer and a
   float`.

2. **`a_conversion_the_body_cannot_hold_is_refused_by_name`** — the two refusal
   strings it expects (`"one fragment has one representation"`, `"no form in an
   Int kernel body"`) **exist only in the test**; no emitter produces them. The
   per-value-class commits on this branch removed the fragment-wide
   representation rule, so `int2float (x + 1)` over `x : Int` now *runs* and
   answers `"6.0: Float"`. The test pins semantics the branch deliberately
   replaced; it must be rewritten, not satisfied.

3. **`a_varying_float_element_is_seeded_from_the_index`** — the CPU half
   passes; the GPU half is refused by `spirv.rs:1155`:
   `Conv { from: Int, to: Float }` in a float module hits
   `if from != ids.class { return UnsupportedConversion }`. The refusal confuses
   the **language class** with the **representation the module declares**: a
   float module declares both `u32` (`ids.uint`, the gid's type) and `f32`
   (`ids.float`), so `OpConvertUToF` of the index is fully representable.
   Note: `Ids::integer()` already routes `Int` to `u32` in a float module
   (`spirv.rs:632`), so integer arithmetic in a float fragment already lowers —
   the `Conv` refusal is the only blocker.

## 1. The semantics decision (superior, settled)

**`int2float (x + 1)` is legal.** A body may compute in one class and cross to
the other; the crossing is the explicit `Conv`. The price, to be documented:

- In a **float** fragment, `Int` data (not just indices) is `u32` on the GPU
  target and `i64` on the wasm target; past 2³² the two diverge. This is the
  same kind of documented price as `Float` being `f32` in kernels while the
  interpreter computes `f64`.
- In an **int** fragment, `Float` data is `f32` on both targets.
- A genuine mix in one operation (`x + 0.5` with no crossing) is still refused:
  `refuse_mixed_classes` is unchanged.

## 2. Work item 1 — `emit_node`: a `Bin`'s class is its operands'

File: `crates/lichen-compute/src/compute.rs`.

At the binary-operator arm (`:4558-4587`), stop using the frame's `class`
binding (the node's result class) for `KernelInstr::Bin`. Compute the operand
class from the two operands instead:

```rust
// The class a Bin carries is its OPERANDS', not its result's: a comparison's
// result is the language's Int 0/1 whatever the operands are, so reading the
// node's own class here would emit Bin(Int, Gt) over two Floats.
// The checker refuses a genuine mix before lowering, so a disagreement between
// the two operands here is a decided Float beside an undecided leaf that
// defaulted to Int — the decided one wins.
let operand_class = match (node_class(module, left), node_class(module, right)) {
    (ScalarClass::Float, _) | (_, ScalarClass::Float) => ScalarClass::Float,
    _ => ScalarClass::Int,
};
```

Then the bitwise/`Rem` refusal (`:4573-4583`) reads `operand_class` directly
(it currently computes the same condition inline — merge them), and the push is
`KernelInstr::Bin(operand_class, bin)`.

The frame's `class` binding stays for the `Const` arm only; if it becomes
unused elsewhere, narrow its scope rather than keeping a dead binding.

Boundary to document in the comment: two *undecided* leaves of a **struct**
parameter still read `Int` (the parallel ABI seeds every scalar leaf `USize`,
`compute.rs:2268-2271`) — making that honest is the specialization work in §6,
not this fix.

## 3. Work item 2 — spirv: every crossing the language has is representable

File: `crates/lichen-compute-gpu/src/spirv.rs`.

1. **Declare `OpTypeFloat 32` in every module**, not just float ones
   (`assemble`, `:1372-1380`): `Float32` is core SPIR-V and costs no
   capability. Move the `TYPE_FLOAT` instruction out of the `match`; the
   `TYPE_INT 64` + `Int64` capability stay int-module-only (`needs_int64` is
   unchanged and keeps answering the device-chooser's question).
2. **Declare the float `1.0`/`0.0` constants unconditionally** (`:1413-1438`)
   for the same reason module-scope unused constants are already declared: an
   unused one is legal, and a comparison materialised into a float position in
   an int module needs them.
3. **Delete the refusal at `:1155`** (`if from != ids.class { UnsupportedConversion }`)
   and the now-unconstructed `SpirvRefusal::UnsupportedConversion` variant with
   its `Display` arm. With both element types declared in every module, both
   crossings always have their types: `Int → Float` is `OpConvertUToF` from the
   module's integer (`u64` in an int module, `u32` in a float one — `as_class`
   + `ids.type_of` already route this), `Float → Int` is `OpConvertFToU` to it.
   Replace the deleted check's comment with one stating the new invariant:
   *every module declares both element types; only the 64-bit integer (and its
   `Int64` capability) is conditional, which is why a float module's `Int`
   data is 32-bit and a value past 2³² diverges from the wasm target — the
   recorded price (`docs/notes/floating-point.md`).*
4. Delete `an_integer_module_refuses_a_conversion_by_name` in
   `crates/lichen-compute-gpu/tests/refusals.rs` — it pins the refusal this
   item deletes. Note: the whole `lichen-compute-gpu` **test** suite does not
   compile on this branch (the per-value `KernelInstr` change predates it; 57
   errors in `gpu_matches_cpu.rs` alone). Repairing that suite is **out of
   scope** here; record it in §7.

## 4. Work item 3 — rewrite the stale test, pin the new semantics

File: `crates/lichen-language/tests/compute.rs`.

Replace `a_conversion_the_body_cannot_hold_is_refused_by_name` with a positive
test, e.g. `a_body_may_compute_in_one_class_and_cross`:

- jit half (CPU): `compute.jit (x : Int => int2float (x + 1))`, launch `5`,
  expect `"6.0: Float"`.
- parallel half (both backends, via `answer_from_each_backend` like its
  neighbours): `compute.write [n, i, float2int (int2float i + 0.5)]` — every
  element comes back equal to its index, and `gpu == cpu`. This half exercises
  float arithmetic **inside an int module** on the GPU, which is what work
  item 3.1/3.2 buys.

Remove both stale refusal strings. Sweep `crates/lichen-compute/src/compute.rs`
comments for the deleted "one representation per fragment" rule and reword
where found (the validator's own `mixed_classes` refusal for a genuine mix
stays).

## 5. Docs

- `docs/notes/floating-point.md`: record the decision in §1 — the "varying
  float element is not writable" record is obsolete (it is the now-passing
  test); add the truncation clause: *in a float fragment, `Int` data is `u32`
  on GPU and `i64` on CPU; in an int fragment, `Float` data is `f32` on both;
  a crossing is always representable on both targets.*
- `docs/notes/type-query-api-proposal.md`: mark the status header —
  its *diagnosis* of the three failing tests is superseded by §0 here; its
  `shape.rs` accessor part (`TypeRef`, `field_list`/`field_type`/
  `field_names`/`field_index`) is **not** built now and is deferred to the
  specialization work in §6.

## 6. What comes after (recorded direction, not this task)

The structural direction, settled in discussion, is **specialize before JIT**:
a kernel is never compiled from a template — at `jit`/`parallel` time the
function is applied to a placeholder typed by the annotated domain, so every
term's type cell is decided in the graph itself. Consequences, when that lands:

- named reads fold to constant indices during specialization (the emitter's
  two-pass `param_path` dies);
- the parallel ABI's all-`Int` seed fiction (`compute.rs:2268-2271`) dies —
  scalar parameter leaves get their real classes;
- the `shape.rs` field accessors from the superseded proposal are built then,
  consumed by the specialize pass;
- `low_type_of` learns to read a struct as its positional tuple **as a JIT
  layout view** (the nominal-identity refusal at `shape.rs:849-852` stands for
  type questions) — this needs an audit of `low_type_of`/`low_type_of_slot`
  consumers first;
- the class question collapses to one channel; no `class_of` adapter is built.

Open risk recorded: `parallel_sig`'s `Parameterized` blocker (see
[compute-param-struct-handoff](compute-param-struct-handoff.md) §5) is an
apply-with-symbolic-argument gap on exactly the path specialization needs —
prototype the typed-placeholder apply before committing to the rest.

## 7. Non-goals and traps

- **Do not build** `TypeRef`, `class_of`, or any new `shape.rs` accessor.
- **Do not** change `refuse_mixed_classes` / `mixed_classes`: a genuine
  same-operation mix is still refused.
- **Do not** repair the stale `lichen-compute-gpu` test suite (pre-existing,
  this branch); only remove the one test named in work item 3. Follow-up:
  port that suite to the per-value `KernelInstr`.
- **Do not** touch the wasm block-type question (`WasmState.class`) or
  `parallel_sig`'s blocker 2.
- `cargo fmt` + `cargo fix --allow-dirty` before commit; keep the existing
  comment style (contract comments on changed items).

## 8. Verification

```bash
# The three formerly-failing tests, green:
cargo test -p lichen-language --test compute -- \
  a_jit_kernel_crosses_the_two_classes_both_ways \
  a_body_may_compute_in_one_class_and_cross \
  a_varying_float_element_is_seeded_from_the_index

# The branch's green list stays green (per compute-param-struct-handoff §6):
cargo test -p lichen-compute
cargo test -p lichen-kernel-ir
cargo test -p lichen-graph-ir
cargo test -p lichen-language --test compute
cargo test -p lichen-language --test pipeline
cargo test -p lichen-language --test examples

# The GPU library still compiles (its test suite is pre-existing breakage):
cargo check -p lichen-compute-gpu
```

## 9. What landed, measured

All of §2–§5 as written. The three tests that were red are green, and the
acceptance list of §8 is green with them: `lichen-compute` 17, `lichen-kernel-ir`
14, `lichen-graph-ir` 3, `lichen-compute-gpu --lib` 0 (no in-crate tests),
`lichen-language --test compute` 54 passed / 2 ignored, `--test pipeline` 132,
`--test examples` 1.

**The parallel half of the new test is the case §3.1/§3.2 exists for, and it runs
on the device**: `compute.write [n, i, float2int (int2float i + 0.5)]` over
`LOCAL_SIZE_X + 5` elements is an *integer* fragment that computes in `Float`, so
its SPIR-V module holds both element types at once — and every element comes back
equal to its index, `gpu == cpu`. The module was also read back with
`spirv-val --target-env vulkan1.1` (via `cargo run -p lichen-compute-gpu --example
emit-spv`), which accepts it: `OpTypeFloat 32` and the float constant pair are
now declared beside `OpTypeInt 64 0` and `OpCapability Int64`.

**One addition the plan did not name, because the measurement found it.**
`as_condition`'s `Kind::Scalar(Float)` arm bitcasts the float to the 32-bit
`uint` and compares it against `zero_of(Int)`. In an *integer* module that zero is
a 64-bit constant, so the comparison was `OpINotEqual` over a `uint` and a
`ulong` — an invalid module. Declaring the float type in every module is what
makes that arm reachable in an integer module in the first place, so the arm now
compares against `ids.zero`, the 32-bit `0` every module already declares. In a
float module the two ids are the same, so nothing there changed.

**Recorded, not repaired:** the `lichen-compute-gpu` test suite still does not
compile on this branch (36 errors in `spirv_validation.rs` and
`gpu_matches_cpu.rs`, all of them the pre-per-value `KernelInstr` shape: `Const`
and `BufferReadCall` without a class, `KernelFragment::results`). Work item 3.4's
deletion is the only edit made there. Porting that suite is the follow-up named
in §7.
