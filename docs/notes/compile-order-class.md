# Which write fills a `Buf` element cell? — measurement

Worktree: `C:\resource\lichen-vm\.worktrees\classfill`, branch `feature/classfill`, base `dev` @ `fc0761c4`.

The question the brief asks: **in the passing program, which write fills the element cell that
`walk_role` reads?** Everything here is measured. The probe and the `#[track_caller]` recorder that
produced the measurement were **removed before this note was committed** — the investigation they served
is closed — so §2's *What proved it* names every write side and the trace positions, which is what makes
the measurement re-derivable. The defect itself was **fixed** afterwards: `7eedb125` (*highlevel: a written type term is decided before the definition pass*) un-ignored the three assertions in `crates/lichen-compute/tests/lichen_compute/buf_element_class_recorded.rs`, and they are **ordinary passing tests now** (3 passed, 0 ignored) — `the_compiled_fragment_names_the_declared_element_class`, `the_module_holds_a_decided_float_element_type`, `the_recorded_node_carries_the_declared_element_class`.

## 1. The cell is not the element node — it is that node's pair **value half**

`crates/lichen-compute/src/compute.rs:2013-2017` reads

```rust
parameter_type_ref(module, field)
    .and_then(|ty| field_type(module, ty, 1))
    .map(|element| low_type_of_slot(module, element))
```

For `attend`'s `.v (compute.Buf Float)`, `field_type(..., 1)` is `NodeId(2404v1)`, and that node
**holds its value in both programs**. The probe prints it:

| program | `2404` cell | `2404` class | `2405` (pair item 0) cell | `2405` class | decoded |
|---|---|---|---|---|---|
| CONTROL (passing), at `attend`'s walk | `Array(..)` | `Array(..)` | `Some(TypeValue(TypeFloat))` | `Some(TypeFloat)` | `Float` |
| STEP (failing), at `attend`'s walk | `Array(..)` | `Array(..)` | **`None`** | **`None`** | `Unknown` |

So the lazily-filled cell is `NodeId(2405v1)` — the **value half** of the element type's
`[value, type]` pair — reached by the pair indirection at
`crates/lichen-highlevel/src/shape.rs:866-876`. `low_type_of_slot` reads the **cell**
(`module.node_value`, `shape.rs:862`), and the cell is empty.

**This also refutes the class route for this defect, with a measurement**: at the failing read
`class_value(2405)` is `None` too (probe line for `buf path=[3, 1]`). The class is *not* decided
yet, so "read the class instead of the cell" cannot work here — consistent with the brief's note
that `class_value`/`class_low_type` were empty.

