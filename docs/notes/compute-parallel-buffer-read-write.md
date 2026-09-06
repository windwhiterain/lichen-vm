# lichen-compute: parallel buffer map (v1, `range` / `read` / `write`)

> Status: **current** — the parallel primitive lifted to a buffer map over a
> **fixed-shape `cfg` = `(n, (buffer…))`** (count `n` at cfg position 0, a tuple
> of input buffers at cfg position 1), with a **single-arg** index function that
> reads inputs via `compute.read` and writes an output buffer via
> `compute.write`.  Replaces the earlier two-level-curry `parallel`/`plrun`/
> `pget`/`pcollect` design (superseded).
> Points at: `crates/lichen-compute/src/compute.lichen` (the wrapper),
> `crates/lichen-compute/src/compute.rs` (`Parallel`/`ParLaunch`/`Range`/
> `Read`/`Write`/`BufferCollect`, `compile_parallel_fragment`,
> `run_parallel_kernel`, the parallel `assemble_module`), and
> `crates/lichen-language/tests/compute.rs`.

## Model

```lichen
f = cfg => {
  n = cfg(0)                       -- the count, fixed at cfg position 0
  i = compute.range n              -- current index, i ∈ [0, n)
  a = compute.read [cfg(1)(0), i]  -- input buffer 0 (the tuple at cfg(1))
  compute.write [n, i, a + 1]      -- write output buffer (length n) at i
}
k = compute.parallel f             -- single-arg `cfg -> ..` index function
out = compute.plrun k (4, (inbuf,))-- cfg = (n, (buffer…)); runs over [0,4)
compute.read [out, 2]
```

- **`cfg = (n, (buffer…))`** — `n` is always `cfg(0)`; the input buffers are a
  **tuple** at `cfg(1)`, read as `cfg(1)(k)` for buffer `k`.
- **`compute.range n`** — a `Int -> Int` op; in the kernel it lowers to the
  loop-index param, and its argument `n` (fixed at `cfg(0)`) is the count.
- **`compute.read [buf, idx]`** — inside the kernel → host import
  `read(cfg_pos_const, idx)`; after `plrun` → read the output buffer element.
- **`compute.write [n, idx, val]`** — inside the kernel → host import
  `write(0, idx, val)`; `n` is the output length (used by the runner to
  allocate).  Returns a `Write`.  v1 has a **single** output buffer.

## Wrapper functions (lichen)

Every native op is hidden behind a lichen function, so the checker sees only
ordinary lichen types (no `__index__` magic):

```lichen
{
  range   = x => $range(x)               -- Int -> Int (the loop index)
  read    = x => $read(x(0), x(1))       -- [Buffer, USize] -> ?elem
  write   = x => $write(x(0), x(1), x(2))-- [USize, USize, ?b] -> Write
  collect = b => $collect(b)             -- Buffer -> [?b]
  -- parallel / plrun wrap $parallel / $plrun as before
}
```

`range`/`read`/`write` take a **single array/tuple argument** and destructure it
with positional slot reads `x(k)` — the parser's `f [a, b]` is one array
argument, not a curried two-arg call, so the wrapper destructures.

## Count is fixed at `cfg(0)`

`plrun k cfg` reads the count from **`cfg(0)`** (always `n`).  No hardcoded
search — the shape is fixed and the index function reads `n = cfg(0)`.

## Runtime (wasm kernel + host buffer imports)

The parallel kernel is a wasm function `(cfg_scalars…, index) -> ()`:
- import `read(cfg_pos: i64, idx: i64) -> i64` — read input buffer `cfg_pos`
  (a compile-time constant from `cfg(1)(k)`) at element `idx`.
- import `write(out_pos: i64, idx: i64, val: i64)` — write into output buffer
  `out_pos` (v1: `0`) at element `idx`.

cfg flattening: `cfg(0)` (`n`) is a wasm scalar param; `cfg(1)` (the buffer
tuple) is host-side — each buffer read by `read` uses its position inside the
tuple.  The index param is last.

`run_parallel_kernel` reads the count from `cfg(0)`, registers the input buffers
(by their position in `cfg(1)`), allocates the output buffer (length `n`), runs
the kernel for each `i ∈ [0, n)`, and returns the output `Buffer`.
`BufferGet`/`BufferCollect` consume it.

## Type checking

- `parallel` gates the index function as `?a -> ?b` (single arg), where `?a`
  is the generic cfg tuple and `?b` is the index function's result (a `Write`,
  v1 single output).
- `range`, `read`, `write`, `collect` are ordinary lichen functions over wrapped
  lichen values, so the checker never sees a raw `ComputeValue::Buffer` in a
  type position.

## Scope

- **Implemented (runtime)**: the parallel buffer map — `compute.range n`
  supplies the loop index, `compute.write [n, i, val]` writes into the output
  buffer, the kernel lowers to a wasm function with host `read`/`write`
  imports, `plrun k cfg` allocates the buffer, runs over `[0, cfg(0))`, and
  `compute.read`/`compute.collect` consume it.  A dependent index function
  (`a = compute.read [cfg(1)(0), i]` then `a + a`) now **typechecks**: a lazy
  `Index` cfg slot-read type unified against a concrete type is **unified by
  joining the classes** (the read *is* the type it reads) rather than
  hard-erroring; the mismatch check is deferred to when the computation runs
  (`force_pending` reconciles the computed value against the value its class
  committed, recording a unify error on a concrete conflict).  The read's `: Int`
  annotation pins the element, and an argument unify no longer contaminates the
  result type.
- v1 is **single output buffer** (one `compute.write` per index function).
- Multi-output (several `compute.write`, tuple-routed to several buffers) is
  future work.
- Read-from-write (reading an output buffer while it is written) is future work.
- Arbitrary element indices are allowed (a gather/scatter within `[0, n)`).
- The parallel run is sequential (the data-parallelism is logical); a worker
  pool is future work.
