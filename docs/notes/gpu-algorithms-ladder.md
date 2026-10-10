# What writing GPU algorithms in lichen actually hits

> Status: **exploration record.** A ladder of GPU algorithms was written and run
> against both backends on a real device (RTX 3060 Laptop, Vulkan 1.4.329).
> This note is what the ladder answered — what runs, what is refused, and what
> is refused *silently*. Nothing here is a proposal; the proposal is written
> against these facts.
>
> The ladder is `crates/lichen-language/examples/algorithms.rs` (run it with
> `-- cpu gpu`); the timings are `examples/bench.rs`; the cross-backend defect is
> `examples/crossbackend.rs`.
>
> **What to do about any of this is
> [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md)**, which reads this note as
> its evidence and splits the gaps into four axes.
>
> Companions: [lichen-compute-gpu](lichen-compute-gpu.md) (the backend),
> [compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md)
> (the `parallel` primitive), [floating-point](floating-point.md) (proposed,
> unimplemented), [attributes](attributes.md) (`Perspective`, checked and unread).

## The shape of the answer

The primitive is a **buffer map**: one invocation per index, reads from a tuple
of input buffers, writes to a tuple of fresh output buffers, all on one index
space. That is a real and well-built primitive. What it is not is a loop, and
almost every GPU algorithm people mean is a loop.

## What runs today, and agrees between the two backends

| algorithm | shape it used |
|---|---|
| fill | one `write` per index |
| axpy, `y = 3x + y` | two input buffers, one output |
| gather / transpose | read index computed from `i` (`i % 4`) |
| stencil | neighbour read at a **clamped** index, via `if` or via arithmetic |
| unrolled dot product, fixed length | four literal-index reads |
| 2×2 matrix multiply | `/` and `%` to split `i` into `(row, col)` |
| tree reduction, four levels | four dependent `plrun`s, each halving |
| 16-link chain as one submission | `compute.graph` + `compute.graphrun` |

The last three are the interesting ones, and they are all the **same** fact: a
kernel body is straight-line, so a cross-index algorithm is expressed by
*chaining kernels* rather than by looping inside one. A tree reduction is a
chain; a 2×2 matmul is a loop unrolled by hand at a size chosen by the author.
Both work. Neither is how anyone would write the same algorithm if the loop were
available.

## The gaps, in the order they block an algorithm

### 1. There is no float

`1.5` does not lex. `LowValue`'s only number is `USize`, `KernelBin` is integer
only, and the fragment ABI is `i64` scalars in, `i64` scalars out. So the
algorithms above are all integer algorithms, and they were chosen because they
are what the language can express: fill, axpy on integers, a 2×2 integer
matmul.

That is not a restriction on a GPU backend. It is a restriction on *the subject
matter*. Matrix multiply, convolution, FFT, tone mapping, a physics integrator,
an ODE step, a gradient — every one of them is a float algorithm, and not one
of them is reachable. [floating-point](floating-point.md) is a written proposal
whose phase 0 is a kind marker, a literal, a printer and a codec.

### 2. There is no loop, in a kernel or in a workgroup

The IR can express a branch and a loop — `BasicBlock`s with `params` and
`CondBr`/`Br` carry them, and a header's parameters *are* the loop's state — so
the obstacle below is **not** the instruction set. **What is missing is the
conversion**: nothing turns a `@loop`-marked recursion into a nest, so no source
program reaches a backend with one. Until that exists every cross-index term must
still appear as its own read. A sum over `N` elements is therefore `N` reads in
one body, or `log N` kernels chained. A prefix sum is `i + 1` reads at index `i` —
quadratic in the source text, and unreachable in practice. A matmul's inner loop
over `K` must be unrolled at a `K` the author chose, because no kernel a program
can currently write branches on a runtime bound.

See [loop-conversion](loop-conversion.md) §8.5 — items 1c and 2a are done, and
item 4 is the one this section turns on.