`.s (compute.Buf Float)` in the same `QSV` reads `Float` even in the failing program, because its
value half `NodeId(1449v1)` was already filled (probe: `FILL NodeId(1449v1)`, trace position 4631
of the STEP run, well before `compile_parallel_fragment` #0 at 8384). That is why the failing
fragment is `in [Float, Int]` and not `in [Int, Int]`.

## 2. The write that fills it

**`Module::fill_class_holes`, `crates/lichen-lowlevel/src/equality.rs:111-118`, called from
`Module::add_equality` at `equality.rs:98-100`.**

```rust
// equality.rs:89-102, Module::add_equality
let representative = disjoint::union(&mut self.nodes, a, b);
...
if let Some(value) = left_value.or(right_value) {
    self.fill_class_holes(representative, value);   // <- equality.rs:99
}
```

`fill_class_holes` writes `Some(TypeValue(TypeFloat))` into every class member whose slot is
empty — including `2405`. The class is one large union-find class of 35+ `Float` type cells
(representative `NodeId(448v1)` in CONTROL, `NodeId(1433v1)` in STEP — the same member set, a
different union winner).

`add_equality` here is reached from `unify_inner`'s **"one side knows a value"** arm:

```rust
// equality.rs:722-727
(Some(_), None) | (None, Some(_)) => {
    // The merge already gave every member the value the deciding side kept.
    self.add_equality(ra, rb);   // <- equality.rs:725 (728 after probe instrumentation)
    true
}
```

### What proved it

A `#[track_caller]` recorder on **every** write side of `Node::value`, with a single ordered log
shared with the reads:

| site | instrumented |
|---|---|
| `Module::add_node` | `module.rs:116-138` (allocation with a concrete value) |
| `Module::write_node_value` | `equality.rs:281` (covers `write_node_value_if_absent`, `write_node_answer`) |
| `Module::propagate_class_value` | `equality.rs:369` |
| `Module::relocate_node_value` | `equality.rs:331` |
| **`Module::fill_class_holes`** | `equality.rs:111` — **the writer** |

plus a counter at the top of `compile_parallel_fragment` and one `note_read` per `walk_role`
buffer leaf, so every read is attributed to the kernel compile it belongs to.

Running it on both programs (the recorder §5 describes how to rebuild):

```
CONTROL (root is `(scores, attend)`, passing)
  7350  == compile_parallel_fragment #0  fid=FunctionId(1v1)  cfg_pair=NodeId(142v1)   <- scores
  7355      -- after lower_index_function
  7422  MERGE NodeId(2405v1)(decided=false) ~ NodeId(5262v1)(decided=true)
         unify stack: ["crates\lichen-lowlevel\src\apply.rs:139"]
  7447  MERGE ... unify stack: ["crates\lichen-lowlevel\src\apply.rs:78"]
  7854  == compile_parallel_fragment #1  fid=FunctionId(3v1)  cfg_pair=NodeId(1135v1)  <- attend
  7856    buf path=[3, 1] element_type_node=2404v1 element_class=Some(Float)

STEP (has `built = compute.graph step`, failing)
  7961  == compile_parallel_fragment #0  fid=FunctionId(3v1)  cfg_pair=NodeId(1135v1)  <- attend
  7965    buf path=[3, 1] element_type_node=2404v1 element_class=None
  8736  == compile_parallel_fragment #1  fid=FunctionId(1v1)  cfg_pair=NodeId(142v1)   <- scores
  8865  MERGE NodeId(2405v1)(decided=false) ~ NodeId(6384v1)(decided=true)
         unify stack: ["crates\lichen-lowlevel\src\apply.rs:139"]
```

**Same write, both programs. The only difference is when it runs.** In CONTROL it runs after
`scores` compiles and before `attend` compiles; in STEP it runs after `attend` has already read.

Both programs also show the *other* writers, which are **not** the answer:

* `write NodeId(2405v1) <- crates\lichen-lowlevel\src\static_module\apply.rs:140` — this is
  `static_node_apply` (`static_module/apply.rs:98-162`), the apply-clone of the frozen `Buf`
  field-list nodes. It writes **`None`**: `static_module/apply.rs:135-140` maps the frozen
  node's value only `if carried`, and a definition carries no answer (the carry rule,
  `apply-clone-ownership.md` §2). So the clone *arrives empty*.
