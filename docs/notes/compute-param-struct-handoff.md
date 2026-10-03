# Handoff: running a parallel kernel whose parameter is a struct

> Status: **open — two blockers, both measured.** Everything else on this path
> works and is verified; a fresh session can start at §4 (blocker 1) without
> re-deriving anything above it.
> Companion: [compute-kernel-struct](compute-kernel-struct.md) (the kernel
> struct itself), [applied-struct-nominal-id](applied-struct-nominal-id.md) (the
> identity property the design rests on), [floating-point](floating-point.md)
> §4.2/§4.4 (why the two classes cannot meet).
> Worktree: `.worktrees/kernel-param-struct`, branch
> `feature/kernel-param-struct`.

## 1. What is being built, and why

A parallel kernel's parameter is being moved from the positional
`cfg = (n, (buffers…))` to a named struct, so that

- a read names its buffer (`compute.read [k.in.x, i]`) instead of counting a
  position, and
- the kernel can take **runtime scalars** (`k.alpha`) beside its count.

The author writes one parameter, `struct<.n Int, .in I, .out O>`; the JIT turns
it into `struct<.n Int, .I I> -> O` — `.out` moves from the parameter to the
result, because the backend allocates those buffers and the host has no value to
put there. That split is what makes the host's argument (`struct<.n Int, .I I>`)
a struct it *can* fill.

## 2. The reproduction

Drop this in `examples/` as `zz-param-struct.lichen` (the harness runs every
file in that directory; scratch files must not be committed). It compiles a
tuple-shaped producer first, so the consumer has a real input buffer.

```lichen
---
  order = "99"
  compute = import "compute.lichen"
  output = "22"
---
g = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 10]
}
kg = compute.parallel g "cpu"
inbuf = compute.plrun kg (3,)
In  = struct<.a _>
Out = struct<.z _>
Par = compute.P (compute.KT _)(.I In, .O Out)
Sig = compute.S (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  v = compute.read [k.in.a, i]
  compute.write [k.out.z, i, v * 2]
}
k = compute.parallel_sig f "cpu" Sig
out = compute.plrun k ((compute.A In)(.n 3, .I In(.a inbuf)))
compute.read [out, 1]
```

`inbuf` is `[10, 11, 12]`, so the answer is `22`.

Run it with:

```bash
cargo test -q -p lichen-language --test examples
```

The harness prints `actual:` for the failing file. Today it prints
`parameterized: ?a`, and the module carries the refusal named in §4.

**The same probe with `compute.parallel f "cpu"` (no signature) reaches the
lowering and reports §4's refusal instead.** That difference is §5's whole
subject, and it is the cheapest way to isolate the two blockers from each other.

## 3. What already works, measured

- **The type lambdas** (`crates/lichen-compute/src/compute.lichen`):

  ```
  KT = _x => struct<.I _, .O _>                      # the input/output pair
  A  = I  => struct<.n Int, .I I>                    # the JIT'd input
  P  = T: KT T => struct<.n Int, .in T.I, .out T.O>  # the author's parameter
  S  = T: KT T => A T.I -> T.O                       # the JIT'd signature
  ```

  Two spelling facts, both load-bearing: the argument must be an *instantiated*
  pair (`P (KT _)(.I In, .O Out)`; a pre-bound pair answers "this value is not a
  container"), and applying a type lambda before instantiating needs parentheses
  (`(A In)(.n 3, …)`, not `A In(.n 3, …)`).

- **The host's argument type is the signature's domain.** Measured:
  `(f : Sig) => f ((A In)(.n 3, .I In(.a inbuf)))` checks, i.e. the `A In` the
  host writes at the call site and the `A _` inside `S` — evaluated by the
  checker once the role wiring resolves the cells — are *one* nominal type. This
  is what `applied-struct-nominal-id`'s fix bought, and it is why the signature
  can be declared in the kernel struct's `.sig` rather than built by a gate.

- **The role table** (`parallel_roles`, `compute.rs:1856`) decodes the
  parameter's type: scalars, `.in` paths and `.out` paths, in declaration order.
  For the probe, `inputs == [[1, 0]]` — `.in` is field 1 and `.a` is its field 0
  — which is exactly the path `k.in.a` should resolve to.

- **The wasm signature and the host-side decode.** The signature is the scalar
  leaves followed by the index, and `ParLaunch`'s walk reads the count at
  `cfg_items[0]` and the buffers at `cfg_items[1]` — which for this parameter
  shape are `.n` and `.I`, i.e. the same two positions the tuple shape uses.

- **The `Parallel`/`ParLaunch` gates need no change.** Declaring the signature in
  `.sig`'s *type* position is enough: `plrun`'s gate already unifies `sig.ty`
  against `arrow(d0, c0)` and then the host's argument against `d0`.

- **The temporary blanket refusal is gone.** `PARALLEL_PARAM_LAUNCH` no longer
  exists; a struct-shaped kernel reaches the lowering and is refused by the read
  arm's own message. (It was there because a *silent* `parameterized` is the
  worst possible answer; the read arm's message is a cause, so the blanket one is
  no longer needed.)

## 4. Blocker 1: `param_path` cannot resolve a named-field path

