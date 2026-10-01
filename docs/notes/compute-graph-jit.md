# Graph JIT: a chain of dispatches as one submission

> Status: **the IR and the seams are in and tested; the lowering is decided and
> not written.** The two lowlevel seams, the graph IR crate, and the return
> recording all exist and are tested. The half that builds a graph — the
> `compute.graph` operator, the `Graph` value, and the recording that fills
> `Graph::push` — is designed below and unwritten. Branch `feature/graph-jit`,
> not pushed.
>
> This note exists so the next session does not re-derive any of it. Most of
> what is here is a decision *and the alternative that was rejected*, because
> the alternatives were not obvious and several of them were tried.

## What this is

`compute.graph` JITs an ordinary lichen function into a **graph IR** — a DAG is
the natural shape of that IR, not a special case detected in the source — and
then optimises on the graph. What the graph returns is decided by the function's
return. Graph-level optimisation (recording a whole chain into one command
buffer, one submit) does **not** belong to the plain kernel-JIT path.

The premise is already in place: `ResidentId` lets an intermediate result stay
on the device, and the recycled-buffer pool is the same pool scoped by *graph*
rather than by *run*.

## The decision the whole design rests on

**A native closure inside a graph function can only reach variables that existed
before the graph JIT ran.** It cannot capture a value the graph itself produces.

That single fact is why this is tractable:

- a closure's dependency set is fixed and known before the graph runs, and lies
  **entirely outside** the graph;
- no edge leads from a graph node into a closure;
- a closure takes nothing from inside the graph.

So **all the graph's edges are statically known**, scheduling is real
scheduling, and any topological order is correct. The closure is a node in the
same graph as a kernel, scheduled the same way, with **no special requirement on
it at all**. It is not a barrier, and it does not force a flush.

**A correction to how that is written below.** The rule is about a native node's
**capture**, and the rest of this section once extended it to its **arguments**:
"no edge leads from a graph node into a closure; a closure takes nothing from
inside the graph". Taken literally that leaves a native node unable to transform
anything, which is most of what a host call is for. The soundness invariant is
the first part; arguments are graph values and the edges into them are ordinary.
`lichen-graph-ir` builds both kinds of node on that reading, and the cost of the
distinction is written down where it lands: **a native node that reads a
dispatch's output has to wait for it and fetch it**, and no arrangement of
submissions removes that. The shape that pays is a native node whose inputs
already existed before the run.

And the second thing falls out of that: **a closure can run while a submission
is in flight**, because it touches nothing the device is writing. That is the
only thing about native logic in a graph that is worth money — and it is the
answer to "native logic in the graph makes it complicated". It is not
complicated; it is the part that pays.

The corollary that must be written down wherever a graph exists: **if a closure
form ever appears that can capture a post-launch value, the scheduler becomes
unsound, silently.** Same class of obligation as `spirv`'s single-`OpLabel`
invariant — see [lichen-compute-gpu.md](lichen-compute-gpu.md).

## What the graph is allowed to change

**Only when it submits. Never what order it runs in.**

Reordering needs to understand the native code. Batching does not: it preserves
order exactly and changes only when the host observes completion. Correctness
then reduces to one sentence — *a node's producer is always recorded before the
demand that asks for it, so submitting everything recorded so far always
includes the producer* — and no stale read is expressible.

That gives three submission policies, and the "insert a hint" knob is the thing
that picks between them per segment:

| policy | when it submits | when it waits | what it buys |
|---|---|---|---|
| `Serial` | immediately | immediately | nothing; this is today's kernel path |
| `Batch` | at the next demand | at the next demand | fewest submissions, no overlap |
| `Async` | as soon as a segment is recordable | at the next demand | host work overlaps device work, same submission count as `Serial` |

`Batch` and `Async` are a real trade-off, submissions against overlap. **`Async`
is the only one that rewards native logic in the graph**, because a pure launch
chain has no host work to hide.

With that, submission count becomes a countable formula:

> **submissions = closure calls + segments**, where a segment is the run of
> launches between two closure calls.

A closure call is a full flush plus a readback, and the program wrote that cost
itself, so the JIT saves none of it. What it saves is everything *between*
closures: 16 launches around 3 closures is 4 submissions, not 16. **That formula
is the diagnostic worth more than the milliseconds** — "your chain has 3 closure
calls, so it is 4 submissions and not 1" tells a user something actionable.

## Two seams, and they are only sound together

Both landed on `feature/graph-jit`.

### Seam A — `ValueExt::traced` (`82a7948`, `c5de5dc`)

```rust
fn traced(&self, context: &dyn TraceContext, out: &mut Vec<NodeId>)

pub trait TraceContext {
    fn node_block(&self, node: NodeId) -> Option<BlockId>;
    fn block_nodes(&self, block: BlockId) -> Option<&[NodeId]>;
    fn function_nodes(&self, function: FunctionId) -> Option<&[NodeId]>;
}
```