* `seed_template_term_low_types` / `infer_template_low_types` (`compute.rs:2416-2417`) run
  **after** the role walk at `compute.rs:2403`, and the probe's stage markers (`-- after
  seed_template_term_low_types`, `-- after infer_template_low_types`, `-- after
  lower_index_function`) show neither contains the merge.

## 3. What has to run before `compute.rs:2403`

The merge is `Module::unify` on the kernel's cloned parameter, and the `unify` stack names the
caller: **`crates/lichen-lowlevel/src/apply.rs:78`** (`apply_parameter_check`) and
**`crates/lichen-lowlevel/src/apply.rs:139`** (`wire_apply_result`) — i.e. the VM *applying a
function*, whose parameter unify descends the struct type and merges the element cell into the
decided `Float` class.

So the answer to "what would have to run before `:2392`" is: **an application whose parameter
unify descends this kernel's parameter type** — the one `compute.parallel` already has in hand as
`operands[0].node` (`crates/lichen-compute/src/compute.rs:1170-1177`, just before
`compile_parallel_fragment(module, function)` at `compute.rs:1178`).

**Not implemented here**, and both reasons are measured rather than guessed:

1. **The class route is empty.** At the failing read `class_value(2405)` is `None`, so reading the
   class instead of the cell cannot work (§1).
2. **Forcing the operand does not re-run the apply.** Measured: inserting
   `module.evaluate_node_deep(operands[0].node, None)` immediately before the
   `module.node_value(operands[0].node)` read at `compute.rs:1170` changes **nothing** — both
   ignored assertions fail with exactly the same `in [Float, Int] out [Int]`. The operand node
   already holds its `Function` value, and a node that holds one is not re-run, so the parameter
   unify that would fill the class never happens again. This is the carry rule
   (`static_module/apply.rs:130-140`, `apply-clone-ownership.md` §2) doing exactly what it says,
   and it is the same reason the brief's earlier `evaluate_node` attempt had no effect.

That last point is the real shape of the defect and the next step: **the class is filled by a
unification that the language's own carry rule skips.** The route this note first proposed — have the
checker, *which builds the `Buf` type term and knows the declared element*, seed the class at
construction — **is wrong, and measuring that was the next slice's first result**: the checker does not
build the term. It builds **one node** for the `compute.Buf Float` application
(`crates/lichen-highlevel/src/checker/lambda.rs:316`) and it *does* know the class there (the argument's
value node is the shared `Float` node and its cell holds `TypeValue(TypeFloat)`), but the type **term** is
materialised later by the VM — `static_function_apply` → `static_node_apply`
(`crates/lichen-lowlevel/src/static_module/apply.rs:112`) **cloning the frozen `compute.lichen`
template** — and the cell `walk_role` reads is **one of those clones**. So **no site both knows the class
and holds the cell**; they are in different crates and different phases. Two seeding experiments were
built and discarded: putting a `Parameter` element's own binding value cell in its slot's value position
(`crates/lichen-highlevel/src/checker/tuples.rs`) does fire
(`kind=Parameter value=Some(NodeId(290v1))`) but leaves the cell empty at the first walk, and evaluating a
literal argument at check time has no effect (the literal's value node already held `TypeFloat`).

**The measured missing fact**: at **every** `compute.Buf` application (`applied = StaticFunctionId(22)`),
`apply_parameter_check` sees `param = [..=None, ..=None]` and `arg = [..=None, ..=None]` — **the binding
the application exists to make never happens there**. The class arrives only when the kernel's own
parameter unify later descends the struct type and merges the cell into the decided `Float` class. A seed
and that merge *could* disagree, and the machinery has an answer for it — `write_node_answer`/
`unify_inner`'s `(Some, Some)` arm calls `record_error(ra, rb, …)`, a recorded unify error naming both
nodes — but with no seeding site reachable that is untested.

**The cells are what is empty, not the values, and that is measured.** A probe at
`static_function_apply`'s parameter check on `BACK_TO_FRONT` (`dev@c0773e12`, instrumented and reverted)
printed, for every static apply including `StaticFunctionId(22)`: `arg_value = true`,
`param_value = true`, `param_op = false` — the argument and the cloned parameter both **hold values**
at the check, and the parameter carries no operation. So the check does run with both sides in hand;
what stays empty is the struct term's **element cell**, and the unify's **descent into it** is what
fails to bind. That narrows candidate (ii) from "make the argument decided" to "make the descent
bind" — the value is present, the merge is not — and it is why the fix is a lowlevel apply/unify
question rather than a program spelling or a read-order move alone.


So the two honest candidates are both **lowlevel** changes to the carry rule or the apply ordering, and
neither was chosen: **(i)** carry the parameter binding into the `.element` slot's value position **at
clone time** in `static_node_apply`, or **(ii)** make the `Buf` application's argument **decided before
its `apply_parameter_check`**. Both need a decision that this note does not make.

## 4. One half of that did land: an undecided class is refused, not guessed

`compile_parallel_fragment` now returns `RoleUnread::Refusal` naming what it could not read
(`crates/lichen-compute/src/compute.rs:2428-2441`) instead of falling through to
`unwrap_or(ScalarClass::Int)`. **Its blast radius was measured as zero**: A/B against the commit reverted,
over the whole `lichen-compute` suite, the runs are identical, failing exactly the six pre-existing
`lowlevel` binaries with the same counts. No legitimate program hit the refusal, so it was neither
narrowed nor widened. One sentence changed to say what the refusal now covers —
*"A named refusal: the parameter is not a kernel ABI, **or a class it declares is not decidable**, and
this is why."* (`compute.rs:1907-1908`); the `RoleUnread` invariant block itself needed no change.

**Its blast radius was zero, and that measurement no longer holds.** It was taken while the refusal
was only *returned*. Once a caller records it as a build failure (§4.6), the same entry reaches
`Module::has_extension_failures`, and one working program — `BACK_TO_FRONT`, §4.1 — fails its check
because of it. The blast radius did not grow because more programs hit the refusal; it grew because
**the refusal now reaches a verdict it did not reach before.**

**It does not flip the pin above, and that is the interesting part**: at the failing read a write *is*
found, so the default was never what produced the `Int` here. The refusal is a net for the *other* sites
where an undecided class used to become `Int` silently — not a fix for this defect.

### 4.1 What is literally undecided when this refusal fires — measured, and it is not a class

The refusal's sentence names a scalar class. The measurement says the class was never consulted.
`crates/lichen-compute/tests/lichen_compute/graph_structure.rs`'s `BACK_TO_FRONT` is the program that trips it (it
answers `[6, 8, 10, 12]` on `dev`, so the program works), and instrumenting the read at
`crates/lichen-compute/src/compute.rs:2428-2432` with `write_node`/`node_value` probes before the
`find_map` gives **three attempts and one refusal**:

```
PROBE output 0 -> write_node None
PROBE outputs named 1
PROBE output 0 -> write_node Some(NodeId(1958v1))
PROBE   operand node Some(NodeId(1959v1))
PROBE   operand value is Some = true
PROBE outputs named 1
PROBE output 0 -> write_node Some(NodeId(2056v1))
PROBE   operand node Some(NodeId(2057v1))
PROBE   operand value is Some = true
PROBE outputs named 1
```

**Attempt 1 refuses, and it refuses because `write_node` returned `None`**: the codomain position
reaches no `compute.write` at all. Attempts 2 and 3 find one and its operand array already holds a
value, so the class read is not what stopped either of them.

So the thing that is undecided is **the shape of the codomain position's node**, not a scalar class:
`write_node` (`compute.rs:2580-2604`) peels at most eight `concrete_element` steps looking for a
`ComputeOperator::Write`, and at attempt 1 none is reachable. The refusal's sentence — *"none of them
is a `compute.write` of a decided scalar"* — packs two different causes into one clause, and the
second is not the one that fired: `node_class_in` (`compute.rs:3396-3401`) returns a `ScalarClass`
and has no spelling for "undecided" at all; it is reached only inside
`write_value_node(..).map(..)`, and `write_value_node` was `None`.

### 4.2 Who decides it, and in which phase

**All three attempts are inside the check.** The test's own helper panics at
`crates/lichen-compute/tests/lichen_compute/graph_structure.rs:32` — the `assert!(report.ok(), …)` — which is
before it calls `evaluate_node_deep` at all, so nothing that follows is the run. Attempt 1 is the
checker evaluating `compute.parallel doubler` while the rest of the program is still unevaluated;
attempts 2 and 3 are the **same checker**, re-evaluating as more of the program is demanded, and by
then the write is there.

That is the same order §3 measured, one level down: the deciding work is an evaluation the checker
performs speculatively and repeats, not a run-time normalize. It is also why the entry is stale. The
channel is append-only (`crates/lichen-lowlevel/src/lib.rs:903-906`: *"Append-only and never cleared,
so a second pass sees the first pass's records"*), so **attempt 1's refusal is still in
`extension_diagnostics` after attempts 2 and 3 have got past it** — the list cannot forget, and the
attempt that supersedes it leaves no trace that it did.

### 4.3 The vocabulary has no word for this, and its own invariant forbids borrowing one

Both sentences of the `RoleUnread` invariant (`compute.rs:1897-1902`), verbatim:

> `[Self::Undecided]` is produced only where the annotation is still a cell with no
> value: there is nothing to read and nothing to report. `[Self::Refusal]`
> always carries the sentence its subject was named in. A caller tells them apart
> by the variant, never by comparing text, so no later edit to a message can turn
> a refusal into a "not yet".

In that vocabulary `BACK_TO_FRONT` is **`Refusal`**: the annotation is not a cell with no value, and
§8's ruling 2 places the decision at run time, on the apply's unify rather than a normalize of the
callee ([function-normalization](function-normalization.md) §2). The second sentence is the sharp
part — a caller must not promote this to `Undecided` by matching on its text, which rules out the
cheap fix by construction.

But `Undecided` records **nothing** (`compute.rs:1196-1197`, `RoleUnread::Undecided => return None`
before any `record_extension_diagnostic`), and §4's own note about `compute.jit` is that this channel
is what "says why a kernel stayed lazy instead of discarding the reason". So the case is **a third
thing the vocabulary does not have: a named fact that a later attempt in the same check has already
superseded.** The list has no slot for it, and no variant spells it.

### 4.4 Transient or permanent — and what not recording would cost

Transient, here: attempts 2 and 3 get past it, and on `dev` the same program answers `[6, 8, 10, 12]`,
so a fragment does compile and run.

Permanent, for any program whose codomain position never becomes a `compute.write` — an author who
wrote `step` with no write at all is the smallest. That program produces no value, and today the
refusal is what says so. **So not recording it would lose a real refusal**: `RoleUnread::Undecided`
would return `None`, the operator would answer `parameterized`/`none`, and `render_build` would have
nothing to refuse on — the exact loss §8's ruling 3 forbids (*"must report it — '报错' — never
silently keep the flat tuple"*).

### 4.5 The JIT's own gate for an undecided type already exists

The ruling asked where a JIT meeting an undecided type reports it. **It does, and in both compile
paths, by name** — `UNDECIDED_DOMAIN` (`compute.rs:2738-2740`):

```rust
// compute.rs:2409-2411, compile_parallel_fragment — the compute.parallel path
let Some(cfg_shape) = module.low_type_of_node(cfg_value) else {
    return Err(UNDECIDED_DOMAIN.into());
};
```

`compile_fragment` has the same gate for `compute.jit` at `compute.rs:2221-2223`, and its seed reads
*"without one, `Unknown`, which the read refuses by name"* at `compute.rs:2209-2210`. So there is no
missing gate to add: the rule "a JIT must report an undecided type" is already met, and what these
paths return is a refusal the caller records.

What is missing is narrower, and it is the thing this note now names: **the recording does not know
whether the refusal it is about to record has already been superseded inside the same check** (§4.2),
and the channel it records into cannot say.

### 4.6 What the evidence supports

Not a reclassification, on this evidence. Moving this case to `Undecided` costs a real refusal
(§4.4) and contradicts §8's ruling 3; leaving it as `Refusal` keeps the reason but makes a working
program's check fail, because the check's verdict reads an append-only list (§4.2). **The honest
third option is a shape change** — the entry carries "superseded" beside `fails_build`, and the
build verdict reads only the standing ones — which is a change to
`ExtensionDiagnostic::fails_build`'s own invariant (`crates/lichen-lowlevel/src/lib.rs:950-955`:
*"It is set by the recording layer, never inferred"*), and therefore not a decision this note makes.

### 4.7 The author's ruling: read once, and an undecided type at the first read is a real error

> **"不要依赖第二次读，现在正在往只读一次这个方向上重构。所以，JIT 第一次读 kernel function 类型未决，
> 就真的报错，不要想恢复"** — *Do not rely on a second read; the work is refactoring toward
> reading once. So when the JIT first reads a kernel function and the type is undecided, it
> should really error — do not think about recovering.*

**This dissolves §4.6's question instead of answering it.** §4.6 offered a shape — the entry carries
"superseded" beside `fails_build` — and that shape exists **only because attempts 2 and 3 re-read**
(§4.2). Under one read there is no attempt 2: attempt 1 is *the* read, and what it finds is a fact
about the program rather than about the schedule the checker happened to run. Removing the re-reading
removes the thing that would have to be recorded.

**The error the ruling names already exists**, in both compile paths, by name —
`UNDECIDED_DOMAIN` (`compute.rs:2738-2740`), returned at `compute.rs:2409-2411` for
`compute.parallel` and at `compute.rs:2221-2223` for `compute.jit`, whose seed reads *"without one,
`Unknown`, which the read refuses by name"* (`compute.rs:2209-2210`). **No new gate has to be added**
for "a JIT must report an undecided type". What the refactor supplies is that this read **happens
once**, which is what §4.1's three attempts show it does not do today.

**§4.1's three attempts are therefore evidence for the refactor, not for a shape change.** What they
measure is precisely what a single read removes: one `compile_parallel_fragment` call for one
`doubler`, entered three times, the first at a point where no `compute.write` is reachable from the
codomain position and the second and third once more of the program has been demanded. Collapsing
that to one read moves the whole decision onto the first attempt — and then `BACK_TO_FRONT` **as
written in §4.1** is a program that should fail, not one that happens to pass.

**So the fixture is corrected on this branch, as the ruling directs.**
`crates/lichen-compute/tests/lichen_compute/graph_structure.rs`'s `BACK_TO_FRONT` declares `compute.Buf Int` on
`OutA`, `InB` and `OutB` instead of `compute.Buf _`. **That is the author's ruling applied to a test
fixture, not a convenience**: it is the same edit a user makes to their own program, and it is what
makes the program honest under the read the refactor is heading for. Its answer, `[6, 8, 10, 12]`, is
all `Int` either way, so nothing about what the test measures changes.

**And `compute.rs:1198` is left a `note` on this branch**, with the reason stated rather than
assumed. The reasoning above makes the *undecided-type* case the refactor's business — but
`compute.rs:1198` is also the recording site for `compute.parallel`'s **standing** refusals (a write
that names the wrong output, an output that is not a write, a parameter that is not a kernel ABI),
and those are not undecided types. Under one read there is a single verdict and each refusal is it;
under today's three, the standing refusals and the transient one share one `RoleUnread::Refusal` and
one recording site, and **the flag cannot separate them** (§4.3). So the branch keeps the split it
measured rather than guessing which half the ruling means: standing refusals record as `failure`, and
the undecided-type refusal is what the refactor will give a real error at `UNDECIDED_DOMAIN`.

**What is measured next, and what each outcome means.** Reverting `compute.rs:1198` to `note` while
the run gate reads `Module::has_extension_failures()` has one predicted consequence, and it is
measured rather than argued:

* if `crates/lichen-compute/tests/lichen_compute/swallowed_refusal.rs` and
  `crates/lichen-compute/tests/lichen_compute/refusal_over_an_answer.rs` still pass, `1198` can be a note at no cost
  and this branch simply leaves it so;
* if they fail, that is the measurement that **`1198` and the gate are one decision**: the misplaced
  write is a refusal *only* because the site records it as one, so with the site a note there is
  nothing for the CLI or for a helper to refuse on. That would make "leave `1198` a note" cost two
  pinned tests, and this note has to say so rather than leave the branch to find out.

## 5. Re-deriving the measurement

The probe and the `#[track_caller]` recorder that produced §1–§3 were **not committed**: their
investigation is closed, and the defect has since been **fixed** (7eedb125), with its three pins now ordinary passing tests in crates/lichen-compute/tests/lichen_compute/buf_element_class_recorded.rs. To re-derive them, instrument the five write sides
of `Node::value` named in *What proved it*, share one ordered log with the `walk_role` reads and a
per-`compile_parallel_fragment` counter, and run the two programs — the same write appears in both, only
its position differs (CONTROL 7422/7447 against a compile at 7854; STEP 8865 against 7961).

