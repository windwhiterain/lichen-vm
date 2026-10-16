# The recorded leg's count: the rule is principled, its own sentence is nearly unreachable

> Status: current — the measurement below is one slice at `dev@dd13f417`; the two diagnostic gaps it
> found are worked in [Fixed](#fixed-both-parts-measured-on-the-thirteen-shapes) (`feature/rcd`), and
> one part of that measurement corrected what this note had only inferred.
> Points at: `crates/lichen-compute/src/compute.rs` (`record_launch` at `:4786`, its extent read at
> `:4847-4896`, `build_graph`'s return-value refusal at `:5187-5196`),
> `crates/lichen-compute/src/compute/graph.rs` (`Extent` and its `# Invariant` at `:312-325`,
> `describe` at `:370-380`, `place` at `:349-367`),
> `crates/lichen-compute-gpu/tests/compute_gpu/graph_on_device.rs:1434`.
> Companions: [llm-operators](llm-operators.md) (the host half — an extent follows the state on the
> direct `compute.plrun` path, and the only refusal left is on the recorded leg, which is what this
> note measures from the other side), [compute-graph-jit](compute-graph-jit.md) (the graph itself, and
> the three sentences that state why the rule exists), [code-audit](code-audit.md) (grepped: **no**
> `wontfix:D<n>` covers `record_launch` or the recording path).

**The finding in one line.** The count rule is **principled** — the graph's value table holds the
caller's arguments and the values dispatches produced, and **no node kind produces a number**, so a
count the host computed while recording is not a dispatch and has nothing replay could reproduce it
from — but **its diagnostic is the gap**: for the spelling a reader would try the rule never speaks,
and where it does speak it names a heap address.

## The shapes, and what each one said

Every shape is a `.lichen` program run by `lichen-compiler` on the `"cpu"` backend, over the two
kernels `crates/lichen-compute/tests/lichen_compute/graph_jit.rs`'s `KERNELS` fixture declares (with
`"cpu"` in place of `"gpu"`, so the CLI can run the launch that happens outside the recording):

```text
In1  = struct{.a Int}
Out1 = struct{.z (compute.Buf Int)}
Par1 = compute.P (compute.KT Int)(.I In1, .O Out1)
f1 = (k : Par1) => { i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10)) }
k1 = compute.parallel f1 "cpu"
In2  = struct{.b (compute.Buf Int)}
Out2 = struct{.w (compute.Buf Int)}
Par2 = compute.P (compute.KT Int)(.I In2, .O Out2)
f2 = (k : Par2) => { i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a)) }
k2 = compute.parallel f2 "cpu"
held = compute.plrun k1 ((compute.A In1)(.n 3, .I In1(.a 0)))
GArg = struct{.n Int, .in In1}
```

`step = (s : GArg) => { first = (compute.plrun k1 ((compute.A In1)(.n …, .I In1(.a 0))) : Out1) ; … }`,
then `built = compute.graph step`. The body is a block whose first item's value is the recorded
return, so the graph's return-value rule is satisfied whenever the body's own value is a value a
graph can hand back — which is the whole point of the shapes below.

| shape | the count in the cfg | outcome |
|---|---|---|
| A | `.n 3`, two dispatches | **records**; prints `raw ?: ?a`, exit `0` |
| B | `.n s.n` — the graph's own argument | **records**; prints `raw ?: ?a`, exit `0` |
| C | `.n n`, `n` a host `Int` off `(compute.collect buf : array{Int, 1})[0]` | **records**; prints `raw ?: ?a`, exit `0` |
| D | `.n (s.n + 1)` | refused — **the graph's return-value rule**, nothing about the count |
| E | D, with the computed dispatch second | refused — the same sentence |
| F | retired tuple spelling `(s.n + 1, (first,))` | refused — the same sentence |
| G | retired tuple spelling `(held, (first,))` | refused — **the count rule**, `found` a raw pointer |
| H | retired tuple spelling `("five", (first,))` | refused — **the count rule**, `found = Str("five")` |
| I | retired tuple spelling `(3.0, (first,))` | refused — **the count rule**, `found = Float(3.0)` |
| J | named struct `.n 3.0` | **a checker type error**, before any recording |
| K | retired tuple spelling `(3, (first,))` | the literal is taken as the extent, then the *buffer* filter refuses it |
| L | retired tuple spelling `(held.z, (first,))` — a `Buf` | refused — the return-value sentence |
| M | retired tuple spelling `(k1, (first,))` — a kernel value | refused — the return-value sentence |

Two sentences account for every refusal. The interception, verbatim, for D, E, F, L and M:

> `error: compute.graph: this function's return is not a value a graph can hand back, so there is no graph to record: a graph's value table holds what its dispatches produced and the arguments it was run with, and this body's own value is not one of those`

and the rule this slice went after, verbatim, for G, H and I:

> `error: compute.graph: a dispatch's count is Str("five"), and a count has to be a literal or one of this function's arguments: a graph's extent is either known while it is built or read from its own value table at run time`

`Float(3.0)` is the other `{found}` shape G is not; and shape G's own `{found}` is the third:

> `error: compute.graph: a dispatch's count is Array(Dynamic(Handle(Pointer { addr: 0x1b76e1cad78, metadata: 1 }))), and a count has to be a literal or one of this function's arguments: a graph's extent is either known while it is built or read from its own value table at run time`

Every one of G, H and I prints the count sentence **and then** the return-value sentence, in that
order: the refused dispatch leaves the body's own value undecided, and `build_graph` reports the
return rule over the top of it. `record_extension_diagnostic` drops only exact duplicates
(`crates/lichen-lowlevel/src/module.rs:397-405`) and the CLI renders every entry, so a rule that
fires is always on the surface.

**Shape J is the reason G, H and I need the tuple spelling.** The named config struct types the
count, so a `Float` there is a type error from the checker and the graph's own filter never speaks —
which is the property `graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one`
(`crates/lichen-compute/tests/lichen_compute/graph_jit.rs:278`) pins, and which
[compute-graph-jit](compute-graph-jit.md) §558-566 already states. Shape G is that test's own source,
run through the CLI.

## Why the rule exists: principled, and stronger than a record/replay bargain

Three places state it, and all three say the same thing — **no node kind produces a number**.

1. `Extent`'s `# Invariant`, `crates/lichen-compute/src/compute/graph.rs:319-323`: *"It carries the
   same `Placed` an edge does, because a count is read from the parameter as often as a buffer is: an
   extent that only knew "a value" would produce a graph with no inputs. Kept apart from `Constant`
   because a count that depends on data is ordinary, and collapsing them would invent a node that
   produces a number for free."*
2. [compute-graph-jit](compute-graph-jit.md) §"A number on an edge": *"`Constant` is not a wart on
   that: it is the one case where the build already had the answer, and collapsing it into a value
   would mean inventing a third kind of node — one that produces a number for free, doing no work. **A
   node that *computes* a number does not exist yet**: a `KernelFragment` writes `outputs` buffers and
   leaves `results` scalars on its own stack, and a node's outputs are the buffers."*
3. The same note's *Not yet*, `:688`: *"**A node that computes a number.** A scalar kernel has no node
   to be, so a count today is either a build-time literal or a value the caller passed."*

So the rule is not a bargain about when the count has to be known — it is a statement about what the
table can hold. A recorded dispatch's value table has exactly two kinds of entry (`Placed::Input`, a
caller argument, and `Placed::Value`, a value an earlier node produced), because every entry is
something a dispatch or a caller supplied. A number the host computed during the recording is neither,
so a replay has nothing to re-read it from.

