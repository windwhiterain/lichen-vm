# Open issues after the slot isomorphism

> **Status:** current. The attribute slot is handled exactly like the type slot — every
> expression term is a full-width `[value, type, attributes]` slot, every reader takes a
> slot the same way, and the checker states its facts instead of reading values that
> the check has not decided. The evidence is the parity suite: **`lichen-highlevel`
> 136/136** (the suite built for exactly this question), `lichen-language` **275/275**,
> `lichen-lowlevel` **164** (one ignored), compute JIT **30/40**. This note lists what is
> still open, in one place, and points at the measurements rather than repeating them.

## 1. The compute graph block is red — a regression of the slot encoding

Ten `graph_jit` tests in `lichen-compute` are red (JIT **30/40**). What matters is the
change of the failure, not the count:

| point | reading |
|---|---|
| `9ed0ee4b`, the attribute squash | the ten `graph_jit` tests **pass** — 10/10, measured in a detached worktree |
| `4645b439`, the pair-shaped slot | JIT **29/40**, "was 20/40": the drop happened in the compound-slot commits *after* the squash |
| now | JIT **30/40**; nine of the ten stop in the low type, one on its refusal message |

So the earlier diagnosis — [graph-recording-rounds](graph-recording-rounds.md), measured on
`e659607e`: no dynamic write, an empty argument cell, the rule side holding no identity —
describes the *pre-squash* failure and no longer matches the tree.

**The shape question is settled** (`a2fd2796`, the ruling in
[lowlevel-low-types](lowlevel-low-types.md) §8): a class states the variant of its node's
value, and the highlevel judges it from the node's type node. The ten tests no longer stop
on the "a class states one shape" assertion.