## 6. The minimal reproduction set

**The smallest program that shows it** is one kernel, one `.in` buffer, one `.out` buffer, one read, one
write — no loop, no claim, no coordinate, no second read:

```lichen
In = struct{.d (compute.Buf Float)}
OutE = struct{.e (compute.Buf Float)}
Par = struct{.n Int, .in In, .out OutE}
raw = (lane : Par) => {
  compute.write ((compute.Write _)(.to lane.out.e, .at 0, .value compute.read ((compute.Read _)(.from lane.in.d, .at 0))))
}
k = compute.parallel raw "gpu"
Arg = struct{.n Int, .I In}
GArg = struct{.n Int, .in In}
built = compute.graph ((a : GArg) => {
  (compute.plrun k ((Arg)(.n a.n, .I a.in)) : OutE)
})
built
```

It compiles **`in [Int] out [Int]`**. The **pair** is committed: `crates/lichen-compute/tests/lichen_compute/`
`buf_element_class_recorded.lichen` (shows it) and `buf_element_class_direct.lichen` (does not), whose
kernel, parameter and buffers are **byte-identical** — **the root expression is the only difference**
(`k` against the `compute.graph` step). Each is its own test binary, so a single process's compile order
cannot be what decides either answer. The historical record of the defect is the pair of assertions below, which
were #[ignore]d with their reason so the suite stayed green while the defect was open; **the fix (7eedb125) removed the ignores**, and one command now shows them passing:

