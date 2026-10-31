# A class carries the call, not only the fact

> Status: current — measured: a declared type's class holds this call's cells too,
> so sharing a fact by merging classes shares them as well.
> Points at: `crates/lichen-lowlevel/src/equality.rs` (`alias_read`),
> `crates/lichen-compute/src/compute/graph.rs` (the recording).

**The finding, measured on the reported defect's own program** (`buf_element_class_recorded.lichen`):
the class of the parameter's declared type holds the template's facts and **nothing of this call** when the
recording has built its cells — three members, all valued, no placeholder among them. The call's cells
enter that same class **later**, while the recording evaluates: `alias_read` joins a reader into the class
of the cell it read, by construction.

## Why that matters for a clone

`alias_read` (`equality.rs`) states its own rule: *join `reader` into `target`'s class*, with one exception
(a wildcard target is shared, so merging it would write its value into the reader), and **a failure found
there is dropped on purpose** — *the reader's own reconcile owns that conflict*. So the join is
unconditional and its conflict is deferred.

That makes the class a place where two different things live at once:

- the **template's decided fact** (`Buf Float`'s element class, a declared field's type), which is the same
  for every apply;
- the **cells of this call** — the recording's placeholders (`ComputeValue::GraphValue(n)`) and whatever
  this call's reads joined in.

Any rule of the form "a member the walk leaves in place means the clone joins its class" therefore shares
more than the fact: `unify` operates on **classes**, so it also shares those per-call cells. Measured
consequences, all from the same two forms:

| form | result |
|---|---|
| merge the clone with every class member the walk leaves in place | the conflict is between two singleton placeholder classes, `GraphValue(0)` and `GraphValue(1)`, reported through `write_node_answer` after an `alias_read` — a conflict the program does not have |
| merge only where the member's class already holds a value | same failure: both classes hold values, and the merge still meets the cells |
| copy the member's fact onto the clone instead of merging | the copied fact and the value this call computes differ, so the conflict appears anyway |
| walk the members and clone them all (no merge) | the topology is rebuilt over clones that should not exist, and the pass closes over the module (52 s against 0.2 s) |

Two figures belong beside the table, from the branch's own commits. The first form — merging every in-place
member — was measured **before** the carry rule changed, and it ran the compute suite to **8 passing targets
against 41 failing**: the shared value reached every test that applies a function at all. And the conflict
itself is worth naming precisely: both sides were **singleton** classes, one holding `GraphValue(0)` and the
other `GraphValue(1)`, reported from `write_node_answer` after the `alias_read` that joined them.

## What the design therefore needs

Sharing a fact **without** sharing the class — the maintainer's own framing of the difficulty ("a class
cannot carry an evaluation chain"). Until a class can distinguish the template's decided fact from the
cells a call joined into it, "in-place reference" cannot be implemented by `unify`, and a clone of a class
member cannot be given the member's fact by merging.

The two measurements above are the boundary: the fact is real (the member's class holds a value), and
copying it is not enough either (the call's own value differs), so the missing piece is not a wider or
narrower criterion but a place to keep the fact apart from the class.

**The maintainer's ruling (this round): implement the separation, do not weaken the
tests.** Measured before the ruling: 23 of the remaining 25 compute failures share
one root — a declared cell (the codomain/argument struct's field requirement, e.g.
`OutR.r`, a decided Buf type expression) never receives the recording's runtime
result: a read through the codomain resolves to the template's wrapper with an open
payload, a kernel config field declared `@any : Int` has no value in the compiled
template, and a parameter buffer reads as closed over. The tests encode the
language's surface (a `compute.plrun` result's codomain fields are buffers), so the
work is the separation below, not a test rewrite.

The separation's shape, as the measurements bound it: the **fact** a template states
(a field's type/requirement) and the **cells** a call fills must be stored apart — a
per-recording registry from a declared cell to the value the call recorded, consulted
where a read walks the declaration, instead of a class merge that carries both. The
kernel template's open cells are the same gap on the compile side: a field declared
`@any` is a cell an apply fills, not a node the compile should demand.

**The mechanism, measured on the chain family's second dispatch (this round).** The
recording assembles the first dispatch's output as a fresh wrapper (`19210`, payload
`19190`, a `GraphValue`), and the declared cell it was built from is `2955`; the
second dispatch's argument read (`first.r`) resolves instead to `14506` — an `Index`
node whose value is a `Buf` struct whose `.native` is an **array** (the declared
type term), not a recorded value. So the read returns the **requirement's class
representative**, with the recorded wrapper a member of the same class. Three
attempts to bridge it without separating the storage were measured and left the
suite at 215/25: forcing the cfg deeply, following `field_sources` from the declared
cell, and inserting that source at the declared cell's class root. The separation
therefore has to keep the actual's **cell** apart from the requirement's, not give
the requirement a source.

**Why a source link at the read is not the answer (measured, this round).** The VM's
`Index` read (`evaluation.rs`) was changed to follow `field_sources` — the directed
requirement-to-actual link the checker already keeps — instead of the class. The
suite fell from 215 to **30 passing / 210 failing**: the link records provenance for
the *decode* paths, and following it at every read rewrites reads that already
answer through the class. So the separation is not a substitution at the read.

**The two sites the chain needs, from the code.** First, the checker's `check_field`
(`checker/structs.rs`) bakes the read's value at check time: it takes the
container's current value slot (`value_slot_read`) and stores it as the read's
`val`, so a template's field declaration becomes the read's answer and the VM never
re-evaluates it. The read node does carry a `LowOperator::Index` operation, but a
baked value means the runtime never runs it. Second, the wrapper's `r = k.O` unify
merges the runtime result's class with the codomain *type* value, and the
representative is the type, so appending `r` to the wrapper still returns the
declaration (measured: the suite stays 215/25). The separation therefore has to
make a read of a template's field stay **unevaluated** at check time and let the
runtime's own binding answer it — not to move a value from one map to another.

**The node the recorder finds is a clone, not the checker's read (measured, this
round).** In one run, the checker's `check_named_field` builds the `first.r` read as
`NodeId(12183)` whose value and type are open, while the recording's input decode
(`value_at_path`) finds `NodeId(14506)` — a clone whose `Index` target
(`NodeId(14504)`) is **undecided** yet whose own value is a decided `Buf` struct.
So the stale answer is **not** the checker's read. **Corrected the next round:** a
probe of the clone walk (`node_apply`) shows the read's `Index` clones are fresh
(`proven false`, `closed false`, `template_decided true`, `value none`), so the clone
does **not** carry the template's answer either — the value is decided after the
clone, when the `Index` runs on its target's value. The question is therefore the
**target**: the `first` binding's runtime value inside the applied body, which the
wrapper's `r = k.O` and the codomain's type leave as the *declaration*. A
`record_launch` probe confirms the recorded wrapper itself is assembled
(`GraphValue` payload); it is the binding's value that loses it.