**Two measurements that keep the rule from being a wall.** Shape C: a count computed **on the host**
from data — read back off a `collect` — records, as `Extent::Constant`. Shape B: a count that is the
graph's own argument records, as `Extent::Value(Placed::Input)`, which is what
[compute-graph-jit](compute-graph-jit.md) §196-198 says the rule buys: *"a count that depends on data
is ordinary (a length off a `collect`), and a graph that cannot take one forces every such program to
rebuild the graph per run, which is the same as not having a graph."* The cost of C is exactly that
re-record per value; B is the way out, and it is the spelling a loop wants anyway.

## What was actually the gap: the diagnostic surface, in two measured parts

> **Both parts are fixed** — see [Fixed](#fixed-both-parts-measured-on-the-thirteen-shapes). What follows is
> kept as the record of what the gap was.

- **The rule does not speak for the spelling a reader would try.** D, E and F — every way of writing
  a count the body computes from its argument — are intercepted by the graph's **return-value** rule,
  which is a different rule about a different thing, and whose stated cause (*"this body's own value
  is not one of those"*) is a **symptom**: the body's value is undecided because a dispatch in it was
  refused or never became one. A reader who writes `.n (s.n + 1)` and is sent after the graph's return
  value is sent after the wrong thing. **Inferred, not measured inside:** that in D/E/F the count cell
  never becomes a value `record_launch` reads, so the operator is not entered for that dispatch — the
  count refusal is not recorded and cannot be, and the rule's absence is not a choice the code makes
  at that point.
- **Where the rule does speak, `{found}` is a `Debug` render, not a name.** `compute.rs:4877-4891`
  formats the value as `LowValue: {:?}` and falls back to `graph::describe` only for a `ComputeValue`.
  So a string and a float read cleanly (`Str("five")`, `Float(3.0)`), while a count that is an array —
  a closed-over dispatch's output struct — prints the arena handle: `Array(Dynamic(Handle(Pointer {
  addr: 0x1b76e1cad78, metadata: 1 })))`. That address differs per run and names nothing a reader can
  act on, and `graph::describe` (`graph.rs:370-380`) already exists for exactly this job: it names a
  buffer, a graph, a parallel kernel and each placeholder kind in words.

## Fixed: both parts, measured on the thirteen shapes

> Status: **one part fixed on the surface, one part fixed underneath it.** Measured at `feature/rcd`
> (`dcdca031` + this slice) from the CLI on the thirteen shapes of the table above. Two probes beyond
> the thirteen, T1 and T3, hold the measurement the second part is about; they are named where they
> are used and were deleted with the rest.

### Part 1 — `{found}` is named, not rendered. `graph::describe_low_value`

The refused count now goes through one helper for **both** leaves of the host's value union
(`named_value` in `compute.rs`, over `graph::describe_low_value` in `compute/graph.rs`), so no
`Debug` render is left on this path. G before and after, quoted:

```
before: error: compute.graph: a dispatch's count is Array(Dynamic(Handle(Pointer { addr: 0x1b76e1cad78, metadata: 1 }))), and a count has to be a literal or one of this function's arguments: …
after:  error: compute.graph: a dispatch's count is a tuple or a struct value, and a count has to be a literal or one of this function's arguments: …
```

H and I read as well now, in words rather than as a variant name: `the text "five"` and
`the float 3.0`. Two runs of G now print the same sentence — the address was an arena pointer and
differs per run.

### Part 2 — the refusal travels, and the recording is closed with it

`graph::Recording` carries `refusals`; `record_launch`'s `refuse` hands the reason to
`graph::record_refusal` before it writes the module diagnostic, so the reason is no longer only a
side effect. `build_graph` closes the recording on **every** path out — `graph::abandon` — and takes
the refusals with it, so the return-value sentence can say that it is what they left behind:

```
G/H/I/K after: error: compute.graph: this function's return is not a value a graph can hand back, … and this body's own value is not one of those: a dispatch in this body refused before that, and this is only what it left behind
```

The deeper sentence is still printed **first** and the refusal is still its own; nothing is traded.

**The closed recording is the part that changes behaviour, and it was a defect on its own.** `RECORDINGS`
is a thread-local stack, and `build_graph` used to return without popping it, so a refused
`compute.graph` left the recording open: **every launch after it in the same thread was recorded into a
graph that is never built, instead of run.** Probe T3 — a graph the count rule refuses, then a plain
top-level `compute.plrun` — quoted:

```
before: … argument 0 of this dispatch is not a compute value at all         (the plain launch, intercepted)
after:  … compute.parallel: a parallel parameter's input buffer is one of the kernel's own `Buf` fields, and position 0 of it holds an array …
```

Before, the plain launch was filed under `compute.graph` and never ran; after, it is dispatched and
refused for what it actually is.

### What the measurement contradicts: the count rule is never entered for D/E/F/L/M

The note above called this **inferred**. It is now **measured**, with an `eprintln!` probe in
`OperatorExt::run` and in `record_launch` (`dev` behaviour, deleted):

| shape | `record_launch` entered | count refusal reached |
|---|---|---|
| A, B | twice (both dispatches) | no — both record |
| D | **never** | no |
| E, F, L, M | **once** — the first dispatch only; the second is never demanded | no |
| G, H, I, K | twice | yes (G/H/I) |

So for D/E/F/L/M the count cell **is not a value `record_launch` reads**: `.n (s.n + 1)` is undecided,
and a `ComputeValue` count (`held.z`, `k1`) leaves the cfg array undecided, so the VM never demands
the `ParLaunch` operator at all. This holds in every spelling — named struct, retired tuple, count
bound inside the body — and `graph::describe`'s fallback *"not a value at all"* is never reached
either. **The count refusal cannot travel from `record_launch` for these shapes, because it is never
produced there.** Making it travel fixes the shapes where the rule *does* speak (G/H/I/K, above) and
the leak; reaching D/E/F/L/M needs a *second* reading of the count outside the VM's demand path —
`build_graph` walking the block's `ParLaunch` nodes — which is **not** in this slice: a walk of
`module.blocks[block]` finds nine launch nodes for a two-dispatch program (clones), so it would report
counts from dispatches outside the recorded body.

### One existing test now fails, and why

`graph_jit::what_a_recorded_body_may_not_reach_for_is_refused_by_name` asserts the *"a graph has no node
for one"* sentence for a scalar-kernel call inside a recorded body. With the recording closed, that
program's body instead refuses with the buffer sentence (`argument 0 of this dispatch is not a buffer`),
because the deep pass no longer forces the `compute.call` node the way it did with a stale recording
underneath. **The three programs that test runs share one thread**, so before this slice the sentence
was produced with the **leaked** recording of an earlier program still on the stack — the assertion was
passing on a defect. Fixing the test is a test change and needs the user's permission; it is not done
here.

## What a fix would touch — the three sizes, and which one was taken

> Kept as the record of the options. **The first two are what the slice took**; the third is the
> capability and is still open. Nothing in this list was *in* place when the list was written.

- **The wording, smallest.** `record_launch`'s extent read (`compute.rs:4877-4896`): render `{found}`
  through `graph::describe`, and name a `LowValue::Array` as what it is rather than as its handle.
  This is a message, not a rule: `Extent`'s `# Invariant` (`graph.rs:319-323`) is about the type and
  is unaffected, and **no `Invariant` or `Safety` sentence needs to change**. **Taken**, through
  `graph::describe_low_value`; measured on G/H/I above.