**Three seams are fixed** (`6c07f147`): `param_path` counts field selections rather than
the encoding's value reads, `assemble_parameter` builds a structure of slots, and
`place_of_node` walks through type cells. Graph JIT is **1/10** (was 0/10); the whole
compute suite is **97 passed** (was 81 at `7ac3e639`'s predecessor measurement).

**Both perspective shapes are green now.** `195600fa` restored the definition-time
record (`return_marker`/`return_field`: the marker and the parameter field a body's
reference follows), which carries the *field* reference (`write`'s `perspective_of
x.to`); `02770d31` materializes the frozen return pair through the **signature's** remap,
so its attribute graph references the parameter clone the type-side check binds and the
*whole-parameter* reference (`read`'s `perspective_of x`) decides per call site. Measured:
`perspective_requirements` **4/4**, compute **99 passed**, language 275, highlevel 136,
lowlevel 164 (1 ignored), graph JIT 1/10.

**The write's value reaches the marked body through a pair peel.** The backtrace from the
loop lowering: `buffer_write → write_call → value → … → conditional → value_item → value →
apply → lower_loop`. So the write's `.value` operand is the **fold's inlined `if`**, not
the call: `NativeArg.value = value_of(field)` takes a compound field's *value*, which for a
call argument is the call's **pair value** and, undecided, its class's definition — the
function's return expression. The emitter then lowers that body as a loop. The next fix is
upstream of the emitter: the write's value operand must be the call **expression**, not its
pair-peeled value.

**The nested binding is correct; a plain kernel's comparison compiles.** Two measurements
this round: (a) a **plain kernel** writing `i >= 7` compiles (`--- ok true`), so the emitter
resolves the frozen `greater_or_equal` correctly for a *checker* node; (b) every
`static_function_apply` binding reports `same: true` — the cloned parameter's value class
equals the argument's value class. So the clone's binding is fine and the difference is
**clone vs original**: the loop's conversion condition is the `Geq` *clone* (origin = the
apply node), and the emitter's `Define::Opaque` origin fallback requires the origin's
function to be the emitting function, which a clone's origin (the apply node) is not. The
next fix is that fallback through a static clone's origin chain.

**User steer (round 75): keep investigating the checker's stability.** Presented with
(a) the checker's type pass and the peel route, (b) switching to the parallel family's
peel-independent failures, (c) freezing the campaign for handoff, the user chose **(a)**:
the checker's stability is the next target, and the peel stays out until it is fixed.

**Site 1 is both the graph family's gain and the frozen overflow's trigger.** Peeling only
the buffer and recorded-scalar sites (the launch's input buffers and the recording's scalars,
**not** the launch's scalar leaf): the frozen test is **graceful** and graph_jit stays
**1/10**; adding the launch's scalar leaf back gives **6/10** and the frozen overflow. So the
five sites split cleanly: four are safe and worth nothing on their own, and the one that pays
is the one that explodes the class-crossing test.

**Regression inventory (user steer, round 79): the peel's neighbourhood is the biggest lever.**
The 133 failing tests of `lichen_compute` group as: **compute 24**, **fuse_licence 10**,
**graph_jit 9**, **fused_region 8**, pack_placement 6, then a long tail of 1–4. The first four
groups (~51 tests) are exactly the families the cfg-leaf peel reaches: with the peel the
graph family alone moves 1/10 → 6/10, and their refusals are the same "the return is not a
value a graph can hand back" and the extent read. So the inventory does not dodge the crash:
**fixing the frozen overflow is worth ~50 tests**, more than any other single root cause on
the list. The tail (pack_placement, the merge/fan families) is separate work.

**The reader channel is in, and the correspondence is the one missing piece.** Landed
(`b0d5c8e`): `compile_fragment` collects the class roots the parameter's **type** tree names,
the body accepts a walk base that stands on one of them (or on a domain leaf), and the
dereference gained a width reading — a pair or a slot is `2 + attributes` wide, a tuple's
elements are slots. Measured **neutral (225)**, because the type tree's nodes and the value
tree's nodes are in different classes: the walk's base (an element pair, class 671) is in
neither. A scan of the template's **slots** would give the correspondence, but reading a
node's array items is `unsafe` and crashed (`STATUS_ACCESS_VIOLATION`) on the frozen handles a
package slot can carry, so it was removed rather than shipped. The two ways to finish are a
**safe lowlevel read** that refuses non-dynamic handles (`slot_items`) feeding a per-fragment
map, or the **checker** unifying each element's value class with its type node's class.

**Settled: the runtime compares the value and the type; the attribute comparison is the
checker's.** Every expression's attributes carry the default (`1` is `[1, Int, 0, 0, 0]`), so no
slot is ever missing and no hole needs filling. `apply_parameter_check` unifies `0..2`: the
attribute positions state `0` (the lattice identity) on one side and a requirement such as `32` on
the other, and `32 | 0 = 32` is an order fact the lowlevel does not have, so widening that loop
refused six perspective programs with `expected Int, found Int`. The requirement is judged where
the order lives — `unify_declared_value` plus the deferred requirement replayed at the apply — and
all four suites measure green (compute 219, highlevel 136, lowlevel 164 with 1 ignored, language
275). **A measured shape worth keeping: an attributed parameter's type and attributes meet in its own
pair.** At `lambda.rs:127` the parameter's **type cell** is empty and the denotation is a pair, and
`parameter_type`'s kind is not `Annotation` — which is why the "inner type" candidate was neutral:
there is no inner annotation to unwrap. The attributes of `i : Int # 32` are the **parameter's**
schema tail, and `type_denotation(parameter_type, Some(parameter))` combines them with `Int` into
the pair the type slot holds. The failure that misled the four probes was not this pair at all: it
was the runtime's attribute compare (the section above).

**Resolved: the six came from widening the runtime's pair unify, not from the parameter type.**
The probes on the type cell were neutral because the type slot was never the site. The
`DiagKind::Runtime` reading, then the mismatch values (`value_a = USize(32)`,
`value_b = USize(0)`), named it: `apply_parameter_check` was comparing the attribute positions
structurally in the lowlevel, where the perspective order does not exist, so `32` against the
default `0` refused. Narrowing that loop to `0..2` and leaving the order to the checker's
`is_subtype` (the mechanism `AttrExt` documents) is what makes the six pass.

**Neutral, and it should have been: the type slot is not the site.** Changing `lambda.rs:127`'s
denotation to the plain type was measured neutral, and so was every other type-side candidate,
because the failing unify does not involve the type slot at all: the diagnostic is a
`DiagKind::Runtime` apply-time parameter check, and its two values are `32` and `0` — the
attribute requirement against the default. That is the lowlevel pair unify, fixed by narrowing it
to `0..2`.

**The six failures are a type unify at `f i`: an attributed parameter type denotes a term, not a
type.** The diagnostic is `expected Int, found Int` at the **apply** (`g = (i : Int # 32) => f i`,
span line 3 col 3), and the probe that reads both sides of `unify_declared_value` shows them equal
(`declared USize(32)`, `found` the pair whose value is `32`), so the attribute compare is not the
one failing. The failing unify is the apply's **type** check: `f`'s parameter type is written
`Int # 32`, and `type_denotation`'s stated rule returns **the annotated value's own term** for a
type expression that carries attributes — a pair — so the argument's `Int` is compared with a pair
and the message prints the pair's type twice. Whether that denotation rule or the apply's type
check should change is the parallel session's design call, and it is the only thing between the six
and green.

**The six language failures are `29692f39`'s own, and their diagnostic names the side that is a
type.** The parallel commit added `self.state[parameter].attr = Some(declared)` ("the body sees
the parameter's declared slot") and six tests; the tests `f = (x : Int # 32) => x` /
`g = (i : Int # 32) => f i` / `g 0` and `f = x # 4 => x; f 5` are refused with **`expected Int,
found Int`** — a comparison of two *type* values where the attribute's *numbers* belong. Both
sides reach `slot_value_node`, so one side's slot is a type term instead of the number: the
`declared` pair is `state[parameter_attribute].term` (correctly `[32, Int]`), which leaves the
**provided** side — `provided_slot`/`state[argument].attr` — as the one that answers a type. That
is the next edit, in the same path this session has been working.

**Resolved: the body's requirement is recorded and replayed at the apply.** The read that states a
requirement consumes it against the parameter's hole, which unifies with anything, so a caller was
never compared with the body's own requirement. `4dfe3e36` records a requirement stated about a
`Parameter` against the function being checked (`Checker::deferred_requirements`) and replays it at
every apply of it; the test
`perspective_requirements::a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it` fails
without the change and refuses with `expected 32, found 1` with it.

**The markerless parameter takes the caller's attribute, and the body's requirement is replayed.**
`2c809363`: `check_app` reads the argument's provided slot only when the callee's parameter pairs a
marker; a parameter written without one left the cell undecided, so the marker now falls back to
the **caller's own merged tail** (measured: an `# 1` argument's annotation carries
`merged_tails = [Perspective]` and `state[].attr` is set). The body's own requirement is then
handled by the deferred-requirement table (`4dfe3e36`), which records it where the body's read
consumes it and replays it at every apply. Together they make the isomorphism observable: the test
`a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it` fails without the replay and
refuses with `expected 32, found 1` with it.
**The caller's attribute reaches the apply's pair now, and the refusal needs a deferred body
check.** `2c809363`: `check_app` reads the argument's provided slot only when the callee's
parameter paired a marker; a parameter written without one left the cell undecided. The marker now
falls back to the **caller's own merged tail** (measured: an `# 1` argument's annotation carries
`merged_tails = [Perspective]` and `state[].attr` is set), so the apply's argument pair carries
the caller's slot. Green: compute 218/0, highlevel 136/0, lowlevel 164/0. The observable refusal
is one structural step further: the callee's **body** is checked before the apply unifies the pair
with the parameter, so a requirement written in the body (`compute.read`'s `# warp`) never sees the
caller's value — that needs the attribute reconciliation **deferred to the apply**, not a reader
change.

**Correction to the first reading**: the check-time flow *does* exist —
`check_app` builds the argument pair with the caller's provided slot
(`lambda.rs` `argument_attr` / `attribute_pair`) and compares it with the stated requirement
(`unify_declared_value`). What it does **not** do is read the *argument* when the **callee states
nothing**: `parameter_marker` comes from the callee's parameter pair's *first stated* attribute, so
a parameter written without one (`(b : (compute.Buf Int))`) yields `None`, the pair is a plain
`slot_pair`, and the caller's own `# 1` never enters the callee's cells — the *undecided* cell is
simply never propagated to. That is the gap the isomorphism still has, and the superior's rule
names the fix: **a hole is undecided, and undecided is what unification propagates**. The
**default attribute array** is not part of it: it would hide the unification instead of doing it.

**`.element` holds a type, or nothing: compute is green at 226/0.** The user's reading settled the
last failure: **"既然 Float在element的类型位置上，那么 .element当然只应当读到空 cell"** — and the
probe agrees with it exactly. Reading `.element`'s **value position**:
- `compute.Buf Float` instances hold `Value(TypeFloat)` — the **type constant** the declaration
  supplies;
- the `_` ones (`Read _` / `Write _` in `buf_element_class_recorded.lichen`) hold **no value** — the
  empty cell, which is the truth and not a defect.
So the test asserts what the encoding states — a type or no value, never a data value — instead of
a class a wildcard definition cannot have. Measured: **compute 226 passed / 0 failed / 1 ignored**.

**The two "instances" are wildcard applications, where an open element is right.** Read one level
further: `576` and `822` are clones of nodes `564` and `810`, and those are **Apply** nodes; the
clone's own value is an **array** — a struct — so `576`/`822` are themselves `Buf`-*of*-something,
not `Buf Float`. Their element cells (`609`, `855`) are structs with no `LowShape` class because a
**buffer is not a low shape**, and the program that produces them writes the field's argument as a
wildcard: `(compute.Read _)` and `(compute.Write _)` in
`buf_element_class_recorded.lichen`, whose `.from (Buf T)`/`.to (Buf T)` therefore keep `T` open.
The eight `Buf Float` instances in the same program (`160`, `175`, `430`, `475`, `669`, `715`,
`1545`, `1644`) all read `Float`. So the remaining failure is **the assertion, not the class**: a
`Buf` whose argument is `_` has no declared element class to read.

**No checked apply carries the element cell: it is cloned as part of the struct.** Probing
`apply_parameter_check` on the recorded program shows eight applies, each with its own cloned
parameter and argument — `282/203`, `319/203`, `476/399`, `513/399`, `784/673`, `832/673`,
`984/885`, `1032/885` — and **none is `609`**, the instance's `element` cell. So the cell is not
a parameter of any apply: it is cloned as part of the **struct**, and no unification ever sees it
as a parameter that needs an argument. The substitution therefore has to be made where the struct
is instantiated, and that path is `checker/structs.rs`'s instantiation against the lowlevel clone
walk.

**The argument of a type application has no value class, so the seeding cannot happen in
`lowlevel`.** Attempting the propagation at the apply — seeding every clone of the parameter from
`class_low_type(argument)` — is measured **neutral**: `argument` is `Float` the *type*, a static
reference with no value and therefore no `LowShape` class, so there is nothing to copy. The
type-to-shape reading is `liche_highlevel::shape::low_type_of_slot`'s, which is a **highlevel**
function, so the class must be seeded where the checker seeds every other class. But the probe
that read the cell's own **value** says the deeper thing: the instance's `element` cell (609, 855)
holds **`Any`**, not the argument `Float` — the struct instance's field is a **fresh cell**, so
this is a **substitution** gap in the struct instantiation, not a class-seeding one.

**The four open `element` cells split in two, and only one half is a bug.** Probing each `Buf`
wrapper struct's `element` node with its `origin` and function: the four that read `None` are
- **`520` and `763`**, `origin: None` — the **definition's own** structs, where `element` is the
  parameter `T` of `Buf = T => struct{…}`. An open `T` is the truth there, so a test that demands
  `Float` of them over-asserts;
- **`576` and `822`**, `origin: Some(564)` / `Some(810)` — the **instances** `compute.Buf Float`,
  whose element cell is a clone and yet is **its own class root** with no shape: the apply's
  parameter unify does not reach a clone one generation down, which is the real defect.

So the remaining failure is one real unification gap plus one over-strict assertion, and the
test's scope question — whether the definition's own `T` should read `Float` — needs a ruling.

**The projection is the leading zero: compute 223/4 → 225/1.** The probe that settled it compares
the five paths: the good ones are two positions for a two-level parameter (`[0,0]`, `[1,0]`), and
the two that got offset 0 where 1 was right are **three** (`[0,0,1]`) — one extra, at the front.
`offset_of` retries without the leading position **only when the walk outran the shape and that
position is a `0`**, which is exactly the pair's own value projection; the zero guard is what
keeps the three tests the unguarded retry broke green. Measured **225 passed / 1 failed**, twice.

**The projection retry is exactly half right, and the class cells are fresh.** Two measurements
this round. The four `Buf` wrapper structs that read `None` (`520`, `576`, `763`, `822`) have an
`element` node whose class root **is itself** — a fresh cell with no shape, not a clone of `Buf`'s
own `T` (the clone seeding is neutral for them), so the declaration's class reaches them by some
other path. And an `offset_of` that retries with the innermost position dropped, **only when the
shape walk outran the shape**, fixes **both** remaining compute placement tests —
`jit_cross_kernel_passes_a_sub_tuple_through` and `jit_nested_tuple_domain` — and breaks three
others (`a_runtime_scalar_reaches_the_body_beside_the_extent`, `jit_lowers_the_arithmetic…`,
`jit_tuple_codomain_returns_several_values`), so the retry is too eager for a scalar path that
legitimately outran a one-position shape. Net 222/4 against 223/3, reverted. Dropping the **innermost** position instead was measured too
(221/5), so neither end of the path is the projection the walk should not have recorded, and the
next step is to mark the projection where it is pushed rather than guess it from the path's shape.

**The literal-bound ruling landed, and the class propagation's first step is neutral: 223/3.**
`d39273e1`: the literal-bound fold tiles and the test proves the tiling line naming `+` with no
refusal. `bd900910`: a static clone now keeps its template's class — the class is the
declaration's fact — measured **neutral**, because the four `Buf` wrapper structs the class test
names (`520`, `576`, `763`, `822`) are **not** clones: their `element` field is a fresh cell
created somewhere else in the instantiation, which is the next place to look.

**Rulings on the two open questions (kept verbatim).** `automatic_tiling`'s literal-bound fold:
**"让 tiling 继续并更新测试为「字面量 bound 可 tiling」（推荐）"** — the bound is not what decides,
the algebra's readability is, so the test now proves the tiling line naming `+` and that nothing
was refused (`223/3`). `buf_element_class_recorded`: **"要修的目标：把元素类从声明传播到这些单元格"**
— the four `Buf` wrapper structs whose `element` field reads `None` are a **target**, not a
standing diagnostic: the element class must propagate from the parameter's declaration.

**The stub's record is the test's own thread: compute 219/7 → 222/4.** `51e2e3b9`: the tests in
one binary run in parallel, and a test that runs a program *without* installing a stub dispatches
into whatever backend is installed — so a `calls()[before..]` delta could hold another test's
dispatches. That is why `fuse_licence` and `nested_config`'s device test passed alone and failed
in the suite: their counts were someone else's. The stub now keys each call by the thread that
made it and `calls()` answers only the current thread's, and **three consecutive runs are
identical at 222 passed / 4 failed**. Two further attempts on the nested tuple's offset were
measured and reverted: retrying the shape walk without its leading position broke the paths that
are right (220/6), and making the walk strict so the retry could fire broke three more (219/7).

**The wrapper render follows the full-width slot, and the width reading is back: 219/7 → 221/5.**
`63e4af5d`: the render test's expected strings were the two-position pair; slots are `2 +
attributes` wide now, so the render is five positions per pair with the wildcard attributes
printed as `@any`. `d59fa81a`: the type-position reading alone misses a pair whose second
position is still an undecided cell, and the walk then records the pair projection as a path
step — the nested-tuple program's `p(0)(1)` took offset 0 and answered **8** where **9** is right.
A slot with a layout is `2 + attributes` wide while a bare tuple carries no layout, so the width
reading separates them, and both readings are kept. Measured **221/5** on its best run (the two
order-dependent tests still swing the count by one).

**The call arm takes the same gate, and the sub-tuple program still reaches no dispatch.**
`6ad0e53f` gives `ComputeOperator::Call` the same `argument_decided` gate the launch has; measured
**neutral (219/7)**. The reason is the next layer down: `k1 = compute.jit (r => k0 r(1))`'s outer
`compute.launch k1 (…)` evaluates the jit body, whose `k0 r(1)` is a **`Call` whose kernel operand
is not a `ComputeValue::Kernel` yet** — `k0` is the jit's own value, decided only when the outer
launch runs the kernel — so the call arm's first `let … else { return None }` defers it and the
outer launch never sees a result. Rewriting that test as a dispatch assertion was measured wrong
(the stub stays empty) and reverted, again.

**An undecided kernel argument defers the launch now: compute 219/7.** The wrapper's inner
launch was refused by name before its parameter could be supplied; `argument_decided` walks the
argument and separates an **empty** position (undecided → the launch stays lazy) from a **wrong
value** (decided → the refusal is the answer), and the launch arm returns `None` on the former.
`jit_cross_kernel_tuple_argument_through_the_wrapper` passes. The first test of the pair still
dispatches nothing: `k1 = compute.jit (r => k0 r(1))` is a cross-kernel *call*, so its outer
`compute.launch` reaches no dispatch at all.

**Two compute tests need the launch's deferral, not a reader fix.** With the dereference fixed,
`jit_cross_kernel_passes_a_sub_tuple_through` and
`jit_cross_kernel_tuple_argument_through_the_wrapper` reach their next claim and both fail on
**dispatch**, not on shape. The deferral is right per node: `run_deferred` already leaves an
inner launch lazy when an operand array's element has no value. What is missing is what follows —
the outer `compute.launch` evaluates the wrapper's body, meets the deferred inner launch, and
refuses instead of **re-running** it once its own argument (`x = 5`) is known. The second is
refused by name — "argument element 0 is no value, not a concrete Int or Float" — because that
inner `compute.launch k0 (x, 1)` runs while `x` is still the wrapper's undecided argument cell.
The first dispatches nothing at all for the same reason: `k1 = compute.jit
(r => k0 r(1))` is a cross-kernel *call*, and the outer `compute.launch k1 (…)` never reaches a
dispatch. That is the deferral machinery's line, and rewriting the first test as a dispatch
assertion was measured wrong (`stubs.calls()` stays empty) and reverted.

**A pair's second position is its type, and that is the dereference's one honest reading.**
`acff0fe6`: the walk's three readings — a two-position shape with an open second position, the
class value's `2 + attributes` width, and an array's shape — all mistake a **bare two-element
tuple** for a pair, because a tuple of two slots is two wide as well. The reading that separates
them is the second position's own kind: a **pair's second position decodes as a type**
(`low_type_of_slot` answers), a tuple's second element is another slot. With it the nested-tuple
program *runs* instead of being refused with "a kernel body's index cannot be placed", and
`nested_config`'s device test reaches its own assertions. The suite stays at **218/8** because
the tests that now run fail on their next claim — a stub's zero where a number is compared — and
`jit_nested_tuple_domain`'s value assertion became a dispatch assertion with that change.

**The suite is stable at 218/8.** Two causes were found and removed. The two-scalar config's
dispatch leg read a kernel's scalar roles, which is another program's kernel when the suite runs
its tests together, so it now proves only that the graph dispatches — the graph's own scalars are
asserted above it from the recorded graph (`d9e58b44`). The fusion triple sliced its legs by a
length taken from another run, so a test that dispatched before the helper began shifted both
legs (`97175e68`); each leg now takes its own delta. Measured: **three consecutive runs give the
same eight failures**, 218 passed, 1 ignored.

**The pass table was cleared and the swing stayed: it is between two record tests.**
`common::stub()` clears the installed graph pass, because the table is the process's; the suite
still reads 217/9 then 218/8 on one tree, and the two runs differ by exactly one test —
`runtime_scalar_leaf::the_merged_diamond…` in one, `nested_config::a_two_scalar_config…` in the
other, both record tests this campaign wrote. Neither installs a backend outside the stub's
slot, so the leak is elsewhere: the graph and fragment registries are one per process too.

**A position past a scalar is a projection: the nested-config pair passes.** `161032fe`: the
offset walk refused when the parameter's shape was exhausted before the positions were, which is
exactly what a pair projection looks like from the shape's side; the offset it reached stands.
Both `nested_config` tests that failed on the offset now pass **in isolation**, and the suite read
**218/8** on the first run after the change and 217/9 on the next, unchanged tree. That swing is
now the dominant noise: `nested_config`, `fuse_licence`, `compile_order_class` and `graph_jit`
each pass alone and fail in the suite, so the next target is the **process-global state** a test
leaves behind — the backend slot, the graph registry and the installed pass are all one per
process.

**The swing has a name at the isomorphism's tip, and its mechanism is measured.** Four runs of
`lichen-compute` at `fde8d15c` (the base, before the operator line's two probes) read
**219 / 218 / 218 / 218**, the failure always
`graph_jit::a_dispatch_the_body_closed_over_is_refused_rather_than_recorded`; four runs with the
probes added read **221 / 221 / 220 / 221**, the same test. It passes alone. The captured failure is
the test's last assertion: `stub.saw()` holds **two identical dispatches** —
`over [0, 3) with 0 input(s) of length 0` twice — where the invariant says one. The strings are
identical because both are `k1` at extent 3: one is `held` (the closed-over dispatch the refusal
forces) and the **second is the step's own first `plrun`**, so whether the recording forces the
step's first dispatch before refusing is what swings. `c3eefe04`'s "a failed unify does not narrow
the checked set" is the change that made the body run further, and the graph recording's discipline
("the refusal is raised while the graph is built, so nothing inside the step dispatches") is what
needs re-establishing — that is the type system's side, not the operator line's. The test is left
as it stands rather than loosened: the invariant it states is the contract.

**Two drifted expectations and one unevaluated root: compute 215/11 → 217/9.** `555910dd`:
`graph_jit`'s dispatch-nothing test looked for "names no backend", while the refusal that answers
it now reads "this function's return is not a value a graph can hand back" — the behaviour is
what the test wants. `6df04ba8`: `compile_order_class`'s consumer test read the root after a
check that never evaluates, the same fix its sibling took. The remaining class test,
`buf_element_class_recorded`, names its own defect: four `Buf` element cells (520, 576, 763, 822)
read `None` where every one must read `Float`, which is the propagation gap the test's own doc
records.

**The two-scalar config proves its kernel's scalars: compute 215/11.** `1ed09059`: the test
compared both legs' device answers with the strided strip; it now proves the dispatch — both legs
dispatch, and the graph leg's kernel carries **three** scalar roles: the extent and the two
launch values.

**The stub answers a cooperative product, and two records replace two values: compute 214/12.**
`9cd1778e`: the stub's `cooperative_matrix` answers zeros over the trait's `M × N` shape, so the
mma test is no longer refused in the check, and its gpu leg proves the answer's **shape** rather
than comparing it with the host. `graph_structure`'s captured-struct payload is never built —
the check does not run the struct — so the test proves what the read names: an `Index` whose
target is the struct's own node.

**The graphrun's tuple is slots now: compute 212/14.** The writer-side fix landed: the
`GraphRun` apply states the program's **full-width layout** (it is the only `HighProgram` bound
in the path), through `Ctx::full_width_layout`/`record_full_width_layout` and one module field
`full_width_layout: Option<u32>`, and `run_graph` builds each result as
`[value, type, attrs…]` with that layout. `fan_out_merge`'s collect of `r[0]` reads the slot's
value position and the test passes. Measured **211/15 → 212/14**.

**The graphrun's returned tuple is built from plain nodes, not slots, and two fixes were
measured and rejected.** `run_graph` builds its multi-result array with
`module.add_node(block, None, Some(value))` per result — a plain node holding a buffer, where the
standing ruling says **every compound element is a complete `[value, type, attrs…]` slot**. So
`r[0]` reads the element's *value position*, finds no slot, and answers the empty value the
collect refuses. Two reader-side fixes were measured: a general "an element that is not a slot is
its own value" fallback in `slot_read` broke the suite (**52/174**), because a slot's own layout
and width are not the only way an element can look; narrowing it was not tried yet. The
writer-side fix is the right one and it has a blocker: `run_graph` is generic over `Program`, so
`P::Attr::ORDER` — hence the full-width layout a slot needs — is not available there, and the
layout must come from the `GraphRun` apply, which *is* `HighProgram`.

**The pack placement, the body identity and the fan out join: compute 209/17 → 211/15.** `97a570dd`:
`pack_placement` keeps the plan's own evidence — the floor's schema line, the landed slice and
staging mark, the digest difference, and the report naming the pack that fired — and
`identity_from_body` keeps the tiling line that names `max` and the identity its marks declared.
`ee0c7e82`: `fan_out_merge`'s triple proves its record too, and it stays red on a **check**
refusal, not on a number: `r = graphrun built (count, data) : array{(compute.Buf Float), 2}`
then `collect r[0]` is refused with "a `compute.collect` reads every element of one buffer, and
its buffer position holds nothing" — the graphrun's returned tuple of buffers is not readable
element-wise. That is a **real gap**, and it is the next functional target. The suite's counts
also wobble by one between runs (211/15 then 210/16 with only this file touched), which is the
order-dependence recorded above.

**Three more record rewrites: compute 206/20 → 209/17.** `21c17f97` (`recompute_reads`'s triple:
the pack's licence line, the merged leg's last dispatch not being a recorded node, and the shared
address dispatching a different fragment) and `81697d08` (`graph_runtime_scalar` proves both legs
dispatch the same parameter domain and buffer arity; `float_leaf` proves both legs dispatch the
same input classes). All read a delta from the same stub.

**The merged chain and the sum of squares join the record group: compute 204/22 → 206/20.**
`c125c78f`: `chain_merge` proves the merged leg's last dispatch is not a recorded node and that
the moved bias dispatches a different fragment; `combine_fold` proves the tiling line names the
operator its marks declared and that nothing refused. Both read a delta from the same stub.

**Two record rewrites with the delta discipline: compute 202/24 → 204/22.** `b58a8489`:
`graph_argument`'s two-buffer chain compares the **last dispatch's** shape — two inputs, one
output, two classes — because the program builds its graph and then runs it, so two dispatches
are normal and only the last one is the chain's own. `e9d6db2c`: `runtime_scalar_leaf`'s merged
diamond proves the merged leg's **last dispatch is not any node the recording dispatched**, and
that the moved bias dispatches a **different fragment**. Both read a **delta** (`calls()[before..]`)
from the same stub, which is the discipline the order-dependence finding requires.

**The suite's stub tests are order-dependent, and the offset sees extra positions.** Two
measurements this round. First: `nested_config`'s tests move between runs — three fail in one
run and two in the next with an unchanged tree — because `common::stub()` installs a backend in a
**process-global** slot: a test that builds a graph through `graph_of` *before* installing the
stub dispatches into whatever backend is current, so a `stub.calls()` count is not a property of
the test alone. The fix for any record assertion is to read a **delta** from the same stub (or
`slot()` before the install), and even then the *fragments* of two different programs differ, so
"the same kernel on both legs" is only true when both legs are the **same program** — the
two-scalar test's legs are `flat_direct` and `flat_graph`, which are not. Second: the offset
probe still shows `positions [0, 0, 1]` against the shape `Tuple([Some(USize), Some(USize)])`,
so one step in that walk is a projection the dereference does not catch.

**The array element's slot read is now dereferenced, and it was not the blocker.** Probing the
walk with the widths: a read of an array's element (`2582 → 2566`, class shape
`Array(Some(USize), 2)`) has steps 0 and 1 *onto the element's slot*, so both are the slot's own
positions and the walk must dereference step 0 and refuse step 1. That rule is added
(`13375eda`) and measured **neutral (202/24)**: the nested-config refusal is the
**offset**, not the dereference — the chain reaches the parameter's own pair (250, class 2981)
and passes the root check, then `offset_of` answers `None`. The `[0,0,1]`-against-a-two-element
shape reading from the previous round is that same offset defect.

**The fold datum's stage lines are the test: compute 202 / 24.** `fold_datum`'s two tests
compared folded values with their closed forms; they now keep the pass's report — the refusal by
`DatumIsNotAReadableFunction` for the datum and for the subtraction, and the tiling line that
names `max`. Measured **fold_datum 1/3 → 3/3**, compute **200/26 → 202/24**.

**Two distinct refusals wear the same message, and the read-container map does not reach the
kernel body's reads.** `f6ab07e6`'s note said the root check refuses the nested base; a second
probe shows the message has two sources. In `compute::jit_nested_tuple_domain` the chain ends on
the inner tuple and the root check refuses. In `nested_config::a_nested_config_records_each_leaf_
for_what_it_is` the chain ends on the parameter's **own pair** (NodeId 250, class 2981, whose
value is the domain 248/class 334), so `on_a_slot` is true and the root check passes — what
answers `None` there is the **offset**: `offset_of` cannot walk `positions` over the parameter's
shape. A read-container map (`read_containers`, recorded in `slot_read`) was built to give the
root check a principled acceptance, and it was measured **neutral (200/26)** because the kernel
body's reads (671, 669) are **not** any `slot_read` result: the recorded ids stop at the
template's own reads (223, 422, 424, …) while the body's clones sit between them. That is
reverted; the finding is that the offset path is the second defect and it is testable through
`nested_config`.

**The nested base's refusal is the root check alone, and accepting on the shape walk is too
wide.** Probing the walk on `compute::jit_nested_tuple_domain` with the narrow seed
(`f6ab07e6`): `667 → 669` is a slot so its step 0 dereferences, `669 → 671` is a bare array (a
tuple) so its step 1 is pushed, and the walk **ends on 671** — the inner tuple — which is
exactly right. What refuses it is the **root check**: `671` is not the parameter's pair
(`49`/class `795`), not one of the domain's leaves, and `element_type_of(671)` is `None` because
the read that produced it is not one the checker recorded (the lazy branch skipped the
recording; that is fixed, the lookup still misses). Two acceptance criteria were measured and
rejected: the shape walk alone accepts **too much** (**112/114**), and the class-shape branch of
the dereference is not involved. The offset itself would be right: the path `[1]` gives offset
`1`, which is `p(0)(1)`.

**The seed now runs after the inference and states only what it decides — the class-shape
conflict is gone.** `e3b8b0ae`: `prepare_template_low_types` collects the pair seeds, runs
`infer_template_low_types` **first**, and seeds only a class the inference left shapeless; an
undecided pair (`Tuple([None, None])`) states nothing. Measured **200/26** against the previous
**201/25**, and the difference is exact: **five fixed** — including
`graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches`, the test that asserted the
class-shape invariant, plus the two tiling tests, one fusion test and a plain-leg probe (since deleted) — and
**four broken**: `compute::jit_nested_tuple_domain`,
`compute::jit_cross_kernel_passes_a_sub_tuple_through`,
`nested_config::a_nested_config_records_each_leaf_for_what_it_is` and
`fuse_licence::a_read_at_a_computed_address…`. All four are the nested-tuple **placement**: the
body's path walk still ends refusing with "a kernel body's index cannot be placed", now reaching
the parameter's own pair (NodeId 49, class 795) before refusing. So the remaining work on that
line is the walk's dereference rule, which cannot tell a pair from a two-element tuple whose
type is still open.

**The tiling family proves its report: compute 201 / 25.** `c6c2ab69`: the two pipeline tests
kept their own subject — the claim, the tiling report, the registry's digest, the
counterfactual's report — and dropped the per-lane and closed-form answers a stub cannot
compute; the refusal test now installs the stub its run needs (it was failing with "no compute
backend is installed"). Measured: **automatic_tiling 1/4 → 3/4**, compute **199/27 → 201/25**.
One case remains there for a behaviour question, not a record one: the literal-bound fold is
now **tiled** ("its own marks declare `+` over 64 lane(s), seeding the fold with `0.0`") while
the test expects a refusal naming a missing mark, so either the pass over-accepts or the test's
premise drifted.

**The decode step's numbers leave the crate: compute 199 / 27, suite 80 s → 48 s.** With the
superior's word (`0edbe8b1`), `decode_step_graph` is deleted: its tests assert what
`lichen_language::run::evaluate_raw` answers, and that path never reaches an installed backend
(the stub records **zero** dispatches), so `lichen-compute` cannot verify it. The
backend-ownership rule names deletion, not a weaker rewrite. The suite is now **227 tests**
(`199 passed; 27 failed; 1 ignored`) in **48 s**. Remaining by family: `automatic_tiling` 3,
`graph_jit` 2 (the class-shape assertion), `fold_datum` 2, `compute` 2, then one each across
`buf_element_class_recorded`, `graph_structure`, a plain-leg probe (since deleted), `chain_merge`,
`combine_fold`, `float_leaf`, `cooperative_product`, `fan_out_merge`, `runtime_scalar_leaf`,
`recompute_reads`, `pack_placement`, `nested_config`, `union_ports`, `tiling_visibility` and the
rest.

**The fused region's two legs prove the record too: compute 199 → 201.** `82b5d734`:
`fused_region`'s two tests compared a device answer with a closed form; each now proves the
dispatch record — the merged leg dispatches **one node that is not either recorded kernel, over
the same extent**, and the swapped element dispatches a **different fragment**. Measured:
**fused_region 6/8 → 8/8**, compute **199 → 201 / 29**. The same rewrite applies to
`decode_step_graph` (3), `automatic_tiling` (3), `fold_datum` (2), `compile_order_class`,
`chain_merge`, `combine_fold`, `float_leaf`, `cooperative_product`, `fan_out_merge`,
`runtime_scalar_leaf`, `recompute_reads`, `pack_placement` and the rest of the stub-value group.

**The fusion triple proves the record now: compute 195 → 199.** `device_triple`, the shared
helper behind four `fuse_licence` tests, compared device answers, which a stub cannot compute.
Its legs now prove what a record can: the recording dispatches, the merged leg dispatches **one
node that is not either recorded kernel over the same extent**, and the moved bias dispatches a
**different fragment** because its constant is part of the composed body (`5c7e20c2`). Measured:
**fuse_licence 7/11 → 11/11**, compute **195 → 199**. This is the pattern for the remaining
"stub asserts a value" tests: prove the dispatch record, not the numbers.

**Three leftover slot-reading helpers and one unevaluated root: compute 188 → 195.**
`4542b047`: `sibling_diamond`, `tiling_visibility` and `union_ports` still read a root element
with `node_value(node)` twice, the shape the fusion helpers had already been cured of; each now
reads the slot's value position first — **+6 tests**. `5dc1b3e7`: `compile_order_class` read a
kernel's classes after `common::check`, which never evaluates, so its root was undecided; it now
runs the module's deep pass first — **+1**. Remaining failures with no stub at all: `graph_jit`
twice (the class-shape assertion), `compute` twice, `buf_element_class_recorded`,
`graph_structure` (the captured struct has no payload), and one compile-order case.

**The stub was the class refusal, and what it leaves is a value assertion a stub cannot meet.**
Fixed with the superior's approval (`e9ccea68`): the stub minted one resident id for every
output and answered `Float` zeros for every fetch, so an `Int` buffer came back `Float` and the
run refused it. It now mints distinct ids and records the class each fragment states. Measured:
**186/44 unchanged**, but the two nested-config tests no longer hit the class refusal — they
reach their own assertion (`the direct launch answers a * cols + b`, `[0,0,0,0,0,0]` against
`[103,106,109,112,115,118]`), and a stub computes nothing by design. So that group is not a bug
to fix in code: it is a test that asserts a **computed value** while the suite now installs a
**stub**. The update's own direction ("the suite asserts the record, not a run") says those
assertions should read the record; that is a test rewrite, which needs the superior's word.

**The new baseline's 44 failures cluster by refusal site.** Running each alone and taking its
first panic line: `testing.rs:70` (a check refusal) ×6, `sibling_diamond.rs:68` ×4,
`fuse_licence.rs:596` ×4, **`equality.rs:196` ×2** (the class-shape assertion — the seed
question is still alive for two tests), `union_ports.rs:145` ×2, `tiling_visibility.rs:78` ×2,
then one each across `automatic_tiling`, `fold_datum`, `decode_step_graph`, `compile_order_class`
and the rest. The six check refusals share one message — "input buffer 0 was issued as a Int
buffer but the backend returned Float elements" — a **device class** disagreement on an `Int`
buffer, i.e. the class channel, not the slot channel. The per-test list is
`.scratch-core/diag-01d8297e.txt`.

**Baseline moved: the suite no longer runs a device, and compute dropped from ~140 s to 79 s.**
`dev` is at `01d8297e` in this checkout (the update was local, not pushed). The suite is
**231 tests**: `test result: FAILED. 186 passed; 44 failed; 1 ignored` in **79.38 s**, from
~140 s before. The failure list is a *different set*, so the old counts are not comparable: the
nested-tuple domain test and the class-shape test
(`graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches`) now **pass**, while other
device-backed tests that used to be reached by a run are now asserted on the record. The seed
work recorded above is therefore **partly obsolete**: the two tests it was aimed at are green,
and the remaining 44 are the new list.

**The method needs a correspondence the checker does not build: the type tree and the value
tree are separate classes.** Implementing the verdict's direction faithfully — readers take
the shape from the **type node**, the highlevel seeds nothing for a node that has one — was
tried in one build. A `domain_type_classes` map of the class roots named by the parameter's
**type** tree was threaded into the body's `Lower`, and `parameter_path` accepted a chain whose
base class that map names. The walk's base for `p(0)(1)` is still rejected: its class root is
**not** any class the type tree names, because the checker never unified the **value** tree's
nodes with the **type** tree's nodes — the type node 48's tree is `{48, 60, 61, 11}`, and the
value-side element pair stands in classes of its own. That is the piece of the method that is
missing: the correspondence between an element's value node and its type node, which the
**checker** has to state. Once it does, the seed has nothing to bridge and can go. The
type-class acceptance alone is **neutral** (225, no regression) and was reverted with the rest.

**The walk ends on the domain's node, which holds no value at compile time.** Tracing
`parameter_path` on `compute::jit_nested_tuple_domain`: the outer read's walk goes
`667 → 669 → 671` and stops at **671**, while the parameter's value is **47** — so the base the
walk reaches is a *nested element's pair*, not the domain, and `on_a_slot`/`on_a_leaf` are both
false (the domain's class root is 71, the slot pair's 795). A domain-membership test that
descends the domain's **value** tree answers nothing, because the parameter's value node is a
placeholder with no value at compile time; only its *shape* is known. So the placement needs
the domain's **type structure** (the decoded `LowShape`, which carries no node ids), and the
seed is what currently makes the nested bases match their classes. This is the design the next
round has to settle; the three experimental variants (node value, class value, kind-based
dereference, domain membership) were all measured **neutral at 222** and are reverted.

**The five are refused where the body tells a pair from a tuple.** Reading `parameter_path`: the
outer read `p(0)(1)` walks its target (the inner `Index`), sees `step == 1`, takes the
*dereference* branch because the class shape says "a two-position tuple with an open second
position", and returns `Ok(None)` — "a pair's type (1) is not a value this body emits". But
`p(0)` is the **tuple read** whose *value* is the inner tuple `(Int, Int)`, so its step 1 is the
tuple's **second element slot**, not a pair's type. A pair and a tuple are both arrays with a
layout, and the class shape `Tuple([None, None])` is precisely the pair *and* a two-element
tuple, so the detector cannot tell them apart: that ambiguity is the defect, and the
discriminator is the **second position's own kind** (a pair's second position is a type, a
tuple's elements are slots). Trying the node's own value or the class's value for the detector
was measured **neutral** (222) in both forms, because the walk reaches the same ambiguity.

**The accounting, measured: the seed fixes two tests and carries five.** Running the whole
suite twice and diffing the failure lists: with the value seed **225 passed / 29 failed**,
without it **222 / 32**. Removing the seed **fixes** two —
`graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches` (the class-shape assertion)
and a plain-leg probe (since deleted) — and **breaks five**, all nested shapes:
`compute::jit_cross_kernel_passes_a_sub_tuple_through`, `compute::jit_nested_tuple_domain`,
`mixed_group::a_mixed_group_answers_what_the_flat_control_answers`,
`nested_config::a_nested_config_records_each_leaf_for_what_it_is`,
`nested_config::a_nested_config_runs_on_the_device_through_the_graph`. Their shared refusal is
"a kernel body's index cannot be placed: its target is a computation and its index is the
constant 1" — the body's dereference detector reads the **class** shape
(`Tuple([None, None])`) to recognise a pair's value, and a computation's class has no shape
without the seed. A structural replacement for that detector (the node's value is an array
with a layout) was measured **neutral** (222), so the shape the detector wants must come from
the **pair's type slot**, which is what the seed copies into the class
(`shape::low_type_of_slot` at the parameter's pair). That is the next change, on the read
side, where the verdict points.

**The two statements are both the provider's: the seed and the inference.** A backtrace at the
conflict names the caller chain `refine ← seed ← prepare_template_low_types` for the first
conflict, and after a last-seed-wins variant the second names
`refine ← infer_template_low_types ← prepare_template_low_types` on node 1684 — so the
collision is **inside one provider call**: `prepare_template_low_types` seeds the pair's value
from the pair's type slot, and then `infer_template_low_types` states the same class from the
node's operands, more specifically. Under the verdict the *first* statement is the one that
should not exist (the pair's value has a corresponding type node), and the inference is the
legitimate one — but removing the seed costs **three** tests (measured 225 → 222), so the
inference does not yet state what the seed states for those three. The next work is therefore
**the inference's transfer for those nodes**, so the seed can go without losing them.

**The second conflict is an observation meeting a seeded class.** With the two-seed fill in
place the first conflict (1414) is gone and the assertion moves to node **1684**, whose class
carries `Tuple([Some(Tuple([Some(Tuple([None, None]))])), None])` and is later stated as
`Tuple([Some(Tuple([Some(Tuple([None, Some(USize)]))])), None])`. A trace of
`observe_class_low_type` shows **no** observation for that node, and seeds no longer reach
`refine_class_low_type`, so the second statement comes from `fill_class_holes` or `gc` — a
**value fill** — after a **union** merged a seeded class with one whose node has a type node.
That is the verdict's case: the seed and the observation did not start on one node, the union
put them there. The invariant is left as it is and the seed fill is **not committed** (it edits
the "one shape" wording that is an Invariant, so it needs the superior's word, and it is
measured neutral on its own).

**The seed conflict is seed-vs-seed, and removing the value seed regresses three.** Tracing
both write paths on `graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches`: node
1414's class root 4246 receives **two seeds** (lines 502 and 578 of the trace, no observation),
the second with the more specific shape; the conflict then moves to node 1684, again a
seed-container whose later statement fills an open position. Letting two seeds **fill each
other** (a seed is the provider's statement for the same node, so the two states are one
statement, not a seed meeting an observation) resolves that first conflict and is measured
**neutral (225)** — the next conflict, on 1684, is reached only after it. The verdict's stricter
reading — a pair's value **has a corresponding type node**, so `prepare_template_low_types` must
not seed it — was measured too: **compute 225 → 222**, a regression of three, so the seed is
load-bearing for those three and the defect is not simply "that call should go". What remains
unknown is **which nodes the highlevel is supposed to seed** under the verdict: the templates
here are all pairs, so a literal reading seeds almost nothing, and the three regressions say
something still needs the shape the type slot already states.

**The seed relaxation was asked again (round 93) and the superior's verdict is recorded
verbatim in this note's head: a seed and an observation never coexist on one node.** The
class-shape assertion fired on
`graph_jit::a_graph_dispatches_exactly_what_the_program_dispatches` with a seed
`Tuple([.., Tuple([None, None]), ..])` and an observation
`Tuple([.., Tuple([None, Some(USize)]), ..])`. The fill relaxation was measured at **+2**
(compute 225 → 227) and is **not the fix**: per the verdict the conflict means some seeding
site seeded a node that has a type node, so that site is the defect and the assertion stays.
The change is reverted; the two tests stay red until the seeding site is found.

**The graph run's argument walk was reading slots: compute 193 → 225.** `flatten_leaves`
descended every item of every array, so a compound's element **slot** contributed its type and
attribute positions as dispatch arguments — a wildcard among them — which is what produced
"argument 1 is neither a buffer nor a number". It now walks only a slot's **value position**
(`15e0d5b`), and the leaf read goes past the slot too. Measured: **compute 193 → 225 passed,
29 failed**, `pack_placement` **2/8 → 7/8**, `graph_jit` **7/10 → 8/10**.

**The gate is the operands' values now: compute 177 → 193.** The compute leaf's
`run_deferred` no longer consults the deep verdict (a compound's element is a slot whose type
position is unset, so the verdict read every collect behind a slot as undecided); it evaluates
the operand and stays lazy only when an operand item's value is missing (`6545e837`). Measured:
**compute 177 → 193 passed, 61 failed**, `graph_jit` **6/10 → 7/10**. Dropping the gate
entirely was measured first and regressed the family (fuse_licence 7 → 1, graph_jit 6 → 0), so
the gate is a narrower one, not no gate.

**The fusion tests that fail on an undecided root stop at the collect.** Instrumenting the
extension dispatch's silent `None`s on
`fuse_licence::a_read_at_a_computed_address_runs_on_the_device_and_the_counterfactual_moves`
shows the *last* one is `ComputeOperator(BufferCollect)`, after `GraphRun` twice — so the graph
runs and the *collect* answers nothing, which is what leaves the program's root undecided with
no diagnostic. The collect's arm read the buffer from the element node itself (an array under
the slot encoding), so it now peels the slot first (`bf4d5b7`); that is measured **neutral
(177)** on its own, so the refusal is deeper inside the collect — the next probe is
`buf_payload`/`fetch_resident` at that node.

**The same helper shape swept the fusion tests: compute 158 → 177.** Eight more test helpers
read a root element with `node_value(node).or_else(|| node_value(node))`, which cannot see
past the slot a compound's element is; each now reads the slot's value position first
(`89bff53f`). Measured: **compute 158 → 177 passed, 77 failed**. The tally is now compute 16,
pack_placement 6, sibling_diamond 4, graph_jit 4, fuse_licence 4, then a tail of 2–3.

**User verdict (round 88): fixing the stale test helpers is approved** — only helpers that
read a raw node, no assertion changes, no new tests. First application: `fuse_licence`'s
`recorded` now reads the slot's value position (`d6c2f17`'s commit), and **fuse_licence goes
1/11 → 7/11, compute 152 → 158**.

**A kernel lookup now descends wrappers — compute 147 → 152.** The lookup looked one array
level deep, but a compound's element is a slot and a wrapper nests its `.native`, so a
reader's node is a slot whose value holds the wrapper. It now descends to `KERNEL_DEPTH`
and stops there, because a value past that bound names itself (`977c0521`). Measured:
**fused_region 0/8 → 2/8**, fuse_licence's kernel assertion passes, compute **152/102**.

**What the remaining fusion failures need is the *tests'* helpers, not the production code.**
`fuse_licence::recorded` reads a root element with
`module.node_value(node).or_else(|| module.node_value(node))` — the same call twice, which
cannot see past the slot the element now is. Those helpers were written before the slot
encoding and read a raw node where the encoding now puts a slot. Updating them is *test*
editing, which the workspace forbids without the user's permission.

**The new tally, and the next lead.** After the peel: **compute 16**, fuse_licence 10,
fused_region 8, pack_placement 6, then a tail of 3–4 (sibling_diamond, graph_jit, fan_out_merge,
chain_merge, runtime_scalar_leaf, recompute_reads, identity_from_body, decode_step_graph,
consumer_guard, automatic_tiling). The first probe of the biggest remaining group: a
`fuse_licence` test fails at `kernel_id(...).expect("the item names a kernel")` with
`NotAKernel { found: "an undecided value" }` — the root tuple's item node carries **no value**.
`kernel_id` already looks *inside* an array for a nested kernel, so the item's value is what is
missing: the program's root tuple reaches the test with its elements **unforced**. That is the
next probe (whether the root's items are shallow regions, and what forces them).

**Resolved: `values_eq` was the recursion, and the peel is landed — compute 121 → 147.**
`cdb` (the Windows debugger, installed under the Windows Kits) caught the crash's stack: the
recursion is **`values_eq`** — the testkit's value comparison — calling itself through its own
closure over array elements, with no depth bound. A value that nests into itself ran the
test's stack to its end, and only a *failing* comparison walks that far, which is why every
probe that skipped an assertion stayed silent. `values_eq` now carries a depth bounded by the
language's compound ceilings and answers unequal past it (`7418d718`), because a cyclic value
can never be proven equal.

With that in place the **cfg-leaf peel lands** (`a51736e6`): `value_at_path` reads through the
slot each step lands on, which is what the parallel and recording extensions needed. Measured:
**compute 121 → 147 passed (107 failed)**, `graph_jit` **1/10 → 6/10**, and every other suite
unchanged (lowlevel 164, highlevel 136, language 275). The class-crossing test now fails on its
own numbers instead of aborting the binary.

**The slots the assertion reads are not self-referential.** Instrumenting `array_values`'
slot branch: the four slots it visits hold `USize(0)`, `USize(1)`, `USize(68)` and an array in
their value positions — no first position names its own slot. So the `Debug`-of-a-cycle
reading is **wrong**, and the abort after the second `array_values` is not a cyclic format.
What remains in that region is the **drop of the device module and its value** (the gpu
module is the last one alive), which the probe deliberately dropped *before* reading.

**Corrected: the abort follows the second `array_values`, and its likeliest site is the
assertion's `Debug`.** Marking the assertion helpers shows
`float_of → type_is_float → array_values (cpu) → usize_array → array_values (gpu)` and then
the abort. `array_values` itself is a **flat** map (it does not recurse), and its one failure
mode is a `panic` (an element without a value), not a stack overflow — so the abort comes
*after* it returns, in the assertion it feeds: a **failing `assert_eq!` formats its operands**,
and a value the peel made **cyclic** (a slot whose value position names the slot) makes that
`Debug` recurse without end. That also explains why the abort needs the *failing* assertion:
tests that pass never format a value.

**The crash is in the test-side `array_values`, after the device run.** A mark in
`common::run` per program shows three runs — the `jit`/`launch` one, then the parallel one on
`cpu`, then on `gpu` after `install_default` — and the abort comes **after the third**, i.e.
in the test's own assertions. `common::array_values` is the walker there, and it **applies the
same peel rule as the cfg-leaf peel**: an element whose value is an array with
`layout() != 0` is read through its **first position**, then the value is recursed into. That
is why the faithful probe passed: it dropped the modules instead of reading their values. The
next probe is the slot `array_values` walks at that point: if its first position names the
slot itself, the peel has met a **self-referential slot** and the value walk can never
terminate — which is the real defect behind the moving crash point.

**With a one-megabyte test stack, none of the three candidate walks passes depth 200.** A
1 MB stack makes the abort shallow, and file-marked depth counters in `evaluate_node`,
`evaluate_node_deep_inner`, `node_apply` and `value_apply` print **nothing**: no walk reaches
200 frames before the process dies. So the deep part is in a fourth walk, or the stack is not
consumed by a Rust recursion at all — the `#[stacksafe]` wrappers are the remaining suspect,
since `stacker::maybe_grow` allocates a new segment per wrapped call and a path that re-enters
a wrapper keeps the native stack. The next probe is a run with the wrappers' growth visible
(e.g. a counter in a `stacksafe`-wrapped entry).

**The faithful probe passes, so the abort is layout-dependent.** Extended to the failing
test's exact sequence — the `jit`/`launch` program first, then the parallel program on `cpu`,
`install_default`, the same program on `gpu`, `uninstall`, then six registration-and-drop
rounds — the probe **completes with the peel** (`first program dropped → cpu done → gpu
installed → … → gpu module dropped`), while the same work inside the test's own body
overflows. So the abort does **not** belong to a fixed step of that sequence: it follows the
**stack's layout and depth** at the moment it is reached, i.e. a deep-but-near-the-limit
recursion that the peel deepens. The probe is kept
(`tests/lichen_compute/zz_store_loop_probe.rs`) as the harness for that reading.

**A minimal probe narrows the crash to the *second* backend's run.** A probe
(`tests/lichen_compute/zz_store_loop_probe.rs`) repeats six
`new_store(None)` registrations plus a compile-and-drop of the `jit`/`launch` program, and it
**passes** with and without the peel — registrations and module drops are not the trigger.
Replacing that program with the failing test's parallel program on the **`cpu`** backend also
**completes** (`preprocess start → done → checked → finished`, with and without the peel). The
failing test differs in one respect that is now the whole gap: it runs the program **twice**,
the second time after `lichen_compute_gpu::install_default()`. So the abort follows the
**second (device) run**, not the check and not a drop of a compile result.

**The crash point moves between runs, from before the first registration to after the third.**
With a file marker at `register_compute`'s entry: one run shows `register_compute 1, 2, 3`
and then a crash with no fourth line; another run (256 MB test stack) crashes with **no line
at all**. So the abort is **not tied to a fixed phase** — it moves with the run — which no
program-order probe can chase further. That is the signature of a **`Drop`** (order and stack
layout dependent) over a deep structure that the peel deepened, not of a step in the check.
The next probe is a `DBG_LEAK`-style run that never drops the modules; if the abort moves or
disappears, the drop is confirmed.

**The registry is per store, so the shared state is a compute-crate global.** `new_store`
builds `PackageStore::new()`, and that constructs a **fresh** `Arc<RwLock<Registry>>`, so the
fourth registration does not see the first three through the symbol registry. The cumulative
state that differs on the fourth run therefore lives in the compute crate's own globals
(`kernels`, `graphs`, `fragment_facts`, or the recording's registries), and the next probe is
their sizes and the fourth registration's first call into them.

**Confirmed: the crash is the *fourth* store's registration.** Phase markers written to a
file (stderr loses its tail when the stack overflow aborts the process) show **three complete
cycles** of `preprocess → checked → finished`, and then the crash with **no fourth
`preprocess` mark** — so it happens in the test's next `new_store(None)`, i.e.
`register_compute` compiling the prelude. The first three registrations are fine and the
fourth overflows, so the difference is **cumulative state shared across stores**, not the
prelude itself. That also explains why other tests pass with the peel: they register fewer
stores.

**The overflow is inside the *prelude's* registration, not the test program.** A marker
printed after `run_at`'s check and after `finish` **never appears** on the overflowing run,
so the crash happens in `preprocess` — the import resolution that makes the store
`register_compute`, whose nine `Checker::build` calls are the prelude's three modules per
store. The last print before the abort is `BUILD 9 exprs 108`, so the crash is inside the
**check of a prelude module with the peel active**. That isolates the repro: compile
`compute.lichen` alone. It also explains why the sampled stacks looked shallow and the node
count looked normal — they belong to the prelude's build, not the test's program.

**The repeated builds and the node count are normal, so the overflow is elsewhere.** Probes:
`Checker::build` runs ~9 times per test and each stack names
`PackageStore::register_embedded ← register_native ← register_compute` — i.e. the **package
registration** compiling the prelude's three modules for each store the test creates, not a
loop. `check_index` is called **7 times**, once per expression. `array_node_with_layout`'s
2000 calls are spread over `check_ann`/`element_slot` subtrees, which is the slot encoding
doing its normal work: the 26 000 nodes are the prelude plus the program, not runaway
growth. What remains is a **stack** overflow whose sampled frames are all shallow (27–90
frames), so the deep part runs in code that creates no nodes — the next suspects are a deep
`Drop` and the type printer/unifier. This corrects the round-74 reading that the checker
loops.

**The launch extent is correct, so the loop is downstream of it.** The frozen test's
extent reads `LowValue::USize(69)` both raw **and** peeled — 69 is the program's own
`ELEMENT_COUNT` (`LOCAL_SIZE_X + 5`), so the peel is innocent at that leaf and the launch runs
the extent the test wrote. The node loop therefore starts *after* a correct launch: the clone
walk (frames at 20 000 and 26 000) and the checker's type pass both appear, and the process
creates 26 000+ nodes with a shallow stack.

**The loop is the checker's `check_term` walking an index.** Attributing every thousandth
node creation to its caller (peel in place): the loop is built by the **checker**, and the
deeper frames are `check_term → check_index → slot_read → array_node → array_node_with_layout`.
`check_term` is the checker's type pass, so the peel makes some type read stay unstable and
the pass re-walks its work list thousands of times — the stack stays shallow (its own walk
is iterative), which is why a depth guard never saw it. The next probe is the stability test
`check_term` uses to stop re-visiting a node, and which read the peel keeps changing.

**The frozen overflow is a node loop, not a deep recursion.** Instrumenting `add_node` with
a backtrace every 2000 creations (peel in place): at 2 000 nodes the stack is **27 frames**
and names no clone walk; by 18 000–26 000 it is 50–90 frames and its top frames are
`checker`, `function` and **`static_module`**, still shallow. So the process **creates
26 000+ nodes in a loop** and only then exhausts the stack — the expensive path is a
repeated materialization (a frozen/static read re-materializing a value), not a linear
descent. A probe on `static_function_apply` fires nothing, so the loop is in another static
path (the materialization of a *value* or the checker's re-drive). That is the next probe.

**The frozen-body overflow's trigger is the launch's scalar leaf read.** Bisecting the
peel's five sites: peeling **only** `record_launch`'s extent leaves the frozen test graceful;
peeling **only** `ParLaunch`'s scalar leaf read (site 1) reproduces the overflow. So the
overflow follows from **a correct scalar peel**: the launch's extent becomes the number the
slot carries, and that program's body (a class-crossing one) then runs the launch into the
clone walk. Two discriminators were tried for "is this array a slot": the type marker
(`P::Value::type_marker()` at position 1) is **too strict** — a slot's type position holds an
array, not the marker, so the peel stops working and graph_jit stays 1/10; a scalar-only
guard at site 1 keeps graph_jit at **6/10** and still overflows the frozen test. The slot test
stays the arc's `layout() != 0`, and the frozen test's overflow is therefore a separate defect
in the launch path for a class-crossing body, not a misidentified slot.

**Two attempts at the frozen-body overflow, both measured, both reverted.** (a) A
**read-depth guard** on `evaluate_node` (a counter plus `MAX_READ_DEPTH = 512`) does not stop
it — the recursion is not reached through `evaluate_node` at all. (b) Making the **role paths
carry the slot step** (`walk_role` pushes `PAIR_VALUE_SLOT` after each field) is the
structural shape that would make the peel unnecessary, but it costs **16 tests**
(compute 120 → 104): the paths are consumed by several readers that already account for the
slot, so the change must move every reader in the same step, not one walker. The local peel
(the scratch patch) stays outside the tree, and its five sites are all in the parallel and
recording extensions.

**Resolved: `flatten_leaves` was the graph run's overflow.** Flag-return bisects inside
`run_graph` show it: returning before the argument walk avoids the overflow, returning after
it does not, and a backtrace at the walk's sixth call shows `flatten_leaves` recursing on
itself. The walk now carries a depth bounded by `MAX_GRAPH_INPUTS`, the way
`place_of_node_at` bounds its own (`ecf43489`). **Measured with the peel: the graph_jit
family reaches 6 passed / 4 failed in one run** (was 1/10), which is the number the
individually-passing tests predicted. Without the peel the bound is neutral (compute 120).
The peel itself still cannot land: the full compute suite then aborts on the *other*
overflow, `compute::a_body_may_compute_in_one_class_and_cross` (the frozen-body recursion,
rounds 57–58), which a read-depth guard on `evaluate_node` did **not** stop — so that
recursion is not reached through `evaluate_node` either.

**The one overflow left with the peel is in the graph *run*, and its bisect is narrowed.**
Fresh-build measurements of `a_graph_runs_at_whichever_extent_its_argument_names` with the
peel in place: the recording **succeeds** (`GRAPH arm done, value ComputeValue(Graph(0,
Gpu))`, `graph::intern` completes and answers a digest), then the process overflows. That
it stays inside `run_graph` is shown by returning early at its top, which removes the
overflow. Every probe inside it was negative: `evaluate_node` depth never passes 200, the
deep pass stays at depth 0, the clone walk below 300, `flatten_leaves` below 100, skipping
`consult_graph_pass` does not help, and skipping `graph::run` does not help either. The
`RUN before lookup` print does not survive the abort, so the last writes are lost and the
bisect needs a *flag-return* at each landmark rather than a print. The peel's six passing
tests are otherwise real (measured alone), so this overflow is the last blocker for landing
it.

**Measured again, the interference is deterministic and not the recording flag.** With the
peel in place: `a_graph_recorded_for_one_backend_runs_on_another` **passes alone** and
**fails in the family run**, and `a_graph_runs_at_whichever_extent_its_argument_names`
fails alone (extent refusal) but **overflows** when it runs after another test — with
`--test-threads=1` too, so it is cumulative global state rather than a race. The recording
flag is *not* stale: `graph::is_recording()` reads `false` at `build_graph`'s entry in every
case (probed), so the recorder is closed. The interference therefore lives in another
process-global store — the stub/`kernels()` registry or the installed backend — and is the
next probe. Until it is understood the family's count (1/10) under-reports the mechanics
(6/10 alone).

**The out-of-suite overflow does not reproduce, and the remaining blocker is cross-test
state.** Re-measured with a build confirmed fresh (the test binary relinked before the run):
with the peel in place `a_graph_runs_at_whichever_extent_its_argument_names` **fails on the
extent diagnostic, it does not overflow**, and the whole suite runs to completion. So the
earlier abort was a stale-binary reading. What stands is a **cross-test interaction**: the
six tests that pass **individually** reach only **1/10 in the family run** (`graph_jit`), so
another test in the same binary leaves process-global state behind — the suite has one
binary and the stub/kernel registry is global. That interaction, not the mechanism, is now
what keeps the peel out: fix the shared-state isolation first, then the peel's six tests can
land. The four failures in every mode are the **extent** read ("a dispatch's count is a
tuple or a struct value"), which `value_at_path`'s peel did not reach.

**With the gate lifted, the peel takes graph_jit from 1/10 to 6/10.** Measured test by
test with `.scratch-core/leaf-fix.patch` on top of `4fdc2bf2`: six tests pass
(`a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one`,
`a_dispatch_the_body_closed_over_is_refused_rather_than_recorded`,
`a_buffer_the_body_closed_over_is_refused_by_the_capture`,
`a_graph_recorded_for_one_backend_runs_on_another`, `one_backend_for_the_whole_graph_is_
checked_while_it_is_built`, `what_a_recorded_body_may_not_reach_for_is_refused_by_name`),
three fail with diagnostics, and **one overflows**:
`a_graph_runs_at_whichever_extent_its_argument_names`. Its recursion is **not** where the
earlier ones were: the shallow evaluation, `evaluate_node_deep_inner` (depth stays 0), the
clone walk (below 300), and the compute helpers `walk_role`/`assemble_parameter`/
`place_of_node`/`assemble_result` (no hit at depth 200) were all probed. The next probe is
the graph **run** path (`run_graph`'s `flatten_leaves` and the dispatch replay), which that
test alone exercises. The peel stays out until it is fixed, because one overflow aborts the
whole binary's run.

**The gate is lifted narrowly: the union dispatches `run_deferred` to its leaves.** The
composition's `OperatorExt` impl defined only `run`, so every union operator took the
*default* `run_deferred` and the compute leaf's own rule was never consulted. The union now
dispatches it (`4fdc2bf2`), and the compute leaf evaluates a **recording's** operand without
consulting its concreteness verdict. Measured: the recording reaches `record_launch` (four
launch runs per recording) and the family's refusal moves from "return not nameable" to
**"a dispatch's count is a tuple or a struct value"** — the extent read, which is the cfg
leaf peel by another name. With `.scratch-core/leaf-fix.patch` applied on top, **three**
`graph_jit` tests pass before the run aborts on the frozen-body recurrence, so the peel is
held out until that recurrence is fixed. compute 120, lowlevel 164, highlevel 136,
language 275 — all unchanged.

**Lifting the gate by ignoring a slot's type position costs 43 tests.** The gate is
`value_is_undecided`'s array arm, which counts *every* item of a slot — including the type
position, which is empty by design in the template. Making that arm skip the type position
made the recording's launch **run** (the recording then advanced to the output assembly and
refused there: "the callee of an instantiation must be a struct type"), but it also changed
the concreteness verdict everywhere: **compute 120 → 77 and highlevel 136 → 134**, because a
type cell being open is a real undecidedness for other readers. Reverted. So the fix must be
**narrow** (the recording's own operand check, not the global verdict) — the next probe is
what the recording-side `unrecordable` gate needs to accept an operand whose type positions
are open.

**The launch is gated by its operand's deep verdict.** Instrumenting `run_deferred` shows
the mechanism behind "no operator runs": it deep-evaluates the operator's operand and
returns `None` **without calling `run`** when the operand's `evaluated_deep` is unset or
undecided. For the recorded launch the operand evaluates to a value but its verdict is
**`undecided: true`**, so `record_launch` is never reached — the `ParLaunch` arm's own probe
(round 61) fires nothing for the same reason. The undecided item is the **cfg placeholder**
(the launch's `[kernel, cfg]` operand: its item's `evaluated_deep` is `undecided: true` and
it holds no value). So the next probe is the placeholder struct: a `GraphInput` cell is
decided by construction, and something under it reads undecided.

**Inside the recording no operator runs at all, and the pair's value read answers nothing.**
Two more measurements on the same test: (a) the returned pair's first item (`Index(5732)`)
evaluates to **`None`** even though its target (`5734`) holds an array and its selector is
the constant `0` — so the descent stops at that read; (b) instrumenting the extension
operator's `run` shows it is **never called while recording** (`is_recording()` true), and
the index reads the walk does reach end at value-only elements. The launch clones therefore
stay unevaluated, `record_launch` never runs, and the recording cannot name its return. The
next probe is why the read of `5734`'s item yields `None` — the item is the launch clone,
whose evaluation is the operator call the recording never makes.

**The recording clones the launches and never evaluates them.** Probes on
`graph_jit::a_graph_recorded_for_one_backend_runs_on_another`: every apply **instantiates**
(`APPLY fn … instantiated true`, including the graph body's apply); after the recording's
apply the module holds **four launch nodes owned by no function** (`6113`, `6188`, `6414`,
`6478`) — the recording's **clones** — and every one has an empty value; the returned pair's
items are all `shallow false`; and instrumenting `evaluate_node_deep_inner` shows the deep
walk **never visits a `ParLaunch` node** (zero hits). So the clone exists and the deep pass
does not reach it: the next probe is the value read the pair's item 0 names
(`Index(5732)`), whose evaluation gates the descent.

**Corrected: no launch ever has a value, and the recording's apply creates none.** The
user's objection is right — a function that never ran returns nothing decided. Measured: the
module holds **only the templates' launch nodes** (owners `9/10/11/14/22`), **every one with
an empty value**, and after the recording's apply the module still holds no launch node with
a value; forcing the value the returned pair carries (`d53b07e1`) does not change that. 

**The graph family's root: the body's launches are decided before the recording.** Probes
on `graph_jit::a_graph_recorded_for_one_backend_runs_on_another` with the recording in
place: the recording's own apply is fresh (`value-before None`, `recording true`), its deep
evaluation **succeeds** (`result Some(Array)`), yet the `ParLaunch` arm is **never entered**
and `recorded` is `None`. The body's launches were already decided by the check's definition
pass (every function's return is deep-evaluated there), so the apply's clone **bakes those
values** (`value_apply`'s baked rule) and the launches never re-run inside the recording —
`record_launch` cannot place a `GraphValue`. The parameter's buffer classes are now the
declared ones (`f6132eb5`), which did not change the family, consistent with this reading.

**A recorded body's return names a `ParLaunch` residual clone with no value.** The graph
recording reaches `returned_value_ids` with a block whose first item peels to a node whose
operator is `ComputeOperator(ParLaunch)` and whose `origin` is another residual — and
**neither the node nor its origin carries a value** (measured on
`graph_jit::a_graph_recorded_for_one_backend_runs_on_another`). So the recording's value
table does not name the node the body returns; reading the origin (landed,
`c8e7907f`) is not enough. The next probe is the node `record_launch`'s `GraphValue` is
placed on, and why the body's return is a *different* residual clone.

**The chain is created by the clone walk, and two sites overflow across runs.** With the
peel in place the chain's node-creation backtrace is `check_lam → check_term → … →
value_apply → node_apply → add_node`, i.e. the **clone walk** creates the contiguous ids,
and its own depth stays below 300 — so the chain is built *breadth* first. The overflow
then appears at one of two places in different runs (the clone walk's own recursion, or the
evaluation's 26 800-deep recursion over the built chain); both are the same defect: the peel
makes a frozen body's clone graph grow without a bound, and whichever walk follows it next
overflows.

**The peel's recursion follows one frozen clone per evaluation step.** The creation
backtrace of the chain's nodes is `check_lam → check_term → … → value_apply → node_apply →
add_node`, and the evaluation depth reaches 26 800 over contiguous node ids — so each
evaluation step clones a frozen body and evaluates *that* clone, which contains another
frozen call: the frozen body evaluates itself without a bound. The peel changes a cfg
leaf's value (a slot's value where a number was), and that value reaches the interpreter.
Peeling through *nested* slots does not stop it (measured: the +1 test still passes and the
cross-kernel test still overflows), so the recursion is not the slot depth.

**The peel's overflow is an unbounded chain the checker builds.** Instrumenting
`evaluate_node`'s depth shows 26 800 nested calls with the peel in place, over a *growing*
chain of contiguous dynamic nodes (`3800, 3801, … 3909, …`) interleaved with the prelude's
static reads (`3`, `7`, `10`, `11`) — so the recursion follows a chain the **check** creates
while it runs, not one the emitter walks. Without the peel the same program terminates, so
the peel sends some checker path into an unbounded expansion (each step a fresh node), which
the evaluation then follows. That is the defect to find before the peel can land; the
`value()` guard (`f9a0a461`) still bounds the lowering itself.

**The peel's overflow is not the lowering's guarded path.** With the peel and the
`value()` cycle guard both in place, the guard **never fires**: its path shrinks between
calls (`4 → 3 → 0`) because each `value` call *completes*, and the process still overflows
after the last print — so the unbounded recursion lies outside the lowering's definition
walk, in the phase that follows it (the graph's own evaluation, whose operand/definition
edges the peel can close into a cycle). That is why bounding the lowering did not stop it.

**The cfg leaf peel is measured again and kept out: it exposes a definition cycle.**
Applying `.scratch-core/leaf-fix.patch` (the peel at `value_at_path`) makes
`a_body_reads_a_scalar_leaf_of_its_input_struct` pass, as recorded before, and again makes
two cross-kernel tests overflow: traced with the peel in place, the lowering walks a cycle
(`value 446 → definition 2643 → value 2682 → … 446`) whose nodes the guard cannot stop
because each round reaches a *different* node (the chain is continued, not repeated). So the
peel is reverted and the cycle is the next defect for that family; `value()` now records the
nodes it is lowering and names a repeat (`f9a0a461`) instead of overflowing. Measured:
compute **120**, lowlevel 164.

**The loop family passes: an element read names the slot's value position.** `115 → 120`
with `peeled_argument` ending on `element_value_node`: the read's buffer argument is the
*value* position of the full `[value, type, attrs]` slot, the same reading the arc states
for elements everywhere. `plan_staging`'s declared plan and `tiled_reduction`'s closed form
now pass, so the `@loop` conversion lowers end to end (the marked recursion, the state
slots, the frozen operator, and the kernel) — the campaign's loop target is met.

**A frozen apply names its function in item 3, and the residual is not materializable.**
Two emitter facts moved the plan program's kernel to a full lowering: an apply records its
function type in **item 3**, so a callee whose own class is empty still names the frozen
function there (`static_callee` consults the value, the read chain and that slot), and the
callee's operator is read **before** any residual (the residual is the operator's own node,
and an operator inside a call's argument has none). Measured: the diagnostic moves from the
kernel-body Apply refusal to the parallel extension's input-buffer check — the kernel now
lowers end to end — while compute stays 115 (the extension check is the next layer).

**The entry is the test's own `Geq`: its first operand is the type-graph chain.** The
emitter's path is `lower_loop_tests → value(condition) → definition(Geq) →
arithmetic → value_item(Geq, 0) → value(4318)` — so the **test comparison's operand 0** *is*
the frozen type-graph read (a clone, origin = the frozen call's apply node), not the state
read. The clone binds that operand directly, so the wrong node comes from the **frozen
module's own graph** (the `>=` inside `greater_or_equal`): the comparison's left is already
the type side there. The next probe is that frozen graph's `Geq` operands — the core
compile's class merge, most likely through the refinement/assert path
(`operands : array{(_ ! in_num), 2}`).

**The type side no longer copies the frozen body; the remaining clone is the value side's.**
Both type-side static applications copied the frozen **return pair whole** — and that pair's
value is the *body*, so the type side dragged `greater_or_equal`'s graph into the caller and
its parameter reads were bound by the type arguments. They now copy only the type and
attribute positions (the value is a hole), measured **114 → 115**. The remaining opaque
(`4318`) is a fresh clone whose origin is the *apply node*, created by
`static_function_apply` **inside the fold's own clone** (a nested static application): that
path does unify the parameter with the argument, so the next probe is the nested clone's
argument binding.

**The inlined body is `greater_or_equal`, and the emitter reaches it as a `definition`.**
`core.lichen` writes `greater_or_equal = operands => { operands : array{(_ ! in_num), 2};
operands[0] >= operands[1] }`, so `s(0) >= 7` becomes an apply whose single argument is the
pair, and the `Geq` node is that body's `BinOp`, cloned into the caller by
`static_function_type_apply`'s `static_node_apply(r#return, …)` with the signature's
remap. The emitter reaches it through `definition` (not `apply`), so it reads the operand
array as `[left, right]` — correct for the `BinOp` form — and the wrong node is the left,
which the remap bound to the `compute.read` callee path instead of the argument pair's first
element. The next probe is `materialize_static_signature_with_classes`' parameter remap.

**The `Geq` node is a cloned frozen body, not a check-time comparison.** Only one `Geq`
is created by the checker (`964`, in block 10) and it is not the traced node: the `Geq` the
emitter walks (`4468`) is owned by the fold but created by the **clone** — the inlined body
of the **frozen `geq`** the preprocess desugars `s(0) >= 7` into. Its first operand is the
`compute.read` **callee path**, where the call's first argument (`s(0)`) belongs, so the
frozen callee's **argument substitution is shifted**: the inlined parameter bound to the
callee operand rather than to the argument. The next probe is that substitution
(`materialize_static_return_apply` / the routed residual's parameter binding).

**The merged operand is a template node, and the merge happens after the check.** Every
node of the traced chain (`1298` the condition, `4450` a frozen path read, `4468` the `Geq`)
is owned by the fold — all **template** nodes. At `check_binop` time, no comparison's left
operand has a frozen chain (measured over the whole program), so the operand array was
built correctly and the frozen chain reached the operand **afterwards**: the check-time
application of the fold (the type-side apply and its clone) merged the left value's equality
class with the `compute.read` callee path, and the emitter's view/definition walk then
follows the class representative into the frozen path. So the defect is the **class merge in
the check-time apply**, not the clone's remap.

**The wrong operand is the *clone's* state read.** The spine root is the fold's own node
(`1302`, owner the fold), and no `check_binop` builds a comparison with a literal right in
this program — so the `Geq` node the emitter walks (`4468`) is the **check-time clone** of
the fold's comparison, whose left operand (`4470`) is the clone's state read. That clone is
`Index(Index(Index(<wildcard mint>, 15), 901), 15)` — a read through the frozen prelude's
node table — where the template states a state read. So `function_apply`'s remap clones the
state read's container to a frozen structure; the comparison and the arithmetic branch are
fine.

**The comparison's left is a state read at check time and a frozen chain after the
clone.** `check_binop`'s left operand chains are dynamic reads at check time (measured over
the whole program), so the checker builds the comparison correctly; it is the **loop's
clone** that turns that left into `Index(Index(Index(<wildcard>, 15), 901), 15)` — a read
through the **frozen prelude's node table** (module 3 indices). So the defect is in
`function_apply`'s **remap**: the state read's target (the parameter) cloned to the wrong
node. The next probe is the remap of the parameter and of the read's container chain.

**The test condition is right; its comparison's left operand is a type-side chain.**
Corrected measurement: the condition (`Index(1249, 16)`) resolves `Views → Views →` a
`TypeOperator(Geq)` apply — the `if`'s own comparison — but that comparison's **left
operand is the frozen node-table chain** `Index(Index(Index(<wildcard>, 15), 901), 15)`, a
*type-side* structure in a *value* position (`docs/notes/apply-type-side-check.md`). So the
spine walk is fine; the **comparison's argument** carries a type where the state read
belongs — a class/unify merge of a value with a type cell is the next suspect. Two
principled fixes landed first: `selection_of` reads a frozen selector through the shared
accessor, and `walk_test` resolves views before reading its arms (both measured neutral).



**Only the type-side apply has an unstated callee, measured over the whole module.** A
scan classifying every `Apply` by whether its callee states a value prints **no value-side
apply among the unstated ones**: each unstated apply has a *sibling* that shares its
result cell and function type, and the sibling's callee is a stated function (mostly
`Function(Static)`, once the fold's `Function(Dynamic)`). That sibling is the **value-side**
apply of the same call, and the unstated one is the **type-side** apply `check_app` builds
(`the type applies, the body does not`). So no empty *value* apply is placed anywhere; the
defect is that the loop's lowering reaches a **type-side** apply for a value. A routing
that lowers the sibling measured neutral (114) and was not kept, because it scans the
module per unresolved apply; the fix must identify the pairing from data the emitter
already holds (the recorded `apply_edges` are checker-only, so the module needs the pair
relation).

**The loop body's failing callee is a read of an undecided pair.** Probing the refusal
itself: the apply's callee is `Index(691, 0)` where `691` is an **empty cell** — no value,
no operation, so `array_items_of` answers `None` — and `static_function_of_callee` returns
`None` for it. So the callee the loop's lowering holds is a `value_of` **read** whose pair
was never filled in the clone, not the shared wildcard and not the type-side apply (those
are the *template's* pairing, measured last round). The fix therefore belongs at the
callee's **definition resolution**: the emitter must follow the read to what defines the
pair (the frozen function value) before deciding it is unresolvable.

**The empty callee is the *type-side* apply, by design.** The fold's template holds the
applies in **pairs** for one call site: `610` with the callee `556` = `Function(Static)`
and the argument pair `605` — the **value-side** apply — and `613` with the callee `611` =
an **empty cell**, the same result-type cell `608` and the same function type `557` — the
**type-side** apply whose callee is `check_app`'s `undecided_callee` ("the same four
elements over an undecided callee: the type applies, the body does not"). The same pairing
repeats for every call in the body (`738`/`741`, `879`/`882`). So the callee is empty
**because the apply the loop's lowering reached is the type-side one**, not because a
wildcard was cloned; the emitter must not lower a type apply for a value, and the value
apply for the same call is identifiable by the shared result-type and function-type
operands.

**No apply in the finished module has a wildcard callee, and node ids do not carry
across runs.** A full scan of every function's `Apply` nodes prints **nothing** whose
callee operand holds the shared wildcard, and `check_app`'s per-apply probe shows every
checker-built apply naming a real function. The empty callee is therefore **not** in the
finished graph as a wildcard: the wildcard exists only in the *template* array from which
the clone walk mints an empty cell, and the mint's clone is what the apply's operand array
holds. Also measured: node ids shift between runs (the round-35 id-based backtrace matched
its own run only), so identification must be structural — the next probe must name the
template array by its *owner and shape*, not by an id.

**The wildcard mints are the `_` arguments, and the apply's array stays four elements.**
Probing the clone walk's wildcard mints shows them at index 0/2/3/4 of 5-item `layout 1`
arrays — **pairs** whose value is the shared wildcard, i.e. the `_` wildcard arguments of
`(compute.Write _)` / `(compute.Read _)`; those are legitimate. The apply the emitter sees
still has its **four-element** operand array (`[callee, argument pair, result cell, function
type]`), so the earlier "operand array is a pair" reading was wrong: the callee's **value**
is the wildcard's clone, and the template apply must have been built with an unstated
callee. Correcting this before the next probe.

**The wildcard is not set at check time.** A second probe of `check_app`'s callee value
(compared through `AsEnum<LowValue>` this time, so the first probe's dead conversion is
ruled out too) fires **nowhere**: every apply the checker builds names a real callee. So
the wildcard reaches the template apply's operand array **after** the check — the
remaining candidates are the type-side apply's undecided-callee cell and the clone walk's
own operand-array construction.

**The empty callee is the apply's only unstated operand.** The template apply's four
operands read `[<none>, array, array, array]` — the callee alone has no value, while the
argument pair, the result type cell and the function type are all decided arrays; the two
sibling applies in the same body read a static function in that position. So the template
stores a wildcard as the callee value; the clone then mints its empty cell. `slot_pair`'s
attribute tail is where wildcards are minted for pairs, so the next question is whether the
apply's callee operand *is* a pair's attribute slot rather than the function value — the
operand order of `value_apply` is the thing to re-read.

**The empty callee is a wildcard minted by the clone walk.** A backtrace on the 383rd
empty node names `node_apply`'s **wildcard branch** inside `value_apply`: the template
apply's operand array holds a **wildcard** (`Any`) at that position, and the clone walk
mints a fresh empty cell for it (the invariant: "each reach mints its own undecided cell").
So the callee the loop body carries is not a materialized static leaf but a wildcard's
clone — the template's apply has a wildcard where the frozen `compute.read` value should
be. `materialize_leaf` and `check_app`'s wildcard-callee probe both fire nowhere, so the
wildcard enters the template operand by another path — that path is the next thing to
find.

**The empty callee is owned by the marked function and has no origin.** Probed:
`693`'s owner is the fold, `origin` is `None`, block 3 — a **fresh** node created while
checking the fold's template, not a clone and not a `materialize_leaf` (those carry the
`Function(Static)` value). `refused_pair` is ruled out as the source: it sets the value
and the type to the *same* cell, and the apply's type operand (`694`) differs from `693`.
The next probe is a backtrace on the checker's cell creation for the callee expression,
not a guess.

**The loop body's prelude call has an empty callee cell.** With the call-lowering in
place, the loop's own lowering starts and refuses at the fold body's `read` call: the apply
`1064`'s callee `693` has **no value**, its class has no defining member, and
`static_function_of_callee` answers `None` — so `routed_operator` cannot route it and the
emitter refuses "kernel body Apply is supported only for a cross-kernel callee". The other
applies in the body resolve (the fold's own `Apply` with a dynamic function, and two
static function values), so the missing fact is specific to the callee cell the loop body
carries. Next: find who mints `693` (a fresh cell, not a `materialize_leaf` — that one
would carry the `Function(Static)` value) and make it carry the static function.

**The superior's reading is confirmed: the record keeps the call's apply.** Measured: the
write's container (the record's element read) defines — through `define_in` — to
**`NodeId(1583)`**, a `LowOperator::Apply` whose `callee_function` is the marked function
(the fold). So the call is *in the graph*; the emitter only misses it because it lowers
the value peel (`Index(pair, 0)`), whose definition is the return expression (the `if`).
Lowering the **call** there instead (a `call_of_value_read` resolver in `write_call`) moves
the refusal to the next layer — "kernel body Apply is supported only for a cross-kernel
(kernel-value) callee" — on the loop body's own `read` call, whose callee cell is not
recognised as a frozen function. Measured 117 → **114**: the three loop-path tests regress
because the loop path is still incomplete, so the fix was reverted; it is the right fix
once the loop body's prelude-call callee resolves.

**The suite's count includes order-dependent failures, measured.** A bisect of
`helper_refuses_a_parallel_refusal::a_compute_parallel_refusal_fails_the_helper` converged
on a **docs-only** commit, which is impossible as a behavioural cause — and the test then
passed **20 of 20** runs in isolation at HEAD while it fails inside the full suite (also
single-threaded, 117/136 both ways). So the failure is *process-global state carried
across tests* (the candidate carriers: the thread-local `GRAPH_POLICY`, the process kernel
registry), not a per-test fact, and the bisect over it is invalid. The suite's 136 is
therefore an upper bound on independent failures.

**The seed rule is the superior's: a node with a type slot is not seeded.** Following
that ruling, `prepare_template_low_types` no longer seeds the pair (its type slot states
the shape); only the pair's **value** — a node without a type slot — takes the seed.
Measured neutral (compute 117, lowlevel 164, other suites green). The `graph_jit`
class-shape panic that motivated a refinement relaxation is **not** from that seed: it
comes from a value seed whose stated type has an **open** position that an observation
later decides. The relaxation was measured **+1 (117 → 118)** with every suite green and the
panic gone, but per the superior's ruling below it is not the fix.

**The superior's verdict on the seed, verbatim (recorded round 93 on request).**

> seed 和 observation绝不同时存在于一个node，seed是高层为没有对应type node的那个node提供的

Read out: a seed and an observation never coexist on one node; the seed is what the highlevel
provides for the node that has **no corresponding type node**. So the fix is never to let the
two statements merge: when a class carries a seed and an observation reaches the same class,
some **seeding site seeded a node that has a type node** (or a union merged such a node into a
seeded one), and that site is the defect. The assertion is the invariant and stays; the
relaxation is reverted (it was measured again at +2 with the peel in the tree, compute
225 → 227, and still rejected). No further question about it is to be asked.

**The approved Invariant change is refuted by measurement.** With the superior's approval
of both changes proposed here, they were implemented and measured: removing the unroll
fallback for a refusing marked recursion takes compute **117 → 114** and fails
`host_loop::an_unconvertible_marked_recursion_still_expands` — that test *is* the
fallback's contract — and adding the parameter-end path walk gives 114 with the same
lowlevel failure. All three variants (unroll-only, walk-only, both) are negative, so both
were reverted and the fallback stays: it is what lets those programs run at all, and the
next work is the **loop path's runtime** for the plan/tiled shapes, not the fallback.

**The opaque is hit outside the nest, and the record keeps the unroll's result.**
`opaque` prints `loops []` for node 3712 — the empty clone is lowered while emitting the
**write call**, outside any nest, and the record's `.value` item is the step's **Add apply**
(not the fold call's apply): the check-time **unroll** substituted the call's result into
the record. The two coordinated changes that would fix this chain are recorded for the
next round: (a) `function_apply` returns `None` (lazy) for a marked function whose
conversion refuses instead of unrolling it, so the record keeps the call, and (b)
`parameter_value_path` follows the chain to the parameter and pops the pair peel so the
emitter's own conversion succeeds. Both touch Invariants (the unroll fallback and the
refusal verdict) and need the superior's ruling before they are made.

**The record's `.value` slot already holds the return expression.** Followed
structurally: the write's operand is `Index(Index(record, 2), 0)`; the record's value array
is `[1493, 525, 1583]` and item 2 (`1583`) is the step's **Add**, not a pair — so
`check_record`'s `state[field].term` for the `.value` field was already the call's *value*
(its return expression), not the apply's pair. A resolver that follows
`Index(Index(array, k), 0)` to the array's item and looks it up in `applied_pairs` fires
(the chain is found) but finds no applied pair there, so it changes nothing (117) and was
reverted. The next fix is at the **call's term**: `check_app` must leave the call's pair as
the field's term so the record keeps the call.

**The write's value loses the call at the record, measured.** The write's third operand is
`Index(3225, 0)`; `3225` is the write's field pair, its class's defining member is the
**step's Add** (the fold body), and `3225` is **not** in `applied_pairs` — so no path in
the emitter reaches the call. The cause is upstream of the emitter: the record's element
slot was filled with the argument's **value** (`pair_value_half` of the call's pair = the
function's return expression), so the call expression itself is not in the record and
cannot be recovered. An `applied_call` interception in the emitter (a value read of an
applied pair lowers the call) is written and measured neutral (117) — the pair is never an
applied pair here — and was reverted. The fix must keep the call's **pair** in the record's
element slot, which is the encoding question this arc is about.

**The loop conversion's refusal still masks the opaque.** Re-tested the alias retry on
the 117 tree: 114 (−3, the same `plan_staging`/`tiled_reduction` three). With the
conversion succeeding, `tiled_reduction`'s kernel compilation reaches the **opaque**
(`a kernel body reached a node with neither a value nor an operation`) — so those three
were passing only because the conversion refused, and fixing the conversion alone exposes
the opaque. `defining_member` now prefers a class's **Apply** member (a call defines the
value it produces; its expanded body is the implementation), measured neutral (117) — the
write's value class holds no Apply for the pack program.

**Nested tuple domains work (+2).** Two changes make `tuple{tuple{Int, Int}, Int}`
compile and run: the transfer's `Index` now states the **element slot pair** for a
compound *value*'s read (the policy `docs/notes/lowlevel-low-types.md` already states —
"A compound's elements are complete slots … `Index(compound, k)`'s transfer states
`Tuple([element, Unknown])`" — and the code indexed the slot as if it were the tuple), and
`low_type_of_slot` decodes a **type pair** by the kind marker its type position holds.
compute went **115 → 117** (136 failed), and the language 275 / highlevel 136 / lowlevel
164 suites stay green.

**The array read's typing is fixed for structured containers.** `check_index` now types
its result by the **container's kind**: a tuple or struct element states its own type cell
(`slot_read`'s `element_ty`), an array keeps the array element type. Measured: no test
flips (compute 115), the embedded prelude still checks, and language 275 / highlevel 136
stay green. The nested domain is still refused: with the type-pair decode added as well,
the class-shape assertion still fires on the pair wrapping the **outer** read — so the
offending type comes from the *tuple read* path (`structs.rs`, which already uses
`slot_read`'s `ty_node`) or from the class seed on the container value, not from
`check_index`.

**The nested conflict is a checker typing, and the obvious repair breaks the prelude.**
`prepare_template_low_types` seeds a pair's *value* with its type slot's shape, and the
pair wrapping `p(0)(0)` (node 88, value a scalar read) states `tuple{Int, Int}` — the
container's type, not the element's. `check_index` types every element read with
`elem_cell` (the **array** element type path
`[TYPE_SHAPE_SLOT, ARRAY_TYPE_ELEMENT_SLOT, PAIR_TYPE_SLOT]`) and throws away
`slot_read`'s own `element_ty` (`let _ = element_ty`); for a tuple/struct container the
type must be the element **slot's** own type cell. Swapping to `element_ty` makes the
embedded prelude fail to check ("this value is not a container — it has no element to
read"), so the repair must keep the type-cell identity: choose the element slot's type
cell for tuple/struct containers and the array element type only for arrays.

**A nested tuple domain decodes, then conflicts with the class channel.** With the
domain reader fixed, the simple `tuple{Int, Int}` jit compiles; the nested program
(`tuple{tuple{Int, Int}, Int}`) still refused. A discriminator that recognises a **type
pair** by the kind marker in its type position (so `low_type_of_slot` decodes nested type
pairs too) makes the nested program *decode* — and both remaining jit tests then **panic**
at `equality.rs`'s class assertion: `a class states one shape`, left
`Tuple([Some(USize), Some(USize)])`, right `USize`. So the nested element's class is seeded
as the tuple while a leaf read observes a scalar: the domain reader and the class channel
disagree about a nested domain, which is the next fix there. The discriminator change was
reverted (it turned a refusal into a panic).

**The alias retry fixes the conversion and flips nothing.** `562` (`Index(parameter, 0)`)
is joined to the state's value cell `548` by `alias_read` when that read is *evaluated*
(`evaluate_node_operation` → `alias_read` → `unify`), which is why later runs convert. A
retry in `function_apply` that joins the peel to the value before re-converting makes the
apply-time conversion succeed (3 of 5 runs flip to `tuple: true`), and **no test flips:
compute stays 115, and the same three plan/tiled tests regress (112)**. So the conversion's
verdict is not what those tests need; with the loop path taken, their runtime answers
differ. Hypothesis worth testing next: the failed apply-time conversion forces the
**unroll** path, and that unroll is what inlines the marked body into the caller (the
write's value reaching the `if`, round 18) — measured only as "the retry changes nothing",
not yet as "the inlining stops".

**The loop conversion's false refusal is measured, and three repairs were tried.** The
emitter's first `loop_conversion` runs before the parameter's value cell is unified with
the pair peel `Index(parameter, 0)`: the walk follows the chain to the parameter, finds no
read, and `state()` refuses a conversion whose later runs (3 of 5) succeed. Three repairs
were measured against the 115 baseline, and **all three regress the same three tests**
(`plan_staging::a_declared_plan_stages_the_reduction_and_answers_the_closed_form`,
`plan_staging::a_plan_the_fragment_refutes_is_refused_by_name`,
`tiled_reduction::a_tiled_reduction_answers_the_closed_form_and_the_per_lane_loop`):
following the chain to the parameter and popping the pair peel (correct classification,
112), accepting the positional state when `arity > 1` (112), and both together. Those
conversions must stay **refused**, so the fix is not the verdict but the unification: the
state's value cell must be unified with the pair peel *before* the emitter's first
conversion. The next step is to find which later evaluation performs that unification.

**The parameter's type slot is a type pair, and the domain reader fixed 16.** A type
expression is encoded as a **term pair** whose value position is its shape and whose type
position is its kind (`TypeValue(TypeTuple)` …), so `kernel_parameter_domain` decoded the
value position alone and answered `None` for every annotated parameter — the jit family's
"parameter's class is not decided". Decoding the *slot itself* first
(`low_type_of(...).or_else(|| low_type_of_slot(...))`) fixes it: compute went **99 → 115
passed** (154 → 138 failed), the other suites unchanged.

**The failure groups, measured** (`--nocapture`, first diagnostic per failing test):

| first diagnostic | tests |
|---|---|
| a kernel body reached a node with neither a value nor an operation | 124 |
| compute.graph: this function's return is not a value a graph can hand back | 97 |
| a `@loop`-marked recursion … is not convertible (`StateShape`) | 83 |
| compute.jit: the kernel parameter's class is not decided | 22 |

so the **opaque** family is the dominant one, and it is the chain this note has been
peeling. A clone of the marked function's node is now recognised by the nest
(`loop_owner`/`loop_value` follow the origin), and the minimal `@loop` kernel's remaining
opaque is the **entering tuple's element**: the state unification merges the initial
tuple's element class with the step's read, so the element's defining node is the read
(`s(1)`'s clone) and the emitter asks it for a value outside the nest.

**The entering state's elements are peeled, and the wildcard is the blocker.**
`loop_state` now takes each element slot's value (the third reader of the same pattern).
The minimal `@loop` kernel's refusal traces to the label-annotated step element: its
clone's operand is the annotation's **wildcard** (`Any`), which the clone walk mints as an
empty cell, so the arithmetic lowering asks that empty node for a value and the emitter
refuses it as opaque. The fix is the emitter's annotation handling — a label-annotated
value must be emitted as its underlying value, not as an application of the label — which
is the next step.

**The empty clone is a leak across functions, measured.** Followed with a minimal
`@loop` kernel: the kernel's body walks `VALUE 3712` (an empty clone, owner the kernel,
origin `665`) into `VALUE 665`, a node of the **marked function's** template, and then
lowers its `Index(562, 1)` (`s(1)`) with no nest active — the index placement refuses.
So the checker's undecided evaluation leaves a kernel-body clone whose origin is another
function's node, and the template walk follows it. Emitting such a clone from its origin
was wrong (it re-emits the marked function's body unbound); the emitter now honours the
origin only inside the body's own function and refuses a foreign one by name. The real
fix is upstream: the checker's evaluation must not leave that clone in the kernel's body.

**The emitter's empty clone is fixed, and the next blocker is measured.** A kernel body
holds clone nodes the checker's own undecided apply left behind (owner the body function,
origin a template node, no value and no operation); the emitter refused them as opaque.
It now emits an empty clone from its template. `parameter_path` also refuses a pair's type
(`1`) and attribute (`2+`) positions as path steps. No test flips; the minimal `@loop`
kernel now stops at "a kernel body's index cannot be placed": a read of `Index(<a
computation>, 1)` whose chain ends at the marked function's parameter pair — the type
position of a pair read as a value. The next step is the caller that asks for a type
node's value.

**The path walk is rewritten, and the blockers moved.** `parameter_value_path` now reads
the chain from the value side (`state_path`): element positions alternate with the slot's
value position, so a pair peel and every slot peel are dropped and a pair's type or
attribute position ends the match. `check_reads` also distinguishes a **use** (an operand)
from a value's own **items** (the encoding), so a record's value cell is no longer refused
as a whole-state read. Measured: no test flips, all suites green; the attribute read that
used to refuse (slot four) is gone, and the refusal site moved to `state()`'s "a
tuple-valued state never read by element" for a conversion whose element reads are not
classified — the next thing to identify (it is a different recursion than `fold`'s).

**The refusing read's chain, measured.** For `pack_placement`'s reduction, the node the
conversion refuses reads `Index(Index(Index(parameter, 0), 0), 4)`:
`Index(parameter, 0)` is the parameter **pair's** value, the next `0` is the state
**tuple's element 0** (the element *slot*), and the `4` is that slot's **attribute 4** —
the pair's attribute tail, not a state position. `parameter_value_path` treats the last
Index on the pair as the peel and returns `[4]`, so an attribute read is validated as
state element 4 against arity 2. Guarding on the value's layout does not work: at
conversion time the values are undecided (`None`), so the walk needs the slot/attribute
split from the **type** side, not the value side. That is the next fix.

**One conversion mismatch is fixed, one measured.** `parameter_value_path`'s two
"chain ends at the parameter" branches now strip the pair's value position the same way
(`loop_conversion.rs`), so a state read no longer surfaces as a spurious `[0, …]` prefix
and stops being refused by the wildcard arm. No test flips on that alone. The remaining
refusal is a read whose chain is `Index(Index(pair, 0), 4)` against a state of arity 2 —
a five-slot structure read as state position four — so the next question is which
function's parameter that read belongs to (the conversion's roots include nodes of the
kernel the recursion sits in).

**The loop conversion is the other half of the fuse/pack family.** `pack_placement` and
`fuse_licence` programs first fail with `compute.parallel: a @loop-marked recursion this
kernel body applies is not convertible ... the shape is a non-scalar carried state`
(`docs/notes/loop-conversion.md` §4), and the graph refusal is the *second* diagnostic.
Probing `LoopRefusal::StateShape`'s sites (`DBG_LOOP` line tags) names `check_reads`
(`crates/lichen-lowlevel/src/loop_conversion.rs`): a whole-parameter read meets
`tuple || arity != 1`, and an element read `s(k)` meets `k >= arity` — so the state's
arity and its addressing disagree for a reduction chain whose step carries
`(i + 1, 0.0 ~ "identity")`. That conversion runs from the fragment compiler, so it is a
kernel-side reader of the same slot encoding.

**The graph block's root is narrowed, not fixed.** Probes on a `compute.graph` body with
two `plrun`s: the body's apply decides to a real `Buf`, `returned_value_ids` misses it,
and **no refusal is recorded** — and `ComputeOperator::ParLaunch`'s `run` arm never
executes (a print there never fires), neither inside nor outside the recording window.
So the launches are executed (or already decided) without the operator path the recorder
intercepts: the two suspects are the checker's forced evaluation deciding the body's
nodes before the graph build, and the fuse pass rewriting the launches. The leaf-fix
patch below is unrelated to this family (the graph tests fail with and without it).
That family is 9 `graph_jit` + 10 `fuse_licence` + the fused-region tests, so it is the
next keystone.

**The cfg leaf walk is the next compute seam** (measured, not landed). `value_at_path`
(job of the role walk in `compute.rs`) takes the element node where the value is: since
the compound encoding, `items_of` over a compound returns **slots**, so a scalar leaf
reads a slot pair instead of its value. Peeling a slot (a `layout != 0` pair's first
item) turns `a_body_reads_a_scalar_leaf_of_its_input_struct` green and lets the
cross-kernel programs read `(1, 2, 8, …)`, but two cross-kernel tests then **overflow
their test thread's stack** (`a_cross_kernel_call_agrees_across_the_two_backends`,
`a_body_may_compute_in_one_class_and_cross`) while a standalone probe of the same program
runs fine even on a 2 MiB thread — so the peel exposes a recursion the recording reaches
only once the launch's extent and leaves are correct. The patch is kept at
`.scratch-core/leaf-fix.patch`; the next step is that recursion, not another reader.

The isomorphism is not complete yet: with the record path disabled the *write* case stays
red (the certificate decides nothing — the field read's attribute provider inside the
materialized clone is still a static node), so the field reference still rides the record
while the whole-parameter reference rides the materialization. Closing it means making
that provider decide under the clone — then the record can go.

## 2. Nothing else

Every other suite is green; the three counts above are the reading. Two smaller defects are
documented rather than left open:

- The apply-time diagnostic renders the compared *types*, so a value-level conflict recorded
  through the apply edge prints both sides alike ("expected Int, found Int"). Left as a
  documented limitation: no program reaches it today — the one that did was the stale-value
  conflict the mint fix closed
  ([shallow-depth-element-values](shallow-depth-element-values.md) §22) — and the smaller
  fix (fall back to the compared values when the two sides read alike) buys little.
- The apply's type-side value merge is load-bearing for refinement reporting
  ([apply-type-side-check](apply-type-side-check.md)).
 The
plan program's remaining opaque is reached through `lower_loop_tests` → `value` →
`lower_definition` → `arithmetic` → `value_item` → `value`: the conversion's
`tests[0].condition` is `Index(apply, 0)` — the **call's value read** (`Index(1249, 0)`,
`1249` an `Apply` whose pair's value is an array) — where the `if`'s own condition
(`s(0) >= 7`) belongs. So the computation chain is wrong at the **spine walk**: the
conversion recorded a call's value as the test's condition, and the emitter then traced
through it into the type-side structure. The next probe is the conversion's `walk_test`
classification of that node.

**Only the type-side apply has an unstated callee, measured over the whole module.** A
scan classifying every `Apply` by whether its callee states a value prints **no value-side
apply among the unstated ones**: each unstated apply has a *sibling* that shares its
result cell and function type, and the sibling's callee is a stated function (mostly
`Function(Static)`, once the fold's `Function(Dynamic)`). That sibling is the **value-side**
apply of the same call, and the unstated one is the **type-side** apply `check_app` builds
(`the type applies, the body does not`). So no empty *value* apply is placed anywhere; the
defect is that the loop's lowering reaches a **type-side** apply for a value. A routing
that lowers the sibling measured neutral (114) and was not kept, because it scans the
module per unresolved apply; the fix must identify the pairing from data the emitter
already holds (the recorded `apply_edges` are checker-only, so the module needs the pair
relation).

**The loop body's failing callee is a read of an undecided pair.** Probing the refusal
itself: the apply's callee is `Index(691, 0)` where `691` is an **empty cell** — no value,
no operation, so `array_items_of` answers `None` — and `static_function_of_callee` returns
`None` for it. So the callee the loop's lowering holds is a `value_of` **read** whose pair
was never filled in the clone, not the shared wildcard and not the type-side apply (those
are the *template's* pairing, measured last round). The fix therefore belongs at the
callee's **definition resolution**: the emitter must follow the read to what defines the
pair (the frozen function value) before deciding it is unresolvable.

**The empty callee is the *type-side* apply, by design.** The fold's template holds the
applies in **pairs** for one call site: `610` with the callee `556` = `Function(Static)`
and the argument pair `605` — the **value-side** apply — and `613` with the callee `611` =
an **empty cell**, the same result-type cell `608` and the same function type `557` — the
**type-side** apply whose callee is `check_app`'s `undecided_callee` ("the same four
elements over an undecided callee: the type applies, the body does not"). The same pairing
repeats for every call in the body (`738`/`741`, `879`/`882`). So the callee is empty
**because the apply the loop's lowering reached is the type-side one**, not because a
wildcard was cloned; the emitter must not lower a type apply for a value, and the value
apply for the same call is identifiable by the shared result-type and function-type
operands.

**No apply in the finished module has a wildcard callee, and node ids do not carry
across runs.** A full scan of every function's `Apply` nodes prints **nothing** whose
callee operand holds the shared wildcard, and `check_app`'s per-apply probe shows every
checker-built apply naming a real function. The empty callee is therefore **not** in the
finished graph as a wildcard: the wildcard exists only in the *template* array from which
the clone walk mints an empty cell, and the mint's clone is what the apply's operand array
holds. Also measured: node ids shift between runs (the round-35 id-based backtrace matched
its own run only), so identification must be structural — the next probe must name the
template array by its *owner and shape*, not by an id.

**The wildcard mints are the `_` arguments, and the apply's array stays four elements.**
Probing the clone walk's wildcard mints shows them at index 0/2/3/4 of 5-item `layout 1`
arrays — **pairs** whose value is the shared wildcard, i.e. the `_` wildcard arguments of
`(compute.Write _)` / `(compute.Read _)`; those are legitimate. The apply the emitter sees
still has its **four-element** operand array (`[callee, argument pair, result cell, function
type]`), so the earlier "operand array is a pair" reading was wrong: the callee's **value**
is the wildcard's clone, and the template apply must have been built with an unstated
callee. Correcting this before the next probe.

**The wildcard is not set at check time.** A second probe of `check_app`'s callee value
(compared through `AsEnum<LowValue>` this time, so the first probe's dead conversion is
ruled out too) fires **nowhere**: every apply the checker builds names a real callee. So
the wildcard reaches the template apply's operand array **after** the check — the
remaining candidates are the type-side apply's undecided-callee cell and the clone walk's
own operand-array construction.

**The empty callee is the apply's only unstated operand.** The template apply's four
operands read `[<none>, array, array, array]` — the callee alone has no value, while the
argument pair, the result type cell and the function type are all decided arrays; the two
sibling applies in the same body read a static function in that position. So the template
stores a wildcard as the callee value; the clone then mints its empty cell. `slot_pair`'s
attribute tail is where wildcards are minted for pairs, so the next question is whether the
apply's callee operand *is* a pair's attribute slot rather than the function value — the
operand order of `value_apply` is the thing to re-read.

**The empty callee is a wildcard minted by the clone walk.** A backtrace on the 383rd
empty node names `node_apply`'s **wildcard branch** inside `value_apply`: the template
apply's operand array holds a **wildcard** (`Any`) at that position, and the clone walk
mints a fresh empty cell for it (the invariant: "each reach mints its own undecided cell").
So the callee the loop body carries is not a materialized static leaf but a wildcard's
clone — the template's apply has a wildcard where the frozen `compute.read` value should
be. `materialize_leaf` and `check_app`'s wildcard-callee probe both fire nowhere, so the
wildcard enters the template operand by another path — that path is the next thing to
find.

**The empty callee is owned by the marked function and has no origin.** Probed:
`693`'s owner is the fold, `origin` is `None`, block 3 — a **fresh** node created while
checking the fold's template, not a clone and not a `materialize_leaf` (those carry the
`Function(Static)` value). `refused_pair` is ruled out as the source: it sets the value
and the type to the *same* cell, and the apply's type operand (`694`) differs from `693`.
The next probe is a backtrace on the checker's cell creation for the callee expression,
not a guess.

**The loop body's prelude call has an empty callee cell.** With the call-lowering in
place, the loop's own lowering starts and refuses at the fold body's `read` call: the apply
`1064`'s callee `693` has **no value**, its class has no defining member, and
`static_function_of_callee` answers `None` — so `routed_operator` cannot route it and the
emitter refuses "kernel body Apply is supported only for a cross-kernel callee". The other
applies in the body resolve (the fold's own `Apply` with a dynamic function, and two
static function values), so the missing fact is specific to the callee cell the loop body
carries. Next: find who mints `693` (a fresh cell, not a `materialize_leaf` — that one
would carry the `Function(Static)` value) and make it carry the static function.

**The superior's reading is confirmed: the record keeps the call's apply.** Measured: the
write's container (the record's element read) defines — through `define_in` — to
**`NodeId(1583)`**, a `LowOperator::Apply` whose `callee_function` is the marked function
(the fold). So the call is *in the graph*; the emitter only misses it because it lowers
the value peel (`Index(pair, 0)`), whose definition is the return expression (the `if`).
Lowering the **call** there instead (a `call_of_value_read` resolver in `write_call`) moves
the refusal to the next layer — "kernel body Apply is supported only for a cross-kernel
(kernel-value) callee" — on the loop body's own `read` call, whose callee cell is not
recognised as a frozen function. Measured 117 → **114**: the three loop-path tests regress
because the loop path is still incomplete, so the fix was reverted; it is the right fix
once the loop body's prelude-call callee resolves.

**The suite's count includes order-dependent failures, measured.** A bisect of
`helper_refuses_a_parallel_refusal::a_compute_parallel_refusal_fails_the_helper` converged
on a **docs-only** commit, which is impossible as a behavioural cause — and the test then
passed **20 of 20** runs in isolation at HEAD while it fails inside the full suite (also
single-threaded, 117/136 both ways). So the failure is *process-global state carried
across tests* (the candidate carriers: the thread-local `GRAPH_POLICY`, the process kernel
registry), not a per-test fact, and the bisect over it is invalid. The suite's 136 is
therefore an upper bound on independent failures.

**The seed rule is the superior's: a node with a type slot is not seeded.** Following
that ruling, `prepare_template_low_types` no longer seeds the pair (its type slot states
the shape); only the pair's **value** — a node without a type slot — takes the seed.
Measured neutral (compute 117, lowlevel 164, other suites green). The `graph_jit`
class-shape panic that motivated a refinement relaxation is **not** from that seed: it
comes from a value seed whose stated type has an **open** position that an observation
later decides. The relaxation was measured **+1 (117 → 118)** with every suite green and the
panic gone, but the superior's verdict recorded at the head of this note says why it is not
the fix: a seed and an observation never coexist on one node.

**The approved Invariant change is refuted by measurement.** With the superior's approval
of both changes proposed here, they were implemented and measured: removing the unroll
fallback for a refusing marked recursion takes compute **117 → 114** and fails
`host_loop::an_unconvertible_marked_recursion_still_expands` — that test *is* the
fallback's contract — and adding the parameter-end path walk gives 114 with the same
lowlevel failure. All three variants (unroll-only, walk-only, both) are negative, so both
were reverted and the fallback stays: it is what lets those programs run at all, and the
next work is the **loop path's runtime** for the plan/tiled shapes, not the fallback.

**The opaque is hit outside the nest, and the record keeps the unroll's result.**
`opaque` prints `loops []` for node 3712 — the empty clone is lowered while emitting the
**write call**, outside any nest, and the record's `.value` item is the step's **Add apply**
(not the fold call's apply): the check-time **unroll** substituted the call's result into
the record. The two coordinated changes that would fix this chain are recorded for the
next round: (a) `function_apply` returns `None` (lazy) for a marked function whose
conversion refuses instead of unrolling it, so the record keeps the call, and (b)
`parameter_value_path` follows the chain to the parameter and pops the pair peel so the
emitter's own conversion succeeds. Both touch Invariants (the unroll fallback and the
refusal verdict) and need the superior's ruling before they are made.

**The record's `.value` slot already holds the return expression.** Followed
structurally: the write's operand is `Index(Index(record, 2), 0)`; the record's value array
is `[1493, 525, 1583]` and item 2 (`1583`) is the step's **Add**, not a pair — so
`check_record`'s `state[field].term` for the `.value` field was already the call's *value*
(its return expression), not the apply's pair. A resolver that follows
`Index(Index(array, k), 0)` to the array's item and looks it up in `applied_pairs` fires
(the chain is found) but finds no applied pair there, so it changes nothing (117) and was
reverted. The next fix is at the **call's term**: `check_app` must leave the call's pair as
the field's term so the record keeps the call.

**The write's value loses the call at the record, measured.** The write's third operand is
`Index(3225, 0)`; `3225` is the write's field pair, its class's defining member is the
**step's Add** (the fold body), and `3225` is **not** in `applied_pairs` — so no path in
the emitter reaches the call. The cause is upstream of the emitter: the record's element
slot was filled with the argument's **value** (`pair_value_half` of the call's pair = the
function's return expression), so the call expression itself is not in the record and
cannot be recovered. An `applied_call` interception in the emitter (a value read of an
applied pair lowers the call) is written and measured neutral (117) — the pair is never an
applied pair here — and was reverted. The fix must keep the call's **pair** in the record's
element slot, which is the encoding question this arc is about.

**The loop conversion's refusal still masks the opaque.** Re-tested the alias retry on
the 117 tree: 114 (−3, the same `plan_staging`/`tiled_reduction` three). With the
conversion succeeding, `tiled_reduction`'s kernel compilation reaches the **opaque**
(`a kernel body reached a node with neither a value nor an operation`) — so those three
were passing only because the conversion refused, and fixing the conversion alone exposes
the opaque. `defining_member` now prefers a class's **Apply** member (a call defines the
value it produces; its expanded body is the implementation), measured neutral (117) — the
write's value class holds no Apply for the pack program.

**Nested tuple domains work (+2).** Two changes make `tuple{tuple{Int, Int}, Int}`
compile and run: the transfer's `Index` now states the **element slot pair** for a
compound *value*'s read (the policy `docs/notes/lowlevel-low-types.md` already states —
"A compound's elements are complete slots … `Index(compound, k)`'s transfer states
`Tuple([element, Unknown])`" — and the code indexed the slot as if it were the tuple), and
`low_type_of_slot` decodes a **type pair** by the kind marker its type position holds.
compute went **115 → 117** (136 failed), and the language 275 / highlevel 136 / lowlevel
164 suites stay green.

**The array read's typing is fixed for structured containers.** `check_index` now types
its result by the **container's kind**: a tuple or struct element states its own type cell
(`slot_read`'s `element_ty`), an array keeps the array element type. Measured: no test
flips (compute 115), the embedded prelude still checks, and language 275 / highlevel 136
stay green. The nested domain is still refused: with the type-pair decode added as well,
the class-shape assertion still fires on the pair wrapping the **outer** read — so the
offending type comes from the *tuple read* path (`structs.rs`, which already uses
`slot_read`'s `ty_node`) or from the class seed on the container value, not from
`check_index`.

**The nested conflict is a checker typing, and the obvious repair breaks the prelude.**
`prepare_template_low_types` seeds a pair's *value* with its type slot's shape, and the
pair wrapping `p(0)(0)` (node 88, value a scalar read) states `tuple{Int, Int}` — the
container's type, not the element's. `check_index` types every element read with
`elem_cell` (the **array** element type path
`[TYPE_SHAPE_SLOT, ARRAY_TYPE_ELEMENT_SLOT, PAIR_TYPE_SLOT]`) and throws away
`slot_read`'s own `element_ty` (`let _ = element_ty`); for a tuple/struct container the
type must be the element **slot's** own type cell. Swapping to `element_ty` makes the
embedded prelude fail to check ("this value is not a container — it has no element to
read"), so the repair must keep the type-cell identity: choose the element slot's type
cell for tuple/struct containers and the array element type only for arrays.

**A nested tuple domain decodes, then conflicts with the class channel.** With the
domain reader fixed, the simple `tuple{Int, Int}` jit compiles; the nested program
(`tuple{tuple{Int, Int}, Int}`) still refused. A discriminator that recognises a **type
pair** by the kind marker in its type position (so `low_type_of_slot` decodes nested type
pairs too) makes the nested program *decode* — and both remaining jit tests then **panic**
at `equality.rs`'s class assertion: `a class states one shape`, left
`Tuple([Some(USize), Some(USize)])`, right `USize`. So the nested element's class is seeded
as the tuple while a leaf read observes a scalar: the domain reader and the class channel
disagree about a nested domain, which is the next fix there. The discriminator change was
reverted (it turned a refusal into a panic).

**The alias retry fixes the conversion and flips nothing.** `562` (`Index(parameter, 0)`)
is joined to the state's value cell `548` by `alias_read` when that read is *evaluated*
(`evaluate_node_operation` → `alias_read` → `unify`), which is why later runs convert. A
retry in `function_apply` that joins the peel to the value before re-converting makes the
apply-time conversion succeed (3 of 5 runs flip to `tuple: true`), and **no test flips:
compute stays 115, and the same three plan/tiled tests regress (112)**. So the conversion's
verdict is not what those tests need; with the loop path taken, their runtime answers
differ. Hypothesis worth testing next: the failed apply-time conversion forces the
**unroll** path, and that unroll is what inlines the marked body into the caller (the
write's value reaching the `if`, round 18) — measured only as "the retry changes nothing",
not yet as "the inlining stops".

**The loop conversion's false refusal is measured, and three repairs were tried.** The
emitter's first `loop_conversion` runs before the parameter's value cell is unified with
the pair peel `Index(parameter, 0)`: the walk follows the chain to the parameter, finds no
read, and `state()` refuses a conversion whose later runs (3 of 5) succeed. Three repairs
were measured against the 115 baseline, and **all three regress the same three tests**
(`plan_staging::a_declared_plan_stages_the_reduction_and_answers_the_closed_form`,
`plan_staging::a_plan_the_fragment_refutes_is_refused_by_name`,
`tiled_reduction::a_tiled_reduction_answers_the_closed_form_and_the_per_lane_loop`):
following the chain to the parameter and popping the pair peel (correct classification,
112), accepting the positional state when `arity > 1` (112), and both together. Those
conversions must stay **refused**, so the fix is not the verdict but the unification: the
state's value cell must be unified with the pair peel *before* the emitter's first
conversion. The next step is to find which later evaluation performs that unification.

**The parameter's type slot is a type pair, and the domain reader fixed 16.** A type
expression is encoded as a **term pair** whose value position is its shape and whose type
position is its kind (`TypeValue(TypeTuple)` …), so `kernel_parameter_domain` decoded the
value position alone and answered `None` for every annotated parameter — the jit family's
"parameter's class is not decided". Decoding the *slot itself* first
(`low_type_of(...).or_else(|| low_type_of_slot(...))`) fixes it: compute went **99 → 115
passed** (154 → 138 failed), the other suites unchanged.

**The failure groups, measured** (`--nocapture`, first diagnostic per failing test):

| first diagnostic | tests |
|---|---|
| a kernel body reached a node with neither a value nor an operation | 124 |
| compute.graph: this function's return is not a value a graph can hand back | 97 |
| a `@loop`-marked recursion … is not convertible (`StateShape`) | 83 |
| compute.jit: the kernel parameter's class is not decided | 22 |

so the **opaque** family is the dominant one, and it is the chain this note has been
peeling. A clone of the marked function's node is now recognised by the nest
(`loop_owner`/`loop_value` follow the origin), and the minimal `@loop` kernel's remaining
opaque is the **entering tuple's element**: the state unification merges the initial
tuple's element class with the step's read, so the element's defining node is the read
(`s(1)`'s clone) and the emitter asks it for a value outside the nest.

**The entering state's elements are peeled, and the wildcard is the blocker.**
`loop_state` now takes each element slot's value (the third reader of the same pattern).
The minimal `@loop` kernel's refusal traces to the label-annotated step element: its
clone's operand is the annotation's **wildcard** (`Any`), which the clone walk mints as an
empty cell, so the arithmetic lowering asks that empty node for a value and the emitter
refuses it as opaque. The fix is the emitter's annotation handling — a label-annotated
value must be emitted as its underlying value, not as an application of the label — which
is the next step.

**The empty clone is a leak across functions, measured.** Followed with a minimal
`@loop` kernel: the kernel's body walks `VALUE 3712` (an empty clone, owner the kernel,
origin `665`) into `VALUE 665`, a node of the **marked function's** template, and then
lowers its `Index(562, 1)` (`s(1)`) with no nest active — the index placement refuses.
So the checker's undecided evaluation leaves a kernel-body clone whose origin is another
function's node, and the template walk follows it. Emitting such a clone from its origin
was wrong (it re-emits the marked function's body unbound); the emitter now honours the
origin only inside the body's own function and refuses a foreign one by name. The real
fix is upstream: the checker's evaluation must not leave that clone in the kernel's body.

**The emitter's empty clone is fixed, and the next blocker is measured.** A kernel body
holds clone nodes the checker's own undecided apply left behind (owner the body function,
origin a template node, no value and no operation); the emitter refused them as opaque.
It now emits an empty clone from its template. `parameter_path` also refuses a pair's type
(`1`) and attribute (`2+`) positions as path steps. No test flips; the minimal `@loop`
kernel now stops at "a kernel body's index cannot be placed": a read of `Index(<a
computation>, 1)` whose chain ends at the marked function's parameter pair — the type
position of a pair read as a value. The next step is the caller that asks for a type
node's value.

**The path walk is rewritten, and the blockers moved.** `parameter_value_path` now reads
the chain from the value side (`state_path`): element positions alternate with the slot's
value position, so a pair peel and every slot peel are dropped and a pair's type or
attribute position ends the match. `check_reads` also distinguishes a **use** (an operand)
from a value's own **items** (the encoding), so a record's value cell is no longer refused
as a whole-state read. Measured: no test flips, all suites green; the attribute read that
used to refuse (slot four) is gone, and the refusal site moved to `state()`'s "a
tuple-valued state never read by element" for a conversion whose element reads are not
classified — the next thing to identify (it is a different recursion than `fold`'s).

**The refusing read's chain, measured.** For `pack_placement`'s reduction, the node the
conversion refuses reads `Index(Index(Index(parameter, 0), 0), 4)`:
`Index(parameter, 0)` is the parameter **pair's** value, the next `0` is the state
**tuple's element 0** (the element *slot*), and the `4` is that slot's **attribute 4** —
the pair's attribute tail, not a state position. `parameter_value_path` treats the last
Index on the pair as the peel and returns `[4]`, so an attribute read is validated as
state element 4 against arity 2. Guarding on the value's layout does not work: at
conversion time the values are undecided (`None`), so the walk needs the slot/attribute
split from the **type** side, not the value side. That is the next fix.

**One conversion mismatch is fixed, one measured.** `parameter_value_path`'s two
"chain ends at the parameter" branches now strip the pair's value position the same way
(`loop_conversion.rs`), so a state read no longer surfaces as a spurious `[0, …]` prefix
and stops being refused by the wildcard arm. No test flips on that alone. The remaining
refusal is a read whose chain is `Index(Index(pair, 0), 4)` against a state of arity 2 —
a five-slot structure read as state position four — so the next question is which
function's parameter that read belongs to (the conversion's roots include nodes of the
kernel the recursion sits in).

**The loop conversion is the other half of the fuse/pack family.** `pack_placement` and
`fuse_licence` programs first fail with `compute.parallel: a @loop-marked recursion this
kernel body applies is not convertible ... the shape is a non-scalar carried state`
(`docs/notes/loop-conversion.md` §4), and the graph refusal is the *second* diagnostic.
Probing `LoopRefusal::StateShape`'s sites (`DBG_LOOP` line tags) names `check_reads`
(`crates/lichen-lowlevel/src/loop_conversion.rs`): a whole-parameter read meets
`tuple || arity != 1`, and an element read `s(k)` meets `k >= arity` — so the state's
arity and its addressing disagree for a reduction chain whose step carries
`(i + 1, 0.0 ~ "identity")`. That conversion runs from the fragment compiler, so it is a
kernel-side reader of the same slot encoding.

**The graph block's root is narrowed, not fixed.** Probes on a `compute.graph` body with
two `plrun`s: the body's apply decides to a real `Buf`, `returned_value_ids` misses it,
and **no refusal is recorded** — and `ComputeOperator::ParLaunch`'s `run` arm never
executes (a print there never fires), neither inside nor outside the recording window.
So the launches are executed (or already decided) without the operator path the recorder
intercepts: the two suspects are the checker's forced evaluation deciding the body's
nodes before the graph build, and the fuse pass rewriting the launches. The leaf-fix
patch below is unrelated to this family (the graph tests fail with and without it).
That family is 9 `graph_jit` + 10 `fuse_licence` + the fused-region tests, so it is the
next keystone.

**The cfg leaf walk is the next compute seam** (measured, not landed). `value_at_path`
(job of the role walk in `compute.rs`) takes the element node where the value is: since
the compound encoding, `items_of` over a compound returns **slots**, so a scalar leaf
reads a slot pair instead of its value. Peeling a slot (a `layout != 0` pair's first
item) turns `a_body_reads_a_scalar_leaf_of_its_input_struct` green and lets the
cross-kernel programs read `(1, 2, 8, …)`, but two cross-kernel tests then **overflow
their test thread's stack** (`a_cross_kernel_call_agrees_across_the_two_backends`,
`a_body_may_compute_in_one_class_and_cross`) while a standalone probe of the same program
runs fine even on a 2 MiB thread — so the peel exposes a recursion the recording reaches
only once the launch's extent and leaves are correct. The patch is kept at
`.scratch-core/leaf-fix.patch`; the next step is that recursion, not another reader.

The isomorphism is not complete yet: with the record path disabled the *write* case stays
red (the certificate decides nothing — the field read's attribute provider inside the
materialized clone is still a static node), so the field reference still rides the record
while the whole-parameter reference rides the materialization. Closing it means making
that provider decide under the clone — then the record can go.

## 2. Nothing else

Every other suite is green; the three counts above are the reading. Two smaller defects are
documented rather than left open:

- The apply-time diagnostic renders the compared *types*, so a value-level conflict recorded
  through the apply edge prints both sides alike ("expected Int, found Int"). Left as a
  documented limitation: no program reaches it today — the one that did was the stale-value
  conflict the mint fix closed
  ([shallow-depth-element-values](shallow-depth-element-values.md) §22) — and the smaller
  fix (fall back to the compared values when the two sides read alike) buys little.
- The apply's type-side value merge is load-bearing for refinement reporting
  ([apply-type-side-check](apply-type-side-check.md)).
