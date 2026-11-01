# Handoff: the six tests still red against the SPEC

This note hands the remaining work of the "基于SPEC修复" session to a fresh
session. It records the measured state, each red test's diagnosis, the
hypotheses already excluded, and the one next measurement each family needs.
Read it together with [class-carries-the-call](class-carries-the-call.md),
which carries the full history, and
[examples-panic-diagnosis](examples-panic-diagnosis.md).

## The state

| Suite | Reading |
|---|---|
| `cargo test -p lichen-compute --test lichen_compute` | **234 passed, 6 failed, 1 ignored** |
| `cargo test -p lichen-lowlevel -p lichen-highlevel -p lichen-language --no-fail-fast` | 136 / 0, 69 / 0, 276 / 0, 164 / 0 (1 ignored) |
| `cargo test -p lichen-compute-gpu --no-fail-fast` | 90 / 4 — the four are **pre-existing** (they reproduce at the session's starting `HEAD`; all are SPIR-V validation tests) |
| Examples survey (`zz_examples_panic_probe`) | 9 panicked, 4 drifted, 36 clean, 1 skipped |
| `comments-check --config comments-test.toml . --max-violations 0` | exit 0 |
| `docs/language-spec.md` | **the maintainer's uncommitted edit — never stage, revert, or commit it** |

The session moved the compute suite from 215 / 25 to 234 / 6. It fixed: the
requirement mark for chain-family calls, the buffer-role path, the recording's
own binding conflicts (`230/10`), two maintainer-approved test-data fixes
(`: Out2` → `: Struct`, `.scale @leaf` → `.scale @any : :leaf`), and the
`Mma` element counts together with the freeze's bound-statement evaluation
(`232/8 → 234/6`).

## The six

### 1. `cooperative_product::the_product_is_warp_uniform_not_wider`

**Symptom.** `compute.check(&source_with_requirement("64", …))` now *accepts*
where it must refuse with "expected 64, found 32".

**Diagnosis (most analyzed family).** The requirement is
`(compute.mma …) # 64`; the certificate should be the `mma` prelude's
`# warp` (32). The checker's certificate path (`checker/lambda.rs`,
`certificate` → `stated_attributes` → `attribute_value`) reads the **applied
clone's return expr's attribute slot**, not the declaration expr's.

Measured chain, warp test:

```
slot 836 (value = an attr expr array, origin = 773)
  → attr item 837 (value = None, origin = 768)
    → 768 (value = None, **no origin**)      ← fresh cell, a wildcard freshening
```

The frozen package *does* state the perspective: the frozen exprs carrying
`# warp` are five-item arrays whose attribute item is the static node 165 with
value `USize(32)` (and `# 0` appears as `USize(0)`). So the freeze is not where
the value is lost — the **program's copy** of the declaration's attribute expr
gets a wildcard-freshened cell instead of the frozen value.

**Excluded by measurement.**

- A full freeze sweep (evaluate every package node) → **compute 180/60**;
  body operators run on wildcards and their answers become errors.
- A targeted sweep of every attribute expression's value (~330 nodes) → inert.
- Reading an attribute value through its class (`class_value`) → correct, kept,
  no flip.
- Following a clone's `origin` chain in `attribute_value` → correct, inert.
- `annotation_value_slot` now evaluates the term's value half (it returned
  early on the expr node) → correct, inert.

**Measured since.** The fresh cell is **not** the clone walk's: every node the
walk's `wildcard` branch freshens records an `origin` (`function.rs`, the
`FRESH` probe showed sources in the `mma` function with `origin = None`), while
the certificate's item 0 has none. It is a **checker-created** cell
(`Checker::fresh_cell`, whose `CELL` backtrace names
`instantiate_requirement_value` and `check_app`). And the annotation's own
`pe` is the root: at the **package's** check the `# warp` reference has an
undecided value half (`pe=38 → term 137 → half 117 = None`), while the same
reference in the **program** is decided (`half = USize(1)`) — a top-level
binding of a package script is a runtime slot, so a body's reference to it has
no check-time value, and `instantiate_requirement_value` then freshens the
expression it sees.

**Measured (round 25): the frozen cell is undecided and has no operation.**
Inside the frozen `compute` module (`ModuleKey(1)`), node 117 — the cell behind
the package-side reference — holds `None` and `operation = false`, while the
literal node 165 holds `USize(32)`. So the freeze's eager statement evaluation
decided the *literal*, not the **capture cell**: a body's reference to a
top-level binding is a capture a *call* binds, and no call runs during the
freeze. That is the semantic shape of the blocker, not a lost value.

