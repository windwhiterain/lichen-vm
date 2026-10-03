# Handoff: running a parallel kernel whose parameter is a struct

> Status: **open — one blocker left.** Blocker 1 (the named-read path) is
> **fixed** and verified; blocker 2 (`parallel_sig`'s extra currying layer) is
> still open, and a fresh session can start at §5 without re-deriving anything
> above it.
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
`parameterized: ?a`, and the module carries the refusal named in §5.

**Blocker 1 is no longer reachable through this file**, because `parallel_sig`
fails first (§5). To exercise the fixed path on its own, drop the `Sig`
declaration and use the plain wrapper:

```lichen
k = compute.parallel f "cpu"
out = compute.plrun k ((compute.A In)(.n 3, .I In(.a inbuf)))
```

That variant now **runs** and prints `22: ?a`: the reads resolve, the kernel
compiles, and the only unresolved cell is the element class of a value read back
from a buffer — the documented limit of `plrun`'s result type
(`compute-kernel-struct.md` §"Runtime / codegen"), not a defect.

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

- **A named read resolves to the parameter type's field order** — the fix in §4.
  `k.n` → `[0]`, `k.in.a` → `[1, 0]`, `k.out.z` → `[2, 0]`, matched against the
  role table's `inputs == [[1, 0]]` and the scalars' `scalar_offset`.

## 4. Blocker 1: `param_path` cannot resolve a named-field path — **FIXED**

**Symptom (then).** With `compute.parallel f "cpu"` (§2's variant), the lowering
refused with

```
compute.parallel: read's buffer argument is not an input buffer of the parallel
parameter (the parameter's inputs are [[1, 0]])
```

The role table (right) and the resolution (failed) disagreed while both were
correct in their own terms.

**Where.** `emit_node`'s `Read` arm calls `peeled_argument` and then
`parallel_buffer_pos`, whose struct branch asked `param_path` for the node's
index path. `param_path` asked `usize_value` for each `Index` selector's
constant and got `None`, so it returned `None` before it could build a path.

**The real cause — and it was not the selector form.** A named read's selector is
a lazy `TableGet(name-table, "name")` (`Checker::check_named_field`), which the
old `param_path` could not read. But folding that lookup is not enough, because
**the type a named read resolves against is not reachable one level at a time**.
`Index(target, selector)` states only that `selector` selects a field of `target`;
the old walk threaded the *child's* type down, so `k.in.a`'s `a` was looked up in
the parameter struct's own field list (`[n, in, out]`) and refused. Measured: the
selector's table is a chain rooted at the **parameter's** type term, so
`k.in.a`'s named step resolves against the *parameter's* names — the `.in` step —
and only then does work descend into `In`.

**The fix (`compute.rs`).** `param_path` is now a two-pass walk:

1. **Collect.** Walk the value chain from the read inward, recording each level's
   selector as an `IndexStep` — a `Position` when it is a constant (`a(0)`), a
   `Named` name when it is a `TableGet` (a name is a compile-time string even
   when its index is not). A chain whose outermost `Index` reads the parameter
   pair's value slot is a whole-parameter read: the empty path.
2. **Resolve.** Walk the steps outermost-first against the parameter's type,
   carrying two parallel lists per level: the value's field *types* (`[shape,
   kind]`'s shape, or a plain field array) and the same value's field *names*
   (`struct_type_names`, read from the type term's name table). A named step is
   its position in the names list; a positional step is taken as it stands.

The path is therefore the parameter **type's** field order, never how the body
spelled the read — `k.n` → `[0]`, `k.in.a` → `[1, 0]`, `k.out.z` → `[2, 0]` —
which is exactly what the role table (`[[1, 0]]` for the probe's `.in`) holds.

**What it refuses, and why that matters.** A named read whose field is in no name
table is an error by name, not a silent `None`: the index of a named field read
must be a compile-time constant, and a kernel is compiled from a concrete
instantiation, so an undetermined index is a compile error rather than something
to defer.

**Verified.** The plain-wrapper variant of §2's probe **runs** and prints
`22: ?a`. `cargo test -p lichen-compute` (17), `-p lichen-kernel-ir` (14),
`-p lichen-graph-ir` (3), `-p lichen-language --test compute` (51),
`--test pipeline` (131), `--test examples` all pass.

**The candidates below are kept as the record of what was considered; (a) was
implemented, in the two-pass form described above.**

- **(a) Resolve the selector through the name table.** *Chosen.* A struct field
  read is a *name* resolved to an index; the checker already does that (see
  `a_named_struct_field_read_resolves_to_the_positional_index` in
  `crates/lichen-language/tests/pipeline.rs`), and `struct_type_names` reads a
  struct type's names. The cost the original note predicted was real — the walk
  grew a type argument — but threading the *container* per level was the wrong
  shape for it; the two passes above are what it needs.
- **(b) Force the selector's value.** Rejected: evaluation during a compile-time
  shape read, and measurement showed the selector cannot be folded this way —
  forcing the parameter's type term does **not** fold the `TableGet`, because the
  name table is not value-reachable.
- **(c) Have the checker emit the resolved index.** Rejected: at check time the
  kernel body's parameter type is an unbound cell, so the checker has no index to
  emit; it is the apply that makes it concrete, which is why the resolution
  belongs to the lowering.

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
struct-shaped `f` passes this check and reaches the lowering — and, now that §4
is fixed, **runs**. The third curried parameter is the only difference between
the two wrappers. Reverting `parallel_sig`'s `.sig s` to `.sig (type_of f)` —
i.e. making the signature declaration identical to the plain wrapper's — **does
not move the failure**, so `.sig s` is not the cause. `f` alone evaluates
concretely (`Function: struct<.n Int, .in struct<.a …>, .out struct<.z …>> ->
raw[…]`), so the argument is not the `Parameterized` one either.

**The wrapper is not alone in this.** A *tuple*-shaped `f` through
`compute.parallel_sig` fails the same way, so the extra currying layer is the
cause rather than the parameter shape — which is what makes this a wrapper
question and not a struct question.

**Where.** `compute.lichen`'s `parallel_sig` (line 11) and
`ComputeOperator::Parallel`'s run (`compute.rs:1283`).

**Candidate lines of attack.** The check is a *laziness* guard — an unevaluated
program is not a mistake, which is why it stays silent rather than recording a
diagnostic — so the question is **which of the two array elements is unevaluated
and why the extra currying layer changes it**. Instrumenting the element (rather
than the whole operand) is the first step, and it is not yet done. Note that the
wrapper's `s` is *not* an operand of `$parallel` (only `f` and `b` are), so `s`
cannot be it directly; the likely mechanism is in how the apply binds the
wrapper's parameters across three currying layers. Instrument at
`ComputeOperator::Parallel`'s `Parameterized` branch: print
`module.node_value` of the operand array's two elements and
`module.node_evaluated_deep` of the operand node, then compare the two wrappers.

## 6. What must not break

- **The tuple shape keeps working.** `compute.parallel`/`compute.jit` (no
  signature) are unchanged, and the existing tests exercise them heavily.
- The suites that cover this work, all green after §4's fix:
  `cargo test -p lichen-compute` (17), `cargo test -p lichen-kernel-ir` (14),
  `cargo test -p lichen-graph-ir` (3),
  `cargo test -p lichen-language --test compute` (51),
  `--test pipeline` (131), `--test examples` (the scratch probe is not committed,
  so this suite passes; with the probe present, only its §5 failure shows).
- The one pre-existing warning is `WasmState.at` being never read
  (`compute.rs:3104`); it is not related.

## 7. Orientation

| Item | Where |
|---|---|
| The type lambdas and the wrappers | `crates/lichen-compute/src/compute.lichen` |
| Role table | `parallel_roles`, `compute.rs:1856` |
| Parallel lowering | `compile_parallel_fragment`, `compute.rs:2208` |
| Instruction emitter (read/write arms) | `emit_node`, `compute.rs:4077` |
| Position resolution | `parallel_buffer_pos`, `peeled_argument`, `param_path` (`IndexStep`/`resolve_steps`), `is_param_value`, `usize_value`, `struct_type_names`, `param_value_shape` |
| Launch walk (count and buffers) | `ComputeOperator::ParLaunch`, `compute.rs:1334` |
| The `Parallel` gate and run | `ParallelOp::build` and `ComputeOperator::Parallel`, `compute.rs:1283` |
| Struct type reading | `struct_term_parts`, `struct_names_any`, `struct_fields_by_shape`, `crates/lichen-highlevel/src/shape.rs` |
