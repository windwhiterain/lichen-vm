# One channel for a class's value and its low type

> Status: **§2 refuted by measurement**, and **§3 has no receiver on the run side**
> (§3 below, two independent blockers).  What is left is §5, which therefore comes
> first: it is the only place a result's type can be stated, and the two red
> targets the operator-polymorphism branch recorded
> (`operator-polymorphism.md` §8.4) are its acceptance.  §4 then lands on top of
> it.  §1 is the incoherence as first read; §2 and §3 record what measurement says
> that first reading got wrong.
> Worktree `.worktrees/kernel-param-struct`, branch `feature/kernel-param-struct`.
> Companions: [compute-runtime-scalars](compute-runtime-scalars.md) (the measured
> case that exposed this — its §4.4 is the symptom, this note is the fix),
> [lowlevel-low-types](lowlevel-low-types.md) (the seed → pass → read chain),
> [defer-pending-type-forms](defer-pending-type-forms.md) (the deferral whose side
> effect is being relied on), [type-query-api-proposal](type-query-api-proposal.md)
> §7 (the same "decide it where it is decided" principle one layer up),
> [kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6 (the
> specialize-before-JIT direction §5 here belongs to).

## 1. The incoherence

A class's value and low type have **one authority in the lowlevel and a second
reading in the highlevel**, and the second one is what the checker and the
compute plugin ask.

The lowlevel is class-routed, and says so:

| Site | What it states |
|---|---|
| `Module::class_value` (`equality.rs:77-87`) | "the class's value, read **through its representative** — the value the unification machinery sees" |
| `Module::class_low_type` / `low_type_of_node` (`equality.rs:89-113`) | "read through its representative … so a read **never depends on which member of the class resolved first**" |
| `Module::write_node_value` (`equality.rs:258-297`) | "the **single choke-point** for value writes"; it replicates the value to every operation-free member and calls `observe_class_low_type` — one of the low-type layer's **two observation sites** (`Module::add_node` is the other) |
| `Module::seed_class_low_type` (`equality.rs:115-125`) | "a layer above the lowlevel **that has the type** calls this" — the write half of the same channel |

The highlevel reads the **node's own slot** for the same questions:

| Site | Reads |
|---|---|
| `shape::low_type_of_slot` (`shape.rs:1092-1111`) | the node's own value and structure; never `class_low_type`/`low_type_of_node` |
| `compute::node_class` (`compute.rs:4302`) | the node's own value, then `low_type_of_node` |
| `checker::names_float_class` (`operators.rs:137`) | rides on `low_type_of_slot` |

That split is what makes today's behaviour accidental rather than stated:

- **A run-decided class reaches the type graph only as a side effect of a
  check-time deferral.** `defer_pending` (`shape.rs:471-482`) fires only when one
  side is a *pending* `Index`/`Apply` and the other holds a type; on `Merge` the
  lowlevel commits the class's value (`equality.rs:554-558`, `pin_committed_value`
  at `:884-903`). Measured: a single-kernel `collect` of a `plrun` result prints
  `array<?a, ?b>` (nothing was ever committed), while the GPU chain test prints
  `array<Int, ?d>` — because the consumer's **array** argument supplied the pending
  read that triggered the commit. The same test with a **tuple or struct** argument
  prints `array<?d, ?e>`: no pending read, no commit
  ([compute-runtime-scalars](compute-runtime-scalars.md) §4.4).
- **Which node a reader asks decides what it sees.** `pin_committed_value` writes
  to the representative (and to the class's pending ops, for the apply cloner's
  sake), so a member that is neither reads nothing — an allocation-order accident,
  exactly what `class_low_type`'s doc says must not matter.

The point of the plan: a fact is decided once, the class is where it lives, and
every reader asks the class.  §2 is where that second clause is measured and
narrowed: a fact *about a value's class* does live on the class, and a fact about
a *type* does not.

## 2. Half one — refuted: a class's low type is not a second reading of a type slot

**What was proposed**: route `shape::low_type_of_slot`'s dynamic slots through
`Module::low_type_of_node`, keeping the node walk as the fallback, so that
"which member of the class a reader asks" stops mattering.

**What the measurement says** (branch at `3d55923`, every suite green before the
change):

- Routing the dynamic slot through the class channel broke **15 of the 58**
  `--test compute` cases, several as `(none, …)` values rather than as type
  differences.  The cause was on the *write* side of the chain, not the read
  side: `compile_fragment` (`compute.rs:2159`) and `seed_template_term_low_types`
  (`:2544`) *seed* a parameter/term slot from what `low_type_of_slot` answers, so
  a class answer there pins a slot to a shape nobody wrote and the pass then
  conflicts with it — `the kernel parameter's type is not decided when the kernel
  is compiled`.