```
cargo test -p lichen-compute --test lichen_compute buf_element_class_recorded
[when the defect was open] left: (1, [Int])   right: (1, [Float])
[when the defect was open] left: [Int]        right: [Float]
```

**What the reduction removed, each measured**: the claims (`~ "combine"`, `~ "identity"`, `~ "element"`,
`~ "claim:Reduction(…)"`), `compute.range` and `.at` (a literal index does), the second extent, the
**`@loop` recursion entirely**, and **even reading the input buffer** — the defect survives all of them.
The backend is irrelevant (`"cpu"` shows it too). Two reductions are blocked by **contracts, not by the
defect**, and were not worked around: moving the step's buffer outside `.in`/`.out`, and dropping the
`.n` extent. Two spellings make it **vanish**, and they matter to the owner:
`compute.P (compute.KT Float)` (applying them is itself a unification that decides the class first — a
**workaround, not a reduction**), and **any float literal in the body** (`2.0 * …`). That second one is
the sharpest form of the propagation gap: **the fact is reachable from the program, just not from the
clone the walk reads.**

**The statement to hand to the type system.** *Known*: `checker/lambda.rs:316` builds one node for the
`compute.Buf Float` application and the class is known there — the argument's value node is the shared
`Float` node and its cell holds `TypeValue(TypeFloat)`; this is asserted, and it **passes for the failing
program**, because every `Buf` element cell in the module reads `Float` **once the module has run**.
*Needed*: `compute.rs:2013-2017` reads those cells through `low_type_of_slot`
(`shape.rs:858-877`) to build the fragment's `input_classes`/`output_classes` — those reads *are* the
classes the fragment declares. *Missing*: the checker never builds the type **term**; the VM materialises
it by **cloning the frozen template** (`static_module/apply.rs:112`), and the carry rule there
(`:129-140`, `apply-clone-ownership.md` §2) carries a frozen node's value only `if carried` — **a
definition carries no answer** — so the clone arrives with an empty value half, and the write that decides
the class is the kernel's **own parameter apply**, which runs *after* the walk. So: **the element class is
declared, checked and decided, but the node that carries it to the compiler is a clone the checker never
sees, and the write that would fill it belongs to a later phase — no site both knows the class and holds
the cell.**

