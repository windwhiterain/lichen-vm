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

**The untraced writer is `fill_class_holes` (measured, next round's condition).** Its
loop is the fifth write site: a merge gives the class's value to every hole, and an
operation-bearing member is a hole — the `Node` invariant's "an operation-bearing
member holds that value while the stamp stays zero" is exactly how the read's node
acquired the declaration. Skipping **every** operation-bearing member in that fill
regresses the suite to 200/40, so the condition must be narrower: skip only a member
whose operation's target is undecided, or whose filled value is a declaration holding
open cells — measure both next.

**Both narrow gates on the class fill were measured and regress.** Skipping an
operation-bearing member when the filled value is a declaration (`answer_elements_are_undecided`
over the value halves) gives 206/34; skipping only a member whose operation target is
undecided gives 209/31. Neither flips a chain test, so the class fill is load-bearing
elsewhere and the read's declaration is not gated by either condition. That closes the
localized-site list: read, clone, wrapper, sources, write paths, class fill — each was
measured, each is either load-bearing or inert. The remaining fix is **global**: the
value model must keep a declaration's fact apart from the cells a call fills, and that
cannot be reached by a condition at one site.

## The defect's minimal form

One expression is enough to read a declaration out of a place, no compute library and no
kernel:

```lichen
(_ = {.r Int}).r
```

It prints `raw none: Type` — the declaration `Int` itself (a value of type `Type`) — where
the field's value should be the actual's, and the actual is undecided. The mechanism is the
one unify: `_ = {.r Int}` is the undecided-against-decided write arm, so the struct value
whose `.r` is the type expression `Int` becomes the class's answer, and `.r` reads it. The
compute form only enlarges the value: `OutR := {.r (compute.Buf (compute.BufParam{.type
Float, .length 4}))}; x := _; x = OutR; x.r` prints `(raw none, Float, 4): Struct`, the Buf
declaration. `plrun`'s `r = k.O` is the same unify with the codomain in place of `OutR`,
which is why the chain family reads the declaration.

**The maintainer's ruling overrules the "pin is check-only" reading.** A template's
determined content is a **requirement the unified object must agree with, and it is the
answer** while the object is undecided: `(_ = {.r 5 : Int}).r` reads `5: Int` and that is
correct. `@any` is the spelling for "no requirement". So the defect is **not** that the
determined value is installed — installing it is the semantics — and the proposal's
"closed value is a pin only / the template is never a place's value" premise is
superseded.

What still holds is the **open** part: a template's open cell (e.g. `@any`, a buffer's
native) must be filled by the actual, and the read must answer the filled value. Measured
on the same template:

```lichen
t := {.r @any : Int}
x := _; x = t
y := t{.r 7}; x = y
x.r
```

answers `7: Int` — the actual joins the declaration's class and the open cell fills. So
the minimal open-cell path already works; the chain family's defect is that its recorded
value **never performs that join**, and the read therefore answers the declaration's open
cell. The fix's locus is the join, not a gate that refuses the install.

**The join is blocked by the directional wildcard rule (measured in the code).** When the
recorded value meets the codomain's declaration in `write_node_answer`, the unify descends
to the declared `@any` cell: the left side is the recorded, decided value and the right side
the held `Any`. `unify_inner`'s wildcard arm returns success for that pair **without
joining the classes** — the documented rule is "a left `Any` joins an undecided right class;
a right `Any` leaves the left class unchanged" ([replay-apply](replay-apply.md)). The
classes stay apart, so the read cannot see the recorded native. The minimal same-template
fill (`x.r` answering `7: Int`) worked for the opposite reason: the actual's class was
merged with the place's, so the class read found the decided member. Changing the right-`Any`
arm is an **invariant change** and needs the maintainer's permission.

## The join's minimal form, and a misuse to avoid

**The maintainer's correction: reading a template directly is invalid usage.** `@any` means
the field is *never specialized*; a template is a **constraint**, not a place. To constrain
something, the *thing* is unified with the template (it is an instance of it), and the read
is of the **thing**, never of the template. So the programs that wrote into `x.r` after
`x = t`, or read `x.r` off the template, measured a misuse, not the join — their expected
answer is not the value written there.

The valid shape, measured working:

```lichen
t := {.r @any : Int}
y := t{.r (1 + 2)}
y.r
```

answers `3: Int`: the instance's cell holds the runtime value, and the read is of the
instance. The language-level instance path is therefore fine.