- Restricting the class answer to the two scalar classes (`USize`, `Float`) left
  **6** failures, all of the same seed-side kind, so the breakage is not the
  compound answers alone.
- A trace of every call that reached the channel shows what it answers with:
  `Array(Unknown, 2)` **for a `[value, type]` term pair** (a pair is a two-element
  array), `Tuple([…])` for tuple values, `USize` for template cells whose class
  was unified with a runtime value.
- Narrowed further — the class read added at one *reader* only
  (`checker::names_float_class`, which asks a class question) — all suites are
  green **and the new branch never fired once** across the crate's test binaries.

**Why**: the low type vocabulary serves two subjects.  `low_type_of` decodes a
**type expression**; `Module::class_low_type` states the machine shape of the
**values** in a class.  The two coincide exactly for the scalar classes — there a
value's shape *is* its class — and nowhere else.  A type cell and a value node can
also share one class (an annotated parameter's type cell holds the annotation's
pair term; a frozen template's type cell resolves to a runtime *value* at the
apply), which is why a scalar answer from the channel is not evidence about a
slot's type either: in the trace the class was describing the *template's* value.

So the "second reading" in §1 is not a reading of the same fact, and deleting it
would delete a correct answer rather than a duplicate one.  `low_type_of_slot`
stays the type decode; `compute::node_class` was already class-routed
(`low_type_of_node`, `compute.rs:4320`) and is the one site that reads a *value's*
class — the right subject for it.

**What survives from the half**: the class channel is the only reading that sees
a class a *run* decided (a value's class), so a reader that asks a class question
can be routed to it — but only once §3 has made a run state one, and only as a
*class* (a scalar answer), never as a type.  That reader-side routing is therefore
part of §3's landing, not a step of its own.

## 3. Half two — the decider states the fact, and where that is not possible today

**What**: the layer that decided a class states it on the class, instead of
leaving it to a deferral's side effect.  Two measurements say the statement has no
receiver on the *run* side, and that a low type would not be enough if it had one.

**(i) The run cannot name the type cell.**  `NativeOp` has exactly one method,
`build` (`native.rs:55-65`), and the run half is
`OperatorExt::run(operand, _block, module)` (`program.rs:830-835`) — an arm
receives the *evaluated* operand array and no node id for its own expression.
`ParLaunchOp::build` creates `out_ty` (`compute.rs:8676`) and references it only
from the pair `[op, out_ty]`; nothing at run time can name that cell, so
"`seed_class_low_type(node, …)` at the `ParLaunch` arm" has no `node` to receive
it.  The one node the arm *does* create for a several-output result
(`compute.rs:1467`) is the buffer **value**, and the observation sites derive
nothing from it (`observed_low_shape`, `equality.rs:1190`: a `Buffer` is not a
`LowValue`).

**(ii) A low-type seed cannot change the printed type.**  The failing reader here
is the printer, and `type_printer::node` (`type_printer.rs:62-79`) renders a cell
that holds no value as its **class name** (`class_name` keys on
`representative`) and never consults low types.  `array<?a, ?b>` →
`array<Int, ?b>` therefore requires the element cell's **class to hold the `Int`
type value** — a `write_node_value`, not a seed.  This is §2's lesson applied to
§3: the reader asks for a value.

**What today's `array<Int, ?d>` actually is** (measured on a "cpu" probe pair,
scratch files, deleted):

| spelling | printed |
|---|---|
| a two-kernel chain with the reads **and** the collect in one tuple | `(20, 22, 24, [20, 22, 24]): <?a, ?b, ?c, array<Int, ?d>>` |
| the same chain, `collect` alone | `[10, 11, 12]: array<?a, ?b>` |

The `Int` exists only where a *consumer's array literal* is present, and the
printer renders it at a node created during that consumer's check — the array
literal's homogeneity puts the buffer's type cell and the ordinal's `Int` type
cell in one class, and the deferral's `pin_committed_value` then replicates that
committed type value into the element cell.  Read strictly, the `Int` printed
there may be the **ordinal's** type rather than the buffer element's, which is
exactly "accidental rather than stated".  So the struct-argument migration does
not lose an answer when it prints `array<?d, ?e>`; it removes a coincidence, and
the decided answer it needs is the one §5 states.

**Where a statement is possible today**: only where types are stated — a
`build` with concrete argument types and a `ctx`, i.e. **after** the signature is
concrete.  That is §5, and it is why §5 now comes first.

**The remaining specification, for whoever lands it** (either as §5 or, if the
extension-private operand route is taken, in `ParLaunchOp::build`):

- the fact for a class reader is `seed_class_low_type(node, low_shape_of(class))`;
  the fact for the printer is the **type value** for that class written through
  `Module::write_node_value` (`equality.rs:273`), into the element cell of the
  result's type rather than into the result's type cell (its class holds
  `[element, BufferKind]`, so writing the element type there would conflict);
