# Runtime scalars in a parallel kernel: the per-leaf ABI and its two blockers

> Status: **the CPU path is end to end.**  Both halves landed — a parallel
> fragment's scalar leaves are typed by the parameter's own fields
> (`scalar_leaf_classes`), and the launch decodes them out of `cfg` and hands them
> to each worker beside the index (§3), so a runtime `Float` scalar reaches the
> body's own argument.  Measured on §6's probe: values from the scalar, and a
> wrongly-classed leaf refused by name.  Two things are **not** written, and both
> are refused rather than mis-run: the device path pushes the extent alone
> (`RunError::ScalarsNotPushed`), and a recorded body carries the extent alone.  §5's
> blocker still applies to the *explicit* `parallel_sig … Sig` spelling; the
> automatic `parallel` path needs no authored signature.
> Companion: [compute-param-struct-handoff](compute-param-struct-handoff.md)
> (the named-struct parameter this exists for),
> [type-query-api-proposal](type-query-api-proposal.md) §7 (the accessors the
> leaf decode uses), [floating-point](floating-point.md) §4.4 (the class model),
> [operator-polymorphism](operator-polymorphism.md) §4 (the array desugar §4
> below needs).  Worktree `.worktrees/kernel-param-struct`.

## 1. What a runtime scalar is, and why it is the point

Moving a parallel kernel's parameter from the positional `cfg = (n, (buffers…))`
to a named struct was for two things: a read that names its buffer, and **runtime
scalars beside the count** — `k.alpha`, a number the host fixes at launch
(`compute-param-struct-handoff` §1).  Everything before this note delivers the
first; the second is what the leaf classes are for.

