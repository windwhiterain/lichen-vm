# Graph JIT: a chain of dispatches as one submission

> Status: current — the IR, the two extension-point seams, the lowering, the
> `compute.graph`/`compute.graphrun` surface and its tests are in.
> `Policy::Batch` is **refused by name** (the backend contract cannot record
> several dispatches into one submission), and what is left is listed under
> [Not yet](#not-yet).
>
> What this note is: what a graph JIT is, why it exists, how to use it, and the
> model that makes it sound. A **graph is a transcript of a run**, not of the
> source: `$graph(f)` applies `f` to a structure of placeholders and records the
> dispatches the body performs, and what comes out is a DAG of kernel nodes in
> evaluation order that can be run under one of three submission policies.
>
> Points at: `crates/lichen-graph-ir/src/` (`Graph`, `Node`, `Count`, `Value`,
> `Runner`, `Policy`, `GraphRefusal`), `crates/lichen-compute/src/compute/graph.rs`
> (the recording, `intern`/`lookup`, `run`), `crates/lichen-compute/src/compute.rs`
> (`ComputeOperator::Graph`/`GraphRun`, `record_launch`, `build_graph`, `run_graph`,
> `assemble_parameter`), `crates/lichen-language/tests/graph_jit.rs`,
> `graph_structure.rs` and `crates/lichen-compute-gpu/tests/graph_on_device.rs`.
>
> Companions: [lichen-compute-gpu](lichen-compute-gpu.md) (the backend and the
> pool of submission slots), [compute-buffer-wrapper](compute-buffer-wrapper.md)
> (the recorded body's parameter), and [loop-conversion](loop-conversion.md).
>
> **How to use it:**
>
> ```lichen
> --- compute = import "compute.lichen" ---
> step = ins => compute.plrun doubler (ins(0), ins(1))
> built = compute.graph step
> compute.graphrun built (4, data)
> ```
>
> A graph function's inputs are its **parameter**, and nowhere else: the
> placeholder tuple is one cell per role path, and reading `ins(i)` yields the
> `i`-th. A buffer the body reads as a **free variable** is refused, naming the
> capture.

## The decision the whole design rests on

**A native closure inside a graph function can only reach variables that existed
before the graph JIT ran.** It cannot capture a value the graph itself produces.

That single fact is why this is tractable:

- a closure's dependency set is fixed and known before the graph runs, and lies
  **entirely outside** the graph;
- no edge leads from a graph node into a closure;
- a closure takes nothing from inside the graph.

So **all the graph's edges are statically known**, scheduling is real scheduling,
and any topological order is correct.

**The rule is about a node's *capture*, not its *arguments*.** A host call
taking graph values as arguments is ordinary — the edges into them are ordinary
edges. The consequence is where the cost lands: **a node that reads a dispatch's
output has to wait for it and fetch it**, and no arrangement of submissions
removes that. The shape that pays is a node whose inputs already existed before
the run — and that is the only thing about host logic in a graph worth money: a
closure can run **while a submission is in flight**, because it touches nothing
the device is writing.

**The corollary to write down wherever a graph exists:** if a closure form ever
appears that can capture a post-launch value, the scheduler becomes unsound,
silently. Same class of obligation as SPIR-V's single-`OpLabel` invariant.

## What the graph is allowed to change

**Only when it submits. Never what order it runs in.**

Reordering needs to understand the host code. Batching does not: it preserves
order exactly and changes only when the host observes completion. Correctness
then reduces to one sentence — *a node's producer is always recorded before the
demand that asks for it, so submitting everything recorded so far always includes
the producer* — and no stale read is expressible.

| policy | when it submits | when it waits | what it buys |
|---|---|---|---|
| `Serial` | immediately | immediately | nothing; this is today's kernel path |
| `Batch` | at the next demand | at the next demand | fewest submissions, no overlap — **refused, see below** |
| `Async` | as soon as a segment is recordable | at the next demand | host work overlaps device work, same submission count as `Serial` |

`Batch` and `Async` are a real trade-off, submissions against overlap. **`Async`
is the only one that rewards host logic in the graph**, because a pure launch
chain has no host work to hide. `Async` needed **no new backend capability**:
`submit` and `Pending::wait` were already enough, which is the strongest evidence
the two seams landed in the right place.

With that, submission count becomes a countable formula:

> **submissions = closure calls + segments**, where a segment is the run of
> launches between two closure calls.

A closure call is a full flush plus a readback, and the program wrote that cost
itself, so the JIT saves none of it. What it saves is everything *between*
closures: 16 launches around 3 closures is 4 submissions, not 16. **That formula
is the diagnostic worth more than the milliseconds** — "your chain has 3 closure
calls, so it is 4 submissions and not 1" tells a user something actionable.

## The graph IR

`lichen-graph-ir` depends on `lichen-kernel-ir` and nothing else. A graph is a
list of nodes in evaluation order, and **a node is one kind**: a **kernel** node
(a fragment over an index range). There is no fused node, no barrier node and no
sync node, and the reason is the point — what decides when the host observes
completion is the *policy*, not the graph. A graph that could name its own
synchronisation would be one whose correctness depended on where somebody put a
keyword.

**The host-call node was removed, and the reason is the model's sharpest point.**
It used to be a bare `fn` pointer, on the argument that a `fn` item cannot
capture, so a native node's environment is necessarily fixed when the `fn` is
named. But a user-written lichen closure cannot become a native node without
replacing that decision — `fn(&[&[i64]]) -> Vec<Vec<i64>>` has no channel to name
one. The answer is not a better pointer: **a closure is a compiled artifact like
any other, so it lowers to a fragment and is dispatched like one.** One kind of
node means one set of rules about what a node's environment is, and nothing a
trait object could make unsound behind it. It also removes, with the node: the
re-entrant apply path, the `Vec<i64>` against `[?b]` type gap, and the "type fact
against discipline" worry.

**Edges are `ValueId`s and the list is the topological order**, so a cycle is
unwritable. `Graph::push` refuses an edge naming a value that does not exist yet —
the only way a cycle could be expressed — and refuses an output count the node's
own body disagrees with, because that misaligns every value number after it.
`Graph::returning` records what the source function returns, once; recording it
twice is refused rather than merged, because a lowering that answers that question
twice has two opinions about its own function.

### What a run hands back, and why it is not the last node

The **whole value table**, not the last node's outputs. What a function returns
from the graph compiled out of it is the *function's* business, and
`Graph::returns` is where that answer is recorded. A runner that picked a node
would be making that decision for the caller, and a graph with a dead tail would
be unable to express its own return. An unrecorded graph takes **every** value it
has — the permissive answer, and the contract `graph_runs.rs` pins.

## The value table, and a number on an edge

```rust
Value::Pending { submission, id, count, class }  // submitted, device may not be done
Value::Device  { id, count, class }              // waited for, contents are there
Value::Host    { data, class }                   // host data, a graph's own input
Value::Int(i64)                                  // a number, so never pending
```

**`Pending` is a value, not a rule.** A value is pending for exactly as long as
the host has other work to do, and spending that window is the whole reason for
running a graph this way — so folding it into "a buffer" would either lie about
the contents or make every reader responsible for knowing whether it had to wait
first. Asking that of the type removes a class of bug rather than documenting it.
**A kernel node can consume a pending value**, because recording a dispatch
against a buffer only *names* it and the producer was recorded first; a demand
point is the match on `Pending`. A dispatch with several outputs shares one
submission, so the wait happens once however many values carry it.

**The class rides on the value**, and that is not decoration: the payload is the
class's elements packed at `ScalarClass::byte_width` bytes each, so a value that
dropped the class would hand a float buffer's four-byte elements to a backend that
reads eight. It is carried across the wait so waiting cannot change it.

**A number on an edge is what makes a dynamic count expressible.**

```rust
pub enum Count {
    Constant(usize),   // a number the build already knew
    Value(ValueId),    // a value in the table, read when the node runs
}
```

`KernelNode.count` used to be a `usize` decided at build time, on the reasoning
that the value table cannot hold a number — which argues *for* adding the scalar
rather than against needing one. A count that depends on data is ordinary (a
length off a `collect`), and a graph that cannot take one forces every such
program to rebuild the graph per run, which is the same as not having a graph.
`Value::Int` is `i64` rather than `usize` because a program's count can be
negative, which is a mistake to be told about by name rather than one to wrap
around into an enormous unsigned number. **A count is never pending** — a number
is not produced by a device — so a count edge never needs a wait.

`Constant` is not a wart on that: it is the one case where the build already had
the answer, and collapsing it into a value would mean inventing a third kind of
node — one that produces a number for free, doing no work. A node that *computes*
a number does not exist yet: a `KernelFragment` writes `outputs` buffers and
leaves `results` scalars on its own stack, and a node's outputs are the buffers.

**`Count` is the first *consumer* of a non-buffer value, and it must not grow
into being the general mechanism for one.** The two roles — a buffer input and a
count — are asked of the value table separately, and each is refused by name:
a count edge that resolves to data and a buffer input that resolves to a number
are different mistakes with different causes. The two filters return *what the
value is*, not a message, because a refusal about a demand has to name **the node
and the edge**, and only the runner holds those. A value is not a node and names
no edge, so no refusal raised inside a value could say them.

**A returned number has no node, and that is a decision rather than an omission.**
`CountNegative` names neither, because a number reaches a graph two ways — a
node's count edge, and a function's own return, which no node asked for. Putting a
node in that variant would mean inventing one. For the same reason, the readback
classifies a returned value by a **total match on `Value`**, not by asking for a
buffer and catching the refusal: that older shape made a number recognised by the
fact that it had been refused, and it would have broken outright the day the
refusal wanted to name a node. The two filters and that match cannot drift apart,
because all three are exhaustive matches over the same enum — adding a kind to
`Value` breaks every one of them at compile time.

## The builder: a graph is recorded, not read

**A graph cannot be built by reading a template.** The parameter struct is
readable, but its leaves — the count and the buffers — are `Parameterized`:
undecided cells until the function is applied, because nothing has applied it.
Walking the structure without evaluating gets the graph's **shape** and **none of
its values**, and the values are the part a run needs.

So `$graph(f)` **applies** `f` to a structure of placeholders — the parameter's
own shape, one leaf per cell — through the VM's own `Apply` path, and every
dispatch the body performs is intercepted on the way past. Nothing runs: an
intercepted dispatch appends a node and hands back a value number instead of a
buffer. The sequence of nodes **is** the sequence the body performed.

Three facts make that work:

- **The body contains no compute operator.** A native call is a
  `LowOperator::Apply`, and the operator lives in a **synthesized per-call-site
  function** the `Apply` enters. A lowering that searched the body would find
  nothing and record an empty graph from a program that dispatches — silently.
  The walk is therefore the body's `Apply` nodes, each one's callee, and the
  operator in the callee's body. That is *better* than what it replaced: the
  lowering sees the program's own calls, so it is generic over the surface and a
  new `$` op is a node rather than a special case. A callee can be **static**
  (frozen in a package) as well as dynamic; a static one has no body in this
  module and yields no operator.
- **The recording hands back a placeholder, not a buffer.** A `ParLaunch`
  normally hands back a `Buffer` (an arena payload) or a `DeviceBuffer`. A
  recording cannot hand back either: a buffer payload is arena data with a
  block's lifetime, and a resident id names device memory rather than a position
  in the value table. So there are two `ComputeValue` variants, and the pair is
  what the whole build is made of:

  | variant | carries | stands for |
  |---|---|---|
  | `GraphInput(slot)` | the parameter's slot | a value the caller supplies at `graphrun` |
  | `GraphValue(id)` | a value number | a dispatch this recording already recorded |

  **Both are invisible to the checker**, which is what makes them usable: the
  type at a `cfg` position is fixed at compile time, and nothing re-derives a
  type from a runtime value. Neither is a handle, so the copy path copies them
  rather than relocating a pointer, exactly as `DeviceBuffer` does. They are
  refused by the artifact codec, because a placeholder means nothing outside the
  recording that made it.
- **The chain needs no side table.** The placeholder is already sitting in the
  node the next `cfg` reads, carrying the value number with it: a second
  dispatch's input is a `GraphValue(v)`, and `v` is the edge. A `produced_by`
  map would be a second copy of the same fact that could disagree with the first.

**Where the walk finds the callee.** An `Apply` names its callee as the **first**
item of a `[callee, argument, _]` operand array, and the callee slot holds a
`value_of`-style `Index(pair, 0)` whose **own cached value is already the
function** — following that `Index` one step further lands on the pair rather than
on the function, so reading the node's own value is the whole of it. Nothing needs
evaluating: the compute operator node's own operand array is `[kernel, cfg]` and is
readable as it stands, which is what keeps a *build* from running a dispatch.

**Applying is sound, and the language gives the reason for free.** Every lichen
function has exactly one parameter and a graph function's body reads it, so what is
applied is exactly the structure the recording builds; the unrecordable case is a
body that dispatches a captured buffer.

**A graph holds no references at all.** The design once said the graph holds its
input buffers as *nodes*, and that this would be `traced`'s first real user.
Check what `traced` buys: `drop_block` removes **by block membership, not
reachability**, and `traced` protects a node across `garbage_collect` compaction
and gives it **no** protection against its block being dropped. So a graph
holding input nodes would be a use-after-free in some *later* `graphrun`, not an
error at build time. **The fix is to hold nothing**: the graph's inputs arrive
through the function's parameter, so the graph value is a `usize` naming a
registry entry of kernel ids, edge numbers and counts — all plain data, with no
block, no arena and no device lifetime in them.

### The invariant boundary the recording draws

**A `plrun` is the only operator that may see a placeholder**, and the check is
drawn in one place, in `OperatorExt::run`. The other two arms that could meet one
are a `collect` of a dispatch's own result (the point at which a `"gpu"` chain
crosses the bus) and a host `read` of one, and each is refused **only when it was
actually handed a placeholder** — a read of a host buffer the body closed over is
ordinary host arithmetic. A scalar `call` is refused outright rather than only
when it was handed a placeholder: **a scalar kernel has no node to be.** The one
node kind is a dispatch with its buffers bound to it, while a scalar fragment
works on its own operand stack and names no buffer whatsoever, so whatever it
computes has nowhere to travel.

The arms all failed the same way, so a boundary drawn in three arms would not be
a boundary; the rule is one sentence, which is worth more than three correct arms
that each know their own case.

### A graph is a transcript of the run, and the run is lazy

A body of three statements — dispatch, bind the result to a name nobody reads,
dispatch again — records a **two-node** graph. That was first read as a missing
dispatch. It is not: run the same body with no graph anywhere in the program and
**the backend receives two dispatches**. There is no expression-level CSE in this
compiler to explain the absence, so **the elimination is laziness, and it is the
program doing it, not the recording** — a `let` inside a block is lazy and nothing
read the name.

So **a graph is a transcript of the run, not of the source**, and a dispatch whose
result nothing reads belongs in neither. A walk that forced the rest would be a
bug, not a completion: it would put a dispatch in the graph that the program never
makes, and the graph would then answer with work its own author did not ask for.
The measurable form of the correct invariant is
`a_graph_dispatches_exactly_what_the_program_dispatches`: one body, run plainly
and through a graph, with the two dispatch traces compared — a count would have
been satisfied by a graph that dispatched the right things against the wrong
buffers; a trace is not.

**The closed gap, and what it cost to close.** The lazy walk reaches exactly what
the program performs; the one walk that reaches further is operand forcing, and it
empties the function's return slot on its own, so every recording refuses rather
than succeeding with an extra node. Both forcing knobs have since been deleted —
operand forcing was pure cost with no reader — so the lazy walk is the only deep
walk. A future change that repaired the return slot without noticing this would
reintroduce the bug the old count was protecting against.

**A statement nobody reads is not demanded, and so cannot be refused.** A block's
value is the tuple of its statements' values, and a statement whose value nobody
reads is never forced: an unused `collect` is not reached at all, so no refusal
speaks for it. The recording sees what the walk forces, and what the walk does not
force is not in the graph — which is why each operator a recorded body may not
reach for has to be nested *inside* the statement that is the body's value.

## The two seams

Both are logged in `crates/lichen-lowlevel`, and both are documented in their own
code. They are extension points for an operator that must keep something alive
past its own call — and the graph uses exactly one of them.

**`ValueExt::traced`** — a holder that keeps lowlevel **nodes** past the operator
call says which, so the GC walks them with the same `garbage_collect_node` it uses
for an array item:

```rust
fn traced(&self, context: &dyn TraceContext, out: &mut Vec<NodeId>)
```

Nothing could see such a holder otherwise: an operator's result is cached, so
**its operand is deliberately not followed**, and `drop_block` deletes by block
membership. The window is not distant, because `evaluate_block` collects at every
block boundary. **It is not enforced** — nothing checks the answer, because
nothing can: the lowlevel cannot see what a value holds, so an unlisted node is
not a detectable omission but a node that quietly disappears. The contract is
held by review and by `lichen-lowlevel/tests/basic/compaction.rs`. **It still has
no user, and the graph is not it** (see the builder above) — say that to anyone
writing the first real `traced`.

**`OperatorExt::run_deferred`** — `run` is handed its operand as a **value**,
which is right for an operator that answers from the value alone and wrong for one
that must decide for itself *when* its operand is evaluated:

```rust
fn run_deferred(&self, operand: Option<NodeId>, block: BlockId, module: &mut Module<P>) -> P::Value
```

The default reproduces the VM's former extension arm verbatim — deep pass,
`Parameterized` read-back, nullary `Error` stand-in — so an operator that does not
override it cannot tell the difference. **This is the seam the graph uses**: the
`Graph` operator decides for itself when its operand is evaluated.

**Overriding `run_deferred` without implementing `traced` is unsound** (a
reference kept past the call is invisible to the GC), and `traced` without
`run_deferred` is inert (nothing can hold a reference past a call). They ship as
one change, recorded in both doc comments.

Two shapes were rejected on the way, and both look like the answer: a
`&[NodeId]` instead of a callback demands the value hold its references as one
contiguous run, which no real holder has; and `&Module<P>` threads a program
parameter through the whole generic layer to buy reading a node's *value*, which a
holder keeping ids does not want. `TraceContext` is the narrow middle — node
block, block's node list, function's scope are all lowlevel's own types.

## How many buffers a fragment reads is not in its shape

`Runner::run` once checked
`kernel.inputs.len() == fragment.param_shape.flat_arity() - 1`, on the reading
that a parallel fragment's parameters are its input slots followed by the loop
index. **That reading is false**: a parallel fragment's `param_shape` is
`(config, index)` — two leaves — *however many inputs there are*, because a buffer
is bound as a storage buffer and reached through a read's position rather than
through a further parameter. The check was not merely useless, it was **rejecting
correct graphs**: a kernel that reads no buffer declares a two-leaf shape, so a
node with no inputs was told it was missing one — while a two-input kernel's
three-leaf shape made the coincidence read as agreement.

The fix is `KernelFragment::inputs`, the missing twin of `outputs`. The two counts
answer different questions:

- **How many was it given** is the dispatch's, read at apply time from the
  call-site parameter struct.
- **How many does it need** is the fragment's, and it is a *max*, not a tally:
  the read positions are a sparse space, so a body reading only the second input
  still needs two buffers bound or the one it read was never bound.

`inputs` is counted by the emitter as it emits the read positions, so it cannot
disagree with the body, and it is hashed by `fragment_digest` for the same reason
`results` is. **The non-graph path needed it too**, since it had no check at all
and would have handed a shader a binding its body never reads.

## A recording numbers things in its own space

Four of the bugs the recording half found are one confusion: a recording numbers
values in its own space and a graph numbers them in another.

- **A recorded count and a recorded edge land in different value spaces.** A
  `Recording` numbers its produced values from zero, because the input count is
  not known until the walk ends; `finish()` puts that count in front of them, and
  an `Input` edge is already final. Storing the count as a resolved `ValueId` and
  shifting it in `finish()` moved a parameter-read count onto the node's own
  output — a count of `0` became `1`, a value that exists, so the graph ran and
  dispatched over a buffer's length. The count now carries an `Edge` too, and
  `finish()` has **one** `resolve` for edges, counts and the return alike; a
  second resolution point is where the two id spaces get confused again.
- **A block's value is wider than what the function returns.** The block's value
  is the tuple of its statements' values, and its tail is machinery, not results.
  `compile_fragment` and `parallel_output_nodes` both resolve a return by reading
  `array_items(body)[0]`; the recording was the third reader and the only one that
  guessed.
- **An unreadable return is a refusal, not an unrecorded one.** Leaving it
  unrecorded looks permissive, but the table holds the count and the buffers, so
  the graph answered `"[3, <buffer>]"` to a program that asked for one of them.
  The permissive reading of an unrecorded return still belongs to a graph nobody
  recorded — `Graph::returns()` is `None` for a hand-built graph — but it is not a
  contract a failed recording may lean on. This is also what forced a **returned
  extent** into the open: a returned count is an *input* value, so the return has
  to say which placeholder kind it is, and `RunResult` grew a `Count` arm because
  a graph that returns a number has to hand it back.
- **`LowShape::USize` is not a `usize`.** The extent's conversion belongs to
  whoever is about to dispatch over `[0, count)`, and a count arrives as a node's
  edge *or* as the function's own return. The conversion now happens once in each
  of those two places, which **deletes two `as usize` casts rather than guarding
  them** — a negative count would have become an enormous length.

## How much fusing is worth

Two measured numbers decide the design, and both are on the development machine
(see [lichen-compute-gpu](lichen-compute-gpu.md) for the full tables):

- **A dispatch's device time is a fixed floor** — about 0.036 ms against a
  resident input, 0.043 ms against a host one — and a kernel inside it is not:
  the per-link cost moves only from ~0.04 ms to ~0.12 ms across a 1024× count
  range. So the floor is ~92% of a 16-link chain's link at a thousand elements
  and ~30% of one at a million. **Fusion is worth 65–81% of a 16-link chain at
  counts up to 65 536, and ~10% at a million** — and the million-element row,
  which every early estimate used, is the one least favourable to it.
- **Async's saving is one submission's device time and no more** —
  `min(host work, device time)`, saturating at the device's own time. So it earns
  **nothing on a chain of pure kernels** (there is no host work to hide) and up to
  a dispatch's worth per node on a graph with host nodes in it. That is a
  different quantity from the Batch ceiling above, and the two schedules are not
  alternatives: `Batch` removes submissions, `Async` removes waits.

**Consequences for the design, stated plainly:**

- `run_chain` records a chain into one command buffer, submits once and waits
  once, and it is faster at every count: 5.7× at 1 024 elements, 3.0× at 65 536,
  1.23× at a million. It is the **Batch** case at depth one.
- The chain is checked against the kernel's **closed form** (`2^n * x + (2^n − 1)`),
  derived rather than run, because a second CPU copy of the same loop would agree
  with a mis-ordered chain just as cheerfully.
- **A kernel-only graph under `Serial` or `Async` is worth no milliseconds**, and
  the measurement says so rather than implying otherwise. So the lowering is worth
  building for reasons that are not speed: it is the prerequisite for specifying
  the fused-submission shape at all — "hand me N dispatches" cannot be specified
  without knowing what a node is — and it produces the repeatable object the
  feature was decided around.
- **There is deliberately no minimum-count gate.** The measurement says the answer
  is not a count: 65 536 elements loses at every chain length while 1 048 576 wins
  from four links on.

## Refusals, each naming its cause

The graph's own refusals are `GraphRefusal`, and one text covers the boundary:
`graph::describe` names what a value *is* (`a placeholder for a graph's own
input`, `a placeholder for a value the graph has produced`) so a refusal about a
role can say which one it found.

| refusal | when |
|---|---|
| `EdgeBeforeItsProducer { node, value, defined }` | a node edge names a value the graph has not produced yet — the only way a cycle could be written |
| `OutputCount { node, declared, claimed }` | an output count the node's own body disagrees with, which misaligns every value number after it |
| `ReturnAlreadyRecorded { recorded }` | a lowering answered what its function returns twice |
| `RunArity { wanted, got }` | the run was handed the wrong number of arguments |
| `UnknownValue { node, value }` | a value number that names nothing in the table |
| `NotBufferData { node, value, found }` / `CountNotANumber { node, value, found }` | a demand asked for the wrong role; the runner assembles it so it can name the node and the edge |
| `CountNegative { number }` | a count below zero — naming neither node nor edge, because a number also reaches a graph as a function's return, which no node asked for |
| `PolicyUnsupported { policy, reason }` | `Batch`, because the backend contract cannot record several dispatches into one submission |
| `Backend { what, reason }` | the backend declined |
| `this graph dispatches to a device, but no compute backend is installed…` | a run with no `ParallelBackend` installed |
| `this graph was compiled for the "cpu" backend, and a graph runs through the `ParallelBackend` contract…` | a `"cpu"` graph: the cpu path is this crate's own wasm engine, driven by a compiled module rather than by a submitted run |
| a dispatch's count is … | the extent is neither a literal nor one of the function's arguments |
| this dispatch's kernel parameter declares N runtime scalars … | a recorded body carries the extent alone; a runtime scalar would have to be an edge ([compute-runtime-scalars](compute-runtime-scalars.md)) |
| a dispatch was recorded with no recording in progress / a return was recorded with no recording in progress | the recording boundary was crossed |

**Mixed backends** are a decision the design records rather than a check it
implements: a run goes to one `ParallelBackend`, and the IR's nodes carry none, so
a graph whose dispatches named different backends could not be run at all — refuse
at build time and name both. The same shape applies to a `cfg` item that is
neither a dispatch's output nor an input slot.

## The recorded body's parameter

**A recorded body's parameter is the named struct**
([compute-buffer-wrapper](compute-buffer-wrapper.md)): the placeholders are the
parameter's own cells — a buffer field's cell is a `Buf` around its `GraphInput`,
a scalar's is bare — a run is handed the cells its dispatches read, and the graph
hands back the body's return, a bare `Buf` when it is one dispatch's result.
`assemble_parameter` builds the placeholder from the declaration and `build_graph`
prefers it, falling back to the path walk and refusing by name when neither yields
a tuple; that is what fixed the silent loss an empty group before a later role path
used to cause.

**The unannotated parameter is still supported, and one test depends on it.**
`build_graph` falls back to the flat ceiling when a recorded body's parameter is
not a named struct, which is what an unannotated `step = ins => …` gets — so a
body that reads `ins(0)` still records and runs. That form is not leftover
spelling: the named struct types the count, so a closed-over buffer in the count
position becomes a *type* error and the graph's own count filter never speaks,
which is the property
`graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one`
pins.

## Checked, on a stub and on a device

**The schedule is checked on a stub, because only a stub can say when the host
waited**: the stub records what it was asked for and in what order, and `run` and
`submit` are told apart because that is exactly what a policy chooses between. A
two-node chain under `Async` submits both nodes and waits **once, at the end**;
under `Serial` every `run` is waited for before the next.

**The arithmetic is checked on a real device**, in `graph_on_device.rs`, because
the stub computes `sum(inputs) + 1` whatever the body says. Two tests: a two-node
chain (the second node recording against a resident id the device has not finished
writing), and a graph whose extent is one of its own arguments, run at three
extents with the data deliberately longer than the count. Both compare against a
**hand-derived expected vector**, and the second is the one a device is worth
having for: the count decides how many elements are allocated, dispatched and read
back, so a count read from the wrong slot would answer with a wrong *length*, not
merely a wrong number.

**And one thing that run does not check, stated so it is not read as a check.**
Neither fragment on the device has `fragment.inputs` disagreeing with what it
reads; the divergence is a stub-side fact, pinned in `refusals.rs` and
`graph_runs.rs`. A device is not where it can be observed, and a green device run
is not evidence about it.

**The backend slot is process-wide, so the stub-backed tests own their binary.** A
test that installs a `ParallelBackend` changes what every other test in the same
binary sees; a second test needing *no* backend, or a real device, would race it,
and the failure would be a **number** rather than an error — the worst shape a test
failure can have. Tests that share one stub take a lock for their whole length,
which is what makes the dispatch log a fact about one test rather than about which
tests the scheduler ran first.

**A body-ignoring stub is the right oracle, because the plumbing is the subject.**
The stub computes `sum(inputs) + 1` whatever a fragment's body says: a fragment's
arithmetic belongs to [lichen-compute-gpu](lichen-compute-gpu.md) on real hardware,
while what these tests ask is which values reached which node — so a graph wired
wrongly answers with *different* numbers rather than the same ones by luck.

## Not yet

- **`Batch`.** Needs a fused-submission capability the contract does not have, and
  is refused by name until it does. `run_chain` is that fusion at depth one.
- **The general builder.** The `compute.graph` operator, the `Graph` value and the
  lowering exist; what remains is the shape the design records for a graph that is
  not a linear chain of one fragment — a graph's fan-out is not that.
- **A node that computes a number.** A scalar kernel has no node to be, so a count
  today is either a build-time literal or a value the caller passed.
- **`traced`'s first real user.** The graph is not it; see the builder.

## Landmines, each of which is a silent wrong answer

- **A recorded-but-unsubmitted command buffer is clobbered by the next recording
  into it**, and a command buffer whose submission is still in flight cannot be
  recorded into at all. This is the whole reason the pool of submission slots
  exists and the reason a slot is not released until its fence signals. See
  [lichen-compute-gpu](lichen-compute-gpu.md).
- **A chain that reuses a buffer has a write-after-read hazard the
  one-buffer-per-link version does not have.** `run_chain` ping-pongs two buffers,
  so link `i` reads what link `i + 2` writes, and the trailing barrier is widened
  to order it. The version that allocated one buffer per link was three times
  **slower** than not fusing at all, all of it in `vkAllocateMemory`.
- **`BufferSlot::Host` inputs are staged by `memcpy` before recording.** Reserve
  for the whole submission up front, and never share staging between slots.
- **An uninitialised output buffer is safe only because a write is reached by every
  invocation.** A selection arm is fine, because it runs on exactly the lanes that
  took it; a **loop body** is not, because a zero-trip count is a lane that never
  runs it, so a write inside a loop is **refused by name** rather than emitted.
  See [lichen-compute-gpu](lichen-compute-gpu.md).
- **A `DeviceBuffer` value dropped by `drop_block` never calls `release`.** Its
  memory lives until `GpuContext::drop`.
- **A count is a value, and a value asked for the wrong role must be refused
  rather than coerced.** The tempting repair for a number in a buffer position —
  treat it as a one-element host vector — is a run that succeeds on a kernel nobody
  wrote. The two roles are separate functions.
- **A graph is a transcript of the run, and the run is lazy** (see above). A graph
  that dispatched *more* than its program would be running work nobody asked for,
  and no refusal would catch it.
- **A graph that captures anything is a use-after-free waiting for a
  `drop_block`.** Nothing in the type says so, because `Graph` is plain data and a
  capture is what the *builder* would have done. The refusal is the only thing
  standing between a program and a graph that reads a freed arena on its second
  run, so it is a build-time refusal and not a run-time check.
- **A fan-out graph's memory profile inverts; a linear chain's does not.** A
  16-link chain at a million elements holds two buffers, not sixteen. The inversion
  is real only where two buffers are live at once — a node with two consumers, or
  two nodes writing from the same input — and there the graph's liveness
  information is the only thing that decides what can be shared.

## Related

- [lichen-compute-gpu.md](lichen-compute-gpu.md) — the backend, the pool of
  submission slots, the measured costs, and the invariants a graph must not break.
- [compute-buffer-wrapper.md](compute-buffer-wrapper.md) — the recorded body's
  parameter and the `Buf` algebra.
- [compiler-plugin.md](compiler-plugin.md) — the `traced` seam as a plugin author
  sees it is **not written yet**; its "Extension point 5" covers the ext-handle
  payload contract, which is a different thing.
- [lichen-compute.md](lichen-compute.md) — the operator vocabulary and the
  `plrun` path a graph sits beside.

## Recovered measurements

**The named-parameter spelling is part of the refusal, not a transcription of it.**
Under `struct<.n Int, .in In1>` a dispatch returns the parameter's `.out` group, so
the offending operator has to name the **buffer inside** it (`first.z`); handing it
the dispatch's result says "this is an array", which is the `compute.parallel`
refusal rather than the graph-recording one. Reaching the rule also needed the
placeholder scan to see through the `Buf` wrapper the recorder builds around an
output path — the operator names the wrapper, so asking only whether an operand
*is* a placeholder answered `false` for exactly the case the rule exists to catch
([compute-buffer-wrapper](compute-buffer-wrapper.md)).

**A graph is content-addressed on its shape, not on its backend.** The backend is
deliberately **not** hashed into `fragment_digest`, so a `"cpu"` and a `"gpu"`
recording of one body intern to a single registry id — and the registry entry must
therefore not carry the first recording's backend, or a `"gpu"` program is refused
with a message naming `"cpu"` for a program that never says it. The registries are
process-global, so before the fix a process that had ever built a `"cpu"` graph of a
shape could never run a `"gpu"` graph of it. The refusal arrives *before* any
dispatch, so a run that reaches the stub at all is the proof.

**Three operators, three refusals, one per repair.** A `collect` of a dispatch's
result, a host `read` of one, and a scalar kernel are three separate refusals with
three separate messages, because the repair differs for each. All three once fell
through to a bare hole with no diagnostic at all, and a body that collected a
dispatch's result mid-chain recorded a graph quietly missing the collect, with the
chain's own numbers looking right anyway — a hole is the worst of the three
outcomes rather than the smallest.

**Why the recording is a deep walk.** A *shallow* evaluation of a recorded body
produces the block's value tuple and stops: the statements inside it have not run,
and the recording taken there is an empty graph that still looks like one. The deep
pass is what demands the tuple, and demanding it is what performs the dispatches.
Forcing was tried anyway (`Module::evaluate_node_forced`): it performed every
statement *and* left the function's return slot empty, so every recording refused,
including bodies with no unread statement at all. The empty slot isolated to the
operand forcing rather than to the shallow descent — a walk descending every
position in order (`skip_shallow` off, `force_operand` off) recorded the same two
dispatches, while `force_operand` on alone emptied the return slot with the shallow
mask untouched.
