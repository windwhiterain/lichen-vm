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

## Round 30: the erasure is load-bearing, measured three ways

State unchanged at the tip: **compute 239 / 2**, engine binaries green. This round
was measurements only; nothing in that number moved.

**First, a correction that never landed (the reviewer caught it).** Round 28 said
"two different `Any`s" and "the clone's hole" in its final report, but the note
kept neither, and its `62f11028` message claims `attribute_value` walks the value
half at every width — **`lambda.rs` was never in that commit** and still keys on
`array.layout() != 0`. Corrected here: the wildcard that swallows a statement's
`# 32` is the checker's per-expr fill (`slot_expr`'s `self.any()`), **not** the
function normalization's `Any` (that one is written on the *parameter's value
cell* by `check_lambda`). And an applied clone turns the wildcard into a **fresh
hole** (`static_node_apply`'s wildcard branch allocates an empty node; measured
`slot=570 [none]`, `slot=123 [none]`). `stated_attributes` skips `Any` but not a
hole, so the certificate *is* found — as an empty slot, which names no operation,
so `certificate_field` cannot follow it and the call site reads the argument's
meet. That hole is why clearing has to happen before the clone.

**The erasure is load-bearing at the lowlevel, three ways.**

1. *Replace the wildcard with undecided at build time.* Filling every unstated
   attribute position with a hole instead of `Any` — the SPEC's own not-decided
   form, §Undecided — gives **107 / 134**, with `compute.parallel: a definition
   cycle: NodeId(…) is lowered while it is already being lowered`. Reason: a
   full-width expr unifies **all** its attribute positions, so a hole joins the
   other side's class and unrelated computations become one. Narrowing it to
   **value exprs only** (type values keep the wildcard) measures the same
   **107 / 134**, so type values are not the cause.
2. *Carry the claim on the declaration.* Added `Function::return_attribute`
   (canonical index + slot), recorded by `check_lambda` from the body value's
   constraint slot (a statement's annotated side included) and read first by
   `check_app`, keeping the applied expr as the fallback. Measured **238 / 3** —
   `a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it` regresses to
   `expected Struct, found Struct`. The declaration cannot replace the erasure's
   *lowlevel* effect: the erasure makes the statement's unify write the slot into
   the value's cell, which the applied clone then carries; a checker-side fact is
   not visible to that unify.
3. *Re-point the value's expr instead of emptying its wildcard.* The expr nodes
   are immutable, so `state[actual].term` was re-pointed at a new expr carrying the
   stated slots (no lowlevel writer at all). Measured **238 / 3**, the same test
   regressed — the unify must see the wildcard emptied, not the state re-pointed.

**A real finding on the side: the pardon post-pass is sensitive to slot
registration.** `Checker::build` (`checker.rs` around 828) walks every failed unify
and, when the error's root is in `Checker::attribute_nodes`, looks the attribute up
and **pardons** the error if `ext.is_subtype(root_a, root_b)` holds. Registering an
annotation's constraint slot there — which the declaration route did in order to
read its marker — made the body-requirement's `32` against `1` failure get pardoned
outright, because the direction is `divides(a, b)`. So *any* future work that adds
an `attribute_nodes` entry changes which failures survive. That pass deserves its
own measurement: a too-narrow provider pardoned against a wider requirement is not
what the acceptance table says.

**What this means for the next round.** The statement's claim needs a
representation that is *both* decoupled (does not join a class on unify) *and*
decidable by a statement. `Any` gives the first, a hole gives the second, and the
two measurements above show each alone collapses something. So the change is a
**rule**, not a value: either attribute positions must be skipped by unify when
both sides state nothing, or the checker must carry the claim in a place the
lowlevel unify can write without the erasure. Do not re-run route 1 or route 3
without a new fact — they are measured.

## Round 31: the statement-attribute explanation was wrong; proposal v15 withdrawn

The user's question — "normalize unifies `@any` last, and `x`'s perspective was
already unified with 32, so how could `Any` be written there?" — broke the
explanation this round had been building on. What follows separates what is
**measured** from what was **refuted** from what is **still unresolved**. Proposal
v15 and its README index are deleted; its premise does not hold. `clear_wildcard`
is untouched: only its irrelevance to the statement-return program is shown, not
its global unnecessity.

### Measured facts

- **The normalize's `Any` is not the attribute's.** `check_lambda` writes it to the
  **parameter's value cell**, *after* the body is checked, and only when
  `identity_return && class_value(cell).is_none()`. It cannot reach an attribute
  position.
- **The attribute `Any` is minted at the expr's birth** (`slot_expr`:
  `items.push(self.any())`), and the statement's unify does not overwrite it.
