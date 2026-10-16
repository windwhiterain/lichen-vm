# A class carries the call, not only the fact

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

The collision this note describes has a ruling of its own, with the chain measured behind thirteen compute failures: [a colliding answer is a unify error](answer-collision.md).
