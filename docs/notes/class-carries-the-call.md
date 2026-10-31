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

The collision this note describes has a ruling of its own, with the chain measured behind thirteen compute failures: [a colliding answer is a unify error](answer-collision.md).