A compiled graph holds the **buffer values it will read on every run**, so its
value holds **nodes** past the operator call that built it. Nothing could see
that: the GC walks an array's items, a table's entries, a function's scope, and an
*unevaluated* node's operand — and an operator's result is cached, so **its
operand is deliberately not followed** (`gc.rs`, "a cached value means the node
is memoized and its operand is dead"). `drop_block` then deletes by **block
membership, not reachability**, so such a node is dropped at the end of the very
block evaluation that produced it, with no diagnostic. `evaluate_block` calls
`garbage_collect` on every block boundary, so that window is not distant.

`traced` closes it, and the GC walks what it names with the **same
`garbage_collect_node` it uses for an array item**.

**It is not enforced.** Nothing checks the answer, because nothing can: the
lowlevel cannot see what a value holds. An unlisted node is not a detectable
omission, it is a node that quietly disappears. The contract is held by review
and by `lichen-lowlevel/tests/basic/compaction.rs`.

**This seam still has no user, and the graph was supposed to be it.** Reading
`gc.rs` while designing the lowering is what settled that it is not: a graph
holding input *nodes* would be relying on this for survival across a
`drop_block`, and this protects against compaction only. The graph therefore
holds no references, and this seam waits for whatever genuinely needs it. Say
this to anyone writing the first real `traced`.

### Seam B — `OperatorExt::run_deferred` (`82a7948`)

```rust
fn run_deferred(&self, operand: Option<NodeId>, block: BlockId, module: &mut Module<P>) -> P::Value
```

`run` is handed its operand as a **value**, structure already collapsed. That is
right for an operator that answers from the value alone and wrong for one that
must decide for itself *when* its operand is evaluated: it already has
`&mut Module<P>`, and the one thing it lacks is the node to start from. The
default reproduces the VM's former extension arm verbatim — deep pass,
`Parameterized` read-back, nullary `Void` stand-in — so an operator that does not
override it cannot tell the difference, and the VM's arm is now one line.

`is_callable(module, callee: AnyNodeId)` was the existing precedent for handing
an operator a node.

**Overriding `run_deferred` without implementing `traced` is unsound** (a
reference kept past the call is invisible to the GC), and `traced` without
`run_deferred` is inert (nothing can hold a reference past a call). They ship as
one change. Recorded in both doc comments.

## Three dead ends, kept because they look like the answer

**`&[NodeId]` instead of a callback.** Rejected: it demands the value hold its
references as one contiguous run, and no real holder has that shape. A graph
interleaves the nodes it keeps with kernel ids, counts and element data, so there
is no slice of it that is "the node list". Hence `out: &mut Vec<NodeId>` — the
GC's own scratch, not the value's.

**`traced(&self, visit: &mut dyn FnMut(NodeId))`.** Rejected: a holder that keeps
a node *because of where that node sits* has to be able to look, and a callback
into a walk cannot let it.

**`&Module<P>` in the signature.** Tried, and it is the obvious fix for the
previous point and the wrong one here. `P` on `ValueExt` reaches
`ValueType: ValueExt + ...` in the highlevel, and from there into every
`V: ValueType` bound in the checker — a program parameter threaded through the
whole generic layer to buy the one thing it adds, reading a node's **value**,
which a holder keeping ids does not want. `TraceContext` is the narrow middle:
node block, block's node list, function's scope are all spelled in lowlevel's own
types, so nothing that matters needs `P`. Read-only also removes the
naming/walking borrow overlap for free.

**One more, on the currency.** The hook was first written over `LowValue`. The
GC's unit of work is the **node** — it asks "which node?" at every edge and
dispatches on shape one level down — so a value is not an address and a node is.
And "function" is not a separate kind at this seam: a closure is kept alive by
naming the node it is the value of.

## Corrections to measurements, all of which changed a conclusion

These are the expensive ones. Each was a wrong number that had already been
written into a document or a plan.

**Submit fusion is not an N-fold win.** A dispatch's round trip — record,
submit, wait — costs 0.036 ms against a resident input (best of 200), and
earlier in this work "N-fold on long chains" was asserted. It is wrong, and
the sweep below says by how much.

**Readbacks are not avoidable.** A `Read` the program performs needs the number
on the host, so the data must cross. There is no "unnecessary readback" to fold
away. An earlier claim that a graph could save one, worth 0.5–1 ms, was
invented — the prize does not exist.

**The first sweep was divided by the wrong floor, and said so by being
impossible.** The sweep's first run reported links that were 105–108%
overhead. A fraction over 100% is not a noisy measurement, it is a wrong
divisor: the floor came from `fixed_cost`, which times a dispatch reading a
**host** input, and that one `memcpy`s into staging and has the device copy it
inside the same submission. A chain link after the first reads a **resident**
buffer and pays neither, so dividing by the host-input floor credits the link
with an upload it never did — at small counts, with more than the link costs.

**Neither floor is a constant, and the tables below were divided by one run's
value.** The host-input floor has been measured between 0.043 and 0.045 ms and
the resident-input one between 0.036 and 0.041 ms across runs of the same
binary on the same machine, so an upload is worth roughly 0.007 ms and no more
precisely than that. Every *per link*, *fusion share* and *overhead* figure
below comes from the 0.036 ms run specifically; a run that measured 0.041 would
report the same chain costs with about 12% less apparent overhead. The chain
times themselves are what the tables are for, and those are stable.

**The 10% was the worst row of the table, not its typical one.** Every estimate
so far computed fusion's value at 1 048 576 elements, because that was the only
count the crossover example happened to chain. The sweep at fixed chain length
(`b4d89c1`, 16 links, best of 20) says:

| count | 1 link | 16 links | per link | fusion share | overhead |
|---|---|---|---|---|---|
| 1 024 | 0.080 ms | 0.675 ms | 0.040 ms | **80.9%** | 91.9% |
| 4 096 | 0.086 ms | 0.681 ms | 0.040 ms | 80.2% | 91.7% |
| 16 384 | 0.111 ms | 0.731 ms | 0.041 ms | 74.7% | 88.1% |
| 65 536 | 0.214 ms | 0.846 ms | 0.042 ms | 64.6% | 86.4% |
| 262 144 | 0.814 ms | 1.555 ms | 0.049 ms | 35.1% | 73.7% |
| 1 048 576 | 3.298 ms | 5.096 ms | 0.120 ms | 10.7% | 30.4% |

*per link* is `(16 links − 1 link) / 15`, a difference of two chain lengths so
that the upload and the download cancel. *fusion share* is
`15 × 0.036 ms / 16 links`. *overhead* is `0.036 ms / per link`.

The shape of the table is the answer, and it is the shape the design predicted:
**the per-link cost barely moves with count** — 0.040 ms to 0.120 ms across a
1024× range — while the kernel inside it grows with count, so the same fixed
round trip goes from 92% of a link to 30%. Fusion is worth **65% to 81% of a
16-link chain at counts up to 65 536**, and 10.7% at a million.

**So: worth building.** The case for it is chains of small and medium kernels,
which is what a real pipeline is made of, and the 10% figure that was quoted
until now was the single row least favourable to it.

### And then the feature was built, and it hits that number (`519c9d1`)

`run_chain` records a chain into one command buffer, submits once and waits once.
The *fusion share* column above was arithmetic done before the feature existed;
this is the feature:

| count | unfused 16 links | fused 16 links | speed-up | predicted | actual |
|---|---|---|---|---|---|
| 1 024 | 0.646 ms | **0.113 ms** | **5.7×** | 86.3% | 82.5% |
| 4 096 | 0.653 ms | **0.120 ms** | 5.4× | 85.4% | 81.7% |
| 16 384 | 0.681 ms | **0.146 ms** | 4.7× | 82.0% | 78.5% |
| 65 536 | 0.801 ms | **0.266 ms** | 3.0× | 69.7% | 66.8% |
| 262 144 | 1.625 ms | **1.091 ms** | 1.5× | 34.3% | 32.8% |
| 1 048 576 | 5.752 ms | **4.669 ms** | 1.23× | 9.7% | **18.8%** |

*actual* is `(unfused − fused) / unfused`. Fusing is faster at every count, and
*actual* lands within a few points of *predicted* — **below** it at the small
counts, where the fused path's own barriers and per-dispatch set allocations are
paid, and **above** it at a million, where removing them shows up because the
0.036 ms floor never counted the host-side work either. The model is
conservative, which is the direction a model should be wrong in.

**Correctness is checked, not assumed.** The example verifies a fused chain
against the kernel's closed form, `2^n * x + (2^n − 1)` — derived, not run,
because checking against a second CPU copy of the same loop would agree with a
mis-ordered chain just as cheerfully. Four shapes pass, the largest a 16-link
chain. That is the first execution of the two rules below, and they hold.

**What the sweep does not settle.** The *actual* column is a **Batch ceiling** —
it assumes fifteen submits *and* fifteen waits all disappear, which is exactly
what `run_chain` does. Async keeps the submits and removes only the waits, so it
collects part of this. That part is measured below.

### And how much of it Async actually reaches

The one measurement the design was waiting on. Each row is **the same submission
and the same host work twice**, once with the work before the wait and once
after it; nothing else differs, so the difference in `wait` is everything
overlapping bought and nothing else is in the answer. The host work is a CPU
pass over the same number of elements the kernel has, which is what a node
written in lichen rather than compiled to a kernel actually is.

| count | passes | host | submit | wait after | wait before | hidden | predicted |
|---|---|---|---|---|---|---|---|
| 262 144 | 0 | 0.000 | 0.072 | 0.199 | 0.198 | 0.001 | 0.000 |
| 262 144 | 1 | 0.064 | 0.072 | 0.198 | 0.133 | 0.064 | 0.064 |
| 262 144 | 2 | 0.126 | 0.072 | 0.198 | 0.071 | 0.127 | 0.126 |
| 262 144 | 4 | 0.214 | 0.074 | 0.200 | 0.002 | 0.198 | 0.200 |
| 262 144 | 8 | 0.398 | 0.075 | 0.197 | 0.002 | 0.196 | 0.197 |
| 1 048 576 | 0 | 0.000 | 0.266 | 0.747 | 0.752 | 0.000 | 0.000 |
| 1 048 576 | 1 | 0.339 | 0.444 | 0.758 | 0.403 | 0.355 | 0.339 |
| 1 048 576 | 2 | 0.647 | 0.317 | 0.801 | 0.100 | 0.701 | 0.647 |
| 1 048 576 | 4 | 1.149 | 0.510 | 0.829 | 0.006 | 0.824 | 0.829 |
| 1 048 576 | 8 | 2.216 | 0.509 | 0.929 | 0.007 | 0.922 | 0.929 |

*hidden* is `wait after − wait before`; *predicted* is `min(host, wait after)`,
because a wait cannot be cut below zero and cannot be cut by more than the device
was busy for. Best of 20, whole runs.

**The shape is the answer, and it is the shape that was predicted.** The *wait
after* column is flat — that is the device's time and host work does not touch
it — while *wait before* collapses to two microseconds. And *hidden* tracks
*predicted* to within 0.05 ms in all ten rows, the two at a million being the
loosest. So **the whole device time can be hidden**, and once the host work
exceeds it the wait is free and further host work is fully exposed. The
crossover is at host work ≈ device time: about 0.2 ms at 262 144 elements, which
is one to two CPU passes, and about 0.8 ms at a million, which is two.

**A first version of this table was wrong in a way worth recording.** It took the
best of each of the three pieces separately, and the hidden column came out at
0.479 ms against 0.353 ms of host work — more was hidden than there was to hide.
The three pieces are a decomposition of *one* elapsed time, so the best wait
generally comes from a different run than the best host work, and pairing them
measures an overlap that never happened. **A hidden amount cannot exceed the work
that hid it**, and a table that says otherwise is telling you about its own
arithmetic. Reporting all three from the run with the lowest total is what fixed
it, and the check that the two columns must agree is what would have caught it
earlier.

**What it does not say.** This is one machine, one kernel, two counts, and best
of 20 rather than a median, so read the *shape* rather than the digits. And
*wait after* is not perfectly flat at a million — it drifts 0.747 to 0.929 down
the table — so the last two rows' *predicted* is partly tracking that drift
rather than a clean ceiling. The quarter-of-a-million block, where it is flat to
0.003 ms, is the one to believe.

**The one cost that grows is the submit side.** At a million elements it goes
from 0.27 ms with no host work to about 0.51 ms with four passes over the same
data, because a pass that size evicts the input the next submit has to copy into
staging. It is flat at 262 144, where the pass does not. This is non-monotonic
row to row, so it is a cost to watch rather than a number to plan against — but
**Async keeps every submit**, so whatever it grows into comes straight off what
the overlap wins, and at these sizes the overlap still wins by more.

**The consequence for the design, which is the point of having measured it.**
Async's saving is not a share of the per-dispatch overhead; it is **the device's
time for one submission, and no more**. That is a completely different quantity
from the 65–81% the Batch table reports, and it settles the question those tables
could not: on a chain of pure kernels Async earns **nothing** — there is no host
work to overlap, which is why the design said so from the start — and on a graph
whose nodes are *not* all kernels it earns up to one dispatch's worth of device
time per node, once the host work in that node exceeds it. The two schedules are
not alternatives to pick between; Batch removes submissions, Async removes waits,
and a graph with native nodes in it is the only shape where Async is worth
anything at all.

## What is built and what is not

**Built and committed** (`feature/graph-jit`, eighteen commits, not pushed):

| commit | what |
|---|---|
| `82a7948` | both seams, plus the compaction test |
| `5b881bb` | delete a false claim from the `traced` docs |
| `c5de5dc` | callback → `TraceContext`; why `&Module<P>` is wrong |
| `a2c661d` | this document, and the stale claims in the backend's |
| `05b5402` | `record_and_submit` / `wait_on` split out of `record_and_wait` |
| `6c5ac4d` | the two rules a fused submission depends on — see below |
| `f2e429f` | the same two rules, recorded here; delete an N-fold claim |
| `91a324c` | delete the single `detached` submit the new design made wrong |
| `e552a36` | the segment must not cross a call boundary, because it cannot |
| `b4d89c1` | the count sweep, and the wrong floor it was first divided by |
| `65582db` | the sweep says build it, and says 10% was the worst row |
| `519c9d1` | `run_chain` — a chain in one submission, verified and measured |
| `751cd2b` | the fused chain is checked and faster everywhere |
| `de5424c` | the pool of submission slots, configurable and defaulting to 2 |
| `082c4d7` | `Pending` on the backend contract — a submission handed back unwaited |
| `ad7f97a` | the submit/wait split, measured |
| `5b58b78` | `lichen-graph-ir`: the graph, its node, and a runner |
| `7447fc0` | a graph records what its own function returned |
| `781ed18` | the lowering's design, and the native-node contradiction it uncovered |
| `7347bff`, `ebfa982` | a probe of the real node structure, which corrected the lowering's walk |
| *this one* | what a recorded dispatch hands back, and what a graph holds |

**Not built:** the lowering that compiles a lichen function into this graph, the
`compute.graph` operator, the `Graph` value in the language, and the
fused-submission capability `Batch` needs. The lowering's design is settled and
recorded — see
[The half that builds a graph](#the-half-that-builds-a-graph-and-what-it-found).

**Measured:** the count sweep, the fused chain it predicted, that two
submissions can be in flight and chained at the same time, how much of a
submission's device time the host can be busy across, and that a graph with both
kinds of node produces on a real device what the same fragments produce outside
one.

## The graph IR, and its one kind of node

`lichen-graph-ir`, depending on `lichen-kernel-ir` and nothing else. A graph is a
list of nodes in evaluation order, and a node is one of exactly two things: a
**kernel** node (a fragment over an index range) or a **native** node (a host
call). There is no fused node, no barrier node and no sync node, and the reason
is the point: what decides when the host observes completion is the *policy*,
not the graph. A graph that could name its own synchronisation would be one whose
correctness depended on where somebody put a keyword.

**Edges are `ValueId`s and the list is the topological order**, so a cycle is
unwritable. `Graph::push` refuses an edge naming a value that does not exist yet
— which is the only way a cycle could be expressed — and refuses an output count
the node's own body disagrees with, because that misaligns every value number
after it.

### `NativeCall` is a `fn` pointer, and that is the invariant

The whole scheduling argument for a graph with host logic rests on a native node
being unable to reach anything the graph produces. **A `fn` item cannot capture**,
so the environment is necessarily fixed when the `fn` is named. The rule is a
property of the type, not a promise in a comment. A caller that has a pre-run
value to work from builds a `fn` that reads it from wherever it lives — safe for
exactly the reason it is allowed to be, that the environment predates the run.

The alternative, a boxed trait object, would let the same rule be broken
invisibly, and the break is a **silently wrong answer** rather than a slow one.

**The arguments are `&[i64]`, not values**, because a host call cannot read
device memory at all. Handing it a graph value would mean handing it either a
buffer whose contents may not be written yet or a transfer the call site has to
know to ask for. So the runner settles and fetches arguments first, which is
where a native node's cost of sitting in the middle of a data path lands.

### The pending state is a value, not a rule

```rust
Value::Pending { submission, id, count }   // submitted, device may not be done
Value::Device  { id, count }               // waited for, contents are there
Value::Host(Vec<i64>)
Value::Int(i64)                            // a number, so never pending
```

This is the one design decision in the crate that everything else leans on. A
value is pending for exactly as long as the host has other work to do, and
**spending that window is the whole reason for running a graph this way** — so
folding it into "a buffer" would either lie about the contents or make every
reader responsible for knowing whether it had to wait first.

Asking that to the type is what removes a class of bug rather than documenting it.
**A kernel node can consume a pending value**, because recording a dispatch
against a buffer only *names* it and the producer was recorded first. **A native
node cannot**, because a host call reads the data — and matching on `Pending` is
what a demand point *is*. That asymmetry is not a rule the runner follows; it is
what the match arms are. A dispatch with several outputs shares one submission
through an `Arc`, so the wait happens once however many values carry it.

**A count is asked in the other direction, and that is the point of `Int`.** A
count edge is a number, so it can never be pending and never needs a wait; a
count edge that resolves to a buffer is a mistake the type already forbids, and
asking for the two roles separately is what keeps one from being coerced into
the other.

### What a run hands back, and why it is not the last node

The **whole tail of the value table**, not the last node's outputs. What a
function returns from the graph compiled out of it is the *function's* business,
and the function is the thing that knows it. A runner that picked a node would be
making that decision for the caller, and a graph with a dead tail would be unable
to express its own return. Every value is settled first, so every id handed back
names a buffer the device has written.

### Two policies work and one is refused

`Serial` is the plain kernel path. `Async` submits each node as soon as it is
recordable and waits only where something needs the data. **`Batch` is refused by
name**, because a backend can only fuse if it can be handed several dispatches to
put in one command buffer, and the contract has no way to ask for that — `submit`
records one run and hands it over. A runner that accepted `Batch` could only run
it as something else, and both things it could run it as report a number for a
schedule nobody asked for. That is a missing capability, named as one, rather
than a silent downgrade.

**Async needed no new backend capability at all.** `submit` and `Pending::wait`
were already enough, which is the strongest evidence yet that the two seams landed
in the right place: the graph runner is written entirely against the existing
contract.

### Checked, on a stub and on a device

Two claims, checked in two places, because they are two claims.

**The schedule is checked on a stub, because only a stub can say when the host
waited.** The stub records what it was asked for and in what order, and `run` and
`submit` are told apart because that is exactly what a policy chooses between — a stub
that treated them alike could not see the difference at all. A two-node chain under
`Async` submits both nodes and waits **once, at the end**; under `Serial` every `run` is
waited for before the next.

The middle case this section used to describe — a host call between two dispatches,
where the wait is a demand point and anything else would read a buffer the device had
not written — **no longer exists**, and its removal is a consequence of the decision to
leave one kind of node: with nothing in the graph that reads data on the host, the
runner has no demand point at all and settles every value before handing the table back.
`Value`'s doc says this, and it is why the pending state survives as a property of a
value rather than as a rule. The two device tests it used to point at were never
written; a design that was retracted cannot have left tests behind, and a note claiming
otherwise is worse than no note.

**The arithmetic is checked on a real device**, in `lichen-compute-gpu`'s
`graph_on_device.rs`, because the stub computes `sum(inputs) + 1` whatever the body says
and so cannot say whether a fragment means what it says. Two tests: a two-node chain
(`adds`, then `sums` — the second node recording against a resident id the device has not
finished writing), and a graph whose extent is one of its own arguments, run at three
extents with the data deliberately longer than the count. Both compare against a
**hand-derived expected vector** rather than against another implementation of the same
reading, and the second is the one a device is worth having for: the count decides how
many elements are allocated, dispatched and read back, so a count read from the wrong
slot would answer with a wrong *length* and not merely a wrong number.

**And one thing that run does not check, stated so it is not read as a check.** Neither
fragment on the device has `param_shape.flat_arity() - 1` disagreeing with
`fragment.inputs`: `adds` declares two leaves and reads one buffer, `sums` declares
three and reads two, so both satisfy the old formula by coincidence. The divergence — a
fragment that reads no buffer at all, which the old check would have refused for having
no input — is a stub-side fact, pinned in `refusals.rs` and `graph_runs.rs`. A device is
not where it can be observed, and a green device run is not evidence about it.

## The half that builds a graph, and what it found

**The design below was wrong in its central walk, and a probe found it.** It is
kept rather than rewritten, because the correction is the expensive part and
re-deriving it would be worse. `crates/lichen-language/tests/graph_structure.rs`
is the probe: it compiles a real `plrun` chain and asserts the structure a
lowering would meet, so the next attempt is written on observation.

### A native call is an `Apply`, and the operator lives in a synthesized callee

**What the design assumed:** the function's own node list contains `ParLaunch`
nodes, so the lowering enumerates `function_nodes(function)` and picks them out.

**What is true:** the body contains **no compute operator at all**. A native
call is a `LowOperator::Apply)`, and the operator that the native op built lives
in a **synthesized per-call-site function** that the `Apply` enters. A lowering
that searched the body would find nothing and record an empty graph from a
program that dispatches — silently, and looking like a program with no work in
it.

So the walk is: the body's `Apply` nodes, each one's callee, and the compute
operators in the callee's body. That is **better** than what it replaced, and
the reason is worth keeping: the lowering now sees *the program's own calls*
rather than compute operators directly, so it is generic over the surface and a
new `$` op is a node rather than a special case.

Two facts that fall out and are now checked:

- a callee can be **static** (frozen in a package) as well as dynamic. A static
  one has no body in this module and yields no operator, so a body mixes both
  kinds and only the dynamic kind is a dispatch.
- the operator node's operand array is `[kernel, cfg]`, and both slots are
  `value_of` extractions — `Index(x, i)` — reached by the same walk
  `kernel_id_of` already does. The `cfg` is an `Index` into the cfg tuple, and
  the tuple's payload is readable with no evaluation at all.

### The body is decided by applying it, so a graph is *recorded*, not read

**This is the correction, and it undoes the "pure structural walk" the design
had settled on.** The probe's next fact:

> The cfg tuple is readable. Its two elements — the count and the buffer — are
> **`Parameterized`**. The `4` and the `data` in the body are unbound cells
> until the function is applied, because nothing has applied it.

So walking the structure without evaluating gets the graph's **shape** and
**none of its values**, and the values are the part a run needs. A graph cannot
be built by reading a template.

It is built by **applying** the function and **recording** what it dispatches —
which is what `ValueId`'s own note said from the first commit: "the graph is
built by recording an evaluation that has already happened". The design drifted
away from that and had to be walked back.

And applying is exactly equivalent to not applying, for a reason the language
gives for free: **every** lichen function has exactly one parameter (see below),
and a graph function does not read it, so what is applied does not matter. Under
laziness nothing else is forced either — the body is a thunk, and the lowering
demands exactly the parts the recording handles. **The "no dispatch while
building" property comes from the recording intercepting `ParLaunch` and
substituting a placeholder, not from refusing to evaluate.** Those are different
mechanisms and only one of them was in the design.

Both are answered, and the answers together are what the build hangs off. See
[The lowering, decided](#the-lowering-decided).

### The lowering, decided

Everything below is decided. It is written here rather than in the code because
the code is the next checkpoint and a design that is only in code cannot be
reviewed against the thing it replaced.

#### A recorded dispatch hands back a value number, and nothing else

`ParLaunch` normally hands back a `Buffer` (an arena payload) or a
`DeviceBuffer`. A recording cannot hand back either:

- a buffer payload is **arena data with a block's lifetime**, and a graph's
  values are not arena data;
- a resident id is plain data that happens to be the *wrong plain data*, because
  it names device memory rather than a position in the value table.

So the recording hands back a **new `ComputeValue` variant carrying a `usize`**.
Two of them, and the pair is what the whole build is made of:

| variant | carries | stands for |
|---|---|---|
| `GraphInput(slot)` | the parameter's slot | a value the caller supplies at `graphrun` |
| `GraphValue(id)` | a value number | a dispatch this recording already recorded |

**Both are invisible to the checker, and that is what makes them usable here.**
The type at a `cfg` position is fixed by `check_unify` at compile time
(`compute.rs:4618-4627` builds `[?b, [TypeBuffer, Type]]` and unifies); nothing
re-derives a type from a runtime value, so a new variant is not a type error
anywhere. Neither is a handle: `is_handle` is `false` for both, so the copy path
copies them rather than relocating a pointer, exactly as `DeviceBuffer` does.

**The chain needs no side table.** The design had planned a `produced_by` map
from node to value number, written before the walk was known to be a recording
rather than a traversal. Once the walk *is* a recording, the placeholder is
already sitting in the node that the next `cfg` reads, carrying the value number
with it: a second dispatch's input is a `GraphValue(v)`, and `v` is the edge.
A side table would be a second copy of the same fact that could disagree with the
first.

#### A graph holds no references at all, which retires `traced`

This is the correction that cost the most, because it deletes a documented
reason for the step.

The design said the graph holds its input buffers as **nodes**, and that this is
`traced`'s first real user. Check what `traced` actually buys
(`gc.rs:186-196`):

> Drops `block` and everything homed in it … **The caller guarantees no live node
> outside the block still references one inside it** — evaluating a surviving
> reference to a released block panics.

`drop_block` removes **by block membership, not reachability**. `traced` keeps a
node alive across `garbage_collect` compaction (ids are stable, the payload is
relocated and re-read through the id) and gives it **no** protection at all
against its block being dropped. So a graph holding input nodes is correct only
if the host never drops a block a live graph points into, and violating that is a
use-after-free in some *later* `graphrun`, not an error at build time.

The fix is not to hold the nodes. **The fix is to hold nothing.** The graph's
inputs arrive through the function's parameter, so the graph value is a `Graph`,
a `usize`, and nothing else: kernel ids, edge numbers and counts, all plain data
with no block, no arena and no device lifetime in them. There is nothing to
trace, no host obligation to state, and no copy to pay.

So this step does **not** give `traced` a user, and the earlier claim that it
would is withdrawn. What it does give a user is `run_deferred`: the `Graph`
operator has to decide for itself when its operand is evaluated, which is
precisely what that seam is for.

#### Inputs come from the function's parameter, and nowhere else

Every lichen function has exactly one parameter (the grammar's
`lambda := annotated ('=>' expr)?` puts a *name* on the left), so there is
exactly one channel a value can arrive through that the graph does not have to
capture. The shape is:

```lichen
step = ins => compute.plrun doubler (ins(0), ins(1))
built = compute.graph step
compute.graphrun built (4, data)
```

**A buffer the body reads as a free variable is a refusal, and the refusal names
the capture.** "A graph that captured a buffer would have to hold it" is the
whole reason, and the alternative — pinning the data at build time — was measured
out as well: buffers are immutable once built (`plrun` makes new ones, `$write`
only builds a type marker), so pinning means holding a node with a block's
lifetime, which is the hazard above.

The slot's index **is** its number, so `ins(i)` is the `i`-th argument and no
ordering has to be guessed.

The build binds the parameter to a tuple of `GraphInput(i)` placeholders and
applies the body, which is the ordinary VM path — `$graph` builds the `Apply`
node itself and evaluates it, so the cloning, unification and pattern walk are
the VM's own and no lowlevel seam has to grow for this. Reading `ins(i)` yields
`GraphInput(i)` and reading a dispatch's output yields `GraphValue(v)`; the two
are distinguishable at the point of use, which is what lets a literal count stay
a constant. See [The arity is the parameter's
length](#the-arity-is-the-parameters-length-and-the-probe-is-why) for how the
tuple is sized and how the one fact that is not structural is checked.

#### The count is a value, because a program's count is

`KernelNode.count` was a `usize`, decided at build time, and the reason given for
it was that the value table cannot hold a number: `Value` is `Pending` or
`Device` or `Host(Vec<i64>)`, and none of those is a scalar. **That reason
argues for adding the scalar, not against needing one.** A count that depends on
data is ordinary — a length off a `collect` is the common case — and a graph
that cannot take one forces every such program to rebuild the graph per run,
which is the same as not having a graph.

So `Value` gains a scalar, and a dispatch's range becomes:

```rust
pub enum Count {
    /// A number the build already knew: a literal, or a value nothing in the
    /// graph produced.
    Constant(usize),
    /// A value in the table, read when the node runs. Never pending, because a
    /// number is not produced by a device.
    Value(ValueId),
}
```

**`Count` did not change and did not need to.** The scalar stayed
`Value::Int(i64)`: an intermediate version of this work renamed it to
`Value::Native` with a `Pointer` variant for a closure, and that was wrong twice
over. `Native` already meant "a host call" twice in this crate
(`NativeCall`, `NativeNode`), and the pointer had nothing to point at, because a
closure is a *node* and not a value. `Count` is the first **consumer** of a
value that is not a buffer, which is not the same thing as being the mechanism
for one.

`Constant` is not a wart on `Value`; it is the one case where the build already
had the answer, and collapsing it into a value would mean inventing a node kind
that produces a number for free.

**A scalar is an input today and nothing produces one yet.** A node that
*computes* a number is a compiled kernel, and the one that produces a bare number
rather than a buffer is not written — a `KernelFragment` writes `outputs` buffers
and leaves `results` scalars on its own stack, and a node's outputs are the
buffers. So a count is either a build-time literal or a value the caller passed,
and both are honest answers.

The two roles are asked of the table separately, and each is refused by name: a count
edge that resolves to data, and a buffer input that resolves to a number, are different
mistakes with different causes. **Those two methods are the filter, and there is no
type category saying which role a value has** — a jit'd function may be handed arbitrary
lichen values, and a recording sorts them into roles by asking, one value at a time.
What each method returns on a refusal is *what the value is* and not a message: the
node and the edge belong to the runner's demand, and only the runner is holding them
(see [the fifth finding](#a-refusal-raised-where-it-cannot-say-who-asked-and-a-number-spotted-by-being-refused)).

#### A number on an edge, and the closure question it exposed

**This was not designed; it was pointed out, and it changed what the open
native-node question actually is.** `Value::Int` reads like a feature about
counts — it exists because a dispatch's extent is a number and a number is not
`Vec<i64>`. But the thing that was added is not "a count can be dynamic". The
thing that was added is **the first value that is not a buffer and still travels
an edge**, and it turned out to be the first evidence that the open question was
being asked about the wrong half of the graph.

**The two invariants are one invariant at two levels, and that is what made the
question visible.** The doc on `NativeCall` said a bare `fn` pointer was
deliberate, because *a `fn` item cannot capture, so a native node's environment
is fixed when the `fn` is named, which is what makes the graph's edges statically
known and any topological order correct.* The input rule, decided a section
earlier, says the same thing from the other side: **a graph's inputs are the
and a free-variable buffer is a refusal, so the graph holds nothing and reaches
nothing at run time that was decided during the run.** One is a type fact in the
IR, the other is a named refusal in the lowering, and they agree because both
say the same single thing — *the graph's entire world is its edges*.

So a closure arriving as a graph value needs no rule of its own; it inherits
that one, and the question it raises is narrower than it first looked.

**A pointer is enough, and it is enough because the trace walk is recursive.**
`gc.rs:149-155` asks a value for the nodes it holds and then calls
`garbage_collect_node` on each, which walks that node's value and whatever
*that* holds, and so on. So a slot in a registry of host-owned values keeps its
whole transitive environment alive without the graph enumerating any of it: *a
`fn` captured `x`, so whoever captured the `fn` has captured `x`*, and nothing
in the graph has to know that. A graph that listed its captures would be a
second copy of a fact the walk already holds, and one that could disagree with
it. `freeze.rs:429-431` does the same thing for a frozen artifact, so this is
the mechanism twice over rather than a corner argued for here.

**And the capture question is empty, for a stronger reason than the walk.** A
graph's products are numbers in a value table, and they only become
`ComputeValue`s after `Runner::run` returns, so a closure cannot capture "value
7 of some graph": that is not a thing the language can name. A closure defined
*inside the recorded body* can capture a `GraphValue` placeholder, and the
placeholder is **inert** — a `usize` in a `Copy` variant, with no operation that
turns one into device memory. So it captures a number it cannot dereference, and
there is no read to be early and no wait to be missing.

**Inert means loudly refused rather than silently ignored**, which is the part
worth checking instead of assuming. Every place that consumes a `ComputeValue`
does so by matching its variant: the codec refuses each one by name
(`compute.rs:594-626`), and an argument that does not match reports what it found
through `argument_kind` and falls back to `Parameterized`
(`compute.rs:780-791`). So a `GraphValue` reaching a buffer position is a
diagnostic naming the variant.

**This withdrew a case an earlier version of this section named, and the case was
mine.** It said the one thing recursion does not answer is a closure defined
inside the body capturing a `GraphValue`, with no demand point at which to wait,
and it offered two repairs: turn the capture into an edge, or refuse it. Both
repairs assumed the captured placeholder could be dereferenced. It cannot, so
there was nothing to wait for and nothing to refuse. **Kept rather than quietly
dropped** — the shape of the worry was right and the consequence of it was not,
and a section that only ever records conclusions is a section that cannot be
checked.

**And then the question turned out to be asked about the wrong half.** A
closure does not have to reach the graph as a *value* at all. `NativeCall` was a
bare `fn` pointer because a host call is a call, and the argument of the
contradiction above is a pointer to something the graph **calls**. The answer is
that **a closure is a compiled artifact like any other, so it lowers to a
fragment and is dispatched like one** — a "closure kernel", whose dependencies
are edges like every other node's. What needs a pointer is the *call*, and the
call is a node.

So `Node::Native`, `NativeCall` and the whole host-call node are **gone**, and
with them: the re-entrant apply path (a call no longer happens on the host's
stack at run time), the `Vec<i64>` against `[?b]` type gap (a lowered fragment
already speaks the IR's own values), and the "type fact against discipline"
worry (one kind of node means one set of rules, and nothing a trait object could
make unsound behind it).

**Two things about that, one of which costs something.** A "closure kernel" is a
closure that **lowers** — `KernelInstr` is a flat stack machine of constants,
arithmetic, `Select`, `LocalGet`, `CallKernel` and buffer reads, so a closure
that allocates, collects, or recurses beyond the static `CallKernel` graph is not
one. That boundary has somewhere to land, because the domain shape is already
refused by name before a kernel lowers. And the cost: a closure kernel is still
**device** work, so `hidden = min(host, device)` is unchanged and a graph of
them is still worth no milliseconds under `Serial` or `Async`. That is not a
regression from the direction — it is the same conclusion the measurement below
already reached, now for a stronger reason.

**None of it blocks the recording.** The second inhabitant is already there —
`Native::Pointer` is the slot, with no producer and no consumer yet, which is
what makes adding a producer a compile error in each role method rather than a
silent pass-through. So a closure later is a new *producer*, not a new
mechanism; the count edge is already the edge it would travel on; and a
kernel-only graph contains no closures, so the lowering has nothing to know
about them. What it does change is a naming discipline: **`Count` is the first
*consumer* of a non-buffer value, not the non-buffer value itself**, so the
count machinery must not grow into being the general mechanism for one.

#### The arity is the parameter's length, and the probe is why

**This paragraph was written twice and the probe deleted the first version of
it.** It said the placeholder tuple has to be built at a ceiling and trimmed to
the highest slot the recording saw, with a named refusal for reading past the
ceiling, because the body's read positions are not visible before the apply.
Half of that survives and half of it does not, and the difference is worth
writing down because the tidier version is the wrong one.

What the probe found:

- a read of `ins(i)` compiles to a **bare cell** — no operation, no subscript,
  and not even a member of the parameter's class. So which read is which slot is
  **not visible in the unapplied body at all**, and the decided `USize(0)` that
  looks like an input position belongs to the extraction of the *cfg slot* out
  of the operand pair. A lowering that read it would conclude the graph takes one
  input and silently drop the rest.
- **but the parameter cell is a tuple with one cell per read**, and its length is
  the arity. It needs no evaluation, so `Graph::with_inputs` gets its number
  before anything runs, and the placeholder tuple is sized from it.

So there is no ceiling, no trim, and no refusal for reading past one. The
ceiling was a symptom of looking for the arity in the wrong place.

**Which read is which slot is settled by the apply, and it is settled by
position.** That is the one fact in this whole input rule that cannot be read
structurally, so it is checked by running a function that reads its parameter
**back to front** and comparing the numbers against the kernel's closed form: a
caller passing `(count, buffer)` has to reach the count slot and the buffer slot
respectively, and read-order numbering would have swapped them into a dispatch
that reads a number as a buffer. A silently wrong answer rather than a refusal is
the only class of bug this repository cares most about, so it is checked by
behaviour and not by inspection.

#### What is left after this

- **The lowering itself**, in `crates/lichen-compute/src/compute/graph.rs`.
- **The `Graph` value and the `GraphRun` operator**, and the `compute.graph` /
  `compute.graphrun` surface.
- **The refusals below**, which are design, not code, and none are written.

### A graph's inputs are not its free variables, and the reason is `drop_block`

**This section was wrong and is kept as the record of why.** It argued that
`$graph(f)`'s inputs are the body's free variables, because nothing has been
applied to the template and so a free variable is the only buffer the body can
name. The argument is sound and the conclusion is not usable: a free variable is
a `Buffer` in a block arena, so a graph taking it has to **hold it**, and holding
it is exactly the hazard `drop_block` creates.

The argument also produced a second claim that has to go with it: "a source
function that dispatches a buffer directly is unrecordable, because its parameter
is not a value at the moment the graph is built". That refusal is **withdrawn**.
The parameter is now the input channel, so a function dispatching its own
parameter is the *ordinary* case and reading it is how an input arrives.

What survives is the observation that every lichen function has exactly one
parameter, because the grammar's `lambda := annotated ('=>' expr)?` puts a *name*
on the left. There is no nullary lambda syntax, which is why there is exactly one
channel and why the design does not have to invent an argument-passing form.

### Node ids survive collection, and that is not enough

`garbage_collect_node` moves a node by writing `self.nodes[node].block = target`
and **"a node keeps its id across the move, so only its block changes, and a
value holds the id"**, so an id held outside the module survives every
collection; the `Array`/`Table` arms prove the same for payloads, which are
reallocated into the target arena with the holder's handle rewritten.

So a registry entry holding a `NodeId` and reading the value back through it
would be correct. **A built graph holds no such id**, because it holds no node at
all, so what is recorded here is a property of the collector rather than a
licence the graph uses. The half that would have needed it is `drop_block`, and
that one is by block membership.

### The walk is structural, and no dispatch runs while building a graph

**The title is now half wrong and the second half is the correction above.** The
walk is structural *in shape* — the body's `Apply` nodes, each dynamic callee,
the operator in it — but the body has to be **applied** for any of it to mean
anything, so the walk happens *during* an evaluation rather than instead of one.

What survives unchanged:

- `Graph::with_inputs` needs the input count before the first `push`, so there
  are two passes. **But they are two passes over the facts the recording
  collected, not two walks of the module.** Pass one is the recording itself and
  produces `NodeFacts { kernel, count, inputs }` in body order; pass two pushes
  them once the arity is known. The earlier plan walked the body twice, which
  would have meant running the recording twice.
- The reason the body is walked rather than the return's operand spine: a spine
  sees only live nodes, so **it cannot find a dead tail**. The return is resolved
  separately and recorded through `Graph::returning`, which is why a dead tail and
  a return are independent facts.

What changes: "the count is a decided value and only it needs evaluating" is
false. In an unapplied body *nothing* is decided.

### Refusals this design owes, all of them about a graph and not a run

Found while designing, none of them written yet. Each names its cause, per the
rule the rest of the tree follows.

- **Mixed backends.** A run goes to one `ParallelBackend` and the IR's nodes do
  not carry one, so a graph whose dispatches name different backends cannot be run
  at all. Refuse at build time, naming both.
- **A non-device backend.** `"cpu"` dispatches through this crate's wasm path and
  never touches a `ParallelBackend`, so a cpu graph would build and then fail at
  run time with "no backend installed" — a capability mismatch reported as an
  environment problem. Refuse at build time and say why.
- **A `cfg` item that is neither a dispatch's output nor an input slot.** A third
  thing in that position would be the interesting one to support and the wrong one
  to accept silently. In practice this is where a **free variable** lands now that
  inputs come from the parameter, so the refusal that matters most is its own
  entry.
- **A buffer the body reads as a free variable.** Name the capture, not "unresolved
  value": the graph would have to hold a `Buffer` that lives in a block arena, and
  `drop_block` takes it away by block membership. A graph function that takes its
  inputs as its parameter has no such problem.
- **A parameter that is not a tuple.** The arity is the parameter cell's tuple
  length, so a function whose parameter is a single buffer or a number has no
  input list to be the argument to. Name that rather than reporting a length of
  zero and building a graph nobody can call.
- **A count that is a negative number, or a buffer input that is a number, or a
  count that is data.** Three different mistakes, so three messages; the runner
  asks the two roles of a value separately and refuses rather than coercing. The
  two that are about a demand also name **the node and the edge**, because a
  refusal that cannot say who asked is a sentence about a graph of one node; the
  negative one names neither, because a number is also what a function may
  return and no node asked for that.
- **A return naming neither a dispatch's output nor an input slot.**

### What this step is worth, stated plainly

A kernel-only graph under `Serial` or `Async` is **worth no milliseconds**, and the
measurement above says so rather than implying otherwise: a pure kernel chain has
no host work, so `hidden = min(host, device) = 0` and `Async` is exactly the plain
kernel path. The win for a pure chain is `Batch` (one submission instead of
sixteen), which is refused for want of a fused-submission contract.

So this half is worth building for reasons that are not speed, and **one of the
three reasons it used to claim is now withdrawn.** It was "it gives `traced` and
`run_deferred` a first real user after two commits of zero", and a graph that
holds no references gives `traced` nothing to trace. `run_deferred` still gets
its user, because the `Graph` operator has to decide for itself when its operand
is evaluated. The surviving reasons are that this is the prerequisite for
designing the fused-submission shape, since "hand me N dispatches" cannot be
specified without knowing what a node is, and that it produces the repeatable
object the feature was decided around. **It is a shape step, and the ordering in
the list below predates the measurement that says so.**

## Five things a recording found, each of them a bug the walk had to have

These are the findings of building the recording half, and all five were live bugs
rather than design questions. **Four share a cause: a recording numbers things in its
own space and a graph numbers them in another, and every one of those four is a place
where the two were confused.** The fifth is a different mistake with the same shape as
the first — a check asked in the wrong place, where the facts it needed to answer with
were not to be had.

### How many buffers a fragment reads is not in its shape, and the check that asked was wrong

`Runner::run` checked `kernel.inputs.len() == fragment.param_shape.flat_arity() - 1`,
on the reading that a parallel fragment's parameters are its input slots followed by
the loop index. **That reading is false, and the tree already said so.**
`crates/lichen-compute-gpu/tests/refusals.rs` states that `param_shape` is
`(config, index)` — two leaves — *however many inputs there are*, because a buffer is
bound as a storage buffer and reached through a read's position rather than through a
further parameter. `compute.rs`'s real launch agrees: it reads the buffer count from
the call site's cfg tuple and never looks at the shape.

The check was not merely useless, it was **rejecting correct graphs**. A kernel that
reads no buffer declares a two-leaf shape, so a node with no inputs was told it was
missing one. It happened to pass for a two-input kernel, whose shape is a three-leaf
tuple, so the coincidence read as agreement.

The fix is `KernelFragment::inputs`, the missing twin of the `outputs` that was
already there. Two counts, both facts of the *compiled* fragment, and the reason they
cannot be one is that they answer different questions:

- **How many was it given** is the dispatch's, read at apply time from the cfg tuple.
- **How many does it need** is the fragment's, and it is a *max*, not a tally: the
  read positions are a sparse space, so a body reading only `cfg(1)(1)` still needs
  two buffers bound or the one it read was never bound.

`inputs` is counted by the emitter as it emits the `Const` positions, so it cannot
disagree with the body, and it is hashed by `fragment_digest` for the same reason
`results` is: two bodies differing only in how many buffers they read are different
programs. The same argument is why the check now reads `kernel.fragment.inputs` — and
why the **non-graph path needed it too**, since it had no check at all and would have
handed a shader a binding its body never reads.

### A recorded count and a recorded edge land in different value spaces

`Recording` numbers its produced values from zero, because the input count is not known
until the walk ends; `finish()` puts that count in front of them. An `Input` edge is
already final — it is the number its slot was allocated.

The first cut stored the count as a resolved `ValueId` and shifted it in `finish()`
like everything else. For a count read from a **parameter** that is already final, so
the shift moved it onto the node's own output: a count of `0` became `1`, which is a
value that exists, so the graph ran and dispatched over a buffer's length. The
symptom was a refusal about a value that did not exist, which pointed at the edge
table rather than at the arithmetic.

`Edge` already documented why the two kinds must stay apart. The count now carries an
`Edge` too, and `finish()` has **one** `resolve` for edges, counts and the return
alike. One resolution point is the whole fix; a second one is where the two id spaces
get confused again.

### A block's value is wider than what the function returns

A block's value is the tuple of its statements' values, and the tuple is **wider than
the one value the function returns** — the tail of it is machinery, not results. The
recording read the whole tuple as the return, which asked the graph to return every
statement's value.

This crate already had the answer twice, in `compile_fragment` and
`parallel_output_nodes`, which both resolve a function's return by reading
`array_items(body)[0]`. The recording was the third reader and the only one that
guessed. It reads `items[0]` now, for the same reason those two do.

### An unreadable return is a refusal, not an unrecorded one

A return that could not be read was left unrecorded, on the reasoning that a graph with
no recorded return takes every value it has — a permissive answer. It is not
permissive, it is **wrong**: the table holds the count and the buffers, so the graph
answered `"[3, <buffer>]"` to a program that asked for one of them. And the way this
surfaced was the sharpest version of the failure: a *refused* dispatch inside the body
left the block's result cells empty, so the return became unreadable, so the graph was
built anyway and returned a value nobody asked for.

So the language path refuses by name when it cannot read what the function returns.
The permissive reading of an unrecorded return still belongs to a graph nobody
recorded — `Graph::returns()` is `None` for a hand-built graph, and *that* is the
contract the tests in `graph_runs.rs` pin. It just is not a contract the recording
gets to lean on after it has failed to read the answer.

One thing this forced into the open: **a function is allowed to return its own
extent.** A returned count is an *input* value, in the table like any other, so the
return has to be able to say which of the two placeholder kinds it is — the same
`Placed` an edge carries. And having made it say so, `RunResult` grew a `Count` arm,
because a graph that returns a number has to be able to hand it back. Refusing there
would have made the permissive answer unreachable for exactly the graphs that need it.

### A refusal raised where it cannot say who asked, and a number spotted by being refused

`Value::slot()` and `Value::as_count()` raised `NotBufferData` and `CountNotANumber`
themselves, and both variants named only `found` — what the value was. That is a
sentence about a graph of one node. A graph has one demand per node and two edges per
demand, so a refusal with neither number in it leaves the reader to work out which of
forty demands was the wrong one, and the two numbers that would settle it are the ones
the **runner** is holding: the position it is at, and the edge the node was built with.
A value is not a node and names no edge, so no refusal raised inside a value could ever
say them.

So the filters now return `&'static str` — what the value *is* — and the runner
assembles `NotBufferData { node, value, found }` and `CountNotANumber { node, value,
found }`. A filter answers about itself; the asker names the demand.

**`CountNegative` keeps no node, and that is now a decision rather than an omission.**
It is about the *number*, and a number reaches a graph two ways: a node's count edge,
and a function's own return, which no node asked for. Putting a node in that variant
would mean inventing one where the mistake is not in any node — which is exactly the
bug `RunArity` was split out of last round, where a run-level refusal carried
`node: usize::MAX` and printed as a node number. Its message lost "a dispatch's" for
the same reason: it is no longer only a dispatch that can hold a negative count.

**And the consequence nobody planned was on the readback side.** `returned_role` used
to classify a returned value by asking for its buffer and *catching* the refusal —
`Err(NotBufferData) => value.as_count()` — so a number was recognised by the fact that
it had been refused. That made the readback depend on a diagnostic it was about to throw
away, and it would have broken outright the day that diagnostic wanted to say which node
asked: a returned number has no node, because no node asked for it. It is now a total
match on `Value`'s variants, which is what a three-way sort actually is.

The two filters and that match **cannot drift apart**, and that is the reason the
readback is allowed to classify on its own: all three are exhaustive matches over the
same enum, so adding a kind to `Value` breaks every one of them at compile time. That
is a stronger guarantee than the one the filters were introduced with, which argued
only that no *third kind* existed.

**The extent moved with it, and it took a real hazard away.** `as_count` returned
`usize` and did the `usize::try_from` itself, so it was the only thing standing between
`LowValue::USize(number as usize)` — written twice in `compute.rs` — and a negative
count silently becoming an enormous length. That was a coincidence and not an
invariant: the conversion belongs to whoever is about to dispatch over `[0, count)`, and
a count arrives as a node's edge *or* as the function's own return. So `as_number`
hands back the `i64`, the runner and the readback each convert once, and
`RunResult::Count` is a `usize` — which **deletes the two `as usize` rather than
guarding them.**

### The contradiction: a graph cannot hold the closures its native nodes call

Both seam doc comments promised that a compiled graph would hold *the closures it
calls later*. `NativeCall` is a bare `fn` pointer, and that is deliberate: a `fn`
item cannot capture, so the environment is fixed when the `fn` is named, which is
what makes the graph's edges statically known. But `fn(&[&[i64]]) ->
Vec<Vec<i64>>` has **no channel to name a closure** — its only parameter is the
arguments. So a user-written lichen closure cannot become a native node without
replacing that decision, and the two committed things disagree.

Three ways out, none of them free:

1. **Native node is a stateless host function from a fixed set.** The `fn`
   invariant stands untouched and no code has to move. But nothing in the
   language's surface today has that shape, so the first version would have to
   invent one, and the graph's native nodes would not be the ones the design
   writes about.
2. **Native node is a user closure, re-entered into the VM at run time.** This is
   what the seams were cut for and the only shape where `Async`'s measured payoff
   is reachable, since a native node *is* the host work that gets hidden. The cost
   is that `NativeCall` stops being a `fn` pointer: the environment moves behind a
   registry slot, which keeps the soundness property (it is still fixed before the
   run) but demotes it from a **type fact** to a **discipline**. It also needs a
   re-entrant apply path the VM does not have yet, and it has to close a type gap:
   the IR's host data is `Vec<i64>` while the language's host arrays are `[?b]`.
3. **Leave it open.** Build the kernel-only half, record the contradiction, and
   decide before writing the first native node.

**3 is what was done**, and the two seam doc comments were corrected to match
rather than left promising something nothing delivers. The doc comments naming
closures were the only place the contradiction was written down, so a reader would
have taken the promise at face value.

**What the count work settled about this, and how.** The question above was
filed as "a graph cannot hold a closure" because the graph IR's native node is a
bare `fn` pointer. But `Int` put the first non-buffer value on a graph edge, so
the real question turned out to be *what a graph node is*, not what a graph value
can hold — and a closure is a compiled artifact, so it is a fragment and a
dispatch. The whole section is settled and the host-call node is deleted. See
[A number on an edge, and the closure question it
exposed](#a-number-on-an-edge-and-the-closure-question-it-exposed).

## The pool of submission slots

`GpuConfig { slot_depth }`, default **2**, configurable only from Rust:
`GpuContext::new()` takes the default and `GpuContext::with_config` takes the
rest. A depth of zero is refused by name, for the same reason a zero-link chain
is: it is not a smaller pool, it is none.

**One slot is a command buffer, a fence, a staging buffer and a descriptor pool,
and they live in one struct because they all die at the same moment.** That is
the whole design. A command buffer cannot be recorded while a previous recording
of it is running; a staging mapping cannot be written while a copy out of it is
in flight; a descriptor pool cannot be reset while a dispatch bound to its sets
is executing. Keeping them together makes recording one submission into another
submission's command buffer unrepresentable rather than merely discouraged.

So the three consequences that were listed as *what a pool would need* are now
properties of the type rather than rules someone has to remember:

- **Staging is per slot.** Not a convention — `Segment::reserve` and
  `Segment::staging` reach only the slot the segment holds, and the segment holds
  the pool lock. There is no API that names a staging buffer.
- **The descriptor pool is per slot**, reset in `acquire` and nowhere else.
  `acquire` is the only place that can safely do it, and it is safe there for a
  reason it establishes itself: it has just waited the slot's fence, or the slot
  was never claimed.
- **`fetch` acquires a slot** like everything else, because it is a submission.

**The claim is the lock.** `GpuContext::acquire` hands back a `Segment` carrying
the pool's `MutexGuard` for its whole life, so "two threads never record into one
command buffer" is something the borrow checker sees. Releasing the lock is what
`Segment::drop` does, and only on a path that neither submitted nor waited: a
segment that submitted leaves the slot claimed, and the next acquisition of that
slot waits its fence. That is the entire mechanism by which a slot is never
reused while the device still owns it.

**`Segment` has three submit methods, and the difference between them is what
they leave the caller holding.** `submit_and_wait` gives the slot back — that is
`run` and `run_chain`, and it is literally `submit` then `sync`. `submit` hands
back a [`Token`] and keeps the slot claimed, which is the only path that leaves
the device behind the host. `submit_and_read_back` reads its own staging *inside*
the claim, which is why it is not `submit` plus `sync`: `sync` gives the slot
away, and the next acquisition would then be free to resize or rewrite the very
mapping being copied out of. That returns the **wrong numbers** rather than
failing, so it is structural rather than documented.

**`Token` is consumed by the wait.** One submission is waited for exactly once,
because by the time a second wait ran, the slot could be running someone else's
submission and the wait would have proved the wrong thing. Taking it by value
makes the second wait inexpressible.

**What depth costs, and what it does not.** At depth 2 with callers that all wait
before returning — which is every caller today — `acquire` never waits: each slot
is released before the cursor comes back to it, so the round-robin costs one
index add. What depth does cost is **VRAM and host RAM**: each slot carries its
own staging buffer, so worst-case staging is `depth` times a run's uploads rather
than once. The fused chain's numbers are unchanged by this — 0.119 ms against
0.113 ms at 1 024 elements and 4.468 ms against 4.669 ms at a million, both inside
the run-to-run spread the tables below already record.

**`acquire`, `Segment` and `Token` are private**, and the entry point that is
public is `ParallelBackend::submit`, which returns a
[`Pending`](#the-pending-submission) rather than ids. A public `Segment::record`
would take a `vk::Pipeline` and a `&[vk::DescriptorBufferInfo]`, and putting
those in a public signature hands the backend's representation to every caller,
which is what `lichen-kernel-ir` exists to prevent.

## The pending submission

`ParallelBackend::submit` records a run, hands it to the queue, and returns a
`Box<dyn Pending>` **without waiting**. The ids it will produce are on
`Pending::outputs`, and their contents the device has not promised to have
written — they are for handing to the next node as `BufferSlot::Resident`, and
that is the chaining the overlapping schedule needs.

**The question this settles is when the ids escape, and the answer is that the
question was badly posed.** It was put as *at submit, or at the sync*, and at
the sync is not Async: chaining node `n + 1` needs node `n`'s id, so ids only at
the sync means waiting for `n` before recording `n + 1`, which leaves the device
idle for exactly as long as the submission was supposed to be overlapped. That is
Serial with an extra step.

The real shape is a third thing. **The pending object holds the ids, the token
and the upload targets, and a demand point is `wait` followed by an ordinary
fetch.** So the ids a *host* ever sees have been waited for — the graph returns
its outputs at the end of a run, after the sync — while the ids the *executor*
chains on have not, and the executor only ever hands them to a recording. The
closure invariant that starts this document is what makes the second population
safe: a native closure cannot capture a graph's own outputs, so no closure can
reach an unwaited id at all.

**The upload targets are the reason a pending object is necessary rather than a
token.** A host input is `memcpy`'d into a buffer the device is about to copy
out of, and that buffer is not resident, so nothing else can name it. It has to
survive until the wait, which means it has to travel with the submission — and a
bare `Token` has nowhere to put it.

**`Pending::wait` consumes the pending, and dropping one waits.** One submission
is waited for exactly once, because by the time a second wait ran, the slot could
be running a different submission and the wait would have proved the wrong thing.
The `Drop` is a backstop rather than a mechanism: a run submits and waits inside
one call, so a pending that is merely dropped should not exist, and if one does,
waiting turns a use-after-free into a slowdown.

**`ParallelBackend::submit` has a default that calls `run`.** A backend that
cannot overlap anything still satisfies the contract; it just never collects from
it. Requiring an implementation would mean every stub and every future backend
wrote a method whose only correct body is the one that does nothing, and a caller
could not tell "cannot overlap" from "has not implemented it yet". The GPU is
the only backend here that overrides it, and it can: it has a pool, so a
submission handed back is a **different slot** from the one the next submission
takes.

**Checked on a real device, both halves.** `a_submission_can_be_fed_to_one_that_is_still_in_flight`
records two submissions, feeds the second from the first's output *without
waiting for the first*, and gets `4x + 3` back — which says the slots really are
distinct and that the trailing barrier orders two *submissions*, not just two
dispatches inside one recording.
`dropping_a_submission_nobody_waited_for_still_frees_it` covers the backstop: at
depth two, a drop that did not wait would hand the next acquisition a staging
buffer with a copy still in flight, and the values afterwards would be wrong
rather than slow.

**What is still missing is the caller.** Nothing schedules: a graph executor that
submits a node, runs the closures between, and waits at the demand points does
not exist, and neither does the measurement of what that is worth.

## Two rules, settled before anything depended on them, and since executed (`6c5ac4d`, `519c9d1`)

Both were landmines on this list. They are decided now, while nothing depends on
them, because each is a **miscompile rather than a slowdown** and so cannot be
caught by watching the numbers.

They were written down first and **executed later**, which is the only order that
works for a rule whose failure mode is a wrong answer rather than a slow one: with
nothing depending on them they are reviewable in isolation, and `run_chain` is
then the first thing that can be checked against them rather than the first thing
that depends on them. It is checked, against the kernel's closed form, at four
shapes up to a 16-link chain, and both hold.

**One descriptor set per dispatch, and no reset under a running command
buffer.** A set is read when the submission *executes*, so one set cannot serve
two dispatches — rewriting it between them changes what the first one sees. Nor
can the pool be reset between them, because a reset frees every set, including
ones an earlier dispatch in the same command buffer is still bound to. So a
submission resets the pool once, at its own start, and takes what it needs
afterwards. This also changes what `MAX_DESCRIPTOR_BINDINGS` means — 32 is now a
*per-dispatch* limit, and 64 dispatches per submission is a second, separate
limit the graph will have to live inside. 64 is a placeholder: the graph-level
node set that ought to own that number does not exist yet, and a larger one
would be headroom for a program shape nobody has written.

**The trailing barrier names both possible readers.** It was
`SHADER_WRITE → TRANSFER_READ`, right only because every dispatch is followed by
a `fetch` in another submission. A dispatch recorded after this one in the same
command buffer reads those results as a *shader*, and a consumer outside the
destination scope does not read stale data, it reads undefined data. The scope is
now `SHADER_WRITE → SHADER_READ | TRANSFER_READ`. That over-covers the
single-dispatch case the backend records today, which is a cost, so it was
measured rather than assumed: 16 links at 1 048 576 elements went 7.79 ms →
7.17 ms and an empty dispatch 0.046 → 0.048, both inside the run-to-run spread of
the example that produced them. The honest reading is **not measurable, not
free**.

What is deliberately left open: **nothing counts dispatches within a
submission.** The constant sizes the pool, but exceeding it is pool exhaustion
on the device rather than a named refusal. Whoever writes the pool code must
carry that count, and it is the only part of this rule with no code behind it.

### The boundary was not drawn anywhere, and where it belongs

`ParLaunch` was the only operator that checked whether a graph was being recorded.
Every other one fell through to a bare `Parameterized`, and three of those fallings
through were silent rather than indirect:

- a `collect` of a dispatch's own result, which is the operator that says "give me
  these as host data" and is the one point at which a `"gpu"` chain crosses the bus;
- a host `read` of one, which is the same boundary at a single element;
- a scalar `call`, which is a different mistake again: **a scalar kernel has no
  node to be.** The one node kind is a parallel dispatch with its buffers bound to
  it, while a scalar fragment works on its own operand stack and names no buffer
  whatsoever, so whatever it computes has nowhere to travel. That is true of every
  shape of it, which is why this one is refused outright rather than only when it
  was handed a placeholder.

The first two are refused only when they were actually handed a placeholder. A read
of a host buffer the body closed over is ordinary host arithmetic, and if its result
is used as a count or an input the value-table filter already refuses that by name —
so refusing it here too would make `read` unusable inside any recorded body, which is
not a boundary anyone asked for.

**Checked in one place, in `OperatorExt::run`, and that placement is the point.** The
arms all failed the same way, so a boundary drawn in three arms is not a boundary; and
the rule is one sentence — *a `plrun` is the only operator that may see a placeholder* —
which is worth more than three correct arms that each know their own case.

**And the probe that found all this found something larger — which turned out to say the
opposite of what it first looked like.** A body of three statements — dispatch, bind the
result to a name nobody reads, dispatch again — records a two-node graph. The middle
dispatch was neither refused nor miscompiled: **it was never performed**, because a `let`
inside a block is lazy and nothing read the name.

**The decision taken at the time was that this must not stand**, on the grounds that a
graph has to be what its function *wrote*, or a dispatch the program performed is missing
from the thing that is supposed to be the program. **That premise was wrong, and the
measurement that corrected it is the most useful thing in this section.**

> Run the same body with no graph anywhere in the program — `out = step (3,)` — and
> count the dispatches the backend actually receives. It is **two**. The program does
> not perform the third one either.

There is no expression-level CSE in this compiler to explain the absence (the `dedup`
in the tree is over diagnostic text, name suggestions, and arena payloads in the static
module freeze — never over expressions), so identical written statements are distinct
nodes and both would run if both were reached. A variant with a `dead` binding that
differs from the live ones in both kernel and operands dispatches twice as well. **The
elimination is laziness, and it is the program doing it, not the recording.**

So the invariant was the wrong one. **A graph is a transcript of the run, not of the
source**, and the recording already satisfies it: `a_graph_dispatches_exactly_what_the_
program_dispatches` runs one body twice — plainly and through a graph — and compares the
two traces, order and inputs included. A count would have been satisfied by a graph that
dispatched the right things against the wrong buffers; a trace is not.

**A walk that forced the rest would have been a bug, not a completion.** It would have
put a dispatch in the graph that the program never makes, and the graph would then answer
with work its own author did not ask for.

**What forcing actually costs, measured while looking for a third option.** The two
knobs in `evaluate_node_deep_inner` make four walks, and all four were run:

| `skip_shallow` | `force_operand` | dispatches recorded | the function's return slot |
|---|---|---|---|
| on (the deep pass, in use) | off | **2** | readable |
| off | off | **2** | readable |
| off | on (`evaluate_node_forced`) | 3 | **empty** |
| on | on | — | **empty** |

The first two rows are the finding: descending every position in order reaches nothing
extra, because the unread statement is not behind a shallow mark — the deep pass already
reaches its node. It hangs on the **operand edge** of the apply, because the frontend
desugars a `let` into a lambda parameter, so the binding expression is an *argument* of
the apply and nothing evaluates an argument nobody reads. Only the operand forcing
reaches it, and turning that on empties the return slot **on its own**, with the shallow
mask untouched — so the cost is attributable to `force_operand` specifically, not to
descending past the mask. That empties the value `returned_value_ids` has to read, and
every recording refuses, including bodies with nothing unread in them. Reading an unread
body item in order does not help either: it reads back `Parameterized` and caches
nothing, which is the VM's own documented contract for an operation whose operands were
unbound when it was last evaluated.

**So the gap closed by being measured, and nothing in `lichen-lowlevel` had to change.**

## The next step, in order

1. ~~**A configurable pool of submission slots — not a `Segment` object.**~~ **Done.**
   Depth 1 was already built and measured (`run_chain`), and it is the whole of
   the **Batch** case: one submission, one wait, no cross-call state. The pool
   adds **depth greater than one**, which only Async needs — several submissions
   in flight at once so the host work between them overlaps the device work. The
   earlier plan here said a segment must outlive the `GraphRun` operator call, and
   everything downstream of it was built on that: a `Box<dyn Segment>` on the
   trait, a token, a `Drop` that has to wait. **That was wrong.** A graph is a
   value that can be run again, so running it is one operator call that returns
   when the run is finished. There is no boundary inside a run for "submitted but
   not yet waited for" to cross, so the state is a local in the executor, and the
   "who waits" question does not arise.

   The shape is two call-scoped methods beside the three that exist, in the same
   style as the three that exist: `submit`, which records a stretch of dispatches
   into one command buffer and submits it without waiting, and `sync`, which
   waits for the last submission. `run` is `submit` then `sync`. The backend
   holds a **pool of slots**, and acquiring one when they are all in flight means
   waiting on the oldest. Depth is configured from Rust — `GpuContext::new()`
   takes the default, `with_config` takes the rest — defaulting to 2 because that
   is the smallest depth at which recording overlaps too, and 2 is already more
   than today. Whether more than 2 earns its keep depends on how much host work
   a closure does, which is unmeasured.

   **What is left of it is the executor, not the pool.** See
   [The pool of submission slots](#the-pool-of-submission-slots) and
   [The pending submission](#the-pending-submission) for the shapes. The
   contract has a `submit` that hands a submission back unwaited and a
   `Pending::wait`, the pool is two deep by default, and a host program can hold
   two submissions in flight. What does not exist is anything that *decides* to:
   a graph executor that submits a node, runs the closures between, and waits at
   the demand points.

2. **Overlap does not need language-level async, and that is worth writing
   down** because it looks like it does. The GPU being in flight is a driver
   property, and "submit, run the closures for the next stretch, submit again"
   is ordinary straight-line host code with nothing suspended and nothing
   resumed. The closures run *inside* `GraphRun`, between a submit and the sync
   that follows it. lichen needs no `await` and this adds none — which also
   means the "who waits" question that the old shape made unanswerable simply
   does not arise.

3. ~~**Three things a pool needs that one command buffer did not.**~~ **All three
   are now properties of the type**, so this is a record of what the code
   actually does rather than a plan. Each was a silent wrong answer rather than a
   slow one:
   - **Staging is per slot, and there is no way to name another slot's.** A host
     input is `memcpy`'d into staging and read by the device later, so writing
     the next slot's input over bytes an in-flight copy has not read yet hands
     that copy the new data. `Segment::reserve` and `Segment::staging` reach only
     the slot the segment holds, and the segment holds the pool lock.
   - **The descriptor pool is per slot, reset in `acquire` and nowhere else.** A
     set is read when the submission *executes*, so resetting under a running
     command buffer frees sets it is still bound to. The rule settled above —
     reset at a submission boundary — was true at depth 1 and stops being true
     the moment a second submission is in flight. `acquire` is the only place
     that can do it safely, and it is safe there because it has just waited the
     slot's fence or found the slot never claimed.
   - **Bundling the pool into the slot is what makes that safe**, rather than
     one context-wide pool with a hand-maintained in-flight counter. Command
     buffer, fence, staging and descriptor pool share a single lifetime and a
     single owner, so "which of these are mine" has one answer instead of two,
     and there is no counter that can be wrong. The per-segment
     `vkFreeDescriptorSets` alternative was rejected because without the
     `FREE_DESCRIPTOR_SET` flag it exhausts the pool — and while that failure is
     loud, the one it is preferred to fails silently.
   - **`fetch` acquires a slot** like everything else; it is a record, submit
     and immediate wait, and after pooling it no longer has a fixed target. Its
     read of the staging happens *inside* the claim, which is the one thing
     `submit` plus `sync` cannot express.

4. ~~**The submit/wait split.**~~ **Measured** — see
   [And how much of it Async actually reaches](#and-how-much-of-it-async-actually-reaches).
   The shape is the one that was predicted: the hidden amount is
   `min(host work, device time)`, to within 0.05 ms in all ten rows, and it
   saturates at the device's own time. **What it changed is the arithmetic, not
   the schedule.** Async's saving is not a share of the per-dispatch overhead; it
   is one submission's device time and no more, which means it earns nothing on a
   chain of pure kernels and up to a dispatch's worth per node on a graph with
   native nodes in it.
5. ~~**Then, and only then**, the IR crate and the node set.~~ **Done** — see
   [The graph IR, and its one kind of node](#the-graph-ir-and-its-one-kind-of-node).
   What is left of the whole feature is the half that *builds* a graph rather
   than running one: the `compute.graph` operator, the `Graph` value in the
   language, and the lowering that fills `Graph::push` — using the two seams
   already landed, which is what they were cut for. `Batch` needs a
   fused-submission capability in the contract before it can be anything but a
   refusal.

6. **The lowering, and the order of this list is now suspect.** The order above was
   written before the submit/wait split was measured, and the measurement changes
   it: a **kernel-only** graph is worth no milliseconds under `Serial` or `Async`,
   because a pure chain has no host work and `hidden = min(host, device) = 0`. The
   only win for a pure chain is `Batch`. So the two remaining halves are not
   obviously in this order, and saying otherwise would let a shape step pass for
   progress. The case for the lowering first is that it is the prerequisite for
   specifying the fused-submission shape at all, and that it gives the two seams a
   first real user; the case for `Batch` first is that it is where the measurable
   value is. **Decided for now: lowering first, with the native-node question left
   open** — see
   [The half that builds a graph](#the-half-that-builds-a-graph-and-what-it-found),
   which also records what the kernel-only version is and is not worth.

7. **The native node, and it is a real contradiction rather than a task.** A graph
   that holds the closures its native nodes call is what both seams were cut for,
   and `NativeCall` is a bare `fn` pointer that cannot carry one. Decide this
   before writing the first native node, not while writing it.

   **The question was bigger than "what does a native node call", and the count
   work is what made that visible** — but it is now **settled and settled in the
   other direction**: there is no host-call node, because a closure is a compiled
   artifact and lowers to a fragment. `Node::Native` and `NativeCall` are gone,
   and with them the re-entrant apply path and the `Vec<i64>` against `[?b]` type
   gap. See [A number on an edge, and the closure question it
   exposed](#a-number-on-an-edge-and-the-closure-question-it-exposed). What is left
   to build is the graph itself.

## Landmines, each of which is a silent wrong answer

- **A recorded-but-unsubmitted command buffer is clobbered by the next recording
  into it**, and a command buffer whose submission is still in flight cannot be
  recorded into at all. This is the whole reason the pool exists and the reason a
  slot is not released until its fence signals. The single `detached: Mutex<Submit>`
  that stood in for it handled exactly one outstanding submission; it is now
  `Mutex<Slots>`, where a slot's `claimed` flag is the one thing that answers
  "is the device still using this", and the fence is the only thing that clears
  it.
- **A chain that reuses a buffer has a write-after-read hazard that the
  one-buffer-per-link version does not have.** `run_chain` ping-pongs two
  buffers, so link `i` reads what link `i + 2` writes; the trailing barrier
  is widened to order it. The first version did not reuse and did not need
  it, and it was three times **slower** — a pool holding a buffer per link is
  empty at the start of every call, so every call allocates the lot and
  discards most of it. Measured: 2.226 ms fused against 0.679 ms unfused,
  all of it in `vkAllocateMemory`. The memory profile does **not** invert for
  a linear chain; it inverts for a fan-out, and there which buffers can be
  shared is a question about liveness that the graph has to answer.
- **`BufferSlot::Host` inputs are staged by `memcpy` before recording.** A
  submission that grows staging after recording has begun is a use-after-write
  on the mapping — and with several slots in flight, a *different* slot growing
  or writing staging is the same hazard across slots. Reserve for the whole
  submission up front, and never share staging between slots.
- **An uninitialised output buffer is safe only because the emitter emits
  straight-line code** — one `OpLabel`, no branch, so every invocation reaches
  its write and the dispatch covers `[0, padded)`. A branch in a body breaks it.
  See [lichen-compute-gpu.md](lichen-compute-gpu.md).
- **A `DeviceBuffer` value dropped by `drop_block` never calls `release`.** Its
  memory lives until `GpuContext::drop`. Same rule as the deliberate
  no-per-value-release decision already recorded there.
- **A count is a value, and a value asked for the wrong role must be refused
  rather than coerced.** A count edge that resolves to a buffer and a buffer input
  that resolves to a number are two different mistakes, and the tempting repair
  for the second — treat the number as a one-element host vector — is a run that
  succeeds on a kernel nobody wrote. The two roles are separate functions.
- **A graph is a transcript of the run, and the run is lazy — so a dispatch whose result
  nothing reads belongs in neither.** This replaced the opposite claim, which was recorded
  here first and was wrong. A body of three statements — dispatch, bind the result to a name
  nobody reads, dispatch again — produced a two-node graph, and that was first read as a
  missing dispatch. **Running the same body with no graph in the program at all also
  dispatches twice**, so the program itself never performs the third one: a `let` inside a
  block is lazy, nothing read the name, and there is no expression-level CSE in this
  compiler that could have accounted for it instead. The graph was faithful all along; the
  invariant "a graph must be what its function wrote" was the thing that had to go. The
  measurable form of the correct one is
  `a_graph_dispatches_exactly_what_the_program_dispatches`: one body, run plainly and
  through a graph, with the two dispatch traces compared.
  **The landmine now points the other way**: a graph that dispatched *more* than its
  program would be running work nobody asked for, and no refusal would catch it. The
  operand-forcing walk reaches the unread statement and is refused only by accident — it
  empties the function's return slot, so every recording fails rather than succeeding with
  an extra node. A future change that repairs that slot without noticing this would
  reintroduce the bug the number 2 was protecting against.
- **A graph that captures anything is a use-after-free waiting for a
  `drop_block`.** Nothing in the type says so, because `Graph` is plain data and
  a capture is what the *builder* would have done. The refusal is the only thing
  standing between a program and a graph that reads a freed arena on its second
  run, so it is a build-time refusal and not a run-time check.
- **A fan-out graph's memory profile inverts; a linear chain's does not.** This
  note used to claim the inversion without the qualifier, and building the fused
  chain is what showed it was half wrong. `run_chain` ping-pongs two output
  buffers, so a 16-link chain at 1 048 576 elements holds 16 MB rather than
  128 MB and needs no pool deeper than a serial run's. The inversion is real
  only where two buffers are live at once: a node with two consumers, or two
  nodes writing from the same input. There the graph's liveness information is
  the only thing that decides what can be shared, and the recycled-buffer cap
  was tuned for one-at-a-time — so a fan-out that keeps more than a handful live
  will be allocating and discarding on every call, at a cost measured above at
  roughly 0.2 ms per buffer.

## Related

- [lichen-compute-gpu.md](lichen-compute-gpu.md) — the backend, the measured
  costs, and the invariants a graph must not break.
- [compiler-plugin.md](compiler-plugin.md) — the `traced` seam as a plugin
  author sees it is **not written yet**; its "Extension point 5" covers the
  ext-handle payload contract, which is a different thing.
- [lichen-compute.md](lichen-compute.md) — the operator vocabulary and the
  `plrun` path a graph sits beside.
