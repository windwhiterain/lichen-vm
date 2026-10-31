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

**Next measurement.** Step through `value_apply`'s **item mapping** for the
declaration expr's attribute slot (not the consumers): the copy's item 0 is a
fresh cell with no origin, which only the clone walk's `wildcard` branch
creates. Find which source node the walk sees as a wildcard there, and why it
is not the static 165. `value_apply` lives in
`crates/lichen-lowlevel/src/function.rs` (`node_apply`'s clone path around
line 840; the wildcard branch freshens `LowValue::Any` cells only).

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

**Measured with a print-only probe (`scalar_kernel_body_diagnostics` in
`crates/lichen-tools/tests/tools/zz_kernel_body_probe.rs`, removed after the
reading).** The scalar variant's program refuses with only "this function's
return is not a value a graph can hand back", and its **direct form**
`out := compute.call one (3)` refuses with `expected Struct, found Int`. The
scalar `jit`/`call` path's types are wrong before any recording begins, so the
expected `unrecordable` arm ("a graph has no node for one") is never reached.
Fix the scalar call path first; the recording will then report the intended
sentence.

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