**Where the chain's refusal is recorded (measured, this round).** The diagnostic is
`stage: Check`, and `common::run` panics on it before any run: `Mode::Evaluate`'s
definition pass "runs every body and every top-level statement", so `built := compute.graph
step` is evaluated there and the recording is what the checker/build-time values drive.
The same program's plain dispatch legs are in the same pass, so the defect is the
recording's environment inside the definition pass — not a separate run-time path. That
is why every attempt to fix the read at run time was inert: the read was answered
before the run.

**Measured after the definition-pass reading.** Two more attempts left the suite at
215/25: following the `field_sources` link in the VM's read (30/210, reverted), and
transferring the link across the clone walk so every remapped template with a source
gives its clone the mapped link (inert — the decode still finds the declaration). The
remaining hypothesis is the wrapper's own body: `r = k.O` is a **unify**, and a unify
writes an answer into the operand's node, so `r`'s node carries the codomain value and
appending `r` to the wrapper returns the type. The next measurement prints the
binding's node value at the decode with and without that unify.

**Measured at the decode, after the origin reading.** The decode's declared-branch does
trigger (`buffer 14506, payload 8505`, the open native), but its `field_source_of` is
`None`, and neither following the node's origin chain for eight hops nor inserting the
declared field's source in `record_launch` (`declared -> assembled wrapper`) connects
it: the suite stays 215/25. So the read's declaration (`14506`) is not linked to the
recorded wrapper by origin or by class — the launch's declared field it *was* built
from is a different node. The separation's remaining question is exactly that
identity: which node the launch's declaration and the body's read share, and whether
the read should resolve through the wrapper the launch assembled rather than the
declaration it was cloned from.

**The read's node is stable and carries a written answer (measured).** Across the
wrapper variants (`r = k.O`, appending `r`) the decode finds the **same** node
(`14506`) in every run: an `Index` whose target is undecided and whose own value is a
five-slot `Buf` struct carrying the declaration's nominal id. A node with an operation
**and** a decided struct value is a node an answer was **written** into — the class
bind wrote the requirement into the read's node, above the undecided target. The
wrapper's return value therefore cannot move it, and the separation's site is the
write: the bind must write the call's actual into the read's node, or the read's node
must not be the class's answer while its target is undecided.

**The written answer is not written by any of the four write paths (measured).**
Instrumented on the chain test, the decode's node (`14506`) appears in **no**
`write_node_answer`, `write_node_value`, `write_node_value_if_absent`,
`set_node_value`, class fill, or birth-with-both trace, while `add_node` never
creates a node holding an operation and a value together. `Node`'s own invariant
names the shape — "an operation-bearing member holds that value while the stamp stays
zero, so it still reads as owed an answer" — so the value is an **asserted** one, but
the asserting write is not one of those five sites. The next measurement is
`Module::node_value`'s class route: whether a member's read resolves through the
class representative rather than its own slot.

**`node_value` is the node's own slot, and the value has no traced writer.** The
accessor lives in `static_module.rs`'s `impl<P> Module<P>`: `Dyn(node)` reads
`self.nodes.get(node)?.value`, the static arm its frozen table — no class route. So
the value really is on the node, yet the four write sites, the class fills and
`add_node`-with-both do not produce it. The next measurement must instrument the
**operation attachment** (`self.nodes[clone].operation = operation` in
`function.rs`/`template.rs`) and `node_value`'s read of the decode's node in one run,
to see whether the value predates the operation (a checker-built value node that an
operation was attached to) or is set by a path outside the traced sites.

The collision this note describes has a ruling of its own, with the chain measured behind thirteen compute failures: [a colliding answer is a unify error](answer-collision.md).
