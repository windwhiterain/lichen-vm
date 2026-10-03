# One channel for a class's value and its low type

> Status: **§2 refuted by measurement** (below), §3 still open and now specified
> more sharply, §4 waits on §3, §5 is the structural change that would delete the
> need for both.  §1 is the incoherence as first read; §2 records what the
> measurement says that first reading got wrong.
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

## 3. Half two — the decider states the fact, in both vocabularies

**What**: the layer that decided a class states it on the class, instead of
leaving it to a deferral's side effect.  §2's measurement splits the one action
into the two statements the two readers need.

- **Where the parallel run decides it**: `ComputeOperator::ParLaunch`'s arm
  (`compute.rs:1329`ff) builds the result values (bare `Buffer` /
  `DeviceBuffer`, or a tuple of them); each carries its element `class`.
- **A class reader needs the low type**:
  `seed_class_low_type(node, low_shape_of(element_class))` — what the value's
  class *is*, readable by anything that asks a class question.
- **The printer needs the value**, and this is the part the earlier plan got
  wrong: `type_printer::node` (`crates/lichen-render/src/render/type_printer.rs:62`)
  renders an unbound cell as its `class_name`, and **the printer never consults
  low types at all**.  So `array<Int, ?b>` is a claim about the element cell's
  *class value*, and only a value write through the choke-point
  (`Module::write_node_value`, `equality.rs:258`) can state it: the type value
  `Int` (the `[int, K]` node the checker's own `int_type` is) written into the
  result's element cell.  A seed alone changes what a *compiler* sees and nothing
  about what a program prints.
- **Where the scalar path decides it**: `kernel_results_value`
  (`compute.rs:6195`) already creates a node per scalar result; the same two
  statements belong there for the multi-result form.
- **The rule to state in the doc comment**: a run that decided a class declares it,
  in the vocabulary each reader asks in — the low type for a class question, the
  value for a type question.  A reader must never have to hope a pending read sits
  in the same class.  The existing seeding discipline is the precedent:
  `compile_parallel_fragment` seeds the parameter slot (`compute.rs:2361`),
  `seed_template_term_low_types` seeds the template's decided terms.
- `low_shape_of` (`compute.rs`, the inverse of `scalar_class_of`) already exists
  for the class → shape step; the type value for a class is the checker's own
  `int_type`/`float_type` and must be reachable from the module at run time
  (open question: the arm holds `module`, not `ctx`).
- The reader-side routing §2 could not justify on its own — `names_float_class`
  and the conversion gate — lands here, where a run has actually stated a class;
  it is added **only** as a class question (`class_scalar_of_slot`-style, scalar
  answers only), never as a type read.

**Acceptance**: the GPU chain test's collected array is `array<Int, ?d>` again with
the **struct/tuple** argument spelling (i.e. without the array's homogeneity), and
the single-kernel `collect` probe becomes decided too.  That second one is the
cleaner assertion: today it is undecided even with the array API.

## 4. The carrier: the struct-argument migration

The migration that exposed all of this, and the reason to do §2/§3 now: replacing
the raw array arguments of `compute.read`/`compute.write` with struct instances
(approved direction, explicit constructors).

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

**Order**: §3 → this.  §2 is refuted, so §3 is the first landed step, and §3 is
what should turn the 57 into 58; if it does not, the remaining difference is a
second commit path and belongs back in §1's terms.

**Not to forget**: the migration touches `crates/lichen-language/tests/*`,
`crates/lichen-language/examples/*`, and the docs' code blocks; the script must
handle nested occurrences innermost-first (`compute.write [a, b, compute.read [c, d]]`).

## 5. The structural removal this makes unnecessary

`ParLaunchOp::build` (`compute.rs:8623`, `out_ty` at `:8676`) leaves the result
type a **fresh cell**, and its own doc says why:

> "The signature's *arity* is what decides the result's shape — a bare `Buffer`
> for a one-write index function, a tuple of buffers for a several-write one — and
> the arity cannot be read here: `build` runs once, on the frozen `plrun` template,
> where `.sig` is an unbound cell that only resolves at run time."

So the class of a `plrun` result is knowable only at run time — which is *why* §3
exists at all.  If the signature were concrete **before** `build` runs, the result
type would be stated where types are stated, and the fresh cell, the deferral
side-effect dependency, and §3's run-time statement would all be unnecessary
rather than merely fixed.  That is the specialize-before-JIT direction
([kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6,
[compute-param-struct-handoff](compute-param-struct-handoff.md) §7), and it is why
§3 is worth doing as a *step*: it is the honest state of the model while the
signature is late, and it is the check that tells us when it has arrived.

## 6. How to verify, at each step

```bash
cargo check --workspace
cargo test -q -p lichen-highlevel -p lichen-compute
cargo test -q -p lichen-language --test compute --test pipeline --test graph_jit \
  --test graph_structure --test examples --test defer_pending
```

The probes that pin the remaining half (scratch files, not committed):

- **§3's own assertion**: a single-kernel `plrun` + `collect` must print
  `array<Int, ?b>` — today it prints `array<?a, ?b>` on the array API and on every
  other spelling.
- **§4's assertion**: `a_gpu_program_chains_two_kernels_on_a_device`
  (`crates/lichen-language/tests/compute.rs:1308-1345`) must keep its
  `array<Int, ?d>` with the struct spelling.
