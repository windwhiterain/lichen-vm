# Runtime scalars in a parallel kernel: the per-leaf ABI and its two blockers

> Status: **in progress.** The *lowering* half landed — a parallel fragment's
> scalar leaves are typed by the parameter's own fields, so a runtime `Float`
> scalar is a `Float` leaf (`scalar_leaf_classes`, `compute.rs`).  The *host*
> half (decoding the leaves out of the launch's `cfg` and passing them to the
> workers) is **not written**, and it is blocked on the two measured facts in §4
> and §5: a struct-parameter kernel cannot write a `Float` element at all, and a
> JIT'd signature can only be spelled with the shipped lambdas.
> Companion: [compute-param-struct-handoff](compute-param-struct-handoff.md)
> (the named-struct parameter this exists for),
> [type-query-api-proposal](type-query-api-proposal.md) §7 (the accessors the
> leaf decode uses), [floating-point](floating-point.md) §4.4 (the class model),
> [operator-polymorphism](operator-polymorphism.md) §4 (the array desugar §4
> below needs).  Worktree `.worktrees/kernel-param-struct`.

## 1. What a runtime scalar is, and why it is the point

Moving a parallel kernel's parameter from the positional `cfg = (n, (buffers…))`
to a named struct was for two things: a read that names its buffer, and **runtime
scalars beside the count** — `k.alpha`, a number the host fixes at launch
(`compute-param-struct-handoff` §1).  Everything before this note delivers the
first; the second is what the leaf classes are for.