The ABI's shape, from the launch side: the parameter's **scalar leaves in field
order**, then the **index**, and the input buffers are bound rather than passed.
The first scalar is the launch extent (`cfg(0)`'s role), which is why the host
reads it as an ordinal.

## 2. What landed: each scalar leaf's class is its parameter field's

`compile_parallel_fragment` used to seed *every* leaf `LowShape::USize`, so a
parameter leaf read `Int` whatever it was annotated with.  `scalar_leaf_classes`
now reads each leaf's class from the field the role names
(`parallel_roles`'s `scalars`, in signature order) through the field accessors of
`type-query-api-proposal` §7, and seeds the `cfg` slot with those shapes
(`param_shape` follows: the leaf list, then the index).

Two leaves stay `Int` whatever the parameter says, and the reason is the same for
both: they are not data.  The **index** is a lane number, and the **first
scalar** is the launch extent — a `Float` one is refused **by name** here, where
the message can name the field, rather than left to surface as an apply-time
class conflict with no span.

Measured, on the probe of §6: the role table decodes
`scalars: [[0], [1]], inputs: [[2, 0]], outputs: [[3, 0]]` and
`scalar_leaf_classes` answers `[Int, Float]`.  The `(n, (buffers…))` parameter
shape is untouched (one leaf, `Int`), which is why the existing suites stay green
— `lichen-compute` 17, `lichen-language --test compute` 58, `--test pipeline`
134, `--test examples` 1.

## 3. The host half, landed

| Step | Where | State |
|---|---|---|
| Decode the leaf list out of `cfg`, then the input tuple | `ComputeOperator::ParLaunch`'s arm | **landed** — the leaves are the cfg's leading positions, the buffers follow them, and how many leaves there are is the fragment's (`parallel_leaf_classes`) |
| Carry the leaf words and their classes on the state | `ParallelState` (`leaves`, `leaf_classes`) | **landed** |
| Pass them beside the index as the `main` arguments | `run_parallel_range` | **landed** — the argument list is the leaves in field order with the index appended, and only the index moves per element |
| Assemble the fragment's signature | `compile_parallel_fragment`'s `param_shape` | **landed with §2** — the assembler already derives `main`'s parameters from it, which is why the old host's two arguments were the wrong count |
| Push the scalar leaves to the shader | `lichen-compute-gpu/src/dispatch.rs`'s `stage_run` | **refused by name** (`RunError::ScalarsNotPushed`): a dispatch pushes the extent alone, so a fragment with a runtime scalar is declined rather than dispatched with a leaf missing |
| Carry a runtime scalar in a **recorded** body | `record_launch` | **refused by name**: a recording carries the extent alone, and a runtime scalar would have to be one of its edges |

The decode reads each leaf at the class **its own field** declares, so a runtime
`Float` scalar is an `f32` argument beside `i64` ordinals.  A decided leaf of the
wrong class is refused by name — the count's own refusal generalised, and the
reason is the one this channel exists for: staying lazy would mean the dispatch
quietly does not run and nothing says so.

The decode is **positional** and that is the ABI's rule, not a convenience: the
host struct is the parameter's scalars in field order, then `.I`.  A parameter
that interleaves scalars with `.in` would need a role table on the host side,
which the fragment does not carry — so the lowering should refuse an interleaved
parameter by name rather than mis-decode it (not written either).

The measurement that fixes this half's shape: with §2 landed and the *shipped*
signature lambdas, the probe now reaches the assembler and fails there —
`compute.parallel: encountered an incorrect number of parameters` — because the
fragment's `main` is `(extent, alpha, index)` while the host still passes
`(extent, index)`.

## 4. Blocker A, and what the struct-argument API has to do with it

A `compute.write [...]` argument is an **array literal**, and lichen's array
literals are homogeneous: every element unifies into one element-type cell
(`check_array_term`).  With the *positional* parameter shape the count and index
cells are undecided, so a `Float` value simply drags the whole cell to `Float`
and the program checks — that is how
`a_varying_float_element_is_seeded_from_the_index` works today.

An **annotated** parameter decides those cells, and then the same program is
refused.  Measured (before §4.2 landed), plain lichen, `Par =
struct<.n Int, .alpha Float, .in In, .out Out>` — all three rows are the **array
spelling**, which §4.3's migration has since removed from the tree; they are kept
as the measurement that made the migration necessary:

| Body | Result |
|---|---|
| `compute.write [k.out.z, 0, int2float v + k.alpha]` | `expected Int, found Float` at the array element (a located `ArrayElement` diagnostic) |
| `compute.write [k.out.z, i, int2float v + k.alpha]` with `i = compute.range k.n` | the same refusal, surfacing **span-less** through the write wrapper's operand destructuring |
| `compute.write [k.out.z, i, float2int (int2float v + k.alpha)]` | checks — the value is `Int` again, so the array is homogeneous |

So a struct-parameter (annotated) kernel **cannot write a `Float` element** while
the argument is an array.  Two layers, and they are different fixes:

- **4.1 the argument container.**  The elements of one array literal share a cell,
  so the index (`Int`) and the value (`Float`) cannot sit in one.  A container
  whose elements have *independent* types — a tuple, or a struct instance — has no
  such cell.  This is the layer the struct API removes, and it is what
  `operator-polymorphism` §4's desugar removal would remove for the array form
  itself.
- **4.2 the ordinals' class — landed.**  `ReadOp`/`WriteOp` unified the index
  (and, for a write, the length) with the **element's** cell, so an ordinal was
  "the class the data is".  The ABI never said that: `assemble_module` declares
  `(i64, i64) -> element` and `(i64, i64, element)`, with the position and the
  index `i64` **in every class** and only the element following the class
  (`run_parallel_range` declares the same closures).  They are now unified with
  the language's `Int`, which is what a length and a lane number are.  The
  undecided positional shape is unchanged (all the suites are green); what
  changed is that an `Int` count no longer drags the written value's cell with
  it, so what remains is a class question about the *container*.

### 4.3 The struct-argument API: the recipe, verified

Approved direction: `compute.read`/`compute.write` take a **struct instance** built
with an explicit constructor, so the elements are named fields with independent
types.

**Landed for it**: a named field read on a **concrete** container folds to its
constant position (`Checker::check_named_field`, `f51e4eb`) — the other half of
`slot_read`'s decided type read, and the property a lowering needs to walk a
struct argument's fields as positions (`Index(value, k)`).  An unbound container
keeps the lazy `TableGet`, which is the case the two-pass `param_path` exists for.

**The types must be lambdas, not type values** (superior's diagnosis, measured).
`Read = struct<.from _, .at _>` is a *value*: one occurrence, one pair of `_`
cells, and the first instantiation specializes them — the next one fails, and the
frozen-module path panicked outright (`invalid SlotMap key` through
`record_unify` → `tag_descent` → `slot0_is_shape`).  `KT = _x => struct<.I _, .O _>`
is a *function*, so every `(KT _)` clones its inner `_`.  Written that way,
everything works:

```lichen
Read  = _x => struct<.from _, .at _>
Write = _x => struct<.to _, .at _, .value _>
read  = (x : Read _)  => $read(x.from, x.at)
write = (x : Write _) => $write(x.to, x.at, x.value)
```

with the call sites spelled `compute.read ((compute.Read _)(.from buf, .at i))` and
`compute.write ((compute.Write _)(.to n, .at i, .value v))`.  Measured on the 213
scripted call sites: **57 of 58** `--test compute` green, the panic gone, and the
`_`-pair leak gone — three applications in one program render
`struct<.from Int, .at Int>`, `struct<.from Float, .at Int>`,
`struct<.to Int, .at Int, .value Float>`, i.e. fresh cells per application and
**concrete** field types.

**Landed.**  The wrappers are that recipe and every call site in the tree is the
struct spelling — 247 sites across tests, examples and the documentation's code
blocks, plus one parenthesized site (`compute.read (compute.plrun …, 0)`) the
bracket-shaped script could not see.  Measured after it: `--test pipeline` 134,
`--test examples`, `--test graph_jit`, `--test graph_structure`,
`--test defer_pending`, `--test compute`'s 58 and both library suites are green
except the two reds §4.4 and [class-channel](class-channel.md) §5.3 own — and one
long-standing oddity is gone with the array: the write's fields have *independent*
type cells, so `.at Int` no longer shares a cell with `.value Float`.

**The `_` spelling is refuted** (superior's suggestion, measured): writing the
argument as `_(.from buf, .at i)` instead of `(compute.Read _)(…)` is refused with
`named arguments require a statically known struct type` — a named-argument
construction needs the struct's name table resolved at check time, and an
instantiation with nothing to instantiate from has none.

### 4.4 What one test still loses, and why it is not the struct

`a_gpu_program_chains_two_kernels_on_a_device` is the single failure: the values
are identical (`(20, 22, 24, [20, 22, 24])`) but the collected array's element
type is `?d` where the test pins `Int`.  Measured, so that the next reader does
not re-derive it:

- **Not the struct argument.** The same test with the positional **tuple**
  spelling (`compute.read (out, 0)`, `compute.write (n, i, …)`) loses the `Int`
  the same way.
- **Not §4.2's ordinals.**  That landed and all suites but this one are green.
- **Not `slot_read`'s eager field type.**  Forcing the lazy `Index` back leaves
  the answer unchanged.
- **The array itself.**  The old argument was `[out, 0]` — a *homogeneous* array
  whose element cell was unified with the buffer's class; `collect`'s element cell
  shared that class, and the deferred commit of the runtime class landed there.
  A tuple's element types are independent and carry no such shared cell, so the
  path the commit rode on is gone.

So the loss is a **commit path**, not a class rule: a `plrun` result's class
currently reaches the type graph through the *read argument's array element cell*
rather than through the result's own type cell (`ParLaunchOp::build`'s `out_ty`).
Re-establishing it there is the honest fix and would make the three scalar reads
in that test decided too — they are `?a, ?b, ?c` in both spellings.  Until it
lands, the choice is between the array (precision, homogeneity) and the
struct/tuple (no homogeneity, one undecided element type).  The migration took
the struct — the precision it gives up was a *coincidence of the argument's shape*
([class-channel](class-channel.md) §1 measures it arriving only where a consumer's
array literal is present), and the three scalar reads in this very test are
`?a, ?b, ?c` in both spellings.

**The fix is planned, with its reasoning, in
[class-channel](class-channel.md)**: this is the symptom of one fact having a
class-routed authority in the lowlevel (`class_value`/`class_low_type`/
`write_node_value`/`seed_class_low_type`) and a node-slotted second reading in the
highlevel (`shape::low_type_of_slot`, `compute::node_class`).  That note's §2 (the
readers ask the class) and §3 (the decider writes through the choke-point) are the
two halves; §4 is this note's struct migration as the carrier, and §5 is the
structural change that removes the fresh cell the commit path exists for.

## 5. Blocker B: only the shipped lambdas can spell a JIT'd signature

`parallel_sig f "cpu" Sig` needs a `Sig` whose domain is the JIT'd input struct.
The shipped `compute.A`/`compute.P`/`compute.S` build it for the one-scalar
shape; a runtime scalar needs a second one, so the author must spell their own —
and that does not currently work.  Measured:

| Spelling | Result |
|---|---|
| `Par = compute.P (compute.KT _)(.I In, .O Out)`, `Sig = compute.S (compute.KT _)(.I In, .O Out)` | works (the shipped tests) |
| `Par = struct<.n Int, .alpha Float, .in In, .out Out>`, shipped `Sig` | **works** — the role table decodes and §2's classes are produced |
| `A2 = I => struct<.n Int, .alpha Float, .I I>`, `S2 = T: KT T => A2 T.I -> T.O`, `Sig = S2 (KT _)(.I In, .O Out)` | fails: `expected raw[<domain>, <codomain>], found TypeFunction` (one located diagnostic at the `Sig` line, two span-less ones from the `plrun` gate) |
| the same `Sig` as a *value* (no `parallel_sig`) | prints correctly, and with the same rendered type as the shipped form |

So the difference is not the arrow's shape or its rendered type — both spellings
render as `struct<…> -> struct<…>: TypeFunction` — but which **apply** produced
the value: the shipped one is a *static* (frozen-module) apply, the author's is a
*dynamic* one, and the wrapper's `.sig` field reads its type lazily.  Something
in that path decides a dynamic type value's cell where the static one leaves a
cell for the lazy read to bind.  That is a checker question, and it is the same
family as the decided-vs-lazy field type of `type-query-api-proposal` §7 — but it
is **not** diagnosed yet, and this note does not claim a cause.

**Narrowed by measurement**: this blocks the *explicit* `parallel_sig f backend
Sig` spelling only.  `compute.parallel f backend` derives the signature from the
function's own type (`type_of f`), so a `Par` with a runtime scalar launches end
to end through it — that is what §6's probe does.  What the author cannot do today
is *state* such a signature themselves; until (a) that path is fixed, or (b) the
runtime-scalar shape is shipped as a lambda in `compute.lichen` beside `A`/`P`/`S`,
`parallel_sig` is limited to the shapes the shipped lambdas build.  (b) is a
convention with a hardcoded scalar name; (a) is the honest fix and is unfinished.

## 6. The probe, and how to run it

Scratch file, not to be committed (the example harness runs every file in
`examples/`):

```lichen
---
  compute = import "compute.lichen"
---
g = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i + 10))
}
kg = compute.parallel g "cpu"
inbuf = compute.plrun kg (3,)
In  = struct<.a _>
Out = struct<.z _>
Par = struct<.n Int, .alpha Float, .in In, .out Out>
f = (k : Par) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.a, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value v + float2int k.alpha))
}
k = compute.parallel f "cpu"
out = compute.plrun k (3, 2.0, (inbuf,))
(compute.read ((compute.Read _)(.from out, .at 0)), compute.read ((compute.Read _)(.from out, .at 1)), compute.read ((compute.Read _)(.from out, .at 2)))
```

```bash
cargo run -q -p lichen-compiler -- <probe>.lichen
```

**Measured, and this is the acceptance**: the launch's `cfg` is the parameter's
scalars in field order and then the buffers — `(3, 2.0, (inbuf,))` — and the
answer is `(12, 13, 14): <?a, ?b, ?c>`: three indices (the extent reached
`k.n`), `inbuf = [10, 11, 12]` (the first kernel), and each element plus
`float2int k.alpha = 2` (the runtime scalar).  Before the host half it refused
with `compute.parallel: encountered an incorrect number of parameters` — the
fragment's `main` is `(extent, alpha, index)` while the host passed two.

The refusal path is measured too: `compute.plrun k (3, 2, (inbuf,))` — an `Int`
where the parameter declares `Float` — reports

```text
compute.parallel: a parallel parameter's scalar leaf is Float here and the launch
passes an Int for it: Int and Float do not convert
```

rather than running with the bits reinterpreted.
