# Proposal: a recording owns the cells it binds

> Status: **proposal, for the maintainer's review.** Written in the form the maintainer asked
> for: `example`, `expected`, `actual`. Nothing here is implemented; the code paths named are
> where the change would land.

## Example

An executable reduction of the five failing tests
(`nested_config::a_two_scalar_config_carries_both_and_runs_on_the_device`,
`float_leaf::a_float_scale_beside_two_extents_records_and_answers_on_the_device`,
`graph_runtime_scalar::what_makes_a_second_extent_admissible_does_not_depend_on_the_reads`).
The whole program, as the tests write it:

```lichen
---
  compute = import "compute.lichen"
---
In := {.d (compute.Buf (compute.BufParam{.type Float, .length 8}))}
Out := {.z (compute.Buf (compute.BufParam{.type Float, .length 8}))}
Par := {.n @any : Int, .in In}
raw := (k = Par) -> {
  i := compute.range k.n
  v := (compute.read ((compute.Read _){.from k.in.d, .at i}) : Float)
  y := compute.buf (compute.BufParam{.type Float, .length k.n})
  Out{.z (compute.write ((compute.Write _){.to y, .at i, .value (v * 2.0)}))}
}
kern := compute.parallel raw "cpu" Out

Fill := {.n @any : Int}
fill_raw := (c = Fill) -> {
  i := compute.range c.n
  d := compute.buf (compute.BufParam{.type Float, .length 8})
  Out{.z (compute.write ((compute.Write _){.to d, .at i, .value 1.0}))}
}
filler := compute.parallel fill_raw "cpu" Out
data := (compute.plrun filler ((Fill){.n 8}) : Struct)

GA := {.n @any : Int, .in In}
step := (s = GA) -> {
  compute.plrun kern ((compute.A _){.n s.n, .I In{.d s.in.d}}) : Struct
}
built := compute.graph step
out := (compute.plrun kern ((compute.A _){.n 8, .I In{.d data.z}}) : Struct)
compute.collect out.z
```

The module is evaluated twice (the checker's normalization and the run — the harness every
test uses), and the direct launch of `kern` with a real buffer comes *after* the graph in the
source.

## Expected

Both evaluations answer the same graph, and neither refuses: a `compute.graph` evaluation is
a **recording** — a call of the body with placeholders — and it must leave no state on the
program's nodes, exactly as an ordinary call leaves none on its caller's. The direct launch
then runs on its real buffer. In the two named tests, the device answer equals the direct
answer and the graph's recording is identical across evaluations.

## Actual

The first evaluation's recording writes its placeholder into a cell the **program** owns (the
`Buf` cell of the body's `In{.d …}` instance; measured: the same `Buf` wrapper and native cell
across evaluations, `GraphInput(3)` in the first). A later launch writes its real buffer into
that same cell, and the second evaluation's recording then refuses:

```text
compute.graph: argument 0 of this dispatch is a buffer and was not read from this function's
parameter, so the graph would have to hold it. …
```

and, once the input resolution is forced through `field_sources`, the stale cell meets the new
placeholder in one class: `expected Struct, found Struct — value_a: Buffer(..),
value_b: GraphInput(3)`.

Measured: the `example` above runs **once** and answers `[2.0, …]` (the CLI, one evaluation),
so the refusal needs the **second** evaluation of the same module — the harness's
`common::run`, which evaluates the program twice. The executable reproductions are the five
tests named at the top.

## Why this is a SPEC question, not a patch

The recording's write is a **clone-ownership** decision: `apply-clone-ownership.md` says a
node outside the call's template is referenced, not copied — and the body's nested `In{…}`
instance is such a node. Either a recording's body is copied deeply (so its binding writes
into the copy), or a recording's binding is a read-side link only (so it never writes). The
SPEC currently says nothing about a recording's ownership, so the two readings are both
consistent with it.

## Measured routes (all excluded)

| route | measurement |
|---|---|
| `record_graph_parameter_sources`' `or_insert` → `insert` | inert, 227/13 |
| input resolution through `field_sources` | reaches the placeholder, then meets the stale class: `Buffer` vs `GraphInput(3)` |
| `node_apply`'s closed-cell rule as the cause | every array-valued node it meets is `closed=false` and cloned |
| changing the clone walk's sharing rule | contradicts `apply-clone-ownership.md`'s "outside the scope is referenced" |
| a `whole_clone` flag copying every struct value in the recording's apply | **worse**: 226/14, `fuse_licence`'s elementwise merge regresses |

## The two candidate rules, in SPEC terms

1. **A recording's body is copied per recording.** `compute.graph f` evaluates `f`'s body as a
   call whose clones belong to the recording, so every cell the body binds is the recording's
   own; the program's nodes are only read.
2. **A recording's placeholder binding is a read-side link.** The body's cells stay the
   program's; the recording's cells are reached only by the reads the checker links
   (`field_sources`), and the binding writes nothing.

Both answer the `example` above identically. They differ where a body *writes* to a value it
read from a shared cell: (1) keeps the write local to the recording; (2) would have to reject
it. The five tests need (1)'s answers.
