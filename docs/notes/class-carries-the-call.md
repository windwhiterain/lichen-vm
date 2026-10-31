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
discriminator. The clone walk's copy of the mark was measured **unnecessary** for the
chain (218/22 with and without it): the checker's reads resolve against the template body's
nodes, not a clone. The kernel compiler's index placement now **looks through a read's
value half** (`Index(Index(container, at), 0)` places the element read, `body.rs`), which
recovers three of the four device regressions and flips `nested_config::a_nested_config_records_each_leaf_for_what_it_is`
with them: 218/22 → **221/19**. `fuse_licence::an_elementwise_producer_feeding_a_reduction_consumer_merges_and_the_device_agrees`
is **fixed** by resetting the body's run stamps before a recording: the function's nodes
are un-stamped in `build_graph` (`Module::reset_run_stamps`), so the recording re-runs the
body it records instead of answering from a cache it inherited from the definition pass.
A probe showed the dispatches were recorded in one recording and the graph's `finish()`
saw none — two evaluations of the graph op, the second answering from the body's run
stamps. Measured: **221/19 → 222/18**, and the engine, language and compute-gpu suites are
unchanged. Seeding the lazy read's value node with the declaration field's value
(`seed_node_value`) was also measured inert (218/22, same four regressions): the
declaration's value child is itself open in these shapes, so no build-time shape can be
seeded from it. The next measurement is the kernel class derivation's own read: which
field of the `Buf` it follows and what the lazy read replaced there.

## The remaining failures' shapes (measured)

With the requirement mark, the recording's run-stamp reset, the three re-pinned digest tests,
the role-path fix, the kernel body's leaf conversion, the maintainer-approved test-data fixes
and the recording's own-conflict rule the suite stands at **232/8**. The
fourteen group by the first diagnostic:

| shape | tests |
|---|---|
| `expected Struct, found none` (an undecided value meeting a struct requirement; the lazy read has no check-time value) | `fan_out_merge::under_a_pack…` |
| `expected Struct, found Struct` — the **declaration's** `.length` pin (16) against the actual buffer (512); the frozen-marker refusal above it is fixed | `cooperative_product`'s three |
| *(fixed)* the annotation `: OutE` — the SPEC's annotation unifies the value's **type**, so a value template is a type error; now `: Struct` | `graph_argument::the_two_buffer_chain…` (green) |
| *(fixed)* the `filled()` bindings and its template annotations, then the recording's own-conflict rule: both tests pass | `nested_config`'s two (green) |
| *(fixed, approved)* the `: Out2` annotation — the same value-template misuse as `: OutE`; now `: Struct` | `graph_runtime_scalar::what_makes…` (green) |
| *(fixed, approved)* the parameter's leaf spelling — `.scale {leaf}` put a **type** in the value position, so the recorded class was Int and no mismatch could fire; now `.scale @any : {leaf}` | `float_leaf::a_float_leaf_and_an_integer_value_are_refused_in_both_directions` (green) |
| a direct launch after a recording: "argument 0 … is a buffer and was not read from this function's parameter" — the recording's argument resolves to the declaration | `float_leaf`'s two, `graph_runtime_scalar::what_makes…` |
| the collect refusal: **measured** — the module's `extension_diagnostics` holds it (first of five, `compute.graph: a collect is asked here …`) and `diagnostic.rs` collects every `fails_build` entry, yet the test's message list shows a `compute.parallel` sentence in its place and no collect sentence: so the loss is in the **report's `out` assembly** (a dedup or an overwrite between the check's attributed diagnostics and the extension entries), not in the recording | `graph_jit::what_a_recorded_body…` |
| the write's perspective: the control refuses `expected 32, found 0` at `.to`, and `expected 0, found 1` at `(w # 0)` — measured site: `check_instantiate`'s decided field branch calls the **plain** `check_unify` on the whole exprs, so the perspective extension's sub-type relaxation (`is_subtype(declared, value)`, which would accept 0 under 32) never runs for an instance's field attribute; the second error is the write's own result perspective, which the prelude's `# (perspective_of x.to)` does not put on the result. Measured: skipping the attribute slots at the instantiation (unifying value and type only) regresses two green perspective tests (225/15), so the attributes must go **through the extension's `unify_slots`**, not be skipped; and the prelude's two spellings (the direct `# (perspective_of …)` and the `r := …; r = …` form) refuse identically, so the second site is the computed-attribute replay, not the spelling. Delegating the attribute slots to the extension's `unify_slots` (per marker, at the instantiation) was measured **worse** twice: all positions 226/14, and only **both-decided** positions 231/9 — `an_imported_requirement_judges_like_an_inline_one` regresses in both, and the write test still refuses. The instantiation is therefore not where the perspective relaxation belongs; the deferred call-site replay is the site, and it needs its own measurement | `perspective_requirements`'s two |
| the documented park | `compute::a_float_domain…` |

