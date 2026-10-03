# One channel for a class's value and its low type

> Status: **planned, not started.** The plan below is the answer to "which change
> makes this unified and simpler"; §1 is the incoherence it removes, §2 and §3 are
> the two halves, §4 is the carrier migration that waits on them, §5 is the
> structural change that would delete the need for both.
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
every reader asks the class.

## 2. Half one — the readers ask the class

**What**: route the highlevel's type-slot decoding through the lowlevel's
class-routed channel, keeping the node walk only as the static/undecided fallback.

- `shape::low_type_of_slot(module, slot)`: for a **dynamic** slot, prefer
  `module.low_type_of_node(node)` (the class's low type, already refined by the
  two observation sites) and fall back to today's node walk; a **static** slot
  keeps the node walk (a static module's classes are its own).
- `compute::node_class` (`compute.rs:4302`): the same, on the value node it
  already resolves.
- The printer: audit its paths for the same split — `type_printer::is_arrow` uses
  `representative`, so the file already has the class-routed form; any path that
  reads a node's own value where the class is the authority belongs here.
- `checker::names_float_class` needs no change: it rides on `low_type_of_slot`.

**Why it is the simplifying half**: it deletes a second reading of a fact the
lowlevel already owns, and it makes "which member is the representative"
unobservable — which is what the lowlevel's own docs promise.

**Acceptance**: all suites green, and *no behavioural change expected on the
current baseline* — this half alone does not create a missing fact. Its proof is
that the same answers survive while the node-slotted path is gone; if any answer
*improves* (a class-decided shape becomes readable through a non-representative
member), that is the half working.

## 3. Half two — the decider writes through the choke-point

**What**: the layer that decided a class states it on the class, instead of
leaving it to a deferral's side effect.

- **Where the parallel run decides it**: `ComputeOperator::ParLaunch`'s arm
  (`compute.rs:1329`ff) builds the result values (bare `Buffer` /
  `DeviceBuffer`, or a tuple of them); each carries its element `class`.  One node
  for the result plus `seed_class_low_type(node, low_shape_of(element_class))` (or
  `write_node_value` where a *value* is the fact) records it on the class.
- **Where the scalar path decides it**: `kernel_results_value`
  (`compute.rs:6195`) already creates a node per scalar result; the same seed
  belongs there for the multi-result form.
- **The rule to state in the doc comment**: a run that decided a class declares it;
  a reader must never have to hope a pending read sits in the same class.  The
  existing seeding discipline is the precedent: `compile_parallel_fragment` seeds
  the parameter slot (`compute.rs:2361`), `seed_template_term_low_types` seeds the
  template's decided terms.
- `low_shape_of` (`compute.rs`, the inverse of `scalar_class_of`) already exists
  for the class → shape step.

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

**Order**: §2 → §3 → this.  §3 is what should turn the 57 into 58; if it does not,
the remaining difference is a second commit path and belongs back in §1's terms.

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
side-effect dependency, and the second reading would all be unnecessary rather
than merely fixed.  That is the specialize-before-JIT direction
([kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6,
[compute-param-struct-handoff](compute-param-struct-handoff.md) §7), and it is why
§2/§3 are worth doing as *steps*: they are the honest state of the model while the
signature is late, and they are the checks that tell us when it has arrived.

## 6. How to verify, at each step

```bash
cargo check --workspace
cargo test -q -p lichen-highlevel -p lichen-compute
cargo test -q -p lichen-language --test compute --test pipeline --test graph_jit \
  --test graph_structure --test examples --test defer_pending
```

The probes that pin the two halves (scratch files, not committed):

- **§3's own assertion**: a single-kernel `plrun` + `collect` must print
  `array<Int, ?b>` — today it prints `array<?a, ?b>` on the array API and on every
  other spelling.
- **§4's assertion**: `a_gpu_program_chains_two_kernels_on_a_device`
  (`crates/lichen-language/tests/compute.rs:1308-1345`) must keep its
  `array<Int, ?d>` with the struct spelling.