The ABI's shape, from the launch side: the parameter's **scalar leaves in field
order**, then the **index**, and the input buffers are bound rather than passed.
The first scalar is the launch extent (`cfg(0)`'s role), which is why the host
reads it as an ordinal.

## 2. What landed: each scalar leaf's class is its parameter field's

`compile_parallel_fragment` used to seed *every* leaf `LowShape::USize`, so a
parameter leaf read `Int` whatever it was annotated with.  `scalar_leaf_classes`
now reads each leaf's class from the field the role names
(`parallel_roles`'s `scalars`, in signature order) through the field accessors of
`type-query-api-proposal` §7, and seeds the `cfg` slot with those shapes
(`param_shape` follows: the leaf list, then the index).

Two leaves stay `Int` whatever the parameter says, and the reason is the same for
both: they are not data.  The **index** is a lane number, and the **first
scalar** is the launch extent — a `Float` one is refused **by name** here, where
the message can name the field, rather than left to surface as an apply-time
class conflict with no span.

Measured, on the probe of §6: the role table decodes
`scalars: [[0], [1]], inputs: [[2, 0]], outputs: [[3, 0]]` and
`scalar_leaf_classes` answers `[Int, Float]`.  The `(n, (buffers…))` parameter
shape is untouched (one leaf, `Int`), which is why the existing suites stay green
— `lichen-compute` 17, `lichen-language --test compute` 58, `--test pipeline`
134, `--test examples` 1.

## 3. What the host half still needs (not written)

| Step | Where |
|---|---|
| Decode `scalar_count` scalars out of `cfg`, then the input tuple | `ComputeOperator::ParLaunch`'s arm, `compute.rs:1329` |
| Carry the scalar values on the state | `ParallelState`, `compute.rs:7173` |
| Pass them beside the index as the `main` arguments | `run_parallel_range`, `compute.rs:7719`; `run_parallel_kernel`, `compute.rs:7434` |
| Assemble the fragment's own signature | `assemble_parallel_fragment`, `compute.rs:7303` |
| Push the scalar leaves to the shader | `lichen-compute-gpu/src/dispatch.rs:904` (today: "two leaves and one output, or refuse") |

The decode is **positional** and that is the ABI's rule, not a convenience: the
host struct is the parameter's scalars in field order, then `.I`.  A parameter
that interleaves scalars with `.in` would need a role table on the host side,
which the fragment does not carry — so the lowering should refuse an interleaved
parameter by name rather than mis-decode it (not written either).

The measurement that fixes this half's shape: with §2 landed and the *shipped*
signature lambdas, the probe now reaches the assembler and fails there —
`compute.parallel: encountered an incorrect number of parameters` — because the
fragment's `main` is `(extent, alpha, index)` while the host still passes
`(extent, index)`.

## 4. Blocker A: `compute.write [buffer, index, value]` is a homogeneous array

A `compute.write` argument is an **array literal**, and lichen's array literals
are homogeneous: every element unifies into one element-type cell
(`check_array_term`).  With the *positional* parameter shape the count and index
cells are undecided, so a `Float` value simply drags the whole cell to `Float`
and the program checks — that is how
`a_varying_float_element_is_seeded_from_the_index` works, and the ABI's all-`Int`
seed fiction is what absorbs the difference at lowering.

An **annotated** parameter decides those cells, and then the same program is
refused.  Measured, plain lichen plus the compute module, `Par =
struct<.n Int, .alpha Float, .in In, .out Out>`:

| Body | Result |
|---|---|
| `compute.write [k.out.z, 0, int2float v + k.alpha]` | `expected Int, found Float` at the array element (a located `ArrayElement` diagnostic) |
| `compute.write [k.out.z, i, int2float v + k.alpha]` with `i = compute.range k.n` | the same conflict, but surfacing **span-less** through `range`'s count/index class unify (`RangeOp::build` unifies the index's cell with the count's) |
| `compute.write [k.out.z, i, float2int (int2float v + k.alpha)]` | checks — the value is `Int` again, so the array is homogeneous |

So a struct-parameter (annotated) kernel **cannot write a `Float` element today**;
it can only write a `Float` computation converted back to `Int`.  The real fix is
the one `operator-polymorphism` §4 already names: remove the homogeneous
array/branch desugar so that a `write`'s elements are positions of their own
rather than one cell.  Until then, this note's end-to-end probe uses the
`float2int`-wrapped form, and that is a **workaround, not the design**.

## 5. Blocker B: only the shipped lambdas can spell a JIT'd signature

`parallel_sig f "cpu" Sig` needs a `Sig` whose domain is the JIT'd input struct.
The shipped `compute.A`/`compute.P`/`compute.S` build it for the one-scalar
shape; a runtime scalar needs a second one, so the author must spell their own —
and that does not currently work.  Measured:

| Spelling | Result |
|---|---|
| `Par = compute.P (compute.KT _)(.I In, .O Out)`, `Sig = compute.S (compute.KT _)(.I In, .O Out)` | works (the shipped tests) |
| `Par = struct<.n Int, .alpha Float, .in In, .out Out>`, shipped `Sig` | **works** — the role table decodes and §2's classes are produced |
| `A2 = I => struct<.n Int, .alpha Float, .I I>`, `S2 = T: KT T => A2 T.I -> T.O`, `Sig = S2 (KT _)(.I In, .O Out)` | fails: `expected raw[<domain>, <codomain>], found TypeFunction` (one located diagnostic at the `Sig` line, two span-less ones from the `plrun` gate) |
| the same `Sig` as a *value* (no `parallel_sig`) | prints correctly, and with the same rendered type as the shipped form |

So the difference is not the arrow's shape or its rendered type — both spellings
render as `struct<…> -> struct<…>: TypeFunction` — but which **apply** produced
the value: the shipped one is a *static* (frozen-module) apply, the author's is a
*dynamic* one, and the wrapper's `.sig` field reads its type lazily.  Something
in that path decides a dynamic type value's cell where the static one leaves a
cell for the lazy read to bind.  That is a checker question, and it is the same
family as the decided-vs-lazy field type of `type-query-api-proposal` §7 — but it
is **not** diagnosed yet, and this note does not claim a cause.

Consequence for this feature: until (a) that path is fixed, or (b) the
runtime-scalar shape is shipped as a lambda in `compute.lichen` beside
`A`/`P`/`S`, a runtime scalar cannot be launched end to end.  (b) is a
convention with a hardcoded scalar name; (a) is the honest fix and is unfinished.

## 6. The probe, and how to run it

Scratch file, not to be committed (the example harness runs every file in
`examples/`):

```lichen
---
  compute = import "compute.lichen"
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
Sig = compute.S (compute.KT _)(.I In, .O Out)
Par = struct<.n Int, .alpha Float, .in In, .out Out>
f = (k : Par) => {
  i = compute.range k.n
  v = compute.read [k.in.a, i]
  compute.write [k.out.z, i, float2int (int2float v + k.alpha)]
}
k = compute.parallel_sig f "cpu" Sig
out = compute.plrun k ((compute.A In)(.n 3, .I In(.a inbuf)))
compute.read [out, 1]
```

```bash
cargo run -q -p lichen-compiler -- <probe>.lichen
```

Today it reaches the assembler and refuses with
`compute.parallel: encountered an incorrect number of parameters` (§3) — which is
the host half missing, and the acceptance test for the rest of this work: the
answer must be `11` (`v = 11`, `11 + 0.5` truncated is `11`).
