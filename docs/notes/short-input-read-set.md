# A read-set declaration for a short host input

> Status: **proposal.** The device refuses a host input shorter than the run's count,
> and that refusal is deliberate; what is missing is the spelling
> [device-leg-of-the-ladder](device-leg-of-the-ladder.md) §1 names — "a way to say
> *this input is short and the body only reads its first `dim`*". This note fixes the
> shape, the layer, and the acceptance, and changes no code.
>
> Points at: [lichen-compute-gpu](lichen-compute-gpu.md) (the rule and why it exists),
> [device-operator-sweep](device-operator-sweep.md) §8 (the padded measurement that
> proves the operator is otherwise correct), [compute-buffer-wrapper](compute-buffer-wrapper.md)
> (the parallel ABI), [code-audit](code-audit.md) `wontfix:D17` (why a narrower launch
> is not the answer), [device-leg-of-the-ladder](device-leg-of-the-ladder.md) §5 (the
> same finding, measured).

## 1. The refusal, and it is four operators

`InputShorterThanCount` (`crates/lichen-compute-gpu/src/dispatch.rs:978`) refuses a
host input whose length is below the run's count. Measured at `dev@213de16d` by
`crates/lichen-compute/examples/operators_on_device.rs`:

| program | refusal |
|---|---|
| `gemv.lichen` | *input buffer 1 holds 3 element(s) but the run covers 4 index/indices* |
| `kvcache.lichen` | *input buffer 2 holds 2 element(s) but the run covers 8* |
| `decode.lichen` | the append's same 2-of-8 sentence |
| `decode_loop.lichen` | the same sentence |

**And padding proves the operator is right.** §8 of the sweep padded gemv's vector
from three elements to the count and the same kernel ran on the RTX 3060 answering
`[14.0, 32.0, 50.0, 68.0]`, element for element what `cpu` answers — and the append's
padded row is §2 of the ladder note's measurement. So this is a **spelling** gap, not
arithmetic, and not a layout.

## 2. Why relaxing the check is not the fix

The upload is already each input's **own length** (`08c15c45`), and the allocation is
that length rounded to a workgroup, with the padding past the length cleared on the
device. So a read past `len` answers `0`, and the refusal is what keeps that `0` from
being the answer to an input the host could not see the read set of.

Two readings make the point in opposite directions:

- **The host still cannot see the body's read set**, so no test it can run decides
  whether a lane reaches past `len`.
- **`cpu` already zero-fills the same tail** (`compute.rs:6234`). So dropping the
  check would not make the backends disagree — it would make them agree *silently*,
  on the same `0`, for every program, including ones that never promised anything.

That is why the answer is a **declaration** rather than a relaxation: a program that
promises its read set gets the run; a program that does not keeps the refusal.

## 3. The declaration: `compute.short buffer length`

```
short = (b : (compute.Buf T)) => n: Int => ($short(b, n) : (compute.Buf T))
```

A buffer value carrying a **declared read bound** `n`: "this buffer is read only at
indices in `[0, n)`". It is not a view — the payload, the class and the length are the
same buffer's — and it is not a truncation: the upload still stages `len` elements.

At the one check that exists:

- an input **without** a bound keeps today's test (`len >= count`) and today's answer;
- an input **with** a bound is accepted iff `n <= len`, and the `len >= count` test
  does not apply to it — because a read set is a property of the **body**, not of the
  launch: gemv reads `x` in `[0, kdim)` while its count is `M`, and `kdim < M` is the
  decode shape;
- a bound **longer than the data** (`n > len`) is refused by name: a promise longer
  than the buffer is false, and it is the one bound the host can check.

**The promise is trusted, exactly like `@shallow` and `@loop`.** Nothing bounds-tests a
lane, and the tail-lane obligation does not change: a body that lies about its read set
still reads real data or the padded `0`. `compute.short` on a value that is not a host
buffer is refused by name — the resident path has no length to check, so a declaration
there would be inert, and an inert promise teaches a false belief.

## 4. Why the three alternatives are worse

- **A `Reads T = struct{.buf (Buf T), .len Int}` in the parameter type.** The bound
  would be walked as an ABI **scalar leaf**, shifting every runtime scalar after the
  extent, and the body would have to name `.buf` at every read. It also changes every
  existing kernel that names such a field. The wrapper leaves every program and every
  body unchanged.
- **A static mark, `x ~ "reads:3"`.** The bound is usually a runtime scalar — gemv's
  is `kdim`, which is a launch argument — and a mark's payload is a string literal.
- **Launching over what the buffer holds (`.n` lowered to the short input's length).**
  Measured, and it is a silent wrong answer rather than a cheaper program: four of the
  five cache slots are never written, and the two backends disagree on what an
  unwritten slot reads ([device-leg](device-leg-of-the-ladder.md) §5, `wontfix:D17`).

## 5. What it touches

| layer | change |
|---|---|
| `lichen-kernel-ir` | the host input carries the declared bound (`BatchInput::Host { data, read }`); `None` is today's behaviour, so every existing caller is unchanged |
| `lichen-compute-gpu` | `staged_bytes` applies §3's rule; the upload and the padding are untouched |
| `lichen-compute` | one native leaf for `compute.short` and the wrapper in `compute.lichen`; `plrun` passes each input's bound into the launch it builds |
| `lichen-graph-ir` | nothing: a graph node declares no bound, so it constructs `None` |
| the example suite | **nothing** — no example and no test is edited in stage 1 or 2 |

**Staging.** (1) the kernel-ir field and the backend check, unobservable because no
caller sets a bound; (2) the native leaf, the wrapper, and `plrun`; (3) the operator
reading, which needs **no example edit** because the sweep can run a *replaced* source:
gemv with `x` wrapped in `compute.short` must flip from *refused* to **agrees**, and the
append's incoming row with a `dim` bound likewise.

## 6. What this does not cover

- **The graph leg's own refusal is a different defect.** A recorded graph with two
  inputs of different lengths is refused by `RaggedInputs`
  (`crates/lichen-graph-ir/src/run.rs:273`) before any dispatch, and its premise — *"a
  dispatch over `[0, count)` reads every input to `count`"* — is false of a legitimate
  body: non-square matmul's `m·K` and `K·n` are equal only when `m == n`. That check is
  a **false refusal**, not a short-input question, and it is worth its own slice; a
  GEMV through the graph is blocked there first, so this note cannot unblock the graph
  leg on its own.
- **The class arm that refuses a recorded float parameter** is
  [compile-order-class](compile-order-class.md)'s, and a declaration does not touch it.
- **Bound analysis.** The compiler does not try to *derive* a read set from the IR; the
  author states it. A derivation would be the better long-term answer for bodies that
  are affine in their leaves, and it is not this proposal.