**Symptom.** With `compute.parallel f "cpu"` (§2's variant), the lowering refuses
with

```
compute.parallel: read's buffer argument is not an input buffer of the parallel
parameter (the parameter's inputs are [[1, 0]])
```

The role table (right) and the resolution (failed) disagree while both are
correct in their own terms.

**Where.** `emit_node`'s `Read` arm (`compute.rs:4077` onward) calls
`peeled_argument` (`compute.rs:4492`) and then `parallel_buffer_pos`
(`compute.rs:4439`), whose struct branch is

```rust
let slot = params.first()?;
if let Some(roles) = &slot.roles {
    return roles.input_pos(&param_path(module, slot.pair, node)?);
}
```

`param_path` is `compute.rs:5081`.

**The measured cause.** `param_path` asks `usize_value` (`compute.rs:4917`) for
each `Index` selector's constant, and gets `None` — so it returns `None` before
it can build a path. Instrumenting the node at the failure gives:

```
node = Index whose target is also Index, index = None (usize_value)
slot.value = a node with no operation
```

i.e. **neither the wrapper's own `x(0)` selector nor the author's `k.in.a`
selector is an evaluated constant at that point.** The peel itself is fine:
`peeled_argument` stops at the author's node, because a parameter read has no
materialized array behind it, so it never reaches `param_value` and does not
destroy the chain.

**Why the tuple shape never hit this.** Its position is not a path at all — it is
the constant the *body* wrote, `cfg(1)(k)` — read off the node with the same
`usize_value`, and a literal in the body *is* an evaluated constant. So the
"selector must be a constant" assumption was the tuple shape's, and the struct
shape is the first caller to break it.

**Candidate fixes.**

- **(a) Resolve the selector through the name table.** A struct field read is a
  *name* resolved to an index; the checker already does that (see
  `a_named_struct_field_read_resolves_to_the_positional_index` in
  `crates/lichen-language/tests/pipeline.rs`), and `struct_fields_by_shape`
  (`lichen-highlevel/src/shape.rs`) reads a struct type's names. Reading the
  index from there makes `param_path` a pure *structural* walk, which is what it
  is documented to be. Cost: it needs the struct *type* at each level, so the
  walk grows a type argument.
- **(b) Force the selector's value.** Evaluate the node before reading it. Cost:
  the walk would run evaluation during a compile-time shape read, which is the
  kind of hidden work the surrounding code avoids; and it is unclear whether the
  selector *has* a value in the frozen template, which is the state
  `compile_parallel_fragment` works in.
- **(c) Have the checker emit the resolved index.** Makes the IR carry the
  constant and leaves `param_path` unchanged. Cost: it moves the problem into the
  checker for one consumer, and the tuple shape proves a constant is not
  generally required.

**Recommendation: (a).** It matches what `param_path` claims to be ("the index
*path* from the parameter to the value `node` reads"), and the name table is
where a struct's index already lives.

## 5. Blocker 2: `parallel_sig`'s extra parameter makes the operand `Parameterized`

**Symptom.** `compute.parallel_sig f "cpu" Sig` with a struct-shaped `f` fails
*before* the lowering, at the operand check in `ComputeOperator::Parallel`'s run
(`compute.rs:1283`):

```rust
ComputeOperator::Parallel => {
    if matches!(AsEnum::<LowValue>::as_enum(&operand), Some(LowValue::Parameterized)) {
        return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
    }
```

The operand here is the `[f, backend]` array `ParallelOp::build` allocates.

**What is ruled out, measured.** `compute.parallel f "cpu"` with the *same*
struct-shaped `f` passes this check and reaches the lowering. The third curried
parameter is the only difference between the two wrappers. Reverting
`parallel_sig`'s `.sig s` to `.sig (type_of f)` — i.e. making the signature
declaration identical to the plain wrapper's — **does not move the failure**, so
`.sig s` is not the cause. `f` alone evaluates concretely
(`Function: struct<.n Int, .in struct<.a …>, .out struct<.z …>> -> raw[…]`), so
the argument is not the `Parameterized` one either.

**Where.** `compute.lichen`'s `parallel_sig` (line 11) and
`ComputeOperator::Parallel`'s run (`compute.rs:1283`).

**Candidate lines of attack.** The check is a *laziness* guard — an unevaluated
program is not a mistake, which is why it stays silent rather than recording a
diagnostic — so the question is **which of the two array elements is unevaluated
and why the extra currying layer changes it**. Instrumenting the element (rather
than the whole operand) is the first step, and it is not yet done. Note that the
wrapper's `s` is *not* an operand of `$parallel` (only `f` and `b` are), so `s`
cannot be it directly; the likely mechanism is in how the apply binds the
wrapper's parameters across three currying layers.

## 6. What must not break

- **The tuple shape keeps working.** `compute.parallel`/`compute.jit` (no
  signature) are unchanged, and the existing tests exercise them heavily.
- The suites that cover this work, all green at `8ac7736`:
  `cargo test -p lichen-compute` (17), `cargo test -p lichen-kernel-ir` (14),
  `cargo test -p lichen-graph-ir` (3),
  `cargo test -p lichen-language --test compute` (51),
  `--test pipeline` (131), `--test examples`.
- The one pre-existing warning is `WasmState.at` being never read
  (`compute.rs:3104`); it is not related.

## 7. Orientation

| Item | Where |
|---|---|
| The type lambdas and the wrappers | `crates/lichen-compute/src/compute.lichen` |
| Role table | `parallel_roles`, `compute.rs:1856` |
| Parallel lowering | `compile_parallel_fragment`, `compute.rs:2208` |
| Instruction emitter (read/write arms) | `emit_node`, `compute.rs:4077` |
| Position resolution | `parallel_buffer_pos` `:4439`, `peeled_argument` `:4492`, `param_path` `:5081`, `is_param_value` `:4888`, `usize_value` `:4917` |
| Launch walk (count and buffers) | `ComputeOperator::ParLaunch`, `compute.rs:1334` |
| The `Parallel` gate and run | `ParallelOp::build` and `ComputeOperator::Parallel`, `compute.rs:1283` |
| Struct type reading | `struct_fields_by_shape` and `struct_term_parts`, `crates/lichen-highlevel/src/shape.rs` |