- a canonical type value is needed at that point (`Int`/`Float`), and one is
  reachable from `ctx` (`ctx.int_type()`) but **not** from a `Module` at run time:
  the compute extension never touches `HighGlobal`, and the universe
  `K = [Type, ↺]` is self-referential so `alloc_array` cannot build one;
- `kernel_results_value` (`compute.rs:6200`) needs none of this for the scalar
  form: `add_node` already observes `USize`/`Float` from the scalar value
  (`observed_low_shape`), which is why a scalar result's class is stated today.

## 4. The carrier: the struct-argument migration

The migration that exposed all of this, and the reason to do §5 now: replacing
the raw array arguments of `compute.read`/`compute.write` with struct instances
(approved direction, explicit constructors — and the construction site is worth
trying with `_` in place of the explicit `(Read _)`, since each site would then
get its own inferred type rather than sharing one lambda application).

**The recipe, verified** ([compute-runtime-scalars](compute-runtime-scalars.md)
§4.3): the types must be **lambdas**, because a type *value* has one occurrence
whose `_` cells the first instantiation specializes —

```lichen
Read  = _x => struct<.from _, .at _>
Write = _x => struct<.to _, .at _, .value _>
read  = (x : Read _)  => $read(x.from, x.at)
write = (x : Write _) => $write(x.to, x.at, x.value)
```

with call sites `compute.read ((compute.Read _)(.from buf, .at i))` and
`compute.write ((compute.Write _)(.to n, .at i, .value v))`.  Written that way the
213 scripted call sites give **57 of 58** `--test compute` green, the frozen-module
panic gone and the `raw[…]` field-type leak gone (fields render concretely).

**Order**: §5 → this.  The one remaining failure after the migration is the
element cell that §3 could not state (§3 above) and that §5 states, so the
migration's 57 becomes 58 with §5, not before it — and until then the honest
answer for the struct spelling is `array<?d, ?e>`, which is *more* correct than
the array spelling's accidental `Int`.

**Not to forget**: the migration touches `crates/lichen-language/tests/*`,
`crates/lichen-language/examples/*`, and the docs' code blocks; the script must
handle nested occurrences innermost-first (`compute.write [a, b, compute.read [c, d]]`).

## 5. The structural removal, and why it is now the first step

`ParLaunchOp::build` (`compute.rs:8623`, `out_ty` at `:8676`) leaves the result
type a **fresh cell**, and its own doc says why:

> "The signature's *arity* is what decides the result's shape — a bare `Buffer`
> for a one-write index function, a tuple of buffers for a several-write one — and
> the arity cannot be read here: `build` runs once, on the frozen `plrun` template,
> where `.sig` is an unbound cell that only resolves at run time."

§3 established that the run cannot repair that cell afterwards, so the cell has to
stop being late: **the signature must be concrete before `build` runs**.  Then the
result type is stated where types are stated, with a `ctx` in hand (so the
canonical `Int`/`Float` type value is reachable, which it is not from a `Module`),
and the fresh cell, the deferral side-effect dependency, and any run-time
statement all become unnecessary rather than merely fixed.  That is the
specialize-before-JIT direction
([kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6,
[compute-param-struct-handoff](compute-param-struct-handoff.md) §7,
[operator-polymorphism](operator-polymorphism.md) §8.4 — which records this as
another workstream and names the two red targets), and it also carries the class
domain that workstream put in the graph as a *value*: that domain is what types
the placeholder the specialization applies the kernel to.

## 6. How to verify, at each step

```bash
cargo check --workspace
cargo test -q -p lichen-highlevel -p lichen-compute
cargo test -q -p lichen-language --test compute --test pipeline --test graph_jit \
  --test graph_structure --test examples --test defer_pending
```

§5's own acceptance is the two targets
[operator-polymorphism](operator-polymorphism.md) §8.4 recorded as red:

- `a_kernel_value_and_type_render_by_name` (`crates/lichen-language/tests/compute.rs:466`)
  — `.sig ?c -> ?c` must become `.sig Int -> Int`;
- `an_imported_package_that_jits_at_its_top_level_still_runs`
  (`crates/lichen-language/tests/runtime_only_package.rs:41`) — the `launch` gate
  must resolve the domain of an open `.sig`.

The probes that pin the rest (scratch files, not committed):

- **§5's own probe** (the decided element cell, stated where types are stated): a
  single-kernel `plrun` + `collect` must print `array<Int, ?b>`; today it prints
  `array<?a, ?b>` on the array API and on every other spelling.
- **§4's assertion**: `a_gpu_program_chains_two_kernels_on_a_device`
  (`crates/lichen-language/tests/compute.rs:1308-1345`) must keep its
  `array<Int, ?d>` with the struct spelling.
