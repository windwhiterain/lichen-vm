# lichen-compute: parallel primitives (`parallel` / `plrun` / buffer read & collect)

> Status: current — implemented as an extension of the `lichen-compute` native
> plugin (see [lichen-compute.md](lichen-compute.md)).  Companion to
> [compute-kernel-struct.md](compute-kernel-struct.md) (a separate, not-yet-merged
> change to split call/launch/run for *scalar* kernels).
> Points at: `crates/lichen-compute/src/compute.lichen` (the wrapper),
> `crates/lichen-compute/src/compute.rs` (`ComputeValue`/`ComputeOperator`,
> `compile_parallel_fragment`, `run_parallel_kernel`, the four new native ops),
> `crates/lichen-language/src/program.rs` / `render.rs` (the `ParKernel`/`Buffer`
> render hooks), and `crates/lichen-language/tests/compute.rs`.

`parallel` is the **data-parallel lift** of a curried index function.  Given
`f : ?a -> USize -> ?b` (a config, then an index), `compute.parallel f` compiles a
flattened kernel `(?a, USize) -> ?b` and exposes a launcher that runs it over the
index range `[0, n)`, collecting the `n` results into a host-owned **buffer** that
you read element-wise or collect into a lichen array.

```
@{ compute = import "compute.lichen" @}
f = cfg => i => cfg + i              -- : Int -> Int -> Int  (?a -> USize -> ?b)
k = compute.parallel f                -- a ParKernel
p = compute.plrun k (10, 4)           -- run over i in [0,4) with cfg=10  → Buffer
compute.pget p 2                      -- read element 2 → 12 : Int
compute.pcollect p                    -- collect → [10, 11, 12, 13]
```

## 1. Vocabulary additions

- **`ComputeValue`** gains `ParKernel(KernelId)`, `TypeParKernel`,
  `Buffer(BufferId)`, `TypeBuffer` — all `Copy` host-owned scalars (ids into the
  process registries), so no arena/GC change, exactly like `Kernel(KernelId)`.
- **`ComputeOperator`** gains `Parallel`, `ParLaunch`, `BufferGet`,
  `BufferCollect`.

```
LangValue    = LowValue + TypeValue + ComputeValue
LangOperator = LowOperator + TypeOperator + GcdOp + ComputeOperator
```

A `ParKernel` reuses the kernel registry (`kernels()`/`NEXT_KERNEL_ID`); a
`Buffer` lives in a new process-global `buffers()`/`NEXT_BUFFER_ID` registry.

## 2. The wrappers (`compute.lichen`)

```
{
  jit      = f => $jit(f)
  launch   = k => a => $launch(k, a)
  parallel = f => $parallel(f)          -- (?a -> USize -> ?b) -> ParKernel
  plrun    = k => a => $plrun(k, a)     -- ParKernel -> (?a, USize) -> Buffer
  pget     = b => i => $pget(b, i)      -- Buffer -> USize -> ?b
  pcollect = b => $pcollect(b)          -- Buffer -> [?b]
}
```

`plrun` is deliberately **2-arg** (kernel, then the `(config, count)` tuple), not
3-arg-curried.  A 3-level curry (`k => cfg => n => …`) nests two closure clones,
and the deep pass's `evaluated_deep.parameterized` flag then sticks to the op's
operand array and makes the launcher read a lazily-`Parameterized` operand; the
2-arg shape mirrors `launch` and keeps the operand concrete.

## 3. Type encodings

- **ParKernel type** = `[sig, [TypeParKernel, Type]]`, `sig = [?a, Int -> ?b]`
  (the *lifted* `?a -> USize -> ?b`, re-headed by `TypeParKernel`).  Keeping the
  codomain a *full* function type is what makes the renderer spell the whole
  thing `?a -> Int -> ?b` (`is_function_kind` on `TypeParKernel`).
- **Buffer type** = `[?b, [TypeBuffer, Type]]` — the element type `?b` is read by
  `pget`/`pcollect`.
- `parallel`'s **curried-arrow gate**: unify `f` with `?a -> r0`, then `r0` with
  `Int -> ?b`.  The config `?a` and element `?b` bind to the cells `plrun` reads,
  and the index cell is pinned to `Int`.
- `plrun`'s gate: unify `k` with the par-kernel type (extracting `?a`, `?b`), the
  tuple argument with `(?a, Int)`, and return a `[?b, [TypeBuffer, Type]]`.
- `pget`/`pcollect` gates: unify the buffer with `[?b, [TypeBuffer, Type]]`; read
  the element type `?b`.

## 4. Codegen: flatten the curried kernel

`compile_parallel_fragment` walks a curried function whose outer body is an index
lambda.  It resolves the outer config parameter and the inner index parameter and
traces the inner body with **two parameter slots**

```
param_shape  = Tuple([cfg_shape, USize])
slot 0: cfg  (base 0)
slot 1: i    (base flat_arity(cfg_shape))
```

The emitter was generalized from a single `(param_pair, param_value)` pair to a
`&[ParamSlot]` list: a parameter read resolves to `slot.base + flatten_offset(shape,
path)`, and the whole-parameter equality check scans all slots.  The scalar `jit`
path passes a one-element list, so it is unchanged.

## 5. Runtime: run over the range, collect into a buffer

`ParLaunch::run` reads the kernel id and the `(config, count)` tuple, flattens the
config to the wasm argument vector, and calls `run_parallel_kernel(id, cfg, n)`,
which deterministically computes `f cfg i` for `i in [0, n)` and returns a
`Vec<i64>`.  A **warm call** (`i = 0`) validates the fragment/assembly first; then
indices are distributed across a `std::thread::scope` pool (bounded by
`available_parallelism`), each worker running its own `run_kernel`, and chunks are
reassembled in index order.  The results are stored under a fresh `BufferId` and a
`Buffer(id)` value is returned.

- `BufferGet::run` returns `USize(results[index])`.
- `BufferCollect::run` materialises each element as a fresh scalar node and builds
  a real lichen array value (`alloc_array`), typed `Int<len>` with a fresh length
  cell.

## 6. Scope & limits

- `?a` may be a scalar or a tuple of scalars (a tuple config flattens like a
  `Launch` argument); `?b` is a single scalar `Int` (the kernel-safe subset).
- A parallel kernel body is the same scalar subset as `jit` (constants, `+ - <= ==`,
  parameter reads, 2-element conditionals, cross-kernel `CallKernel`).
- No recursion / higher-order kernels inside the compiled region, mirroring `jit`.
- `pcollect`'s array length cell stays `?` until observed (a runtime count has no
  static length); reading elements (`pget`) is the fully-typed path.

## 7. Tests

`tests/compute.rs` covers the scalar config (`plrun`/`pget`), `pcollect` to an
array, a **tuple config** (`(Int, Int) -> Int` domain), and the `ParKernel`
value/type render (`ParKernel: Int -> Int -> Int`).  The full file (25 tests)
passes alongside the original `jit`/`launch` suite.