- **The interception, middle.** `build_graph` (`compute.rs:5184-5196`) reports the return-value
  sentence over the top of a refusal that already happened deeper. Reporting the deeper one needs the
  reason to travel: today `record_launch` records a diagnostic and returns `None`, so the refusal is
  a side effect that `build_graph` cannot see. That touches `record_launch`'s return path and the
  recording state in `compute/graph.rs` — and, if the refusal were suppressed when a deeper one
  exists, the sentence a *reader* sees changes for every recorded body.
- **The capability, large.** A count computed inside a recorded body needs a node kind that produces
  a number: `KernelFragment`'s `results` scalars becoming a node output, `lichen-graph-ir`'s `Count`
  gaining an operand rather than an `Edge`, `record_dispatch`/`finish` recording the computation, and
  the runner evaluating it. `compute-graph-jit.md:204-208` rules out the shortcut in both directions:
  collapsing it into `Constant` would "invent a node that produces a number for free", and it is not
  available anyway, because at build time the value does not exist — in shape D `s.n` is a placeholder,
  so `s.n + 1` has no number to bake. The graph value table itself needs no change for the spelling
  answer: pass the count as an argument (shape B), or compute it on the host before recording (shape C).

## Does a device loop need the recorded leg?

- **Measured.** The graph's own runner takes a run-time count on real hardware, today:
  `compute_gpu::graph_on_device::an_extent_that_is_one_of_the_graphs_own_values_runs_at_that_extent`
  **passed** on this machine's RTX 3060 Laptop GPU in 4.09 s — a graph whose node's `count` is
  `Count::Value(1)`, run at extents 1, 37 and 100 with the data deliberately longer than the count,
  answering element for element. That is the whole claim "the extent follows the state, on the
  recorded leg", on a device.
