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
