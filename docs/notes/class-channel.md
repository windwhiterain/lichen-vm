# One channel for a class's value and its low type

> Status: **§2 refuted by measurement**, and **§3 has no receiver on the run side**
> (§3 below, two independent blockers).  What is left is §5, which therefore comes
> first: it is the only place a result's type can be stated, and the two red
> targets the operator-polymorphism branch recorded
> (`operator-polymorphism.md` §8.8) are its acceptance.  §4 then lands on top of
> it.  §1 is the incoherence as first read; §2 and §3 record what measurement says
> that first reading got wrong.
> Worktree `.worktrees/kernel-param-struct`, branch `feature/kernel-param-struct`.
> §5.1 (the class a lowering runs in) and §5.3 (the wrapper owning its signature
> arrow) are landed.  The remaining half — §5.2's statement and §5.4's carrier —
> is the **open class** itself, which is the operator-polymorphism workstream's to
> decide ([operator-polymorphism](operator-polymorphism.md) §8.4,
> [operator-polymorphism-handoff](operator-polymorphism-handoff.md) §5: the class
> domain is that workstream's value, and its reader is what commits to a member).
> This worktree therefore proceeds with §4, the struct-argument migration, whose
> 57-of-58 acceptance leaves exactly the element cell the two red targets want.
> **Then the operator routing landed** (§5.1.1): the operator *is* the prelude's
> binding, so a kernel body's `+` is an apply of a frozen function.  The kernel
> side of that — the static operands and the one class reader — is landed and
> measured (12 → 58 of 60 in `--test compute`), and **§5.1 is superseded by the
> requirement that a kernel function be explicitly specialized**: the class a
> lowering runs in is the author's statement (the parameter's type), never a
> domain the compiler reads or a default it invents.  What remains is the
> cross-kernel/`launch` argument shape §6 names.
> Companions: [compute-runtime-scalars](compute-runtime-scalars.md) (the measured
> case that exposed this — its §4.4 is the symptom, this note is the fix),
> [lowlevel-low-types](lowlevel-low-types.md) (the seed → pass → read chain),
> [defer-pending-type-forms](defer-pending-type-forms.md) (the deferral whose side
> effect is being relied on), [type-query-api-proposal](type-query-api-proposal.md)
> §7 (the same "decide it where it is decided" principle one layer up),
> [kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6 (the
> compiler-side "specialize before JIT" direction §5 here **supersedes**: the
> author specializes, and §6's placeholder apply is the rejected alternative).

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

**Order**: §5 → this.  **Landed**: the wrappers are the recipe above and all 247
call sites in the tree (tests, examples, the documentation's code blocks, and one
parenthesized site a bracket-shaped script could not see) use the struct spelling
([compute-runtime-scalars](compute-runtime-scalars.md) §4.3 records the measured
result and the refuted `_` spelling).  The one remaining failure after the
migration is the element cell that §3 could not state and that §5.2's statement
owns — so the migration's acceptance is 57 of 58, with the honest answer for the
struct spelling being `array<?d, ?e>`, which is *more* correct than the array
spelling's accidental `Int`.

**Not to forget**: the migration touches `crates/lichen-language/tests/*`,
`crates/lichen-language/examples/*`, and the docs' code blocks; the script must
handle nested occurrences innermost-first (`compute.write ((compute.Write _)(.to a, .at b, .value compute.read ((compute.Read _)(.from c, .at d))))`).

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
statement all become unnecessary rather than merely fixed.

**Superseded naming: "specialize before JIT" means the *author* specializes.**
The direction recorded at the time
([kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6) described a
*compiler-side* pass — "at `jit`/`parallel` time the function is applied to a
placeholder typed by the annotated domain".  That is **not** the requirement: the
requirement is that the **user states the class before `jit`** (the parameter's
type), and the lowering reads that statement and nothing else (§5.1).  No
placeholder apply, no call-time evaluation, and no reading of the class domain is
part of it; the compiler-side framing is kept only as the rejected alternative
§6 names, because it is where the residual question below was once expected to be
answered.

**Measured: the gap is exactly the open class.**  With the parameter annotated
there is nothing left for the lowering to decide —

```lichen
annotated = compute.jit ((y : Int) => y + y)
annotated
```

prints `(Kernel, parameterized): struct<.native raw[?a, ?b], .sig Int -> Int>` —
the target string, already, on today's tree.  The red target is the *unannotated*
body, where `+`'s class is open and the recorded choice is the class domain's
`default`.  So the slice is: **choose the class where the domain says to choose
it, and state it as the kernel's signature** — not a new lowering path.

### 5.1 Superseded: the class a lowering runs in is the **author's** statement

`open_class_of` (the first landing here) walked the function's own `asserts` for
the `InDomain(cell, set)` condition `check_binop` registered and took the set's
**first** member as the class to lower in.  **The routing removed the premise**,
and the decision that replaces it is the requirement itself: **a kernel function
must be explicitly specialized** — a kernel is lowered for one class and compiled
before any apply, so the parameter's type is where the language says which class
this artifact is for, and a body that left its class open is refused by name
([`UNDECIDED_DOMAIN`]: "the kernel parameter's class is not decided when the
kernel is compiled … annotate the parameter").  Measured: `compute.jit (y => y +
y)` is refused, `compute.jit (y : Int => y + y)` is `(Kernel, parameterized):
struct<.native raw[?a, ?b], .sig Int -> Int>` and launches `3 + 3 = 6`, and the
function itself stays polymorphic for its other uses (`f = y => y + y; k =
compute.jit f; f 1.5` is `3.0: Float` — the refusal does not touch the class).
`open_class_of` and `compile_fragment`'s `open_class` parameter are **deleted**;
the seed is the parameter's own type and nothing else.

### 5.1.1 The routing took the domain out of the caller — measured

**Landed in `feature/operator-std` (§6.1 of
[operator-polymorphism](operator-polymorphism.md)) and re-measured here**: the
surface operators now lower to the prelude's bindings, so a kernel body's `y + y`
*is* `Apply(Static(<core's add>), [y, y])`.  Two consequences for this pass, both
measured on this tree:

- **The domain is no longer in the caller's function.**  `open_class_of` reads
  `InDomain` out of `module.functions[fid].asserts`, and after the routing the
  jitted function's assert list is **empty** while the only `InDomain` in the
  program is inside frozen module `158` (the `core` prelude): the *callee's*
  refinement (`add = operands => { operands : array<(_ ! in_num), 2>; … }`) is
  what confines the class now, and it is enforced by the apply, in the callee's
  own module.  So `compute.jit (y => y + y)` is refused with `UNDECIDED_DOMAIN`
  again — the parallel path is unaffected only because the ABI seeds every scalar
  leaf `USize`.  **This is the one red target of the two §5 named that is a
  *typing* question rather than a printer's**, and the answer taken is §5.1's:
  the author states the class (a kernel must be explicitly specialized).  The
  three candidates that were weighed — the routing restates the domain at the
  call site, the pass reads a frozen function's own statements through a new
  lowlevel accessor, or an open body must be annotated — are settled on the
  third: **no default is invented and no frozen structure is read**.
- **The frozen callee's constants reach the caller as static refs.**  The static
  apply's residual clone keeps the callee's unchanged subterms as references into
  the frozen module, so a routed body's `operands[0]` selector arrives as
  `Static(…)` beside nodes of the caller's own.  A frozen node is a **value** and
  never a computation (a static module is fully solved), and `low_type`'s own
  reading says so (`low_type.rs`: "a static ref is a decided leaf with no class
  to refine").  The emitter therefore reads an operand's *value* from either kind
  (`emit_operand`, `AnyNodeId::dynamic`, and the readers that take
  `impl Into<AnyNodeId>`) instead of refusing it, and a static apply emits the
  residual the lowlevel already wrote for it.

**Landed with that reading: the class of a value had two readers and they
disagreed.**  A body lowered from a template is made of parameter reads the
low-type pass cannot transfer through, so `node_class` declined and fell back to
the integer default while the *emission* used the operands' class.  Measured on
the routed shapes: `compute.jit (x : Float => x * x)` declared
`result_classes = [Int]` over a body of `[LocalGet(0), LocalGet(0), Bin(Float,
Mul)]`, which the wasm validator refused ("expected i64, found f32"), and the
float index function of `a_varying_float_element_is_seeded_from_the_index` wrote
through `BufferWriteCall(Int)` a value it had computed as `f32` ("instruction 6
mixed an integer and a float").  `node_class_in` is now the one reader: the
emission's own order — a parameter read resolved through its slot's shape, the
same peel `node_class` makes, a whole-parameter read matched by equality class,
and an arithmetic operator's class taken from its operands with a comparison's
`0`/`1` the one exception.

**The same reader had a third site, and it was the one that mattered for a
*chain*.**  `compile_parallel_fragment`'s pre-scan — the class a buffer **read**
is declared in (`Positions::element_class`) — still used `node_class`, so a
routed `0.0 + a + a` was declared `Int` there while the write itself was emitted
in `Float`: the consumer's read of that buffer then declared the wrong class and
the chain's value stayed `Parameterized` (measured through the new structural
comparison: a whole two-kernel float chain, both sides, every element `false`).
Read through the slots, the chain computes.  `--test compute` went 12 of 60
(after the routing) to **58 of 60** with the other suites unchanged.

### 5.2 Open: the signature the wrapper publishes

`.sig`'s type must be a **decided arrow**, and four measurements constrain how it
can be stated.  Each is a trap that cost an attempt:

1. **A node that appears only in the type graph is never evaluated.**  An operator
   whose whole life is `.sig`'s field type never runs: the printer reads a value
   that nothing computed.  The statement must be made by an operator that already
   runs (`$jit`/`$parallel`), so the wrapper has to hand the signature expression
   to it as a value — `.sig s` **and** `$jit(f, s)`.
2. **`run_deferred` reports an unstamped node as `parameterized`.**  Putting the
   function's *type* node (`f.ty`) in an operand array makes the whole array read
   `parameterized` and the arm never runs at all (`OperatorExt::run_deferred`,
   `lowlevel/src/lib.rs`); the deep pass never stamps a type node.  So the
   signature expression must reach the operator without a type node among its
   operands.
3. **A `LaunchOp` domain read has to land on a pure cell.**  When the arrow's two
   sides are `Index` op nodes, `LaunchOp`'s `check_unify(a.ty, d)` against one of
   them defers and never resolves: measured, the four `jit_cross_kernel_*` tests
   print `parameterized` and the launch computes nothing, while the identical
   wrapper with a cell-sided signature passes 7 of 7.
4. **The generic wrapper's type must still render as an arrow with the function's
   own variables.**  `wrapper_functions_render_with_named_type_variables` pins
   `.sig ?a -> ?b` — the *same* classes as the wrapper parameter's own type.  A
   fresh arrow renders its own names, and a shape slot that is an unset operator
   renders `raw[?c, TypeFunction]` (the printer's kind branch needs the shape
   slot's *value* to be a two-element array).

Traps 3 and 4 pull apart: the sides must be *cells in the function's own type
classes* (4) yet must never be written (the pin the handoff forbids), while a
launch's read of them must resolve (3).

**Measured, this session — the pin is real, and per-application cloning is what
makes any statement of it possible:**

- an annotation written *inside* a wrapper body reaches the argument
  (`pin = g => {t = type_of g; h = g : Int -> Int; t}`, then `pin f; f 1.5`) is
  refused — "expected Int, found Float": the applied parameter's cells are one
  class with the argument's, so a class written there *is* the parameter cell the
  handoff forbids writing;
- the same function called twice with different classes is accepted
  (`f (1 : Int)` then `f (1.5 : Float)`), so a *callee's* cells are cloned per
  application — which is why a wrapper *definition* can stay generic while an
  *applied* result states its own class;
- the printer's arrow branch reads the **shape slot's value** at print time
  (`type_printer::elements`: `elements[0]`'s value must be a two-element array),
  so the statement is exactly "rewrite the shape node's value" — and it has to be
  a shape the *wrapper* owns, because the function's own shape node is what every
  other use of the function reads.

### 5.3 Landed: the wrapper owns its signature arrow (`$sig`)

`$sig(f)` (`compute.rs`) builds the arrow `JitOp`'s own gate builds — two fresh
cells through `ctx.arrow`'s shape/kind/pair triple — and unifies it against `f`'s
type, so its sides are *cells in the function's own classes* (trap 4) while the
shape node belongs to the wrapper (no pin).  `jit`'s `.sig` field is
`($sig(f))` instead of `(type_of f)`.

Measured: the wrapper definition still renders
`Function: ?a -> ?b -> struct<.native raw[?c, ?d], .sig ?a -> ?b>` — a native call
*is* accepted in a type position and its term is what the field type becomes — an
applied result still renders `.sig ?c -> ?c`, and the tree stays `57 of 58` with
every other suite unchanged.  `$sig`'s *value* node is the shape: the node §5.2's
statement has to rewrite.

### 5.4 Open: who rewrites the shape, and how it reaches them

The member is knowable **only at run time** (`open_class_of` reads the function's
asserts, and the frozen template's parameter has no body), and every node an
operator's operand array holds is deep-pass gated by `run_deferred`'s default —
the gate the LaunchOp idiom's inert read rides on and that an open signature
therefore fails.  So the writer needs one of:

- **an operand that is already concrete** — **refuted by measurement**: giving
  `ComputeOperator::Jit`'s operand array the shape as an inert second element (the
  `LaunchOp` idiom) turns the deep pass's answer for the whole array into
  `parameterized`, so the default gate returns before the arm runs and the artifact
  is never compiled: `35 of 58` `--test compute` cases go red, every one of them
  reading `parameterized`.  A bound array whose items are still-open cells is
  *not* concrete, and the shape's items are those cells by construction;
- **a `run_deferred` override** on the compute vocabulary: the one op that must
  read its operand *structurally* is exactly the case the default gate cannot
  serve.  The default's body (deep-evaluate the operand node, refuse when its
  stamp says parameterized, hand the arm the value) is **one policy**, so the
  clean landing is to factor it into a shared step both the default and the
  override call — otherwise the override restates it for all ten other arms; or
- **a node-carrying opaque value**: a `ComputeValue` variant holding the shape is
  concrete for the pass and carries the node to the arm — at the cost of a value
  variant whose GC tracing and codec must keep that node alive.

The write itself is the same in every case: for each shape item whose class is
still open, use the class domain's first member as that item, rebuild the shape
array, `Module::write_node_value` it — never a write into the function's cells.

## 6. How to verify, at each step

```bash
cargo check --workspace
cargo test -q -p lichen-highlevel -p lichen-compute
cargo test -q -p lichen-language --test compute --test pipeline --test graph_jit \
  --test graph_structure --test examples --test defer_pending
```

§5's own acceptance is the two targets
[operator-polymorphism](operator-polymorphism.md) §8.8 recorded as red:

- `a_kernel_value_and_type_render_by_name` (`crates/lichen-language/tests/compute.rs:466`)
  — `.sig ?c -> ?c` must become `.sig Int -> Int`;
- `an_imported_package_that_jits_at_its_top_level_still_runs`
  (`crates/lichen-language/tests/runtime_only_package.rs:41`) — the `launch` gate
  must resolve the domain of an open `.sig`.

**Where the tree stands after §5.1 and §5.1.1** (`--test compute`, 60 cases): 58
pass.  The two reds are one shape each, and neither is a printer:

| case | what it needs |
|---|---|
| `jit_cross_kernel_call`, `jit_cross_kernel_wrapper` | a cross-kernel call or `compute.launch` whose **argument** is a routed operator (`k0 (x + 1)`): the emitter reaches the argument's cell and its equality class holds a *bare cell*, not a computation, so there is nothing to emit.  **This is not a class question and no author-side specialization answers it** — the parameter is annotated and the body still has the shape.  **Measured root**: the *routing* lowered `x + 1` to a **call of the prelude's binding** (`Apply(Static(add), [x, 1])`), and inside a kernel **template that call is never evaluated** — `wire_apply_result` (the lowlevel's own wiring, whose doc says "the apply node *is* the return pair") is never called for it — so the body the call stands for has no edge from the call.  Three routes were tried and measured: (a) reading the residual from the call's own value — nothing is written there for a template call; (b) merging call with body in `wire_apply_result`'s scalar/undecided arm — **breaks** `lichen-highlevel --test dependent` (`dependent_type_resolves_per_argument_via_laziness`), so it cannot be unconditional; (c) forcing the body once at `jit` time (`evaluate_node_deep(ret_value)`) — the class still does not hold the residual.  What is left is the operator workstream's own recorded alternative: **expand the binding's body at the call site** instead of calling it (`operator-polymorphism` §7.1), which leaves `x + 1` an `Add` node in the body and removes the question entirely; until then the refusal stands |
| `examples/compute_jit.lichen` (`--test examples`) | the same shape through a `compute.launch` argument |

The `jit_cross_kernel_*` cases and the example are annotated as the rule requires;
their remaining failure is the residual edge above, not the class.  Both
`a_kernel_value_and_type_render_by_name` and
`an_imported_package_that_jits_at_its_top_level_still_runs` — the two targets
§5.2/§8.8 recorded as red — **pass** with the class stated, which is what the
rule says they need.

The probes that pin the rest (scratch files, not committed):

- **§5's own probe** (the decided element cell, stated where types are stated): a
  single-kernel `plrun` + `collect` must print `array<Int, ?b>`; today it prints
  `array<?a, ?b>` on the array API and on every other spelling.
- **§4's own assertion, measured and corrected**:
  `a_gpu_program_chains_two_kernels_on_a_device`
  (`crates/lichen-language/tests/compute.rs:1308-1345`) does **not** keep its
  `array<Int, ?d>` under the struct spelling — it renders `array<?d, ?e>` with the
  same values, which is the element cell §5.2 states and the same commit path
  [compute-runtime-scalars](compute-runtime-scalars.md) §4.4 measures.  The earlier
  reading of this bullet as "must keep" was written before §1's measurement showed
  the `Int` arriving only where a consumer's array literal is present.