- **Measured.** From the CLI, **neither** leg reaches a device. A `"gpu"` kernel refused at run time:
  *"error: compute.parallel: this parallel kernel was compiled for the "gpu" backend, but no compute
  backend is installed: a host program has to install one before a run can be dispatched to a
  device"*; and a graph cannot be run on the host at all, because `compute.graphrun` wants a device:
  *"error: compute.graph: this graph dispatches to a device, but no compute backend is installed: a
  host program has to install one before a run can be dispatched"*. So a recorded graph is **only**
  runnable on a device, and the CLI is not the place this question gets answered.
- **Measured.** The recording leg records the count spelling a device loop needs — shape B, the count
  as the graph's own argument — so record once, pass the count per step, is expressible today.
- **Inferred.** A device loop does **not** need the recorded leg to have a state-following extent.
  The direct `compute.plrun` leg already takes its extent from the run's own cfg, which is what
  [llm-operators](llm-operators.md) measured on the host in three spellings; what the recorded leg
  buys a device'd loop is **one submission for a chain**, not a dynamic count. On that reading the
  gap this note measures is a convenience — and the convenience is already available through the
  argument spelling.
- **Unknown.** A **source-recorded** graph — `compute.graph` over a `.lichen` file — run on a device.
  This session cannot reach it from either end: the recording is measurable from the CLI and the
  running only from the Rust tests, and the CLI installs no backend. The two halves are each measured
  and their join is not.