**Proposed fix direction (needs a ruling).** Materialise a package function's
**top-level captures** at freeze: when a frozen function's body references a
top-level binding, write the statement's decided value into the capture cell
before freezing, so the frozen body states what it captured. The machinery
exists in part — `StaticFunction::open_captures` is computed at freeze
(`StaticModule::fill_open_captures`), and the checker knows which binding each
capture names — so the change is to carry that link into the module rather than
to re-derive it. It is a semantic addition (a frozen body's captured constant
becomes part of the artifact) and should be ruled on before implementing.

### 2 and 3. `perspective_requirements::a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it`

### `perspective_requirements::a_write_returns_the_input_buffer_perspective`

Same mechanism as #1: the attribute a call's certificate states is not read
back from the instantiation. `a_write_returns_the_input_buffer_perspective`'s
control refuses "expected 32, found 0" at `.to` and "(w # 0)". Four routes have
been measured and excluded (see the class note's table): routing the attribute
slots through the extension's `unify_slots` (all positions 226/14; only
both-decided positions 231/9), the instantiate attribute routing, and the
perspective replay. Read [array-attribute-view](array-attribute-view.md) before
touching them.

### 4. `fan_out_merge::under_a_pack_the_device_answers_the_program_and_the_counterfactual_moves`

**Measured.** `run_graph` is entered with the recorded graph and **succeeds** at
the definition pass — two results — returning a plain two-slot array. The
failure is the annotation's *type*: `check_native_call`
(`checker/native_call.rs:19`) mints a **fresh cell** for a native operator's
type, and `$graphrun` declines `low_type`
(`compute.rs`'s `low_type`, `ComputeOperator::GraphRun`), so
`r := (… : Array)` unifies a hole against the denotation and `r[0]`/`r[1]` then
have no container type. Removing the annotation instead refuses
"this array read cannot be checked: the container's type is not decided"; `: Struct`
moves the refusal to the graph op's own return check.

**Needs a ruling.** What a graph run's result type should be — a `Tuple` of its
returns (the value is an array of slots) or the annotation's own denotation.

### 5. `graph_jit::what_a_recorded_body_may_not_reach_for_is_refused_by_name`

**Corrected reading (round 26).** The earlier note here was wrong: the direct
form `out := compute.call one (3)` **compiles cleanly** (`ok=true`). The
`expected Struct, found Int` came from the probe's own `out : Struct`
annotation — a scalar result is `Int` by the `jit` prelude's `r : type_of (k.O)`.
So the scalar `jit`/`call` typing is not broken.

**What the recording does instead, measured.** With `DBG_OPS` on
`unrecordable`'s gate, **no compute operator runs at all** while the scalar
variant records, and the recording's result is the `call` body's **record
expr** (a five-item expr array, layout 1) whose value is undecided: the scalar
kernel's `.O` is the **open template** `_ + 1`, so `r = k.O` (the prelude's
return match) leaves `r` a hole, and the recording refuses at the return
before any dispatch — the expected `unrecordable` sentence ("a graph has no
node for one") is unreachable because the dispatch is never reached.

**Next measurement.** Why the `$call` node is not deep-evaluated inside the
recording: the `call` prelude's body is a block whose value is the record of
its bindings; its field `r` holds the `$call` apply. Instrument
`evaluate_until_the_return_is_nameable`'s deep pass on the record's fields, or
check whether the block's value short-circuits the pass. If the field is
visited, the `unrecordable` arm fires and the test passes with no other
change.

### 6. `compute::a_float_domain_is_permitted_at_every_position_the_walk_reaches`

Parked and documented as such in the class note.

## The examples

9 panicked, 4 drifted, 36 clean. The three named drifts and the eight panics
are in [examples-panic-diagnosis](examples-panic-diagnosis.md).
`worldgen.lichen` reports an answer collision at `63:10` that the previous
reading did not; disabling both `compute.rs` commits of that round still
panics, so it is latent and deserves its own round.

## Rules learned the hard way this session

- **A lowlevel change must be verified against the engine binaries, not only
  against `lichen-compute`.** A name-table comparison by content stayed
  `232/8` on compute and turned three engine tests red
  (`two_struct_values_conflict_on_their_name_tables`, …); it was reverted. The
  language's contract is that a struct value's name table is its
  **declaration's**.
- **A freeze-wide evaluation is not free.** Both the full sweep (180/60) and
  the targeted attribute sweep (inert) were measured.
- **A capability claim is a hypothesis until measured**: `run_graph` was
  assumed to fail; it succeeds, and the gap is the type.
- Verify with `comments-check --max-violations 0` (the default prints only the
  first violation), then `cargo fmt --all`.

## Key commits this session

- `2704a6a8` — a runtime buffer states the declared `Buf` type's identity.
- `677ad523` — struct identity items are the declaration's (marks + clone rule).
- `ef8d5a4d` — the wrapper package registers as linked, statements bound.
- `c05c3466` — a bound statement is evaluated at freeze (**232/8 → 234/6**).
- `8684d33c` — an attribute's value may live at its class.
- `4d46b595` — an annotation's term may hold an undecided value half.

## Round 27: three fixed, three left

`68085942` fixes `graph_jit::what_a_recorded_body_may_not_reach_for_is_refused_by_name`.
The old "deep pass does not visit `$call`" reading is not the mechanism. The
unknown-role placeholder path built a ceiling-width tuple of **bare** `GraphInput`
nodes. The parameter pattern instead carries full `[value, type, attrs…]` element
slots, so the apply's structural unify failed and the recording discarded the
binding error. `ins(0)` consequently had no value and `$call` stopped at its
operand gate before `unrecordable` could run. The fix reads the positional
parameter's real arity and gives every placeholder a complete expression slot.
The whole `graph_jit` module is now **8 passed / 0 failed**. The no-backend
diagnostic keeps the old substring but says "without a backend", because a
recorded input now reaches that later refusal correctly.

The other current findings:

- `compute::a_float_domain_is_permitted_at_every_position_the_walk_reaches` is green
  after correcting its test data. The mixed domain now declares `(Int, Float)`
  instead of requiring an illegal Float-to-Int conversion. The compound case uses
  the SPEC's actual array-instance spelling `[@any : Float, 3][…] : Array`, and its
  assertion names the aggregate's missing scalar encoding rather than the retired
  three-locals model. The Float-to-Int class-refusal test still passes.
- `fan_out_merge::under_a_pack…` is fixed after the maintainer chose **Tuple** for
  graph's multiple returns. The test now uses both `: Tuple` and the SPEC's tuple
  access spelling, `r(0)`/`r(1)`; `r[0]` is an Array read by definition. Each
  result's complete slot now carries its actual type, registered with
  `record_element_type`: Struct for buffers, Int for counts and Float for floats.
  Struct's type is self-referential, not `[TypeStruct, Type]`. The focused test and
  the whole `fan_out_merge` module (**4 passed / 0 failed**) are green.
- For the three perspective/cooperative failures, the SPEC's clone rule is that
  an out-of-scope undecided node is a **reference**. A measurement of an ordinary
  imported `c := 32; f := x => c` answers `32: Int`, so a blanket freeze-time
  capture materialization is not justified and would add semantics rather than
  repair this path. The remaining work belongs to the attribute certificate /
  requirement instantiation path; do not change the Any or capture invariants.

## Round 28: one fixed, two left, and the certificate's real seam

The suite stands at **compute 239 / 2**, and the engine binaries are green
(lowlevel 164 / 0 with one ignored, highlevel 136 / 0, language 276 / 0, and the
artifact subset inside it).

**Fixed: a body's statement states its return's attribute.** The decisive
reading is a pair of inline programs:

| program | before | after |
|---|---|---|
| `g := x => (x # 32); (g 0) # 64` | refused `expected 64, found 32` | unchanged |
| `g := x => {r := x; r = (r # 32)}; (g 0) # 64` | **accepted** | refused `expected 64, found 32` |

So this half was never a package-boundary or freeze problem: a statement's stated
attribute did not reach the value it names at all, because the returned expr's
attribute position holds a **wildcard** (`Any`), and `Any` unifies with
everything and merges with nothing. `Checker::adopt_stated_attributes` now clears
the wildcard the statement's annotated side states a slot for, and the statement's
own unify writes it through. `Module::clear_wildcard` is the one licensed writer:
only a **lone** wildcard may be emptied, so the shared default attribute array
cannot be touched. `attribute_value` also walks the slot's **value half** at every
width; the old walk keyed on a non-zero layout, so a two-item `[value, type]` slot
answered its own array instead of the number.

**Fixed: an apply's attribute refusal names the attribute.** `apply_parameter_check`
compares every position, but the recorded `ApplyError` named the parameter's and
argument's **type** slots, so a perspective mismatch printed
`expected Struct, found Struct`. `ApplyError` now records the failing position
(`failing`) and whether it is an attribute one (`attribute`: `value@0`, `type@1`,
so every later slot is an attribute's); the diagnostic is an `Attribute` mismatch
read requirement-first, and `TypePrinter::attribute_half` spells a slot side's
element 0 (reading through static nodes). Measured: `expected 32, found 1`.

`perspective_requirements::a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it`
also annotated its parameter with a struct **value** (`x : (compute.Buf …)`), the
same mistake the two earlier test-data fixes repaired; the parameter is a
template, so it is spelled `x = (compute.Buf …)`.

**Corrected reading: the write's certificate was never measured as its field.**
[attributes](attributes.md) said `(w # 0)` printing `expected 0, found 32` was
"only possible if the field, not the whole argument's meet, is what the call site
took". It is not: with `.at 0, .value 0` the meet is `gcd(32, 0, 0) = 32`, the
same number. With lane-varying siblings the sentence names the meet instead
(`expected 0, found 1`), so the call site falls back to the argument's whole
provided slot and the field reference is never resolved. `zz_attribute_certificate_probe`
pins both readings.

**The two left turn on one measurement: a program's call into the frozen package
has no callee.** `check_app`'s `callee = self.module.function_apply_function(callee_expr)`
answers `None` for `compute.write` and `compute.mma` (probe print, every call in
both programs), so the whole signature path is skipped: `parameter_expr` is
`None` (no signature requirement), `applied_exprs` stays empty (no certificate),
and the `deferred_requirements` replay is skipped too, because it needs a
`Dynamic(fid)` callee. That is why the certificate is the meet. The next round
starts at `function_apply_function` for an imported package binding — its value
must be `LowValue::Function(AnyFunctionId::Static(..))`, and something between the
freeze's binding table and the field read loses it.

The second remaining defect is independent and smaller: `check_instantiate`'s
decided-field branch calls the **plain** `check_unify` on the whole exprs, so the
perspective's order never runs for an instance's field attribute and
`.to (b.b # 0)` is refused `expected 32, found 0` where `32 | 0` holds (the top
satisfies any requirement). A per-marker delegation at the instantiation was
measured worse at 232/8; a narrower shape — value and type strictly, attributes
through the extension — has not been measured since.

**Landed:** `adopt_stated_attributes`, `clear_wildcard`, the `attribute_value`
walk, `ApplyError`'s failing position, the attribute-mismatch diagnostic and its
renderer, the test-data fix, and the probe.

## Round 29: the callee is one evaluation away, and that is not safe yet

The frozen callee's gap is now measured end to end. `compute.write` and
`compute.mma` resolve to **nothing** at check time, and the reason is narrow: the
callee's expr value half is a **field read that was never evaluated**. Measured
(`DBG_CALLEE` on `check_app`):

```
expr=NodeId(1147) own=Some(array layout 1) half=NodeId(1135) half_own=None
  half_class=None half_op=Some(LowOperator(Index))
```

An `Index` node over the frozen export, no value. Evaluating it answers the
function:

```
after evaluate: Some(Static(StaticFunctionRef { module: ModuleKey(1), index: StaticFunctionId(6) }))
                value=Some(Function(Static(..)))
```

So the missing piece is one `evaluate_node_deep` on the callee's value half before
`function_apply_function`, and four calls in the write program resolved that way.

**It is not safe to apply yet.** Doing exactly that takes the suite to
**233 / 8**: the six `compute::jit_cross_kernel_*` tests turn red with
`expected TypeFunction, found Struct` at the `compute.launch k1 6` argument. That
sentence is the signature check running for the first time on those programs, and
`TypeFunction` is `jit`'s parameter type, not `launch`'s — so the resolved
**index** is not the binding the read named, one position off in the export's
field order. The next round's single subject is that mapping: validate the
resolved function against the frozen binding's *name* (`compute.launch` must be
the frozen `launch`, whose parameter's type is `K`'s struct), then the same
`evaluate` becomes the callee fix, and with it the last two perspective tests have
their certificate (`mma`'s stated `# warp`, `write`'s field reference).

Two cautions for whoever picks it up:

- The evaluation must stay **conditional on the resolve failing**, and it must not
  become a general "evaluate every callee": the six regressions above are what
  happens when a wrongly resolved callee changes which signature is checked.
- The level of the bug is the **read**, not the freeze: `write`'s frozen return
  expr already carries the reference now (round 28's fix), so nothing else gates
  the certificate.