**Two contracts a fix must respect**, both already decided: **D11** removed `Module::node_shape`/
`set_node_shape`, because a per-node low-type accessor is a second reader that can disagree with
`refine_class_low_type`; and `a68e3320` deleted `Module::class_value` after measuring 817 corpus programs,
which closes the class route (and matches this note's own measurement that it was `None` at the failing
read). There is **no `wontfix:D<n>` line over the classing**.

## 7. A second witness, from a real operator

A GEMV written as an ordinary operator reaches the same wall without anything hand-built
(`examples/gemv.lichen`, `da584de5`): recorded through the graph path, it is first refused at submit by
**`RaggedInputs`** (`crates/lichen-graph-ir/src/run.rs:273`, `ragged_host` at `:520-529`; **both removed
since**, the length check gone) — *"node 0's
inputs are not all the same length: 48 and 12"*, comparing **payload bytes**, which a GEMV's two inputs
(`M · kdim` and `kdim`) can never satisfy unless `M = 1` — and **once the two inputs are equalised**, submit
is refused by the class arm:

```
instruction 0 mixed an integer and a float in one operation … this is a malformed fragment
rather than an unsupported shape
```

That is the same `unwrap_or(ScalarClass::Int)` chain as attention's `attend`
(`compute.rs:2424-2430`), reached by an operator that a decoder would actually run. It refuses
**earlier** than the eight-byte payload count the KV-cache slice measured (`§` the operators note): those
are two different bugs, and neither this note nor that slice conflates them.

