# One answer per class

> Status: **current** — the maintainer's verdict that a "second answer" is not a concept (a
> class holds one answer, so the engine has no path that names one), the measured defect that
> used to produce `expected ?, found ?` on the examples, and the rule that removes it.
>
> Points at: `crates/lichen-highlevel/src/checker/templates.rs`
> (`instantiate_requirement_value_with`), `crates/lichen-lowlevel/src/equality.rs`
> (`write_node_answer`, `value_holds_open_cell`).

**The verdict.** A class holds one answer (`equality.rs` fills a class once and never
replaces it), and a node has one value. There is no second answer for the engine to
reconcile, so there is no "two answers on one node" concept in the implementation, no
diagnostic sentence that names one, and no code path that exists to report one. Where a
write disagrees with the class's value the engine reports an **ordinary unify failure**
(`DiagKind::Runtime`, rendered `expected X, found Y`), and the only correct response to one
is to remove the code that produced the second value.

## The defect that produced them

`instantiate_requirement_value_with` returned every frozen-typed requirement in place ("a
frozen type half names a value outside every applying function"). That included a struct
instance's own template: `CIn`'s `.x` field is declared `compute.Buf (…)`, a frozen `Buf`
type value with an open `.native` cell, so one instance's buffer was written into the shared
type value and the next instance's buffer met it.

The rule now separates the two uses. A **struct instance's** template copy passes
`copy_frozen`, so a frozen-typed field whose value holds an open cell
(`Module::value_holds_open_cell`) is copied for that instance; every other requirement still
references a frozen value, which is what the graph recording needs (`5b016643`: copying it
turned the shared node into a fresh cell and the recording lost the caller's placeholder).

Measured with `examples/convolution2d.lichen`'s own program, each run through
`lichen-compiler`:

| program | before | after |
|---|---|---|
| one kernel call | checks | checks |
| three calls of one kernel, different buffers | checks | checks |
| two kernels, the same `CIn` argument values | checks | checks |
| two kernels, different buffers | `expected ?, found ?` | checks, declared answer |
| the second kernel given its own `CPar2` but sharing `CIn` | `expected ?, found ?` | checks |
| the second kernel given `CPar2` and `CIn2` | checks | checks |

Suites: language **289** (+69), highlevel **136**, compute **283** / 1 ignored, lowlevel
**167** / 1 ignored, compute-gpu **94**. The examples survey moves
`convolution2d.lichen` from a failing check to a running program.

## What the diagnostic is now

`write_node_answer` still writes the operator's answer into the node's class; a class that
holds nothing takes it, and a value a unification wrote is not the operator's answer, so the
operator runs (the run stamp is the other axis). Where a different value does reach one
class, the renderer prints `expected X, found Y`, spelling the two values when neither side is
a handle. The earlier note that named a second answer, the sentence that named one, and the
`value_a`/`value_b` special case for two open sides are removed.
