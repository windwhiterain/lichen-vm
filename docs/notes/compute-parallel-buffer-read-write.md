# lichen-compute: parallel buffer map (`range` / `read` / `write`)

> Status: **current** — the parallel primitive lifted to a buffer map over a
> **fixed-shape `cfg` = `(n, (buffer…))`** (count `n` at cfg position 0, a tuple
> of input buffers at cfg position 1), with a **single-arg** index function that
> reads inputs via `compute.read` and writes one or more output buffers via
> `compute.write`.  Replaces the earlier two-level-curry `parallel`/`plrun`/
> `pget`/`pcollect` design (superseded).
> Points at: `crates/lichen-compute/src/compute.lichen` (the wrapper),
> `crates/lichen-compute/src/compute.rs` (`Parallel`/`ParLaunch`/`Range`/
> `Read`/`Write`/`BufferCollect`, `compile_parallel_fragment`,
> `parallel_output_nodes`, `run_parallel_kernel`, the parallel
> `assemble_module`), and `crates/lichen-language/tests/compute.rs`.

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
  `write(out_pos_const, idx, val)`; `n` is the output length (used by the
  runner to allocate).  Returns a `Write`.  The index function's **codomain is a
  `Write` or a tuple of `Write`s** — one per output buffer (below).

## Several output buffers

An index function's codomain decides how many output buffers one `plrun`
produces:

```lichen
f = cfg => {
  n = cfg(0)
  i = compute.range n
  a = compute.read [cfg(1)(0), i]
  (compute.write [n, i, a],        -- output 0
   compute.write [n, i, a + a])    -- output 1
}
k = compute.parallel f
outs = compute.plrun k (4, (inbuf,))   -- a tuple of two buffers
(compute.read [outs(0), 1], compute.collect outs(1))
```

- **The count is the codomain's arity**, read at *compile* time: a bare `Write`
  is one output, a tuple value is one per element
  (`parallel_output_nodes`).  It is a fact of the function, so the fragment
  carries it (`KernelFragment::outputs`) and `plrun` allocates exactly that many
  buffers — never discovering the count from which slots happened to be written.
- **Ordinals are assigned by emission order**: the `k`-th `compute.write` in the
  body is emitted with `out_pos = k`, a compile-time constant exactly as
  `read`'s `cfg_pos` is.  `run_parallel_kernel` reads the ordinal from the
  import's first parameter and writes that buffer.
- **The outputs are flat.**  A nested tuple is refused: one buffer per position.
- **One output is a bare `Buffer`** (the single-output form, unchanged); several
  are the **tuple** of them, addressed as `outs(k)` and consumed by
  `compute.read` / `compute.collect` like any other tuple.

### The every-ordinal-written invariant

**Output ordinal `k` is written on every index.**  The invariant is *structural*:
the lowered body is straight-line.  The kernel-safe subset's only conditional is
the emitter's 2-element **scalar** `select`, which selects a *value* and has no
branch to jump to — and `compute.write` is a side effect that produces no value
at all, so it cannot sit in a `select` branch.  Every emitted
`BufferWriteCall` therefore runs on every index, and each ordinal
`0 .. outputs` is always written.

Two refusals guard it, and each names its own cause:

- `CONDITIONAL_WRITE` — a `compute.write` reached inside a `select` branch
  (reached in the emitter's `Select` arm, by inspecting each branch's emitted
  instructions before they are concatenated).  It is refused because that
  ordinal would be written on one path only.  In today's language this is
  reached one layer earlier: a same-module call inside an `if` branch is not
  reduced, so the branch still holds an `Apply` and the inline-call refusal
  fires first (see `a_write_inside_a_conditional_is_refused`).
- **A codomain position that is not a `compute.write`** is refused *by position*
  ("output 1 … is not a `compute.write`"), and a write the walk reached nested
  inside a position's value — which would consume an ordinal of its own — is
  caught by the emitted-count check.  Nothing is ever defaulted to `0` and
  nothing is silently reduced to a single output.

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
  `out_pos` (a compile-time constant: the write's emission ordinal) at element
  `idx`.

cfg flattening: `cfg(0)` (`n`) is a wasm scalar param; `cfg(1)` (the buffer
tuple) is host-side — each buffer read by `read` uses its position inside the
tuple.  The index param is last.

`run_parallel_kernel` reads the count from `cfg(0)`, registers the input buffers
(by their position in `cfg(1)`), allocates one output buffer per output
(`KernelFragment::outputs`, each of length `n`), runs the kernel for each
`i ∈ [0, n)`, and returns the buffers.  `Read`/`BufferCollect` consume them (and
a single one as a bare `Buffer`).

## Type checking

- `parallel` gates the index function as `?a -> ?b` (single arg), where `?b` is
  the index function's result — a `Write` or a tuple of them.
- `plrun` gates the signature as a single-arg function and unifies the `cfg`
  against its domain.  Its **result type is a fresh cell**, deliberately: the
  arity that decides the result's shape (a `Buffer` versus a tuple of them) is
  not knowable at check time, because `NativeOp::build` runs once on the frozen
  wrapper template where `.sig` is still an unbound cell.  A tuple type is a
  value node with one element per position, so no check-time node can name a
  tuple of unknown arity; naming it `[?b, BufferKind]` instead would be
  check-time *decided*, and a decided non-positional type is exactly what the
  checker's field-read guard refuses (a **single** output's `outs(1)` would not
  check at all, so the one-output kernel could not be read positionally).
- The fresh cell costs **static precision, not safety**.  The element type is no
  longer named by the signature, so `read` binds its own element cell and
  resolves it from the value the run produces (always `Int`, since `write` pins
  its value to `Int`).  An ordinal that does not exist is still **refused, at
  check time, with a span**: the checker's evaluation pass reconciles the
  constant index against the tuple the launch produced and records an
  out-of-bounds `Index` — *"index 5 out of bounds (array length 2)"*.
- The codomain's *shape* is checked by the emitter instead, which is where the
  writes are: one `compute.write` per position, by name.
- `range`, `read`, `write`, `collect` are ordinary lichen functions over wrapped
  lichen values, so the checker never sees a raw `ComputeValue::Buffer` in a
  type position.

## Scope

- **Implemented (runtime)**: the parallel buffer map — `compute.range n`
  supplies the loop index, `compute.write [n, i, val]` writes into an output
  buffer, the kernel lowers to a wasm function with host `read`/`write`
  imports, `plrun k cfg` allocates the buffers, runs over `[0, cfg(0))`, and
  `compute.read`/`compute.collect` consume them.  A dependent index function
  (`a = compute.read [cfg(1)(0), i]` then `a + a`) **typechecks**: a lazy
  `Index` cfg slot-read type unified against a concrete type is **unified by
  joining the classes** (the read *is* the type it reads) rather than
  hard-erroring; the mismatch check is deferred to when the computation runs
  (`force_pending` reconciles the computed value against the value its class
  committed, recording a unify error on a concrete conflict).  The read's `: Int`
  annotation pins the element, and an argument unify no longer contaminates the
  result type.
- **Implemented**: **multi-output** — a tuple codomain of `Write`s, ordinals by
  emission order, one buffer per output (see above).
- Read-from-write (reading an output buffer while it is written) is future work.
- Arbitrary element indices are allowed (a gather/scatter within `[0, n)`).
- The parallel run is sequential (the data-parallelism is logical); a worker
  pool is future work.