## 8. The author's rulings, and what each one rules out

Asked directly, the type system's author decided the following. They are contract, not commentary, and the
measurement under each is what the ruling has already been tested against.

1. **"检查期间和函数 normalize 期间尽可能的先把能 unify 的 unify 了，让剩下的求值能一次出结果，不再需要
   进一步的 unify。"** A class that is *determinable* is meant to be decided by checking or by a normalize;
   a later phase reads it rather than creating it.
2. **"检查期间只能 normalize 已经存在的函数，apply clone 出来的函数的 normalize 只能在运行期。"** For a
   kernel reached as an apply clone the deciding write is therefore the apply's **runtime unify**, not a
   normalize of the callee — a callee is not re-born per apply, so there is no new function to normalize
   ([function-normalization](function-normalization.md) §2) — which is what §3 measured: the same field
   reads `None` when the fragment is recorded and `Float` later in the run.
3. **A graph step recorded while the parameter reads `Undecided` must report it** — "报错" — never silently
   keep the flat tuple and the ABI's `Int` default. Measured, this is the *record* path's job alone: refusing
   inside `walk_role` instead fixes the four pinned assertions and **breaks four targets and 38 tests**
   (`compute`, `graph_jit`, `graph_structure`, `swallowed_refusal`), because site A's `Undecided` is the
   ordinary "annotation is still a cell" case that must stay lazy (`compute.rs:1178-1195`).
