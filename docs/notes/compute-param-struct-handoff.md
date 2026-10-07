# Handoff: running a parallel kernel whose parameter is a struct

> Status: **resolved — both blockers fixed and verified.** Blocker 1 (the
> named-read path) and blocker 2 (`parallel_sig`'s extra currying layer) are
> fixed; §5 records what blocker 2 turned out to be. A fresh session can pick
> up any remaining work from §7 without re-deriving anything above it.
> Companion: [compute-kernel-struct](compute-kernel-struct.md) (the kernel
> struct itself), [applied-struct-nominal-id](applied-struct-nominal-id.md) (the
> identity property the design rests on), [floating-point](floating-point.md)
> §4.2/§4.4 (why the two classes cannot meet).
> Worktree: `.worktrees/kernel-param-struct`, branch
> `feature/kernel-param-struct`.

## 1. What is being built, and why

A parallel kernel's parameter is being moved from the positional
`cfg = (n, (buffers…))` to a named struct, so that

- a read names its buffer (`compute.read ((compute.Read _)(.from k.in.x, .at i))`) instead of counting a
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
producer kernel first, so the consumer has a real input buffer.

**The `parallel_sig` wrapper this reproduction originally used has since been
deleted** — the signature is the function's own `f: I -> O` annotation, read
back through the kernel struct's `.I`/`.O` — so the reproduction is the
wrapper-free form:

```lichen
---
  order = "99"
  compute = import "compute.lichen"
  output = "22"
---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
g = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
kg = compute.parallel g "cpu"
inbuf = (compute.plrun kg ((compute.A In)(.n 3, .I In(.a 0))) : Out)
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f = (k : Par2) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value v * 2))
}
k = compute.parallel f "cpu"
out = (compute.plrun k ((compute.A In2)(.n 3, .I In2(.b inbuf.z))) : Out2)
compute.read ((compute.Read _)(.from out.w, .at 1))
```

`inbuf` is `[10, 11, 12]`, so the answer is `22`.

Run it with:

```bash
cargo test -q -p lichen-language --test examples
```

**Both blockers are fixed**: this file now runs and prints `22: ?a` — the only
unresolved cell is the element class of a value read back from a buffer, the
documented limit of `plrun`'s result type (`compute-kernel-struct.md`
§"Runtime / codegen"), not a defect. The binding is annotated with its codomain
type (`: Out`) once, which is `compute-buffer-wrapper.md`'s migration recipe,
and its fields are then read by name (`inbuf.z`).

## 3. What already works, measured

- **The type lambdas** (`crates/lichen-compute/src/compute.lichen`):

  ```
  KT = _x => struct<.I _, .O _>                      # the input/output pair
  A  = I  => struct<.n Int, .I I>                    # the JIT'd input
  P  = T: KT _ => struct<.n Int, .in T.I, .out T.O>  # the author's parameter
  S  = T: KT _ => A T.I -> T.O                       # the JIT'd signature
  ```

  Two spelling facts, both load-bearing: the argument must be an *instantiated*
  pair (`P (KT _)(.I In, .O Out)`; a pre-bound pair answers "this value is not a
  container"), and applying a type lambda before instantiating needs parentheses
  (`(A In)(.n 3, …)`, not `A In(.n 3, …)`).

- **The host's argument type is the kernel's own domain.** Measured (through the
  then-shipped `Sig` lambda, since deleted): `(f : Sig) => f ((A In)(.n 3, .I
  In(.a inbuf)))` checks, i.e. the `A In` the host writes at the call site and
  the domain `S` builds — evaluated by the checker once the role wiring resolves
  the cells — are *one* nominal type. That is what
  `applied-struct-nominal-id`'s fix bought, and it is why the signature can ride
  in the kernel struct's `.I`/`.O` rather than being built by a gate.

- **The role table** (`parallel_roles`, `compute.rs:1856`) decodes the
  parameter's type: scalars, `.in` paths and `.out` paths, in declaration order.
  For the probe, `inputs == [[1, 0]]` — `.in` is field 1 and `.a` is its field 0
  — which is exactly the path `k.in.a` should resolve to.

- **The wasm signature and the host-side decode.** The signature is the scalar
  leaves followed by the index, and the run reads its leaves and inputs at the
  role paths the fragment carries: the extent is the `.n` leaf and the inputs live
  under `.I` in the JIT'd input `compute.A` builds — positions 0 and 1 of the
  host's argument struct.

- **The `Parallel`/`ParLaunch` gates need no signature argument.** `f: I -> O`
  states the signature, the kernel struct carries `.I`/`.O`, and `plrun`'s
  `a: k.I` / `r: k.O` gate the argument and type the result.

- **The temporary blanket refusal is gone.** `PARALLEL_PARAM_LAUNCH` no longer
  exists; a struct-shaped kernel reaches the lowering and is refused by the read
  arm's own message. (It was there because a *silent* `parameterized` is the
  worst possible answer; the read arm's message is a cause, so the blanket one is
  no longer needed.)

- **A named read resolves to the parameter type's field order** — the fix in §4.
  `k.n` → `[0]`, `k.in.a` → `[1, 0]`, `k.out.z` → `[2, 0]`, matched against the
  role table's `inputs == [[1, 0]]` and the scalars' `scalar_offset`.

## 4. Blocker 1: `param_path` cannot resolve a named-field path — **FIXED**