The chain's failure is in the **launch/recording path**, not in this language shape: the
launch's result should be the thing (the recorded buffers), the codomain only constrains
it, and the read `first.r` should read the recorded thing. Measured, it reads the codomain's
declaration instead (the decode finds the declared `Buf`, `raw none` native), so the
dispatch's result is resolving to the template rather than to the recorded instance. The
fix's locus is that result value, and the measurement is the chain tests' answers.

**The answer-over-requirement rule (implemented, measured).** `write_node_answer` now
lets a slot **holding a template's requirement — a value with an open cell — take the
answer**, while a slot holding a decided value is still left alone (`equality.rs`). A
requirement is not the thing's value, so a read of the thing answers what ran. Measured:
compute 215/25 → **216/24**, the flip being `compute::jit_inline_nested_function`, the
accepted regression recorded in `apply-clone-ownership.md`; the language, engine and
highlevel suites stay green. The chain family is **not** moved by it, so its answer
arrives through a node the rule does not reach — the recorder's decode path is the next
site.

**The chain family's current refusal is a check-time dispatch diagnostic, not a decode
result.** Measured on `chain_merge::the_merged_chain_answers_the_un_passed_program_and_a_moved_bias_moves_it`:
the build reports "compute.graph: argument 1 of this dispatch is not a buffer" (span
39:9, stage `Check`) at the body's `first := compute.plrun total ((Arg){.n s.n, .I
In{.d s.in.d}})`, and the second diagnostic is its consequence. So before any answer is
read, the dispatch's argument loses its buffer shape — the `In{.d s.in.d}` instance's
`.d` leaf is seen as something else at the place walk. That is the next site, and it is
upstream of the decode the proposal targeted.

**The chain's refused input, measured.** At the build, the failing input is the second
dispatch's `.p` (path `[1, 1]`): the node is the assembled `Buf` (`14506`), and
`buf_payload` answers its native **element slot** (`8505`, an `Array`), whose value item is
the payload the recording placed — an open slot, so `recorded_buffer_payload` cannot reach
a decided `ComputeValue` and refuses "argument 1 of this dispatch is not a buffer". Reading
the slot's decided value item first was measured **inert** (216/24 unchanged), so the
payload is not merely behind the slot's item; the item itself is open at this point. The
next measurement is where that item's value is written: the payload the recording placed
for the first dispatch's output.

**The refused input is the declaration read, and gating the read on openness is far too
broad (measured).** The focused probe shows the second dispatch's `.p` input is the
codomain's **declaration** `Buf` (`14506`): its native field is an open node (`8505`, no
value), so no payload can be recovered from it. The read `first.r` therefore answered the
declaration — the answer-over-requirement rule fires only when the *container's node* takes
the answer, and `first`'s value was the declaration at check time, so the read's field was
baked from it.

Making the checker's `value_slot_read` skip its bake for a container whose value holds an
open cell — so the read stays a lazy `Index` — measured **72/168**: nearly every decided
struct and buffer value contains a type-level field whose tree has an open cell, so the
bake was disabled for ordinary reads. The read's bake cannot be gated on openness alone; it
needs a narrower marker of "this container is a requirement, not a thing".

**The requirement mark makes the chain family green (measured).** A match of an undecided
place against a template now marks the place's value node
(`Module::requirement_nodes`; the checker sets it in `check_unify_expr` when
`requirement_is_open` holds for the expected side), and `value_slot_read` keeps a marked
container's read **lazy** — the value stays an `Index`, while the declared type is still
the read's — so the runtime answer is what the read answers. The clone walk copies the
mark. Measured: the four `chain_merge` tests and two `runtime_scalar_leaf` tests flip
green, and four tests regress (`compile_order_class` two, `fuse_licence`'s elementwise
merge, `graph_runtime_scalar`'s second scalar), a net 216/24 → **218/22**. The
regressions are all device-compile/answer tests: their dispatch path needs the baked
read's check-time shape, so the mark must be narrowed (or the read must keep both a
check-time shape and a lazy value) before the device tests can pass. Narrowing the mark to
`apply_result_owners` was measured: it loses the chain flips (216/24, the chain red), so
the plrun result is not an apply result in that map and the narrowing cannot be the
discriminator. Seeding the lazy read's value node with the declaration field's value
(`seed_node_value`) was also measured inert (218/22, same four regressions): the
declaration's value child is itself open in these shapes, so no build-time shape can be
seeded from it. The next measurement is the kernel class derivation's own read: which
field of the `Buf` it follows and what the lazy read replaced there.

The collision this note describes has a ruling of its own, with the chain measured behind thirteen compute failures: [a colliding answer is a unify error](answer-collision.md).