4. **"JIT 肯定不能回过来影响类型系统，JIT 对类型系统应当是只读/evaluate 的。"** No back-fill: the JIT may
   read and may evaluate; it may not write a class into the type system.
5. **"随便 evaluate 哪个都无所谓"** — subject to ruling 1, the aim stays a read that needs no forcing.
6. **"跨模块的行为应当 mirror 不跨模块的行为。"** The same type term must read the same way whether or not a
   module boundary and a materializing clone sit in between.
7. **"啥叫错误的值？没有值会是错误的。有的只有 unify 冲突。"** A carried value is never "wrong"; a
   disagreement with a later computation is a **unify conflict**, and it is reported. The carry question is
   "is this a template fact", never "might it freeze something".

**What ruling 6 does not licence here, measured.** Carrying more does not reach this cell: with the static
carry relaxed to `operation.is_none() || value.is_some()`, both pinned assertions still read `[Int]`,
because the frozen template's own node answers `ctx.module.read(local) == None` — there is nothing on the
template side to carry. The class is written later, by the apply's runtime unify (the callee is not
re-born per apply; ruling 2 is about a function's own birth —
[function-normalization](function-normalization.md) §2 and §3), so the gap is the **order in which the
recording reads it** — which is what rulings 1 and 6 together demand be fixed at that read rather than by
materializing more.