**Symptom (then).** With `compute.parallel f "cpu"` (§2's reproduction), the lowering
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
  kernel body's parameter type is an undecided cell, so the checker has no index to
  emit; it is the apply that makes it concrete, which is why the resolution
  belongs to the lowering.

## 5. Blocker 2, resolved: nested static closures lost their captures' bindings

**The wrapper this blocker was diagnosed through has since been deleted.**
`parallel_sig` (and its `Sig` argument) is gone: the signature is the function's
own `f: I -> O` annotation, read back through the kernel struct's `.I`/`.O`. The
two static-module defects below are still what the shipped `compute.parallel f
"cpu"` path needed, so the record stands; read `parallel_sig` as the
then-shipped spelling, and `.sig s`/`.sig (type_of f)` as the single field that
`.I`/`.O` later split into.

**Symptom (as it was).** `compute.parallel_sig f "cpu" Sig` failed *before* the
lowering, at the operand check in `ComputeOperator::Parallel`'s run: the
`[f, backend]` array stayed `Parameterized`. `compute.parallel f "cpu"` with the
*same* `f` passed; a tuple-shaped `f` through `parallel_sig` failed the same
way — so the extra currying layer was the cause, not the parameter shape, and
not `.sig s` (reverting it to `.sig (type_of f)` did not move the failure).

**Diagnosis, measured.** Instrumenting the operand element by element (a
minimal tuple-shaped `parallel_sig` probe, plus the `$parallel` op node's clone
chain) showed the undecided element was the **backend**: the innermost closure's
body read the *first* apply's generation of the backend cell — a fresh clone no
unify ever bound — while the second apply's unify had bound a *different*
clone. Two defects in the static-module apply
(`crates/lichen-lowlevel/src/static_module/apply.rs`), both about **nested**
closures:

1. **`static_function_captures` was not transitive.** The walk that decides
   whether a same-module static closure must be re-homed as a dynamic one
   followed operation operands and array items but stopped at function values,
   so a capture one closure layer down (`f`, read inside `s => …`'s body) was
   invisible: the middle closure (`b => s => …`) was **baked** as a frozen
   static ref instead of re-homed. Its later apply then materialized the body
   fresh from the static template, where `f`'s cell is undecided and nothing can
   bind it. Fixed by descending into same-module static function values' entry
   points — a nested closure's captures are the enclosing closure's captures.
2. **`static_clone_function` hung every re-home under `parent: None`.** Sound
   while re-homes never nested; with (1) fixed they do (the middle re-home
   walks the inner one during its own scope walk). `parent: None` made the
   inner closure's nodes invisible to the middle closure's later **dynamic**
   apply (the membership chain reached no dynamic ancestor), so that apply
   handed the inner closure on **uncloned** — its body kept reading the
   previous generation of the backend cell, which the apply's own unify bound
   only a clone of. Fixed by threading a `branch_top` through
   `StaticApplyCtx` — the static mirror of the dynamic path's
   `ApplyCtx::branch_top` — so a nested re-home's `parent` is the enclosing
   fresh closure and every later apply re-instantiates it per call.

**Verified.** §2's probe runs and prints `22: ?a`; the minimal tuple-shaped
`parallel_sig` variant ran and printed `[1, 2, 3, 4]` (that wrapper is since
deleted, and the shape it covered is now the tuple parameter the decision
retires). Both are committed as regression
tests: `a_tuple_kernel_runs_through_the_signature_carrying_wrapper` and
`a_struct_parameter_kernel_runs_through_the_signature_carrying_wrapper`
(`crates/lichen-language/tests/compute.rs`). `cargo test -p lichen-lowlevel`,
`-p lichen-compute`, `-p lichen-kernel-ir`, `-p lichen-graph-ir`, and
`-p lichen-language --test compute --test pipeline --test examples` all pass.

## 6. What must not break

- **The tuple parameter shape still runs, but it is retired.**
  `compute.parallel`/`compute.jit` (no signature) still accept a
  `cfg = (n, (buffers…))` body and the existing tests exercise it; the shipped
  shape is the named struct (`compute.P (compute.KT _)(.I In, .O Out)`), and the
  tuple-form call sites are the migration's phase 3
  (`compute-buffer-wrapper.md`).
- The suites that covered this work, all green after §5's fix:
  `cargo test -p lichen-lowlevel`, `cargo test -p lichen-compute`,
  `cargo test -p lichen-kernel-ir`, `cargo test -p lichen-graph-ir`,
  `cargo test -p lichen-language --test compute`, `--test pipeline`,
  `--test examples`. The later struct-parameter migration moved that measurement:
  `lichen-language --test compute` is at 35 passed / 21 failed / 5 ignored, the 21
  being the retired tuple-form call sites (`compute-buffer-wrapper.md`).
- The one pre-existing warning is `WasmState.at` being never read
  (`compute.rs:3104`); it is not related.

## 7. Orientation

The runtime scalars this parameter shape exists for are their own work item:
their state, the two measured blockers, and the acceptance probe are in
[compute-runtime-scalars](compute-runtime-scalars.md).

| Item | Where |
|---|---|
|---|---|
| The type lambdas and the wrappers | `crates/lichen-compute/src/compute.lichen` |
| Role table | `parallel_roles`, `compute.rs:1856` |
| Parallel lowering | `compile_parallel_fragment`, `compute.rs:2208` |
| Instruction emitter (read/write arms) | `emit_node`, `compute.rs:4077` |
| Position resolution | `parallel_buffer_pos`, `peeled_argument`, `param_path` (`IndexStep`/`resolve_steps`), `is_param_value`, `usize_value`, `struct_type_names`, `param_value_shape` |
| Launch walk (count and buffers) | `ComputeOperator::ParLaunch`, `compute.rs:1334` |
| The `Parallel` gate and run | `ParallelOp::build` and `ComputeOperator::Parallel`, `compute.rs:1283` |
| Struct type reading | `struct_term_parts`, `struct_names_any`, `field_names`/`field_list`/`field_type`/`TypeRef` (`crates/lichen-highlevel/src/shape.rs`), `struct_fields_of_slot` (`compute.rs`) |
