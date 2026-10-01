# Graph JIT: a chain of dispatches as one submission

> Status: **designed, framework seams landed, nothing built on top yet.** The
> two lowlevel seams a graph needs are in and tested; the graph itself, the IR
> crate, and the first measurement are not. Branch `feature/graph-jit`, three
> commits, not pushed.
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

A computed graph holds the closures it will call later, so its value holds
**nodes** past the operator call that built it. Nothing could see that: the GC
walks an array's items, a table's entries, a function's scope, and an
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
and by `lichen-lowlevel/tests/basic/compaction.rs`. Say this to anyone writing
the first real `traced`.

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

## Three corrections to measurements, all of which changed a conclusion

These are the expensive ones. Each was a wrong number that had already been
written into a document or a plan.

**Submit fusion is not an N-fold win.** The empty-dispatch floor is 0.048 ms
(best of 200) and 16 links are 7.17 ms. Removing 15 submissions is 0.72 ms →
6.45 ms, **about 10%** — and 0.048 ms is an *upper* bound, because an empty
dispatch's fence wait has no GPU work to hide behind. Earlier in this work
"N-fold on long chains" was asserted; it is wrong. Note which half of that
arithmetic is trustworthy: the floor is a 200-sample best and reproduces, while
the 7.17 ms is one pass of an example whose chain column is not monotonic in
links, so treat 10% as 10%-ish and the numerator as the solid part.

**Readbacks are not avoidable.** A `Read` the program performs needs the number
on the host, so the data must cross. There is no "unnecessary readback" to fold
away. An earlier claim that a graph could save one, worth 0.5–1 ms, was
invented — the prize does not exist.

**The ~10% is a function of kernel size, not chain length.** 0.048 ms is a
per-dispatch floor *independent of count*, so what decides graph's value is the
ratio of per-dispatch overhead to per-kernel work — and ~10% was computed at
1 048 576 elements, where a kernel is ~0.10 ms and the overhead is noise. A chain
of many small kernels is the realistic target and would show a far higher
overhead fraction. **The decisive measurement is a count sweep at fixed chain
length**, and it has not been run. The crossover example already has the
machinery (200 runs reporting best and median, CPU baseline swapping two buffers
per link); it needs count as a swept variable.

## What is built and what is not

**Built and committed** (`feature/graph-jit`, seven commits, not pushed):

| commit | what |
|---|---|
| `82a7948` | both seams, plus the compaction test |
| `5b881bb` | delete a false claim from the `traced` docs |
| `c5de5dc` | callback → `TraceContext`; why `&Module<P>` is wrong |
| `a2c661d` | this document, and the stale claims in the backend's |
| `05b5402` | `record_and_submit` / `wait_on` split out of `record_and_wait` |
| `6c5ac4d` | the two rules a fused submission depends on — see below |
| `91a324c` | delete the single `detached` submit the new design made wrong |

**Not built:** the graph IR crate, the `Graph` value, the `GraphRun` operator,
the pool of submission slots, and any of the measurements above.

## Two rules settled before anything depends on them (`6c5ac4d`)

Both were landmines on this list. They are decided now, while nothing depends on
them, because each is a **miscompile rather than a slowdown** and so cannot be
caught by watching the numbers. Neither is exercised: no code records two
dispatches into one command buffer, so what exists is the rule and the capacity
for it, not a fused path.

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

## The next step, in order

1. **A configurable pool of submission slots — not a `Segment` object.** The
   earlier plan here said a segment must outlive the `GraphRun` operator call,
   and everything downstream of it was built on that: a `Box<dyn Segment>` on
   the trait, a token, a `Drop` that has to wait. **That was wrong.** A graph is
   a value that can be run again, so running it is one operator call that
   returns when the run is finished. There is no "submitted but not yet waited
   for" state to carry across a call boundary, because there is no boundary
   inside a run.

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

2. **Overlap does not need language-level async, and that is worth writing
   down** because it looks like it does. The GPU being in flight is a driver
   property, and "submit, run the closures for the next stretch, submit again"
   is ordinary straight-line host code with nothing suspended and nothing
   resumed. The closures run *inside* `GraphRun`, between a submit and the sync
   that follows it. lichen needs no `await` and this adds none — which also
   means the "who waits" question that the old shape made unanswerable simply
   does not arise.

3. **Three things a pool needs that one command buffer did not.** All three are
   settled, because each is a silent wrong answer rather than a slow one:
   - **Staging cannot be shared between in-flight slots.** A host input is
     `memcpy`'d into staging and read by the device later, so writing the next
     slot's input over bytes an in-flight copy has not read yet hands that copy
     the new data. Each slot carries its own staging.
   - **The descriptor pool cannot be reset while anything is in flight.** A set
     is read when the submission *executes*, so resetting under a running
     command buffer frees sets it is still bound to. The rule settled above —
     reset at a submission boundary — was true at depth 1 and stops being true
     the moment a second submission is in flight. Each slot therefore carries
     **its own** pool, reset at its own fence.
   - **Bundling the pool into the slot is what makes that safe**, rather than
     one context-wide pool with a hand-maintained in-flight counter. Command
     buffer, fence, staging and descriptor pool then share a single lifetime and
     a single owner, so "which of these are mine" has one answer instead of
     two, and there is no counter that can be wrong. The per-segment
     `vkFreeDescriptorSets` alternative was rejected because without the
     `FREE_DESCRIPTOR_SET` flag it exhausts the pool — and while that failure is
     loud, the one it is preferred to fails silently.
   - **`fetch` acquires a slot** like everything else; it is a record, submit
     and immediate wait, and after pooling it no longer has a fixed target.

4. **The overlap measurement**, which needs the pool: time the record-and-submit
   side and the wait side separately (this decomposes the 0.048 ms, which every
   estimate so far has treated as one lump), then put a controlled amount of
   host work between submit and wait and look at the slope. Flat while the host
   work is under the device time, 1:1 above it — that knee is the proof.
5. **The count sweep**, which decides whether any of this is worth building.
6. **Then, and only then**, the IR crate and the node set.

## Landmines, each of which is a silent wrong answer

- **The fused-submission capacity exists and has never run.** One set per
  dispatch, the widened barrier and 64 sets of pool are all in place, and none
  of it is exercised, because nothing records a second dispatch into a command
  buffer yet. Both rules are miscompiles if they are wrong, so the first fused
  dispatch is the first real test of them — judge it by reading its output, not
  by watching whether it is fast.
- **A recorded-but-unsubmitted command buffer is clobbered by the next
  `record_and_submit` into the same target**, and a command buffer whose
  submission is still in flight cannot be recorded into at all. This is the
  whole reason the pool exists and the reason a slot is not released until its
  fence signals — the single `detached: Mutex<Submit>` that stood in for it
  handles exactly one outstanding submission and is replaced by the pool.
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
- **A graph's memory profile inverts.** Today one output buffer is live at a
  time; batching holds every output of the segment until the flush. The
  recycled-buffer cap was tuned for one-at-a-time. The graph's liveness
  information is what makes this survivable — recycle at flush time, for buffers
  whose last consumer is inside the flushed region — but the peak between
  recording and flush is real and unmeasured.

## Related

- [lichen-compute-gpu.md](lichen-compute-gpu.md) — the backend, the measured
  costs, and the invariants a graph must not break.
- [compiler-plugin.md](compiler-plugin.md) — the `traced` seam as a plugin
  author sees it, under "Extension point 5b".
- [lichen-compute.md](lichen-compute.md) — the operator vocabulary and the
  `plrun` path a graph sits beside.