Deferring the whole-term unify for **every** undecided actual against an open template was
measured **75/165**: the value unify is load-bearing for the ordinary matches, so the fix
for `fan_out_merge` must be narrower than "a requirement never unifies".

**`cooperative_product` has two layers, measured.** The first refusal is the checker's
`check_unify(template_ty, struct_template())` for the **frozen** `compute.Mma` template:
its type is the frozen `Struct` marker (a static module's node), and the unify against the
checker's local marker reports "expected Struct, found …". Skipping that unify (measurement
only) reveals the second layer: the `Mma` declaration pins `.a`'s `.length` to 16 while the
test passes 512-element buffers, and the instantiation's value unify reports
`expected Struct, found Struct` with `value_a: USize(16)`, `value_b: USize(512)`. So even
after the frozen-marker unify, the test's program and the prelude's `Mma` declaration
disagree about what `.length` means — a question the maintainer's ruling should settle.

**`float_leaf`'s scale program has a placeholder leak, measured.** After a graph recording of
`scale`, the program launches `scale` directly with a real buffer, and that launch meets
`expected Struct, found Struct` with `value_a: ComputeValue(Buffer(..))` against
`value_b: ComputeValue(GraphInput(3))`: the recording's placeholder is compared with the
graph's own input, so a placeholder from the recording reached the *shared* kernel
parameter that the direct launch reads. The graph's own dispatches are refused first
("argument 0 of this dispatch is a buffer and was not read from this function's parameter"),
so the recording links `s.in.a` to the declaration rather than the placeholder. Neither
`assemble_parameter` (it builds a new struct) nor `field_sources` (a map) mutates the
declaration, so the next probe is the read's source in the recording (`s.in.a`).

The probe answered it: at the recording's input path `[3, 0]`, the first recording's payload
node (`1610`) holds `GraphInput(3)`, and the later recordings of the same test hold
`ComputeValue(Buffer(..))` in that same node — a recording's placeholder node is
**overwritten by a runtime buffer** between runs. The write is what the next round hunts:
the placeholder node is created by `build_graph` and nothing in the recording writes it, so
the later program run's parameter unify does.

**The recording-argument family, measured.** The five tests that refuse with "argument 0 of
this dispatch is a buffer and was not read from this function's parameter" share one shape: a
module that is evaluated more than once. The first evaluation's recording resolves the
dispatch's input path to the program's own instance node (the same `Buf` wrapper across
evaluations, its native cell the same node), and that cell holds `GraphInput(3)` — so the
first recording's parameter binding wrote a placeholder into **the program's shared
check-time node**. The second evaluation finds the same cell holding a runtime `Buffer` (a
later launch's answer), and the dispatch refuses. The fix is the recording's parameter
binding: it must write into the per-recording clone, never into the program's instance cell.

A second reading narrows where the sharing is: the three recordings' **cfg** nodes differ
(`6051`, `9666`, `10242` — one fresh cfg per evaluation), while the node the input path
resolves to is the **same** (`3179`) in all three. So the clone walk shares the program's
decided `Buf` cell into every cloned cfg, and the recording's write lands there; the fix
belongs to the clone walk's sharing rule (or to the unify's write), not to the placeholder
assembly.