## Measured, inferred, unknown

- **Measured, from the CLI at `dev@dd13f417`**: all thirteen shapes, with the refusals quoted above
  and the exit codes; that a literal count (A), the graph's own argument (B) and a host-computed
  count (C) each record; and that the count rule's own sentence is reachable from the CLI in three
  spellings, which no earlier probe of ours managed (see the correction to [llm-operators](llm-operators.md)).
  **Corrected since**: C as spelled in the table — a count read back off a `collect` — does **not**
  record from the CLI; only the return-value sentence prints. The property C stands for (a count
  computed on the host records, as `Extent::Constant`) does hold for a count read off a host
  `array{Int, 1}` subscript, which is the spelling the fixed measurements use. The table's spelling
  for C is wrong; its claim about the rule is not.
- **Measured, inside**, on `dev` behaviour: `record_launch` is **never entered** for the dispatch whose
  count D/E/F/L/M compute, so the count refusal is **never produced** for those shapes — the inference
  this note carried below, now a measurement. See [Fixed](#fixed-both-parts-measured-on-the-thirteen-shapes).
- **Measured, from the CLI, `feature/rcd`**: the naming fix on G/H/I and the clause on G/H/I/K, quoted
  per shape in [Fixed](#fixed-both-parts-measured-on-the-thirteen-shapes); and that closing the
  recording on the refusal path is a behaviour change of its own — probe T3.
- **Measured, on the device**: the graph runner's run-time count, one test, one adapter, 4.09 s.
- **Measured, by reading**: the three sentences that state the rule's reason, and the absence of any
  `wontfix:D<n>` over `record_launch` or the recording path in [code-audit](code-audit.md).
- **Inferred**: that a device loop needs nothing from the recorded leg beyond one submission.
- **Unknown**: the source-recorded graph run on a device; and whether the `{found}` address in shape
  G is stable across a release build (it is a handle into an arena, so stability is not expected; after
  the fix there is no address left to be stable — see [Fixed](#fixed-both-parts-measured-on-the-thirteen-shapes)).

## Probes

Every program above was a probe file under `probe/` in the worktree, deleted before the commit: the
thirteen `.lichen` shapes, the shared kernel header, and the two device probes (`compute.plrun` on
`"gpu"`, `compute.graphrun` on a `cpu` graph). No production file, no test and no example was added.

The fix slice added its own probes under the same directory, all deleted before its commit: the
thirteen shapes re-measured before and after, **T1** and **T3** (a refused `compute.graph` followed by
a plain launch), and a dozen intermediates used to find a spelling that records (C1–C8, P1–P5, Q1–Q3,
R1–R8, S1–S3, U1–U2). It also added temporary `eprintln!` probes to `OperatorExt::run` and
`record_launch` to measure which shapes enter `record_launch` at all; **those were reverted, not
committed** — `git diff` for this slice names only `crates/lichen-compute/src/compute.rs`,
`crates/lichen-compute/src/compute/graph.rs` and this note plus [llm-operators](llm-operators.md).