- **The annotated side's expr also carries `Any`.** Measured on
  `g := x => {r := x; r = (r # 32)}`: the expr for `(r # 32)` is five items whose
  positions 2–4 are `NodeId(135/136/137)`, all `own=Some(LowValue(Any))`. The
  settled slot exists only in `Checker::state[annotated].attr` (`Some(NodeId(448))`
  for that program).
- **`adopt_stated_attributes` is not what makes the statement program refuse.**
  With the call removed, `(g 0) # 64` still refuses `expected 64, found 32`.
- **It is load-bearing elsewhere**: removing it gives compute **238/3**, the extra
  failure being `perspective_requirements::a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it`.
- Still-valid readings from rounds 30: unstated positions as undecided gives
  **107/134** (broad, and value-exprs-only); the declaration route
  (`Function::return_attribute`) and the re-point route both give **238/3**.
- Base suite unchanged: compute **240/2** (240 = 239 + the probe's passing test),
  long-level 164/0, highlevel 136/0, language 276/0; comments and fmt gates clean.

### Refuted — do not repeat these

- "The wildcard swallows the statement's `# 32`, and clearing it lets the
  statement's unify write the slot in." **Refuted**: removing the clearing changes
  nothing for that program.
- "The annotated side carries the settled slot; only the actual side is a
  wildcard." **Refuted**: both carry `Any` (measured above).
- "The erasure is what the statement case needs." **Refuted** for the statement
  case; it is needed for one other test, whose path is not yet identified.

### Unresolved — the next two measurements

1. **What carries `32` to the call's certificate.** The certificate slot exists
   and `attribute_value` yields 32 on the applied expr's attribute position, but
   the route is untraced. Hypothesis: `check_ann`'s provider unify
   (`ext.unify_slots(provider, slot)`) puts the provider's class in the same class
   as the constraint slot, which holds 32, and the returned expr's position shares
   that class. Read `state[.].attr`, the slot's class, and `attribute_value` at the
   certificate branch to pin it.
2. **Why `adopt_stated_attributes` is load-bearing for
   `a_bodys_requirement_refuses_…`.** Identify which unify needs the cleared
   wildcard, and whether that need is the same defect or a different one.

No proposal is written until both are measured. The rule question ("an attribute
position's unify rule") may still be real, but v15's premise — the erasure being
what carries a statement's claim — is dead.

## Round 32: the wildcard rule, read from the code

Round 31 left "what carries 32 to the call's certificate" open. It is not the
erasure, and it is not a `class` merge of the attribute position. The rule is in
the lowlevel:

> `unify_inner` (`crates/lichen-lowlevel/src/equality.rs`, "Directional wildcard:
> only a left `Any` may join an undecided right class"): if **either** side is the
> wildcard, the unify succeeds and does nothing at all — except one case: a **left**
> `Any` against an **undecided right** side **joins their classes**
> (`add_equality(ra, rb)`).

So the SPEC's "Only merge if right side is undecided" (`language-spec.md` §Any) is
implemented **directionally**: a right-hand `Any` never acts, and a left-hand `Any`
against a *decided* right side writes nothing.

Measured consequences (with instrumentation, then reverted):

- With `adopt_stated_attributes` **removed**, the statement program
  `g := x => {r := x; r = (r # 32)}` / `(g 0) # 64` still refuses
  `expected 64, found 32`. So the erasure is **not** the statement-attribute
  mechanism — this confirms round 31's refutation by a second route.
- The applied expr's Perspective position for that call holds an expression array
  (five items, layout 1) whose element 0 is `USize(32)`; `stated_attributes` finds
  it, `attribute_value` returns 32, and the call carries `# 32`. Where the array
  enters the returned expr is still untraced, so the *write* that puts it there is
  open; what is settled is that it is not the erasure.
- Removing the erasure costs exactly one test: `238/3`, the extra failure being
  `perspective_requirements::a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it`.
  That is the erasure's **only** measured benefit; its path is still unidentified.

**Therefore the two red tests do not depend on the erasure at all.** Both come from
the frozen boundary, measured in round 29: a program's call into the imported
compute package has `callee = None`, so the whole signature path (parameter
requirement, `applied_exprs`, certificate) is skipped, and the certificate falls
back to the argument's provided slot. The second test's positive case additionally
needs the instantiation to consult the attribute's order.

Next work is therefore on those two, and the erasure is parked: not needed by the
statement family, needed by one unrelated test, and neither removed nor extended
until that test's path is measured.

## Round 33: the round-31 refutation was itself wrong, and two frozen readings

**Correcting round 31.** Its three "refuted" claims were based on a patch that
never applied: the `adopt_stated_attributes` call was edited with a scripted
replace that silently did not match, so the "removal" measurement ran the
unchanged binary. Re-measured with the edit verified against `git diff`:

- With adoption **disabled**, `g := x => {r := x; r = (r # 32)}` / `(g 0) # 64` is
  **admitted** (`ok=true`).
- With adoption **enabled** (the tip), it is refused `expected 64, found 32`.

So round 28's original account was right and round 31's refutation of it was
wrong: the erasure *is* what carries a statement's claim into the returned value's
expression — it empties the wildcard so the statement's unify takes the annotated
slot, and the applied clone then carries it.

- Round 31's other reading — "the annotated side's expression also carries `Any`" —
  is **invalid for this program**: that dump belongs to another compilation. Do not
  cite it.
- Round 31's "what carries 32 is unresolved" stands, but its hypothesis space
  narrows to: the erasure empties the position, the statement's unify writes the
  annotated slot, and the clone carries it. Round 32's directional-wildcard rule
  (`unify_inner`: only a **left** `Any` against an **undecided right** joins; a
  right `Any` does nothing) is the rule that makes the emptied position writable.

**A pure callee resolver resolves the frozen binding and is safe, but the
signature it hands out is malformed.** `Module::static_function_of_callee` applied
to the callee's **value half** resolves a program's call into the frozen package:
`compute.launch` resolves to `Static(ModuleKey(1), StaticFunctionId(10))`, with no
evaluation and none of the run-stamp side effects the evaluate route had. Measured
suite effect: **239/3**, the six `compute::jit_cross_kernel_*` tests restored to
their earlier failure (`expected TypeFunction, found Struct` at
`compute.launch k1 6`). Reading the materialised parameter expr of that resolved
function:

- its five items are `[value array, type array, hole, hole, hole]`;
- the **type slot** holds an array of **layout 1** whose element 0 is
  `TypeValue(TypeStruct)` and whose element 1 is another expr array.

A type value is `[shape, kind]` — two items, no attribute tail — so a *type slot*
carrying a full-width layout-1 array is a slot/expr confusion in the frozen
signature, and it is the first thing to check for the `jit_cross_kernel_*` family.
The resolver change was reverted, not landed: it must not go in while it turns six
tests red.

**State**: tip `f5cb630b`; compute **240/2**, lowlevel 164/0, highlevel 136/0,
language 276/0; comments and fmt gates clean. The two red tests are still the warp
certificate and the write perspective, both gated on the frozen callee path.

## Round 34: the import projection, read end to end

All readings below are measured with prints on the tip and then reverted; the tree
is clean at `2ad755f1` with compute **240/2**.

**The export array is the bindings alone.** `compute.lichen`'s export is a
**30-item** array with **no name-table/id header**, so `compute.jit` is at position
**10** and `compute.launch` at position **11** (source order). The read chain is
`Index(<call result>, 0)` at the top and, at depth 2, `Index(<export>, k)` with the
*physical* `k`: measured `position=10 width=30` and `position=11 width=30`. So
`Module::static_function_of_callee` receives the right position — its offsets are
**not** the defect it was suspected of.

**The artifact's binding value names the function.** The export item the resolver
reads is a `Static` node holding `LowValue::Function(AnyFunctionId::Static(..))`:
export[10] (`jit`) → `StaticFunctionId(7)`, export[11] (`launch`) →
`StaticFunctionId(10)`. The offsets are the nested lambdas (each `f => b => …`
creates a closure entry), so `jit`=7 is consistent with `type_of`=0,
`perspective_of`=1, `A`=2, `P`=3, `S`=4 (+its inner), `Buf`=6.

**The frozen return type is what fails the guard.** For the outer apply
`(compute.launch k1) 6`, the callee is the inner call, and `is_function_type` is
asked about the inner call's declared return type. Read as a frozen ref (see
below), that type's element 0 is `TypeValue(TypeStruct)` — the *struct* marker —
which is why the guard reports `expected TypeFunction, found Struct`. Two readings
fit and are **not yet separated**: either static function 10 is not `launch`, or
`launch`'s frozen return type is wrong (a body that returns a lambda should state
the function marker).

**A materialised copy can never satisfy a shape predicate that reads identity.**
`shape::is_universe_any` compares a **Dynamic** id to the universe **by equality
class**, and accepts a **Static** id by content (`shape::is_static_universe`:
element 0 is the type marker and element 1 is *the same* static node). The frozen
signature is materialised as *copies*, so its universe is a different class: the
guard fired on a copied type whose element 0 was `TypeValue(TypeFunction)` — a
correct-looking type that the predicate still rejected. Measured fix direction:
reading the frozen ref instead of a copy (`Module::static_return_type_item`, a new
reader that returns the frozen return expr's type item without materialising)
moved the diagnostic's path from `Many([Type, Value])` to `One(Type)` but did not
fix the family, because the type it then reads is the Struct one above. The reader
is reverted, not landed.

**Next measurement (single subject)**: print the frozen function table's
parameter and return type per index, and identify each index by shape (a function
parameter is the function marker; `launch`'s `k = K` is the struct marker). That
separates "wrong ref" from "wrong frozen return type", and only then is a fix
landable.

## Round 35: the resolver is right; the vocabulary's callable `K` is the wall

Measured with the pure resolver (`static_function_of_callee` on the callee's value
half) plus prints, then reverted. The tree is clean at `a7e92cc6`, compute 240/2.

**The mapping is correct.** The export is a 30-item array of bindings, so
`compute.jit` is at position 10 and `compute.launch` at 11; the artifact's binding
value names the function, and the resolved refs and their declared return types are
right:

| call | resolved | declared return type, element 0 |
|---|---|---|
| `compute.jit (…)` (twice) | `StaticFunctionId(7)` | `TypeValue(TypeStruct)` — `jit` returns `K{…}`, a struct ✓ |
| `compute.launch` | `StaticFunctionId(10)` | `TypeValue(TypeFunction)` — `launch` returns `a => …` ✓ |

**So the six `jit_cross_kernel_*` failures are not a resolver defect.** With the
resolver, `compute.jit`'s result type becomes known (`K`, a struct), and the guard
in `check_app` — "a decided non-function callee is refused" — then fires on
`k1 := compute.jit ((x : Int) => k0 x)`, whose body applies the jitted value
`k0` to `x`. The program is legitimate in this vocabulary: `K{…}` is exactly what
`compute.launch` and `compute.call` apply, so a `K` value **is callable** even
though its type is a struct. The baseline passes only because the callee's type is
undecided (`compute.jit` does not resolve), so `type_is_concrete` is false and the
guard never runs — an accident, not a guarantee.

**Consequence for the plan.** The resolver is a prerequisite for both red tests
(the frozen certificate path), and it cannot land before the checker learns that a
vocabulary may declare a struct-typed value callable. That is a `NativeCall`/
extension question (`checker/native_call.rs`), not a shape question: the guard must
consult the vocabulary rather than infer callability from the `Function` template
alone.

Also measured and reverted as not needed yet: `shape::is_universe_any` recognises a
**static** universe by content (`is_static_universe`: type marker in the value slot,
the same static node in the type slot) but a **dynamic** one only by equality class,
so a materialised copy of the universe can never satisfy the function-type
predicate. A structural fallback (a self-describing `[TypeType, Type]` node) is
faithful to SPEC §Type and measured neutral on the suite, but nothing exercised it
once the guard above turned out to be the real blocker, so it is not landed.

## Round 36: the frozen callee resolves — compute is 241 / 1

**Fixed** (`b34828fa`): a program's call into an imported package now has a callee.
`Module::static_function_of_callee` — which existed for exactly this projection and
had no caller — resolves the callee's value half without evaluating anything, so
the signature path (parameter requirement, `applied_exprs`, certificate) runs for
frozen calls. Two consequences had to be handled in the same change, both measured:

- a **resolved** callee is a function whatever its reference type says: a frozen
  export member reads as the export's own struct, so `is_function_type` answered
  false and the type-side apply was skipped;
- a frozen callee states its **signature**, not the call's own type cell. Pushing
  `compute.jit`'s frozen result (`K`, a struct) into the call's type made the
  checker refuse the legitimate cross-kernel call `k0 x`, which the compute layer
  applies by fusing — the six `jit_cross_kernel_*` tests are that measurement.

Measured: compute **240/2 → 241/1**. The cooperative product's warp test now refuses
`# 64` with `expected 64, found 32` (the certificate reads `mma`'s stated `# warp`),
and the six `jit_cross_kernel_*` tests are green. lowlevel 164/0, highlevel 136/0,
language 276/0, gpu 90/4 (the four SPIR-V validation failures are the pre-existing
ones). Comments and fmt gates clean.

**The last red test, `a_write_returns_the_input_buffer_perspective`, is now two
precise defects** (`zz_attribute_certificate_probe` prints both programs):

| program | diagnostic | meaning |
|---|---|---|
| positive, `.to (b.b # 0)` + `(w # 0)` | `[0] expected 32, found 0`, kind `Annotation`, at the instantiation; `[1] expected 0, found 32`, kind `Attribute`, at `(w # 0)` | `[0]` is the instantiation's field unify comparing the numbers without the order; `[1]` is the certificate reading the **declared** `.to` requirement (`# warp` = 32) instead of the caller's provided slot (`# 0`) |
| negative, `.to (b.b # compute.warp)` | `[0] expected 0, found 32` | the expected sentence, and it cannot distinguish the two readings because declaration and caller agree here |

So the remaining work is exactly: (a) the instantiation's attribute positions through
the attribute's order, and (b) `certificate_field` resolving the field to the
**caller's** field expression, not the declaration's requirement slot. Both are
narrow and independent; `[0]` is what a previous round's `unify_field` attempt
addressed, and its regression (`an_imported_requirement_judges_like_an_inline_one`)
came from the helper not reading a **static** field expr — worth re-checking now
that frozen signatures resolve.

**The instantiation's attribute order: a second attempt, and its constraint.**
Written and measured, then reverted: split the field unify into value/type (strict)
and attribute positions (through the extension, reading each side's *carried* slot
by own value or class, so a materialised frozen field is read too). Result:
**239/2** — `an_imported_requirement_judges_like_an_inline_one` lost its attribute
refusal and reported `expected Struct, found Struct` instead of `expected 32, found
1`. So for the imported case the **strict value/type split is what breaks it**, not
the attribute reader: the declared field's own type slot is the thing the whole-expr
unify was enforcing the attribute order through. Any further attempt must keep the
whole-expr unify for value+type and take the attribute order *without* re-implementing
the type comparison — e.g. run the whole-expr unify and retain only an attribute
failure whose order holds, rather than splitting the positions.

## Round 37: why the write certificate is a hole — a class-carried call, dropped at the freeze

The last red test's second defect is now explained from the code and measured
(`DBG_WRITE` on the certificate branch, reverted).

**`certificate_field` never runs.** For `compute.write`, the certificate slot is
found (`stated_attributes(applied_expr)` returns the Perspective item) but
`attribute_value(slot)` answers `None`, so the hole branch runs — and
`certificate_field` returns `None` immediately because **the slot has no
operation**: `node_operation(certificate)` is `None`. The call then falls back to
`provided_slot(argument)`, the whole argument's meet, which is `32` in the positive
program (the declared `# warp`), where the field would have given `0`.

**Why the reference lost its operation.** In the prelude, `write`'s body is
`r = (Buf _) # (perspective_of x.to)`; the statement's adoption empties `r`'s
wildcard and the statement's unify then **merges the item's class** with the
annotation's reference node — the operation lives on the *reference*, and the item
is a member of that class. The freeze copies each node's **own** value and operation
(`freeze.rs`), and `static_node_apply` clones a member with its own operation — so
the frozen item is a value-less, operation-less node, and the per-call clone is a
bare hole. The class link that carries the call is not reproduced.

**Resolving the member to its class representative is measured wrong.** Doing it at
the top of `static_node_apply` (undecided && no operation → representative) makes
several tests **overflow the stack**: representatives resolve to each other through
the class list, so the substitution recurses. A fix must carry **the class's
operation** deterministically — e.g. copy the representative's operation onto the
frozen member at freeze, or keep a per-class operation lookup that cannot cycle —
not substitute one member for another.

**State**: tip `d7c49ccf` + this note; compute **240/1**, the one red being the write
perspective with its two defects (the instantiation's attribute order, round 36's
second attempt recorded; and this certificate hole). Engines 164/0, 136/0, 276/0;
gpu's four SPIR-V failures pre-existing; comments and fmt clean.

**And carrying it universally at the freeze is too broad (measured).** A
`Module::class_operation` reader plus using it in `freeze.rs` (an undecided member
takes its class's operation) gives **229 / 12**: every undecided member then looks
like a computation, so `jit_conditional_then`/`else`, `jit_inline_lichen_function`,
`jit_closes_over_constant`, `a_refused_call_argument_shape_says_why` and
`a_float_argument_to_an_int_parameter_is_refused_by_class` and six more break.
Reverted. Two measured limits for whoever takes it next: substituting a class
representative recurses (round 37), and copying the class's operation onto every
member over-claims. What is left is a *narrow* carrier — the operation the
certificate needs is the one on the return expr's attribute **reference**, so the
carrier must be the class's operation **for the members of a returned expr's
attribute position**, not for a class in general.

## Round 38: the reference is not in the frozen graph at all

Two more attempts, both measured and reverted; the state is unchanged (compute
240/1, tree clean).

**A frozen-side carrier cannot work.** `class_operation_owner` — the first class
member with an operation, used only for a return expr's attribute positions in
`static_return_type_apply` — finds **no owner**: measured `own_op=false
undecided=true owner=None` for the write family's attribute items. The reason is
structural: the adoption empties the actual's wildcard and the statement's unify
merges the item's **class** with the reference node, whose value is undecided and
whose operation is the only content. The freeze's root set is built from values and
operations, so a value-less class member drags **no** reference into the arena — the
reference node is simply not in the frozen module, and no lookup can find it.

**Dropping `reference_slot` from `check_ann`'s certificate selection is also wrong.**
With `applied_attribute_slot`/`annotation_value_slot` only, both write programs
report `expected 0, found 1` — the certificate falls back to the argument's meet,
because `certificate_field` receives the (still unresolved) reference through the
applied clone's attribute item, which is the bare hole again. So `reference_slot`'s
declared field slot (32) is what the negative program happens to match, and the meet
is what the fallback gives; neither is the caller's field.

**Conclusion, and the direction it fixes.** The field case cannot be served from a
frozen return expr whose attribute position is a value-less class member. The
certificate for such a callee must come from the **checker side** — the declaration's
stored slot, replayed per call — which means the artifact has to carry a function's
stated return attribute (`Function::return_attribute`, `StaticFunction`'s mirror and
the format bump) rather than the applied clone's expression. That is the one route
left for the last red test's certificate half; its order-gate half keeps round 36's
constraint (retain the whole-expr unify, do not re-implement the comparison).

## Round 39: what the annotation actually settles for `perspective_of x.to`

Three attempts this round, all reverted; the state is unchanged (compute 240/1).

1. **Spell `write`'s return as the annotated expression** (`{…; r = (Buf _) # …; (r # …)}`)
   — measured: no change, both programs still report the declared `32` on the
   positive case. So the spelling of the prelude is not the lever.
2. **`certificate_field` reads the argument expr's value half before its operation**
   (the reference's argument is an expr, so its `Index` lives in element 0) — inert
   on its own.
3. **A computed attribute's slot is its own expression, not the applied return slot**
   (`check_ann`: `Apply`/`NativeCall` → `annotation_value_slot`; otherwise
   `reference_slot` → `applied_attribute_slot` → `annotation_value_slot`) — with (2)
   the certificate becomes the argument's meet (`expected 0, found 1`) instead of
   the declared `32`; neither is the caller's field.

**The blocking reading.** With (3), the slot the annotation settles is an **expr
array (layout 1) whose value half is a bare, operation-less cell** — measured
`slot=… class=Some(Array layout 1)` and `slot_value_node(slot)` a node with
`own=None class=None value_op=None`. `certificate_field` requires
`node_operation(certificate)` and gets `None`, so it returns `None` and the call
site takes the meet. So the reference expression (`perspective_of x.to`) is not what
the annotation's slot holds in either spelling; the only two candidates measured
are the *declared field slot* (32) and the *applied return slot* (a hole).

**Next measurement (single subject)**: print the annotation value's `ExprKind` by
name (not its discriminant) and `state[pe].term`'s own value in `check_ann` for the
write body, to identify which expression the slot comes from; the round established
that none of the three selection routes yields a node with a walkable operation.

## Round 40: the declaration carrier is in, and the exact last gap

The user approved the artifact route; the plumbing is landed (`4f7c8dba`), neutral on
every suite (compute 240/1, lowlevel 164/0, highlevel 136/0, language 276/0 — the
artifact round-trip is covered by the persist tests at format **31**):

- `Function::return_attribute: Option<(u32, NodeId)>`, recorded by `check_lambda` from
  the body's value; `check_ann` records the slot's **marker** in
  `Checker::attribute_markers` (its `own_tail` is empty — measured round 32 — so the
  marker has to come from the loop variable at the `constraint_slot` assignment), and
  `check_unify_expr` inherits both onto a statement.
- `StaticFunction::return_attribute: Option<(u32, LocalNodeId)>`, filled by the freeze
  through `node_map`; written and read by the artifact (tag + u32 + node, version 31).
- `Module::materialize_static_return_attribute` copies the frozen slot per call;
  `check_app` prefers the declaration's claim over the applied clone's expression, and
  the hole branch looks for a reference on the **slot** before its value half.

**The last gap, measured.** The claim reaches *a* frozen function but not the one the
call resolves to: `DBG record` shows `Perspective` markers recorded for dynamic
functions **22, 23, 24**, and the frozen table holds `return_attribute=Some` for
static **21**; the probe's `compute.write` call resolves to
`Static(ModuleKey(1), StaticFunctionId(28))`, whose entry is `None`. So
`declared_certificate` is `None` and the certificate still comes from the applied
clone (the declared `32`).

**Next measurement (single subject)**: the static index the export's `write` binding
names, against the index that carries the claim. If they differ, the freeze's
function mapping (`function_map`: dynamic id → static position) and the recorded
`return_attribute` disagree — i.e. the claim is attached to the wrong static entry —
and the fix belongs to that mapping, not to the carrier.

## Round 41: the mapping is N -> N-1, and statement bodies still record nothing

Measured (`DBG_MAP` at the freeze, reverted):

- **The static function table is a traversal order**: dynamic id `N` maps to static
  `N-1` for every entry (dynamic 22..24 → static 21..23). So a static index equals no
  dynamic id, and nothing may compare them.
- The claims reach **static 21** (and, before this round's change, only that one):
  dynamic 22..24 are the annotation-bodied functions (`range`/`row`/`col`, each
  `… # 1`), which the existing `state[r#return].attr` recording already covered.
- `compute.write` resolves to `Static(ModuleKey(1), StaticFunctionId(28))` — dynamic
  **29** — whose entry is **None**. So a **statement** body never records its claim.

**Landed anyway** (`54d65e22`): `Checker::body_claims` records each function's stated
statement claim (set where `check_unify_expr` inherits it, keyed by the current
function so a nested lambda cannot clobber its parent), and `check_lambda` falls back
to it. Neutral on every suite (compute 240/1, lowlevel 164/0, highlevel 136/0,
language 276/0). It did **not** give `write` a claim, so the inheritance itself is
not firing for that body.

**Next measurement (single subject)**: for the prelude's `write` body, print the
body value expr's `ExprKind` **by name**, plus `state[actual].attr` and
`state[expected].attr` at the statement — the earlier discriminant readings were not
identified, and the inheritance may be skipped because the actual already states an
attribute or because the annotation's slot is unset.

## Round 42: no function body is a Unify — the statement claim never reaches the recorder

Measured with `DBG_W` on `check_unify_expr` and the body-value kind (reverted):

- Across the prelude and the probe's programs, **48 `check_lambda` calls: only three
  bodies are annotations** (dynamic 22/23/24 — `range`/`row`/`col` — each recording a
  `Perspective` marker), and **no body value is a `Unify`, `Record` or `Apply`**; the
  rest are "other" kinds.
- `check_unify_expr`'s statement dump shows **no statement whose expected side is an
  annotation**: the only two confirmed shapes are `parameter = apply` (attr `None` on
  both sides) and `native = named-field`.

So `r = (Buf _) # p` is not compiled as a unify whose expected side is the
annotation, and it is not the body's value either — which is why neither
`state[r#return].attr` nor `body_claims` ever gets a claim for `write`, while the
annotation-bodied `range`/`row`/`col` do.

**Landed** (`33254a17`, neutral at compute 240/1 with the engines green): a clone of a
function keeps its template's `return_attribute` (a clone is the same function), and
the slot a claim names is a root of the frozen closure (a claim about the return is
not part of the returned value's graph). Both are needed for the carrier to be
correct even before the last link works.

**Next measurement (single subject)**: what `r = (Buf _) # p` *is* in the IR. Print,
for the `write` body only, every child expr's kind by name in order (the block's
statements), so the shape of a source `name = expr # attr` line is identified instead
of inferred. Then the recorder can be attached where that shape's claim is decided.

## Round 43: the instantiation's order is fixed; the claim still reads 32

Committed `434fc907`. Compute **240/1** — the write test is down to **one**
diagnostic: `(w # 0)` gets `expected 0, found 32`, i.e. the certificate reading the
declared `.to` requirement. Engines 136/0, 164/0, 276/0; comments and fmt clean.

**Defect (a) is fixed, and the route matters.** The failing site was not a
`unify_slots` call — it is the instantiation's *undecided* branch per-position
`check_unify` (`structs.rs`, `check_instantiation`), which compared the provided
`# 0` against the stated `# warp` by equality and reported `kind=Annotation` at the
argument's attribute slot. Two measured constraints:

- **Check-only, never a unify.** Routing the pair through `AttrExt::unify_slots`
  (positionally, or guardedly by each side's layout) gives **235 / 6**:
  `fan_out_merge`/`fuse_licence` then fail at the *apply* with
  `expected 32, found 32` — the slot unify joins the argument's slot class with the
  frozen requirement's, and the apply-time parameter check reads the joined class's
  array value where a number belongs. `AttrExt::is_subtype` on the two numbers
  writes nothing and is green.
- **The frozen side must be read as a number, and only the number.** `live_slot_value`
  opens exactly one level of a *live* slot (its element 0 is the value, whatever
  shape that value has) and unwraps the *static* chain only, re-storing the number
  as this module's node. A single "descend while the value is an array" loop walks
  into a buffer's struct value and answers an array (`expected 32, found 32` again);
  the positional item index is not the canonical attribute index either, so reading
  the pair through `slot_attribute` by position does not apply here.

**Defect (b) is unchanged in outcome but its carrier now exists.** Measured with the
probe and `DBG_C`/`DBG_F` (reverted): the call site finds
`declared=Some((0, slot))` and `claims=Some((0, slot))` with
`params=Some(clone)`, `certificate_field` is entered for the claim, and the claim
slot it receives still has **no operation** — round 37's finding stands after the
claim is materialised through the apply's own remap (`Module::applied_claims`) and
after recording the class's operation owner (`Module::class_operation_owner`). So
the answer stays the declared 32.

What the carrier needs next, precisely: the claim node whose operation is
`perspective_of x.to` must (1) exist in the frozen module, (2) be the node
`StaticFunction::return_attribute` names, and (3) survive
`clone_static_signature_node` through the apply's remap with its operation. The
round-37/38 traps still apply: substituting a class representative recurses, and
copying a class's operation onto every member over-claims (229/12). **A capability
claim is a hypothesis until measured** — round 43 measured that neither the
per-apply claim nor the operation-owner lookup supplies (1).

## Round 44: the claim carries the operation, and the next refusal is nested

Committed `9d6d2c20`. Compute still **240/1**, engines 136/0, 164/0, 276/0.

**The carrier works.** A body whose return states a **hole** now claims the
*reference* its annotation named: `check_ann` records each computed attribute value's
`(marker, slot, term)` per function, and `check_lambda` takes the last one when
`state[return].attr` holds no class value. Measured at the call site: the claim
materialised through the apply's remap is a node with `op=Some(Apply)`, and
`certificate_field` reaches `field=Some(ExprId(114))` — the caller's `.to` field.
Two details were needed:

- the reference's **argument is an expr** in a frozen copy, so the field read's
  `Index` operation is the argument's *value half* (`slot_value_node`), not the
  argument node's own operation;
- the claim must be recorded as the computation's **term**, not as the slot the
  annotation filled: the slot is the same node (`NodeId(3783)`) the block's return
  already names, so it carries no operation. Measured with the slot-only variant:
  `certificate_field` returns `None` and the answer stays 32.

**The refusal moved, and it is one defect further out.** The positive program now
fails at the write *call* (`span (16, 22)`) with `expected Struct, found Struct` and
`value_a=USize(32)`, `value_b=USize(0)`: the apply's parameter check descends into
the record and compares `Write.to`'s slot against the caller's `# 0` without the
order. Measured in the pardon pass (`DBG_P`, reverted): the surviving
`unify_errors` entry has `root_a`/`root_b` = the two **records** (both `layout 0`),
while the clashing pair is the error's own `a`/`b` — the record fields' slots.
`Checker::attribute_nodes` therefore finds no marker (13 entries, neither root
among them), so `is_subtype` is never consulted.

**The next fix, precisely** (not yet attempted): pardon on the error's **innermost**
pair (`UnifyError::a`/`b`) as well as its roots, and make the record fields' attribute
slots known — registering each template field's attribute slot in
`check_instantiation` (the position loop already computes `actual`/`expected` there).
Both were measured to be necessary but not sufficient alone: registering the defaults
or the compared slots without the innermost-pair lookup changed nothing (`known`
stayed 13).

**Traps reconfirmed**: `AttrExt::unify_slots` at the instantiation (235/6, apply
errors `expected 32, found 32`), and a "descend while the value is an array" reader
that walks into a buffer's struct value.

## Round 45: the innermost pair is not in the error

Committed `60cc18e3`. Compute **240/1**; engines 136/0, 164/0, 276/0; comments and
fmt clean.

The pardon pass now tries the error's clashing pair (`UnifyError::a`/`b`) before its
roots, and `check_instantiation` registers each field's attribute slot on **both**
sides, so a descended attribute failure is recognisable. Measured, and insufficient
on its own: the array compare records the failure at the **frames**, not at the items —
for the write call's refused compare `error.a == error.root_a` and both are the two
records (`layout 0`), so the pair the order would need (32 against 0) never appears in
the error.

**So the next change is in the lowlevel**, and it is small: where the array-item unify
records a conflict (`equality.rs::record_error`, whose `ra`/`rb` are the framed
operands), record the **item** pair that clashed — the two item nodes at the failing
position — keeping `root_a`/`root_b` as they are. With that, the pardon's pair-first
lookup finds the registered attribute slots and `is_subtype(declared, value)` decides
`0` under `# warp` for the caller that provided it.

Measured along the way: registering the module's **default** attribute node as that
attribute's (so a compare against a default is judged by the order) changes nothing by
itself, and was reverted; `AttrExt::unify_slots` at the instantiation is still 235/6.