A third reading rules out `node_apply`'s own closed-cell rule: a probe printed every
array-valued node the clone walk meets, and **none** was judged closed (`closed=false,
depends=true` on every one), so every such node *is* cloned. The shared cell is therefore not
a node the walk visits at all — it is reached only as an **item of another node's value**
(the cfg's nested `In1{…}` field), and the walk copies a node's own slot without walking its
value's items. The next probe is that value-item path: the clone must freshen the open slots
inside a value, per `docs/notes/apply-clone-ownership.md` (or the recording's unify must copy
before it writes).

A fourth reading answers it: the shared nodes are the ones `node_apply` returns **as-is**
because they sit outside the apply's scope (`class_inside=false`, printed `SHARED … block=1 …
items=5`; block 1 is the program's top level). That is the owner-tag rule
[`apply-clone-ownership`](apply-clone-ownership.md) states — a node outside the call's template
is referenced, not copied — so the sharing is **by design** and the defect is the recording's
reliance on it: the body's read of the parameter's buffer resolves to the shared cell, and the
placeholder binding writes there. The fix is the recording's read resolution (it has the
per-recording cell in `field_sources`, recorded by `record_graph_parameter_sources`) or a
copy-before-write in `record_launch` — not a change to the clone's sharing rule.

**The writer is the launch, not the recording, measured.** A value journal over the
recording's apply (every write to a node the window did not create, restored on every exit,
`Module::begin_value_journal`/`rollback_value_journal`) left the five tests' answers
**unchanged**: the Buffer reaches the shared cell from the **direct launch's** apply
(`out := plrun shape (Arg3{…})`), which runs between the first recording and the second in the
definition pass. So a journal scoped to the recording cannot help; the write is the ordinary
call's parameter binding.

**And the clone walk shares value-less cells.** The apply's `wildcard` branch freshens only a
cell whose value is `LowValue::Any`; a template cell that holds **no value** falls through and
the outside-the-scope return shares it. Broadening `wildcard` to
`value.is_none() && operation.is_none()` was measured **worse**: 225/15.

**A compute-free example does not reproduce it.** `T := {.a @any : Int}; f := x = T => x.a;
(f (T{.a 1}), f (T{.a 2}))` answers `(1, 2)`; the nested-template shape
`step := (s = P) => Arg{.n s.n, .I In{.d s.in.d}}` applied to two instances answers
`((1, (2,)), (3, (4,)))`; and a placeholder argument followed by a concrete one answers
`((none, (none,)), (3, (4,)))`. Repeated applies are correct at the language level, so the
defect is the compute layer's binding of a **runtime buffer** into a shared template cell.

**And the compute-side mapping does not reach it.** `record_graph_parameter_sources` maps a
body node to the recording's cell by `param_path(module, parameter, candidate)`, which walks
an `Index` chain; the shared cell that the argument resolution reads is the declaration's
**bare native cell** (the `Buf`'s `@any`, no operation), so `param_path` answers `None` and no
source is recorded — measured: `PAY payload=NodeId(1810) source=None` on every recording.
A structural pairing of the declaration's tree with the placeholder's (the placeholder is
built from that declaration's field list, `assemble_parameter`) is the route; the first
attempt crashed on a cyclic value walk and the safe version needs a `visited` set and the
pairing itself, which is the next step.

**The writer, named: the prelude's `plrun` binding.** `plrun := k = K => a = (k.I _) => …`
unifies the runtime argument `a` with `(k.I _)`, and `k.I` is the **declared input group**
(`In1`) substituted for the prelude's implicit parameter `I`. So the unify merges the
argument's cells with the *declaration's* cells — the `In1` native is the cell measured as
`1810` — and a later launch's buffer lands there. The pure-language shape is not expressible:
`In := {.a @any : Int}; K := {.I In}; apply := (k = K) => (x = (k.I _) => x.a)` is refused
("this field read cannot be checked: the container's type is not decided here"), because `k.I`
is the compute package's implicit type parameter and not an ordinary value field. The next
site is the checker's **implicit-parameter substitution**: substituting a declaration for `I`
must instantiate its open cells per call (`instantiate_requirement_value`), which is the same
rule the language already applies to a value template.

**The class merge is the SPEC's own rule.** `docs/language-spec.md` §Lowlevel states it: *"Nodes
inside same class must share same value, class is merged by unify."* So the unify of a runtime
result with a declaration (`r = k.O`) **must** merge their classes, and every open cell of the
declaration then holds the runtime answer; no engine-side exception can be added without
changing that sentence. And the requirement is load-bearing: dropping `r = k.O` from `plrun`
alone answers **163/77** — the chain's fixes depend on it. The two remaining routes are the
declaration's instantiation (the `I`-style type function, which changes the surface) or the
recording's argument resolution (take the placeholder at the argument's path and validate by
provenance rather than by class, which is the next measurement).

**The provenance resolution is in** (`recorded_input_node`): a marked read's chain is walked
from the cfg, and each step whose **container** is linked to a recording cell resolves at that
cell — a value-half read (`Index(container, 0)`, the mark's lazy value) resolves at the
container's own cell, a field read resolves at the same field of it. Measured: the graph
refusals disappear from the five tests; they then failed one step later, at the **runtime
unify** of the polluted declaration slot against the new placeholder (`expected Struct, found
Struct`, `Buffer` vs `GraphInput(3)`).

**Three more readings** (after 230/10): `graph_jit`'s scalar variant never runs the `Call` op
at all — a probe of `unrecordable`'s gate during the recording printed only `Read` entries, so
the "a graph has no node for one" sentence cannot come from that arm and the body ends with the
return refusal instead. `float_leaf`'s class refusal lives in `lichen-graph-ir`'s
`refusal.rs` (`ScalarClassMismatch`), but the compute runner's `RunArgument::Count`/`Float`
skip the class comparison (`compute/graph.rs`'s `classes` walk), so an Int handed to a Float
leaf answers instead of refusing — the test's wording names a check the runner does not make.
`fan_out_merge` gets one value back from a body whose return is two sibling dispatches
(`returned_value_ids` does record `[Value(1), Value(2)]`); the run's multi-value result is a
plain array of expr slots, and `: Array` on the graph run then fails at check with "expected
Struct, found none" — the graph run's **value** is undecided at check (`$graphrun`'s
`NativeApply` is `decided: false`), so no annotation on it can be checked. Removing the
annotation instead makes `r[0]` refuse ("array read cannot be checked: the container's type is
not decided"), and `: Struct` moves the refusal to the graph op's own return check ("expected
Array, found Struct"). The test needs a **declared result type for the graph run**, which is a
typing decision, not a local patch.

**`cooperative_product`'s next layer, measured.** After the `Mma` lengths became element
counts, the unify's two sides print byte-identically (`Buf Int 256`) and still disagree: the
first difference is the value's **name table** — a **dynamic** handle against a **static** one —
and `LowValue`'s equality compares the handles. A table is a mapping of names to positions, so
the unify now compares tables by content (`name_tables_agree`: same length, same
`(name, position)` entries, whichever module holds them). That removes the table difference and
the next one is the **nominal type id**: the frozen `compute.Buf` type carries the **static**
module's occurrence id (`BUF_NOMINAL_ID = 0`), a local `compute.Buf` occurrence mints the
**dynamic** IR index (99), and `TypeValue::TypeId`'s invariant — *equal ids unify, different ids
do not* — makes them two types. That is a cross-module nominal-type identity question, not a
data fix. The maintainer's ruling: **static and dynamic ids must mirror** — a materialized
frozen template's nominal id is the static module's id, so a frozen `Mma.c` and a locally built
`Buf` are one type. The check has no static origin on an IR expression (`ExprState` carries
none), so the mirror needs a mechanism, not a local patch.

**Where the ids collide, measured.** The value error's root is one node compared with itself
(`root=(10884, 10884)`) holding `TypeId(0)` against `TypeId(99)`: two nominal-id cells were
unified into one class by the field unify, so the conflict is a **class value conflict on the
type id** — the frozen `Buf` declaration's id (0) meeting a locally minted occurrence id (99).
`check_instantiate` already reuses the template's id for an instance (its `value_items` copies
`nominal_node` from the template), so the local id is minted earlier: the occurrence whose
struct value is *lowered locally* rather than materialized from the frozen body. The mirror
therefore belongs where a frozen template's body is lowered for a local call, not in the
unify's comparison.

**And that unify is the recording's own.** `build_graph` now snapshots `unify_errors` before its
apply and truncates them after it: the binding conflicts a recording meets are the recording's,
not the program's. Measured: **230/10** — `nested_config`'s two tests and
`float_leaf::a_float_scale_beside_two_extents_records_and_answers_on_the_device` pass, and the
other recording-argument tests fail one step later (graph_jit's reporting gap,
graph_runtime_scalar's annotation unify, float_leaf's empty report).

Measured the first route: resolving the input path through `field_sources` reaches the
recording's own placeholder, but the shared cell then meets it in a class the *earlier* run
already wrote — the unify reports `Buffer` against `GraphInput(3)` — so the resolution alone
does not help while the shared cell keeps its stale values. The remaining route is the
copy-before-write: the recording must hand the body its own copy of the argument tree.

The run-stamp reset is **not** the cause: disabling `build_graph`'s `reset_run_stamps` leaves
the five tests red and turns `fuse_licence`'s elementwise merge red again (226/14), so the
reset is needed and orthogonal. The recording's re-run is what exposes the shared cell, not
what creates it.

Cloning every struct value in the recording's apply (a `whole_clone` flag that skips the
clone walk's two sharing returns) was measured **worse**: 226/14 — `fuse_licence`'s elementwise
merge regresses again and the five tests keep their refusal. So the copy must be narrower than
"every struct value the walk reaches", and the family needs the maintainer's design input:
which values a recording owns, and where the placeholder binding's write belongs, is a
clone-ownership decision, not a local patch (`apply-clone-ownership.md`).

**`graph_jit`'s missing refusal, measured.** The "a collect is asked here…" sentence is
recorded (`unrecordable`'s guard is true during the recording, and the module's diagnostic
list shows it), but it never reaches the report: the collected error list is a *different*
collection (its count restarts between two records, so the run path's diagnostics are not
the `evaluate_raw` report's). The placeholder check now also recognizes the assembled `Buf`
**value** (`[names, id, native, type, length]`, whose native slot's first item is the
payload the recording placed) instead of only the two-item wrapper; that arm is true for
this program and the test still fails for the reporting gap.

**The example drift is the look-through's placement, measured.** A `matmul` probe prints its
inputs correctly (`a = [1..6]`, `b = [7..12]`) and the product wrong (`[29, 32, 101, 112]`
against `[58, 64, 139, 154]`), so the kernel computes from right buffers with a wrong body
value: disabling the look-through in `body.rs`'s `index` makes the compilation **refuse**
("a kernel body's index cannot be placed") rather than answer, so the look-through is
required for the body to compile and its placement is what changes the numbers — it returns
the element **slot** where the value half names the slot's **value item**. The next round's
fix is in that placement, not in the mark.

**The mark does fire inside a kernel body.** `write`'s prelude body is `r := $write(..); r = (Buf
_) # ..`, so a kernel body that calls `compute.write` marks the write's own result, and the
kernel's `.c` field read is then a lazy `Index(Index(..), 0)`. The look-through places that
read, so the write's output position can be placed as an input read — which is the shape a
wrong product needs (the two `s(0)` uses in the fold saw different iteration values in the
measured `[29, 32, 101, 112]`). The next probe is the compiled body of a one-write kernel.

**The compiled matmul body, measured** (`lichen-tools`'s `zz_kernel_body_probe` prints it):
the body's parameters are `[n, cols, k, index]`, and inside the fold's blocks the reads
`r * k.k` and `(s(0) - 1) * k.cols` compile with **leaf 0** (the extent) where leaves 2 and 1
belong, while `i / k.cols` outside the fold compiles with leaf 1 correctly. The write's
value is a **fold-block parameter** (`BlockParam(3, 1)`) instead of the fold's result
(`Value(33)`). So the drift is the kernel compiler's leaf resolution for reads homed in a
nested block, not the arithmetic: the reads' steps are dropped or collapsed to the first
leaf. A one-write kernel's body (no fold) compiles correctly, which is why only the
fold-using examples drift. The fix converts the steps through `physical_param_path`
before the shape walk: the steps are physical item indices (the struct's two header slots
included), while the shape speaks role positions. Measured: `k.k` and `k.cols` resolve to
leaves 2 and 1, the matmul example's first product element answers `58.0` again, the
survey's drifts drop from six to four (`gemv` and `matmul` clean, `topk` still drifts), and
the compute suite stays 226/14.

The collision this note describes has a ruling of its own, with the chain measured behind thirteen compute failures: [a colliding answer is a unify error](answer-collision.md).