What a GPU algorithm actually needs here is a *workgroup*: lanes that can see
each other, a barrier, shared memory, and a sub-group reduction. None of those
exist, and none of them can be built out of what is here — they are a different
primitive, not a bigger version of this one.

**Recursion is the other way to write a loop, and it already works** — on
unmodified `dev`. `square (i + 1)` in a kernel body gives `[1, 4, 9, 16]`, and a
self-recursive `steps = k => if k == 0 then 0 else steps (k - 1) + 1` applied
as `steps 4` gives `4 : Int`. The deep pass reduces a same-module call and
unifies the substituted parameter with the argument *before* the emitter walks
the body, so there is no call left to expand — see
[below](#and-the-thing-this-corrected-static-expansion-already-exists).

What does **not** work is a callee or an argument bound *inside* the body, or a
trip count the host computed at run time, and all three are one fact: a kernel
is compiled from a **template, before any apply**, so a binding the body would
fill in at run time is still empty. See
[gpu-algorithm-roadmap §4.1](gpu-algorithm-roadmap.md#41-axis-b-already-in-the-language-and-what-it-does-not-reach).

### 3. There is no way to get data in

A `Buffer` is a handle to `[i64]` and the only producer is a kernel's output.
There is no `compute.buffer`, no upload, no way to turn a program value into an
input.

The symptom is worse than a missing feature, because it is **silent**:

```lichen
data = [3, 1, 4, 1, 5, 9, 2, 6]
k = cfg => { n = cfg(0); i = compute.range n
             compute.write ((compute.Write _)(.to n, .at i, .value compute.read ((compute.Read _)(.from cfg(1)(0), .at i)) + 1)) }
out = compute.plrun k (8, (data,))
compute.collect out
```

answers `parameterized`, on both backends, with no diagnostic. The `read` of a
non-buffer stays a lazy `Index` that nothing ever forces; the `collect` of a
value that is not a buffer is likewise lazy. A program written this way is a
plausible-looking program that computes nothing, and its *type* still prints as
`array<?a, ?b>`.

> **Since this ladder: the silence is gone and the feature is not.** A decided
> non-buffer at any of the three sites is refused by name now — see
> [below](#a-read-of-a-non-buffer-is-parameterized-not-a-refusal) — so the
> program above fails with
> *"a parallel launch's `cfg(1)` is the tuple of buffers its kernel reads, and
> position 0 of it holds an array. A buffer is what a `compute.plrun` or a
> `compute.graphrun` hands back, and there is no way to make one out of a
> program value"*. The gap is still a gap; what changed is that it says so.

So today the only way to have data on the device is to run a fill kernel first
and derive everything from the index.

### 4. A kernel has no scalar parameters

`cfg = (n, (buffer…))` is the whole configuration, and `cfg(0)` is the count.
Reaching `cfg(2)` is refused by name:

> `compute.parallel: unsupported index in kernel body (only parameter reads,
> value_of extractions, and 2-element conditionals)`

So a coefficient (`axpy`'s `3`), a stride, a bound, a shift — anything not in a
buffer — must be a literal in the source, or a value the author pre-computed
into a one-element buffer and read back per lane. `3 * xv` works because `3` is
a constant; `alpha * xv` with `alpha` a program value does not.

### 5. A scatter-accumulate is silently wrong

```lichen
hist = cfg => { n = cfg(0); i = compute.range n
                key = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
                compute.write ((compute.Write _)(.to 3, .at key, .value 1)) }
```

64 elements into 3 buckets answers `(1, 1, 1)` — last writer wins, on **both**
backends, with no diagnostic and no refusal. This is the documented
determinism caveat in
[compute-parallel-buffer-read-write](compute-parallel-buffer-read-write.md#scope)
("the one place the run's result stops being reproducible"), and the ladder
confirms it is reachable from ordinary-looking code. There are no atomics.

### 6. An out-of-range read is undefined, and differs per backend

```lichen
v = compute.read ((compute.Read _)(.from cfg(1)(0), .at 4))   -- the buffer has four elements
```

reads element 4 of a four-element buffer. CPU answers `0`; the GPU answers
`5`, because the device's padding is the last value written. Both are
plausible, neither is a diagnostic. A kernel that indexes out of range is
therefore a program that computes different numbers depending on which backend
it was named for — the one thing the design is most careful about elsewhere.

### 7. One element per lane

`Perspective` is fully checked, in the persist codec, with an acceptance table —
and nothing reads it. A dispatch gives each lane one index, `LOCAL_SIZE_X` is
64, and the shader is branch-free by construction. So the arithmetic is scalar
width, which for an integer add is a fraction of what the hardware does with
the same dispatch. This is the gap between "runs on a GPU" and "uses a GPU".

### 8. Graphs are the gpu's alone

`compute.graph` records through the `ParallelBackend` contract, and the CPU path
is *not* a `ParallelBackend` — it is the wasm engine inside `lichen-compute`. So
a graph over `"cpu"` kernels is refused, and a graph over `"gpu"` kernels needs
an installed backend while a `plrun` over `"gpu"` kernels also needs one but a
`plrun` over `"cpu"` kernels does not. A program that is correct on one backend
is not portable to the other, and the refusal arrives at run time.

### 9. Device memory is never released

A `"gpu"` run's outputs are resident and live until the context drops. An
algorithm that loops — an iterative solver, a tree reduction written as a loop,
a training step — accumulates buffers until the device refuses the next
allocation. The note states this plainly; the ladder did not need to try hard
to hit it.

## What the timings say

Whole program, best of 20, `y = 3x` over `n` with the read-back, and a 16-link
chain as one `compute.graph`. Release build, one device.

| count | 1 kernel, cpu | 1 kernel, gpu | 16 links, gpu |
|---|---|---|---|
| 65 536 | 108.5 ms | 5.49 ms | 14.98 ms |
| 262 144 | 401 ms | 4.61 ms | 13.46 ms |
| 1 048 576 | 2839 ms | 6.54 ms | 17.86 ms |

Two things to read out of it, and one to be careful about.

**The GPU column is nearly flat** — 4.6 ms to 6.5 ms across a sixteen-fold count
— which is what a device looks like once the fixed cost is paid. The CPU column
grows linearly, at roughly 2.2 µs per element, because each element is one
`wasmi` call with its own store. **This is the strongest argument the ladder
produced for the backend**, and it is much stronger than the dispatch table in
[lichen-compute-gpu](lichen-compute-gpu.md#what-the-gpu-actually-costs-and-where-the-crossover-is),
which compared a single dispatch against a *scalar loop* and found the GPU
losing at every count. Comparing a whole lichen program changes the answer,
because the CPU path's per-element cost is an interpreter's.

**Be careful with the level, and the table shows why.** These are whole
programs, so each figure includes parse, check and evaluation of the lichen
source — and at these counts that host work dominates a dispatch. The GPU column
is **not monotonic**: 65 536 is *slower* than 262 144, and across three runs of
the same binary the 65 536 figure read 2.95, 3.90 and 5.49 ms, a spread wide
enough to invert a row. So the honest reading is the *shape* — flat versus
linear — and the *slope*, not the level and not the ratio. The 16-link column
is host-dominated for the same reason, and it is here to show that a fused chain
*works* at that size rather than to be compared against the CPU column.

**A single dispatch still does not repay itself** on the device's own numbers,
and a chain only repays it from about four links at a million elements. What
the ladder adds is that the alternative is not a good CPU loop: it is an
interpreter.

## A fourth thing this found, and it was already filed
The probe was also asked whether **a `loop` operator** could serve as the unroll's
surface, and the answer turned out to be **P1-33**, which
[code-audit](code-audit.md) already carries as *todo*:

```lichen
loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)
-- expected Int -> Int, found Int -> Int
```

P1-33 is *"A self-recursive call in a conditional's branch is refused as
`expected Int, found Int`"*, and its own narrowing table identifies the shape
precisely — one-argument self-recursion in a branch **runs**, a self-reference
with no conditional **runs**, a non-recursive two-argument callee in the branch
**runs**; a self-referential two-argument one is the trigger. And it records the
escape, which is the whole difference:

```lichen
loop = (f => n => x => if n == 0 then x else loop f (n - 1) (f x))
     : (Int -> Int) -> Int -> Int -> Int
inc = x => x + 1
(loop inc 3 0, loop inc 10 5, loop inc 0 7)   -- (3, 15, 7): <Int, Int, Int>
```

**So the operator is one annotated line of lichen and works today** on the
interpreter. In a *kernel* it is still refused, and that is the same missing
**apply** as everything else here rather than a loop problem. The measured
ceilings for the expanded form are in the proposal's §4.1.

**The first explanation of the type error here was wrong**, and P1-33 corrected
it: it is not a function argument threaded through a partial application
comparing two function *kinds*. It is a VM **apply-time parameter check** whose
two sides hold the call's argument *values* — so the message is comparing
values and calling them types, which is why both sides render `Int`.

The first two are silent-wrong-answer defects — a plausible answer that is not
the right one — and both are **fixed** on `feature/gpu-algorithms`, each with a
test that fails without the fix. The third is a refusal that named nothing; its
**message is fixed** and the three cases it names are not, because all three are
one missing *apply*. The proposal's
[§8](gpu-algorithm-roadmap.md#8-the-defects-and-which-are-fixed) is the
one-line version of all three.

## Three defects the ladder found, and what became of them

The first two are silent-wrong-answer defects — a plausible answer that is not
the right one — and both are **fixed**, each with a test that fails without the
fix. The third is a refusal that named nothing; its **message is fixed** and the
three cases it names are not, because all three are one missing *apply*. The
proposal's [§8](gpu-algorithm-roadmap.md#8-the-defects-and-which-are-fixed) is
the one-line version of all three.

### The graph registry freezes the backend of the first graph of a shape

`graph_digest` (`crates/lichen-compute/src/compute/graph.rs`) hashes the
`Graph` and deliberately **not** the backend, on the stated ground that "it is a
property of how the graph is *run*, not of what it computes". But `intern`
stores the whole `BuiltGraph`, backend included, under the id that digest
produced. So the first build of a shape in a process fixes the backend for
every later build of that shape, and a later build is handed the earlier one's
`BuiltGraph` back.

The observable: in one process, build the 16-link chain with `"cpu"` and then
with `"gpu"`, and the second is refused —

> `compute.graph: this graph was compiled for the "cpu" backend, and a graph
> runs through the ParallelBackend contract — which the cpu path is not`

— for a program that never mentions the cpu. The registries are process-global
by design, so this crosses program boundaries: a process that has ever built a
cpu graph of a shape can never run a gpu graph of it.

The invariant the note wanted ("a cache can never serve one backend's graph for
another's") is true of the fragment registry, where the backend is genuinely
absent from what is stored. It is false of the graph registry, where the
backend is stored and simply not keyed.

**The fix** moves the backend onto `ComputeValue::Graph`, so the registry stores
a graph and nothing else — the same rule `ComputeValue::ParKernel` already
follows, and what `graph_digest`'s own reasoning assumed.

### A `read` of a non-buffer is `parameterized`, not a refusal

The symptom in gap 3. Where a native op's argument does not match, the fallback
is `LowValue::Parameterized`, so a kernel that reads a program array instead of
a buffer produces a value that no one forces. The same fallback is what makes
the whole body lazy, which is why it cannot simply become an error — but a
*fully forced* `read` whose buffer is not a buffer is decidable, and refusing it
by name would turn a silent no-op into a diagnostic.

**The fix** covers the three sites that reach it — `compute.read`,
`compute.collect`, and a launch's `cfg(1)` — each naming its own position. The
launch site is the one the ladder actually reached, and it is the one that
matters: a program that passed a plain array where a buffer belonged used to
finish and print a type.

### An unresolvable node is reported as a `NodeId`

Found later, by the recursion probe below. **The message is fixed; the three
cases it names are not.**

A kernel body that reached a node with neither a value nor an operation answered

> `compute.parallel: kernel body hits a node with neither value nor operation
> (node=NodeId(394v1))`

on **both** backends — a compiler-internal number, and the only refusal in the
compute surface that named nothing. The message now names the fact all three
cases share (*a kernel is compiled from a template before any apply, so a
binding the body would fill in at run time is still empty*) and lists the three
shapes, **without claiming which one it is**, because naming the wrong cause
sends the reader to the wrong place.

The three, all confirmed on both backends:

| program | result |
|---|---|
| a `compute.call` inside a **parallel** body — the identical call in a *scalar* body runs and answers `5 : Int` | refused |
| a module-level helper called with a **body-local alias** fed by a buffer read | refused |
| a helper **defined in the body** and called there | refused |

The first is the one that matters, for a reason the message does not change:
**only `parallel` names a backend**, so a device cross-kernel call is reachable
*only* from a parallel body, and that path fails in the compiler before any
backend sees it. `SpirvRefusal::CrossKernelCall` is therefore a refusal no
lichen program can currently provoke.

**What is left is the three cases, and all three are one missing *apply* — see
[gpu-algorithm-roadmap §4.1](gpu-algorithm-roadmap.md#41-axis-b-already-in-the-language-and-what-it-does-not-reach).
Naming them was the cheap half.

### And the thing this corrected: static expansion already exists

The probe was written to ask whether **recursion could stand in for a loop** as
the unroll's surface. It cannot be *added*, because it is already there: on
unmodified `dev`, `square (i + 1)` in a kernel body gives `[1, 4, 9, 16]`,
`square 3` gives `[9, 9, 9, 9]`, and a self-recursive `steps 4` gives `4 : Int`.
The deep pass reduces a same-module call and unifies the substituted parameter
with the argument before the emitter ever walks the body, so by then there is no
call left to expand. The `// Style 1 … deferred` comment in `emit_node` names
the *unreduced* case, and reading it as "the feature is missing" is what the
first version of this note did.

**The probe.** `crates/lichen-language/examples/recursion.rs`, run with no
arguments. It asks the questions in order — is a same-module call expressible,
with a decidable and with an index-dependent argument; is recursion expressible;
is a call expressible in each body shape; what does an un-expanded `CallKernel`
chain cost against the same arithmetic inlined — and prints a refusal or a
number for each.

**And the probe's four cases are the acceptance suite of the thing that answers
it.** The decidable cases (`RECURSIVE_LITERAL`, `HELPER_INDEX_ARG`) pass today
because the deep pass expands them. The two that decide gap 2's ceiling —
`RECURSIVE_INLINE` and `LOOP_RUNTIME_COUNT`, both a trip count the kernel's own
index supplies — are refused, and moving them to a number on **both** backends is
how [loop conversion](loop-conversion.md) would be known to have landed. That
note is where the design is; nothing here needs to change for it.

## What the ladder did not try

Sorting, atomics, and anything with a data-dependent trip count — all three are
outside the primitive rather than at its edge, so probing them would have
measured refusals rather than behaviour. Sub-group width and shared memory were
not probed because no source construct reaches them.

The histogram (a contended write) is the exception and it is not one of them: a
contended write is *expressible*, which is what makes it worth having written
down — it runs, on both backends, and is silently wrong. Sorting and a
data-dependent trip count have no such shape, so there was nothing to measure.

**The reason this note is evidence rather than a specification is that its
blind spots are structural.** No probe can distinguish "the primitive cannot say
this" from "the surface has no word for it", because both look like a refusal
or like a program that quietly does nothing. The four axes in
[gpu-algorithm-roadmap](gpu-algorithm-roadmap.md) are a claim about which of the
two each gap is, made from what the ladder could reach — and the ladder is what
makes the claim checkable rather than merely plausible.
