# The Any removal: function templates replace the wildcard

> Status: **in progress.** The migration plan and decisions for the SPEC edit
> (dev `174379fb`) that deletes `@any`/`Any` and makes polymorphism entirely
> function templates. The measurements it builds on are
> [ideal-api-review](ideal-api-review.md) (the new forms refused),
> [one-answer-per-class](one-answer-per-class.md) (the collide and the strict
> construction's `raw none`), and [value-templates](value-templates.md) (the
> direct-template model this migration supersedes).

## What the SPEC changed

- `atom` no longer lists `@any`; both `### Any` sections (Subtype, Clone) are
  deleted, including the auto sugar `'x -> {…; @any = x;}`.
- New sugar: `a <: b` => `a = b _`. Every compound construction (array element,
  struct destructuring, table key/value, set element) now uses `<:` where it
  used `=`.
- The Ideal API is rewritten on **function-typed types**: `Div := _ -> {…}`,
  `div := 'x = Div _ -> …`, nested `B := _ -> {.a A _, .'z: String}`. The old
  wildcard-generic form (4) is gone; form 4 is now the function generic.
- §Struct: a struct type declaration is `s := _ -> {…}`; the instance
  destructures with `let _{…} <: s`.
- §Clone: an undecided node is cloned when **the outermost scope among its
  class members** is or inside the applying function's scope; cloned nodes live
  in the scope where the apply happens; "Clone more nodes should make not change
  to the language behaviour".

## The model reversal

[value-templates](value-templates.md) records the current model: a template is a
**direct compound value** with `@any` open slots, instantiated per use by the
checker's `instantiate_requirement`, never applied; `<:` is not a spelling.
The new SPEC reverses each of those rulings: a template is a **function**
(`_ -> compound`), freshness comes from **application** (the apply's clone), and
`<:` is the sugar that applies the template. The wildcard value `Any` — whose
unify rules (always succeed, recurse into arrays, directional merge) propagated
it into classes and compounds, the contamination this migration removes — has no
replacement value: an open position is a plain undecided cell (`_`, no value),
and its per-use freshness is structural (clone), not a special value.

[one-answer-per-class](one-answer-per-class.md) traced why the direct model
could not be repaired in place: with the check-time copies removed the collide
disappears but the launch's operand stays undecided, because "the apply's clone
covers that whole expression — the cloned parameter's cells are what the
argument unifies with". The function-template model is that sentence made
syntax: `'x = Div _` is an application, so the clone covers it by construction.

## Decisions

- **M1 — `@any` becomes `_`.** Every wildcard value position is a placeholder
  (undecided cell). `LowValue::Any` is deleted; an open cell is a node with no
  value. All Any special cases in unify/evaluation/clone/codec go with it.
- **M2 — `<:` is surface sugar** `a = b _`, desugared to the existing
  unify/apply/placeholder nodes. Its right side is a template function; compound
  types store function templates (Ideal API form 1: `[_ -> (_: Int), 3]`).
- **M3 — `_ -> e` declares one hole parameter.** The SPEC's `_ -> {…}` is a
  function of one undecided parameter, not a zero-parameter thunk. This
  supersedes [placeholder-anywhere](placeholder-anywhere.md)'s "declares no
  parameter" reading; that note is updated with the migration.
- **M4 — a struct type is a function; an instance is fresh construction.**
  `s := _ -> {…}`; `s{…}` builds the SPEC's `v` with fresh field cells; a type
  used as an annotation is applied (`Div _`). The generic `Struct` template
  unifies with a concrete struct type value in the annotation check (the
  `expected @any, found Struct` finding).
- **M5 — the clone rule is the SPEC's.** Outermost scope among class members;
  clones land in the apply's scope; over-cloning is behaviour-preserving. The
  measured compute regression of the class-wide verdict
  ([value-templates](value-templates.md), commit `14285cd8`, 112 → 67) is the
  reason the rule is read in the cloning direction: `strict-clean`'s `5fcc181a`
  implements the outermost-member verdict and is the starting point.
  **Superseded 2026-11-16:** the SPEC's clone selection is now the polymorphic
  flag ([polymorphic-flag](polymorphic-flag.md)); the outermost-member verdict
  is the pre-refactor rule, and `5fcc181a` is the starting point the migration
  replaces, not the target.
- **M6 — the auto `@any = x` sugar is deleted.** An unannotated parameter part
  stays undecided; nothing guarantees it "never decides" — per-application
  freshness replaces that guarantee.
- **M7 — requirement instantiation retires.** The checker's
  `instantiate_requirement` copies are replaced by application: struct instance
  → fresh `v`; array/table/set element → unify against `template _`; annotation
  → plain unify; application → clone. The three-construction table of
  [one-answer-per-class](one-answer-per-class.md) collapses into application +
  construction.
- **M8 — the artifact format bumps.** `Any`'s codec tag dies; frozen artifacts
  are rebuilt, not loaded (no forward compatibility).
- **M9 — SPEC typo:** form 5's `String` is the builtin `string`
  ([ideal-api-review](ideal-api-review.md) measured it unresolved).
- **M10 — internal wildcard fills become origin-marked empty cells.** The
  `loop_run.rs:222/256` attribute fills and the checker's shared default
  (`checker.rs:1031`) are internal mechanisms, not source spellings; they retire
  with the variant, not by a `_` rewrite (a `_` hole is open on both the
  concreteness and closure axes, while these relied on Any being "decided
  content"). The replacement keeps the distinction `indexing.rs:639-680` already
  reads: an unstated attribute position is an **empty cell whose origin marks it
  a wildcard fill**, so a computed hole (empty, no such origin) stays
  distinguishable. This preserves the `indexing.rs:674` invariant's rule ("empty
  alone is not wildcard") by moving the marker from value to origin — a
  migration of the same rule, not a reword. The `evaluate_block` release test
  (`evaluation.rs:1013`) and `class_answer_is_decided` (`function.rs:605`) must
  be re-argued on emptiness, not silently re-keyed (recon §5 names the hazard).
- **M11 — the refinement predicate's input is the WHOLE EXPR (maintainer
  verdict, 2026-11-15; corrected same day).** A refinement attribute feeds the
  **whole expr** (the full `[value, type]` term) to its predicate, which must
  return 1 — **not just the value slot**. First stated as "the value attribute";
  the maintainer corrected: "refinement 不是要求把 value slot 输入满足返回 1，而是
  整个 expr 输入返回 1". So `Refinement::constraint` (`refinement.rs:88`) must
  pass the **whole expr** to `Apply(predicate, …)`, not `value_expr` /
  `Index(expr, 0)`. The prelude's `in_num` predicate reads what it needs from the
  whole expr (the type slot) to constrain the operand's class to `Num`. To refine
  a type, place the refinement attribute on the **type's expr** (the type
  position, as the contract test `x : (_ ! in_num)` does).

## Work packages

1. **surface** (`feature/any-migration-surface`): the additive `<:` sugar.
2. **templates**: function-template semantics with `@any` still present —
   hole-parameter lambdas, template application, instantiation of an applied
   result, the M5 clone rule, the undecided formal interior. Verified against
   the SPEC's five Ideal API forms.
3. **any-removal**: delete `Any`/`@any`/`AnyLit`/the auto sugar/requirement
   instantiation; migrate every `.lichen` usage (examples, std, compute, tests)
   to the new forms; codec bump. Lands on top of templates.
4. **docs**: this note, the superseded notes, the SPEC typo, the code invariants.

## Open questions, resolved by the recon measurement

- **Apply is partial; `<:` requires a function template.** A decided
  non-function callee is refused today (`EvalError::ApplyTarget` at run time,
  `evaluation.rs:336-400`; a Guard unify against the `FunctionType` template at
  check time, `lambda.rs:540-545`). The SPEC only ever applies templates, so no
  identity-application rule is added: a compound type's element/key/value
  template is a function (form 1), and a bare type value in a template position
  migrates to one (`[Int, 3]` → `[_ -> (_: Type), 3]` for type-valued elements,
  `[_ -> (_: Int), 3]` for Int elements).
- **The instantiation gate is where the new form dies.** `check_instantiate`
  (`structs.rs:820-876`) accepts only a struct layout; `Div _` is applied
  lazily, so the gate sees the unapplied `TypeFunction` and refuses
  (`InstantiateCallee`). The core fix evaluates the template application before
  the gate.
- **The auto-`@any` sugar is five cooperating mechanisms, not one site**
  (recon report §3a): the parser's value-less annotation fold and `.'name`
  formal head, the checker's shared default wildcard, the identity-lambda Any
  write (`lambda.rs:126-142`), normalize reading Any as decided content
  (`evaluation.rs:884`), and the clone Any→hole arms. All become hole-based;
  the equivalence to prove is at `class_answer_is_decided` (`function.rs:605`)
  and `answer_elements_are_undecided` (`apply.rs:464`), which read Any, not
  emptiness.
- **The clone verdict change is `strict-clean`'s `5fcc181a`** (outermost member
  scope): it applies cleanly on dev. It is the SPEC's own reworded sentence
  (the language spec is authoritative, so this is a migration, not an
  unpermitted invariant reword). It does NOT fix the CIn collide by itself
  (the declared class has a root member); the function-template application is
  what fixes that. `8552d500` does not apply on dev (dev has no
  `parameter_value` field) and is not needed.
- The compute frozen mirror's `@any` → fresh-hole handling migrates with M7.
## Baseline (dev `174379fb`, measured by the recon worker)

`cargo check --workspace` green (warnings in lichen-serve). The suite is
**known-red**; the migration's bar is no new failures and the SPEC forms working.
Per binary (`cargo test --workspace --no-fail-fast -- --test-threads=8`):

| binary | passed | failed | ignored |
|---|---|---|---|
| lichen_compute (integration) | 135 | **148** | 1 |
| compute_gpu | 90 | **4** | 0 |
| language (integration) | 278 | **11** | 0 |
| language_server (integration) | 21 | **1** | 0 |
| serve | 10 | **1** | 0 |
| highlevel | 136 | 0 | 0 |
| basic (lowlevel) | 167 | 0 | 1 |
| lichen_language (lib) | 69 | 0 | 0 |
| parser | 62 | 0 | 0 |
| kernel_ir | 46 | 0 | 0 |
| language_server (lib) | 32 | 0 | 0 |
| preprocess | 26 | 0 | 0 |
| lex | 22 | 0 | 0 |
| graph_runs | 17 | 0 | 0 |
| package | 15 | 0 | 0 |
| lichen_compute (lib) | 14 | 0 | 0 |
| line_model | 13 | 0 | 0 |
| fusion_bridge | 10 | 0 | 0 |
| registry | 8 | 0 | 0 |
| runtime | 4 | 0 | 0 |
| analyze | 2 | 0 | 0 |
| normalize_infinity | 2 | 0 | 0 |
| pack_install / directive_span | 1 each | 0 | 0 |

The compute failures are the incomplete compute migration
([value-templates](value-templates.md)); the dominant reading is
`compute.graph: this function's return is not a value a graph can hand back`.

## Acceptance probes (the SPEC's forms, run through `lichen-compiler`)

The core work is verified against these programs (the language front end is the
faithful runner; an IR probe carries no core prelude). Each must answer a decided
value of the stated class; `none` is a failure.

1. Uniform: `sum3 := [_ -> (_: Int), 3]['a, 'b, 'c] -> 'a + 'b + 'c` then
   `sum3 (_[1, 2, 3])` → `6: Int`.
2. Named: `div := {.'dividend: Int, .'dividor: Int} -> dividend / dividor` then
   `div _{.dividend 2, .dividor 1}` → `2: Int`.
3. Struct signature: `Div := _ -> {.dividend _: Int, .dividor _: Int}`;
   `div := 'x = Div _ -> x.dividend / x.dividor`;
   `(div _{.dividend 6, .dividor 3}, div _{.dividend 8, .dividor 4})` → `(2, 2)`
   — both calls decided, the second not contaminated by the first.
4. Function generic: `div := 'T -> {.'dividend: T, .'dividor: T} -> dividend / dividor`;
   `(div _ _{.dividend 4, .dividor 2}, div _ _{.dividend 1.0, .dividor 2.0})` →
   an Int-class and a Float-class answer, neither `none`, no cross-class leak.
5. Nested: `A := _ -> {.'x: Int, .'y: Float}`; `B := _ -> {.a A _, .'z: string}`;
   `f := {.a A _, .b B _, .'w: Type} -> ...` checks; a concrete call's field reads
   through the nested instance answer.
6. Wildcard freshness: `D := _ -> {.a _, .b _}`;
   `(D{.a 1, .b 2}, D{.a 1.5, .b 2.5})` → both instances answer (the old
   `expected Int, found Float` collide is gone).
7. Capture + per-call freshness: `a := 42; k := 'x -> x + a; (k 1, k 2)` →
   `(43, 44)`.
8. The compute regression probe ([value-templates](value-templates.md)):
   `a := 42; k := compute.jit ('x -> x + a); compute.launch k 1` → `43: Int`
   (compute path; bar: no worse than baseline).

The 11 red `language` tests at baseline are exactly this surface
(`template_instances`, `callee_inference`, `field_read_kinds`, `pipeline`); the
migration turns them green. The 148 compute failures are the incomplete compute
migration; the bar there is no new failures.

## Templates package implementation status (core worker, branch feature/any-migration-templates)

> Status: **Any removed; the language core works; the compute replay hangs.**
> This records what landed and the one unresolved fork, for the compute follow-up.

**Landed.** `LowValue::Any` is deleted workspace-wide; an open position is an
undecided cell. The function-template mechanism works end to end: acceptance
probes 1–7 answer through `lichen-compiler` (`6: Int`, `2: Int`, `(2, 2)`,
`(2, 0.5)`, `5: Int`, `((1, 2), (1.5, 2.5))`, `(43, 44)`); probe 8 stays `none`
(the baseline compute regression). Green anchors held: highlevel 136, lowlevel
basic 167/1-ignored, language lib 69. The M5 clone verdict (`5fcc181a`) is
cherry-picked. The wildcard-fill marker is persisted (`StaticNode.wildcard_fill`,
artifact format 32, codec tag 9 reserved).

**The marker duality (the rule that works).** Two predicates, two jobs:
`is_wildcard_fill` follows origin (a cell is a wildcard position if it is the
marked template or a clone of it) — used for rendering, openness, stated/carried.
`is_wildcard_template` is self-origin only (the cell IS the shared template) —
used for the non-merging unify rule, the deep verdict's decided-content, and
`alias_read`'s skip. A per-call clone is an ordinary open cell the argument fills;
only the shared template is a non-merging requirement. This keeps the core prelude
(`1 + 2`) contamination-free AND the per-call freshness.

> **Superseded by Rework 3 below: both premises of this fork were measured false.**
> The remap does not fix the compute hang, and the refinement conflict is not a
> clone conflation. Read Rework 3 before acting on this paragraph.

**The unresolved fork: the compute replay hangs.** The frozen clone arm
(`static_node_apply`) must mint a fresh cell per wildcard fill. Without a remap it
mints unbounded fresh nodes and the compute graph tests hang (chain_merge, serve
schema_probe). Adding `ctx.remap` bounds the nodes but breaks the core prelude's
`! in_num` refinement ("predicate did not answer 1") — the refinement reads an
unfilled clone. Self-origin keying of the non-merging rule / deep verdict /
alias_read (above) was tested and does NOT resolve the remap/refinement conflict.
The language probes need the non-remap arm; the compute replay needs the remap.
This is the contamination/freshness duality at the frozen-clone boundary and is
the compute follow-up's crux. `frozen_function_type::a_function_template_in_a_frozen_module_is_a_function`
is the one new language failure (same frozen-kernel-struct family).

**Still red (baseline surface, not yet flipped).** The 11 baseline language tests
(template_instances, callee_inference, field_read_kinds, merge_carry, pipeline)
remain red; the direct-template instance now builds fresh field value cells
(`fresh_instance_field`) but the reads still resolve through the declaration.

### Instrumented root cause of the remap/refinement conflict (core worker, rework)

Traced `1 + 2` with `DBG_WILD` on the frozen wildcard arm, the assert drain, and
`TypeOperator::InDomain`. With the arm on `ctx.remap` (bounded nodes):

- The refinement subject's slot halves ARE filled: value half `NodeId(211) = USize(1)`,
  type half `NodeId(212) = Int`. The argument reaches the subject.
- But the failing `InDomain`'s operand 0 (`left_ty`, the refined operand's **type**)
  resolves to `NodeId(349)`, whose value is the raw `USize(1)` — the argument **value**,
  not a type. (`InDomain` operand 0 is built from `left_ty` in
  `checker/operators.rs`; `set::contains` decodes it with `shape::low_type_of`, which
  reads a **type value** — a marker or `[shape, kind]` — and answers `None` for a raw
  scalar.) So `known_shape` is `None`, `same_member` falls to `value_eq(Int, 1)` = false,
  and the refinement answers `0`.
- `NodeId(349)` is a **normal-path** clone (not a wildcard-arm mint). The refined
  operand's type cell is conflated with the value cell once the remap/regroup runs; the
  type resolves to the argument-filled value node. Without the remap the wildcard mints
  bypass `ctx.remap`/regroup, the type cell stays a separate open cell, the `InDomain`
  gate (`node_value(operand0)` empty) keeps the refinement **pending**, and the probes pass.

So the conflict is a **type/value cell conflation in the frozen clone remap**, not the
subject/parameter-image mismatch first hypothesized: the subject is filled, but the
refinement reads the operand's *type*, which the remap resolves to the *value*. The fix
must keep the refined operand's type cell distinct from the value cell through the
remap/regroup (or make the read's result type carry the element's type half), which is a
deeper change than the wildcard arm alone. Reverted to the non-remap arm (source language
green; compute replay still hangs) rather than weaken the non-merging rule or re-key the
predicates, both of which were measured not to resolve it.

### Rework 2 — refined evidence (WIP commit f63396c8, remap + instrumentation)

Re-enabled `ctx.remap` on the frozen wildcard arm and instrumented the regroup, the
wildcard mints, and `InDomain` operand 0. For `1 + 2`:

- The value/type cells are **not** unified by regroup: value `t=147 rep=147 → clone 211`,
  type half `t=148 rep=233 → clone 212`; regroup unifies `212↔217` (both **type** cells,
  rep 233), never type-with-value. So the earlier "type/value class conflation via regroup"
  reading is **not** the mechanism.
- The failing `InDomain` operand 0 (`left_ty`, the read's result type) is an **Index node**
  (`NodeId(349)`, has_op=true) whose slot operand is `USize(0)` — it reads the element's
  **value slot 0**, not the type slot 1. Its element operand is the argument-filled slot.
  The **type-side** `InDomain` (`NodeId(254)`) correctly reads slot **1**. So the value-side
  read's result type resolves to the value slot; the type-side one does not.
- The slot-index node (`NodeId(351)`, value 0) is **not** a wildcard mint and **not** in
  `ctx.remap` — it is created during the apply, so the value-side read type is being built
  from the value slot rather than carrying the element's type half.

> **Superseded by Rework 3: this next action rests on a disproven reading.**

**Next action:** make the value-side read's result type carry the element's **type half**
(`value_slot_read`'s `ty_node`, `checker/values.rs:31`, lazy arm `index_of(element, one)`)
through the frozen clone, so `InDomain` operand 0 resolves to a type value. Compare the
value-side clone (`static_node_apply`) vs the type-side signature clone
(`clone_static_signature_node`, `static_module/apply.rs:477`) — the type side reads slot 1
correctly, the value side reads slot 0. The divergence between those two clone paths is the
defect to fix. Instrumentation is gated on `DBG_WILD` (run `target/dbg.ps1`); revert it and
the remap arm to return to the green source-language state (probes 1-7, anchors 136/167/69).

### Rework 3 — the directed root cause is disproven; the blocker is the refinement's subject

> Status: **stopped and reporting**, under the brief's "if it exposes a different
> defect, stop and report evidence" rule. **No code changed**: the tree is at
> `527398e8` with the WIP remap arm intact. Two premises of the fork above were
> measured false, so the fix Rework 2 directs would not repair anything.

#### 1. There is no type/value slot conflation in the frozen clone

The failing `InDomain` operand 0 is **not a type read**, so "the value side reads
slot 0, the type side reads slot 1" is not a divergence between two clone paths:
slot 0 is *correct* for the node that actually fails.

`@in` is a **value**-membership operator. `check_binop` builds the operator node
from `self.value_of(left)` / `self.value_of(right)`
(`crates/lichen-highlevel/src/checker/operators.rs:121`), and `BinOp::In` maps to
`TypeOperator::InDomain` (`crates/lichen-highlevel/src/ir.rs:86`). So
`in_num := 't -> t @in Num` tests **`t`'s value** against `Num`'s members — which
are the *type values* `Int` and `Float`.

A refinement applies its predicate to the annotated expression's own term:
`crates/lichen-highlevel/src/checker/annotations.rs:800` passes `value_term`, and
`Refinement::constraint` (`crates/lichen-highlevel/src/refinement.rs:88`) builds
`Apply(predicate, value_expr, result)`. The contract is stated by
`a_refinement_written_on_a_type_refines_the_class`
(`crates/lichen-language/tests/language/membership.rs`): *"A refinement on a type
applies the predicate to the type value, so `in_num` needs no type read at all"* —
and it writes the refinement in a **type** position, `x : (_ ! in_num)`.

The prelude writes it in a **value** position
(`crates/lichen-language/src/core.lichen:3`, and the seven siblings):

```text
add := 'operands -> { let Pair := (@any ! in_num, @any ! in_num); operands = Pair; operands(0) + operands(1) }
```

The subject is therefore the operand's **value** (`1`), never its class (`Int`),
and `1 @in {Int, Float}` is `0`. Measured node chain for `1 + 2`, remap arm,
`DBG_WILD=1`:

```text
DBG-INDOMAIN module=0x5a9cf4410 left_node=Dynamic(NodeId(349v1)) left=Some(USize(1))
DBG-D0 node=NodeId(349v1) op=Some("LowOperator(Index)") origin=Some(NodeId(265v1))
DBG-D0   operand[0]=Dynamic(NodeId(325v1)) val=Some(Array(… metadata: 5 …))
DBG-D0   operand[1]=Dynamic(NodeId(351v1)) val=Some(USize(0))
DBG-D1 node=NodeId(325v1) op=None origin=Some(NodeId(267v1))
```

`325` is a plain expr cell (`op=None`), so `349` is `Index(expr, 0)` — a **value**
read, exactly what `value_of` builds. `value_slot_read` was instrumented and builds
both slots correctly (`value_node = Index(e, zero)`, `ty_node = Index(e, one)`);
neither it nor either clone path produced `349`/`351`, and neither node appears in
any `ctx.remap` dump. The third `InDomain` of the same run
(`left_node=NodeId(254v1)`, slot operand `USize(1)`, value an `Array`) is
`arithmetic_class`'s gate on the operand's **type** — it reads slot 1 and is
satisfied. The two that fail are the two `! in_num` refinements, matching the
diagnostic's columns `3:36` and `3:51`.

#### 2. The green state does not enforce this refinement; its pass is vacuous

The non-remap arm does not make the refinement *pass* — it keeps it **undecided**,
so nothing is enforced. A `string` walks straight through a refinement whose
domain is `{Int, Float}`. Readings, each the tool's own output:

| probe | non-remap (green) | remap (WIP) |
|---|---|---|
| `1 + 2` | `3: Int`, `PROBE_EXIT=0` | `error: the refinement's predicate did not answer 1`, `PROBE_EXIT=1` |
| type position `x : (_ ! in_num)`, `f 5` | — | `5: Int`, `PROBE_EXIT=0` |
| type position, `f "a"` | `error: … did not answer 1 --> 3:19`, `PROBE_EXIT=1` | — |
| prelude form `(@any ! in_num, …)`, `f ("a", "b")` | **`"a": string`, `PROBE_EXIT=0`** | fails |
| value position `(_ ! in_num, …)`, `add2 (1, 2)` | **fails**, `3:37` / `3:49`, `PROBE_EXIT=1` | — |

The last two rows are the mechanism: with `_` (a plain open cell) the same
value-position refinement **already fails at green**, with no remap involved. The
remap only makes `@any` behave like `_` — it lets the constraint decide, and a
constraint on the wrong subject then fails. So this is a **pre-existing defect of
the value-position refinement, not a regression of the clone work**, and the
refinement mechanism itself is sound: the type-position form is enforced at green
and answers `5: Int` with the remap enabled.

A tuple **annotation** is not the escape hatch: `operands : (_ ! in_num, _ ! in_num)`
is refused — `error: expected Tuple, found raw[raw[raw[?a, ?b], raw[?c, ?d]], Tuple]`.

#### 3. The remap does not fix the compute hang either

The fork's other premise — "the compute replay needs the remap" — is false.
`chain_merge` hangs under **both** arms, measured with a hard `timeout` so a hang
cannot block the run:

- non-remap: 4 tests, each `has been running for over 60 seconds`, `RUN_EXIT=124` (90 s ceiling).
- remap: 4 tests, each `has been running for over 60 seconds`, `RUN_EXIT=124` (120 s ceiling).

The harness is sound, not the filter: the same binary answers
`test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 283 filtered out`
for `lichen_compute::pack_placement::a_node_schema_hashes_the_text_it_prints`.
(A bare `--exact` name filtered to `0 tests` and printed `ok` first — the trap
[the Verify rules](../../AGENTS.md) name; the full path is required.)

So the wildcard arm is **not** the compute hang's cause, and switching it cannot
be the compute follow-up's fix.

#### What the maintainer has to decide
> **Resolved by M11 (corrected) — see Rework 4.** The predicate receives the whole
> expr and reads the class itself; the prelude is migrated and the refinement enforced.

The blocker is a **language/prelude** question, outside this brief's "do not touch
the source-language semantics" boundary, so it is reported rather than changed:

1. What is `@any ! in_num` in a **value** position supposed to refine — the value,
   or the value's class? The prelude's eight arithmetic operators depend on the
   class reading; the contract test only exercises the type position.
2. Until that is answered, the non-remap arm's green is **vacuous** for the
   prelude's refinements (a `string` passes `! in_num`), so probes 1–7 and the
   anchors do not attest that the refinement works.
3. The compute hang needs its own investigation; it is independent of the
   wildcard arm. `chain_merge`'s program uses the un-annotated `@loop fold` shape
   that `wontfix:D18` covers (`code-audit.md`, P1-50), which is a place to look,
   not a conclusion.

The instrumentation used here is reproducible: gate on `DBG_WILD`, log the
`InDomain` operand chain in `program.rs`, the `value_slot_read` triple and
`index_of` in `checker/values.rs`, the `constraint` nodes in `refinement.rs`, and
both clone paths' returns in `static_module/apply.rs` (note that
`clone_static_signature_node`'s undecided arm returns early, so a log at the end
of the function misses it).

#### State left by rework 3 (verified)

The WIP remap arm was **reverted** (`51b7ee00`, reverting `f63396c8`): it broke the
source language and, per §3 above, bought nothing for compute. The remap experiment
stays recoverable at `f63396c8`. The tree is clean, `cargo fmt --all -- --check`
answers `FMT_EXIT=0`, and `comments-check --config comments-test.toml . --max-violations 0`
answers `no comment compliance violations found`.

Acceptance probes, the CLI's own answers (`target/probes-all.ps1`):

```text
PROBE 1  6: Int                          PROBE 5  5: Int
PROBE 2  2: Int                          PROBE 6  ((1, 2), (1.5, 2.5)): Tuple
PROBE 3  (2, 2): Tuple                   PROBE 7  (43, 44): Tuple
PROBE 4  (2, 0.5): Tuple                 PROBE 8  none: Int   (baseline compute regression)
```

Anchors, each the tool's own `test result:` line:

```text
language-lib      test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
highlevel         test result: ok. 136 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
lowlevel-basic    test result: ok. 167 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
language          test result: FAILED. 277 passed; 12 failed; 0 ignored; 0 measured; 0 filtered out
```

The 12 are the documented surface, unchanged: `template_instances` (2),
`callee_inference` (2), `field_read_kinds` (4), `merge_carry` (1), `pipeline` (2),
plus `frozen_function_type::a_function_template_in_a_frozen_module_is_a_function`.

**The green is still vacuous for the prelude's refinements** (§2): `f ("a", "b")`
through `(@any ! in_num, @any ! in_num)` answers `"a": string`. That is the open
language question, not a clone defect.

## Rework 4 — M11 applied: the predicate's subject is the whole expr

> Status: **the refinement is enforced and the language oracles are green.** The
> compute hang is a separate, still-open defect (§5); it is unchanged by this work.

### 1. What M11 (corrected) required, and what actually needed changing

The maintainer's correction: *"refinement 不是要求把 value slot 输入满足返回 1，而是
整个 expr 输入返回 1"* — the predicate's input is the **whole expr**, not its value
slot.

**The mechanism already did this.** `annotations.rs:700` binds
`value_term = self.state[value].term`, and `.term` is the full `[value, type, attrs…]`
expr (`check_binop` sets `term = expr_of(value, result)`, beside `val` and `ty`).
`Refinement::constraint` passes that node straight to `Apply(predicate, …)`. The
rework-3 trace confirms it: the subject node `325` was a **5-item** expr cell, not a
scalar. The `Index(apply, 0)` in `constraint` reads the **apply's own result** — the
predicate's answer — not the subject's value slot.

So the defect was the **predicate**, not the plumbing: `in_num := 't -> t @in Num`
read `value_of(t)`, the operand's value (`1`), and tested it against a domain of
*type values*. The fix is `in_num` reading the class from the whole expr, and the
rule is now stated at the code (`refinement.rs`, `# Invariant` on `constraint`,
parameter renamed `subject_expr`).

### 2. The prelude form (`crates/lichen-language/src/core.lichen:2-10`)

```text
in_num := 't -> { c := _; t : c; c @in Num }
add := 'operands -> { let Pair := (_ ! in_num, _ ! in_num); operands = Pair; operands(0) + operands(1) }
```

`{ c := _; t : c; c }` is the language's own type-read idiom (the `type_of` of
`membership.rs`): it binds a fresh cell to the subject's type, so `c` holds the
class. The refinement sits on the **value** expr because the predicate now reads the
type itself; `@any` is gone, the holes are `_`. All eight operators share the form.

The intermediate type-position form (`(_ : (_ ! in_num), …)`, commit `ad2de6df`)
is **superseded**: it worked only with a value-reading predicate, which M11 rules out.
A direct `t(1)` is refused — `this tuple read cannot be checked: the container's type
is not decided here` — which is why the annotation idiom is used.

### 3. The frozen-domain read (kept)

`set::members` required the raw set node to be dynamic
(`items.get(1)?.node.dynamic()?`), so a **frozen** `Num` answered `None` and every
membership test failed. `array_items` is documented to read "a dynamic node **or a
static ref**"; the fix passes the ref through (`crates/lichen-highlevel/src/set.rs`).
Measured before the fix: `DBG-MEMBERS value=Static(… index: 58) … item[1]=Static(… 57)`,
and `same_member` was never reached.

### 4. The arm chosen: remap

Both arms now satisfy the language oracles, so the deciding criterion is node
bounding. The remap keys one cell per template **per apply**, so a diamond shares it
(`static_module/apply.rs`, the `wildcard_fill` arm).

| | non-remap | remap |
|---|---|---|
| probes 1–7 | green | green |
| `"a" + "b"` | refused | refused |
| nodes per apply | unbounded mints | one per template |

### 5. The compute hang — still open, trigger isolated
> **Resolved by Rework 5 below.** The spin was `Module::marks_at` walking the M10
> self-origin forever; one guard clause removed all 58 hangs.

**Unchanged by this work.** A per-test sweep (hard 6 s timeout each, so no hang can
block the rest) over all 284 tests, run at `cc0ade10` and at `44ae4002`:

```text
before: 122 OK, 104 FAIL, 58 HANG
after:  122 OK, 104 FAIL, 58 HANG
```

The status+name sets are **identical** (`diff` empty) — no new failures, no new hangs,
none fixed. `chain_merge` still hangs under both arms.

**The trigger is the `@any` marker, not the refinement.** Minimal repro, one kernel:

```text
Par  := {.n @any : Int, .in In}
total_raw := ('k = Par) -> {
  i := compute.range k.n
  r := compute.buf (compute.BufParam{.type Float, .length k.n})
  @loop fold := 's -> if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + 1.0) ~ "combine")
  ((OutR){.r (compute.write ((compute.Write _){.to r, .at i, .value fold (k.n, 0.0 ~ "identity")}))})
}
total := compute.parallel (total_raw ~ "claim:Reduction(combine,identity)") "gpu" OutR
```

- `@any : Int` → `lichen-compiler build` hangs (`RUN_EXIT=124`).
- `_ : Int`, one word changed → `built … type: Struct`, **0.575 s**.
- No `@loop fold` → fast refusal: `the kernel claims Reduction and its body carries no
  mark for its \`combine\` role`.
- No `compute.parallel` (same `@any` + fold) → `type: TypeFunction`, 0.523 s.

So all three are needed: the marker, the `@loop`, and `compute.parallel`.

**Where it is not.** Phase markers put the hang in **compilation** (`build` hangs; the
graph passes are never reached). The loop runner enters exactly one iteration and
returns: `DBG-LOOPBODY iter=1`, `DBG-LOOPTEST step=1`, `DBG-ITER after-eval some=false`,
`DBG-APPLYLOOP returned some=false` — then silence. Sampling the process: **flat
memory** (15.7 MB over 30 s) and **one core busy** (CPU 4.9 s at 5 s wall, 29.6 s at
30 s) — a read-only spin, not recursion or allocation.

Guards that did **not** fire (each ruled out): `evaluate_node`, `apply`, `unify`,
`check_expr` call counters; `class_root`, `descends_from`, `evaluate_class_first`,
`is_shallow_region`, `narrow_element`, `masked_expr`, `is_region`,
`class_operation_owner`, `nested_in`, `parameter_path`, and both `loop_run` loops.
`disjoint::find` is recursive (a cycle would overflow, not spin); the layout merge and
the `seen`-guarded worklists advance by construction.

**Next lead:** the spin is after the loop returns `Undecided`, in a read-only walk not
yet named. The cell the replay waits on is the `@any`-marked parameter slot that the
non-merging rule keeps undecided; the walk that re-reads it without a bound is the
defect. `wontfix:D18` (un-annotated recursive fold) is a **contract** and was not
touched — the fold alone answers `?a: ?a` in 0.4 s, so D18 is not the hang.

## Rework 5 — the compute hang: `marks_at` walked the M10 self-origin forever

> Status: **fixed.** The hang was a read-only spin in `Module::marks_at`, which walked a
> node's origin chain with no bound and no cycle guard — and M10 makes a wildcard fill's
> origin **itself**. One guard clause removes all 58 hangs.

### The spinning walk

`crates/lichen-lowlevel/src/module.rs`, `marks_at` (was line 403):

```rust
match self.nodes[current].origin {
    Some(origin) => current = origin,   // a self-origin never advances
    None => return None,
}
```

`mark_wildcard_fill` (same file) **deliberately** writes `origin = Some(node)` — the
self-origin *is* the M10 wildcard marker, as its own invariant says: *"The self-origin is
the marker a clone's origin chain reaches."* `marks_at` predates M10 and followed that
edge forever. Zero allocation, one core busy — exactly the measured signature.

The instrumented proof, on the one-kernel repro:

```text
DBG-MARKS SPIN node=NodeId(468v1) current=NodeId(468v1) origin=Some(NodeId(468v1))
```

### How it was found (the earlier guards missed it)

Phase markers down the fragment compile: `P13-before-lower` printed, `P14-after-lower`
never → `Lower::lower_index_function`. Then `L0-before-state` printed, `L1-after-state`
never → `loop_state`. Then `value_inner` was called **6 times** and the 6th never returned;
its branch markers reached `B3-parampath` and `B9-aftermatch`, and the last marker was
`publish_mark` — which calls `marks_at`.

**Why rework 4's guard on `parameter_path` did not fire:** it counted *loop iterations*,
but the spin was in a helper called on iteration 1 (`publish_mark` → `marks_at`), so the
counter never advanced. A guard on a loop's iterations cannot see a spin inside its body.

### The fix

```rust
match self.nodes[current].origin {
    Some(origin) if origin != current => current = origin,
    _ => return None,
}
```

A self-origin is the marker, not a clone edge: the cell has no template to inherit a mark
from, and `marks.get(&current)` was already checked, so `None` is the correct answer. This
mirrors `is_wildcard_fill`, which already treats `cell.origin == Some(n)` as its terminator.
The rule is stated in `marks_at`'s `# Invariant`.

### Oracles (verbatim, final build)

```text
one-kernel repro (@any : Int + fold + compute.parallel):  built … type: Struct   EXIT=0
same with _ : Int:                                        built … type: Struct   EXIT=0
1 + 2:      3: Int          PROBE_EXIT=0
"a" + "b":  error: the refinement's predicate did not answer 1 — the value does not
            satisfy the refined type   (core.lichen:3:36 and 3:48)   PROBE_EXIT=1
PROBE 1  6: Int              PROBE 5  5: Int
PROBE 2  2: Int              PROBE 6  ((1, 2), (1.5, 2.5)): Tuple
PROBE 3  (2, 2): Tuple       PROBE 7  (43, 44): Tuple
PROBE 4  (2, 0.5): Tuple     PROBE 8  none: Int   (baseline compute regression)

chain_merge:  test result: FAILED. 0 passed; 4 failed; 0 ignored; 0 measured; 280 filtered out; finished in 0.13s
              — RUNS TO COMPLETION, no hang; the refusal is now a real diagnostic
language-lib    test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
highlevel       test result: ok. 136 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
lowlevel-basic  test result: ok. 167 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
language        test result: FAILED. 277 passed; 12 failed; 0 ignored; 0 measured; 0 filtered out
FMT_EXIT=0 · comments-check: no comment compliance violations found
```

### Compute sweep vs the original dev baseline

Per-test, hard 6 s timeout each, all 284 tests, same runner:

| tree | OK | FAIL | HANG |
|---|---|---|---|
| dev `174379fb` (original baseline) | 135 | 149 | **0** |
| branch before this fix (`44ae4002`) | 122 | 104 | **58** |
| branch with this fix (`95081e58`) | 133 | 151 | **0** |

**The fix removed every hang and broke nothing:** pre-fix → fixed is 58 HANG → 47 FAIL +
11 OK, with **zero OK→FAIL** transitions.

Against dev there are exactly **two** regressions, and **both pre-date this fix** (already
FAIL at `44ae4002`, i.e. from the Any-removal work, not from `marks_at`):

- `graph_structure::a_body_dispatches_through_an_apply_whose_callee_holds_the_operator`
- `perspective_requirements::a_read_takes_its_perspective_from_the_buffer_and_the_index`

They are the remaining compute-migration surface, not a new defect of this change.
Both are now closed — `graph_structure` by `f448d8e7`, `perspective_requirements` by
rework 6 below — and the compute suite matches dev exactly.

## Rework 6 — the perspective demand: a read of a frozen fill is not content

> Status: **the last regression is closed; the compute suite is at dev parity.**

### The regression, and what actually blocked it

`perspective_requirements::a_read_takes_its_perspective_from_the_buffer_and_the_index`:
an unstated attribute position in a slot must stay unstated, but the demand on the read's
result was **never checked** — `(r # warp)` merged silently whether the index was uniform
or lane-varying.

The trace (`DBG` instrumentation on `GcdOp::run`, the deep walk and `run_deferred`, `1 + 2`
and the failing program, both sides of every A/B below):

1. The demand's provider is the meet of the argument's fields. Each field's slot is the
expr `provided_slot` returns: `[value, type, refinement, doc, perspective]`, where a
position the source did not spell is a **lazy `Index` read** of the annotation value's own
expr (`element_expr`).
2. For a field built from a **frozen** value (`compute.warp`, a module constant) those
positions are *static* cells with the persisted `wildcard_fill` flag. Their reads answer
nothing, and nothing will ever fill them.
3. The deep verdict reads every position, so the slot read as **undecided**; `run_deferred`
then declined without running `Gcd`; the meet had no value; `unify_slots` merged
undecided-against-decided and wrote the demand into the class. Assertion 2's refusal never
happened, and assertion 1 passed for the same reason — **both were vacuous**.

### The rule

`Module::frozen_fill` (`crates/lichen-lowlevel/src/module.rs`) names the frozen marker, and
`value_is_undecided` no longer reads a position that is a **read of a frozen fill** — the
same exception it already makes for an array's template slot (`evaluation.rs`,
`reads_a_frozen_fill` + `position_states_nothing`). The read is classified by the position
it reads, never by its own answer.

### Why frozen, and not "any unstated position"

Measured, each an A/B on the same tree:

| rule | compute | language |
|---|---|---|
| none (`72a670df`) | **134 / 149**, target test red | 277 / 12 |
| read of a **dynamic** fill decided, or its parent decided | 135 / 148 | **276 / 13** — `pipeline::a_block_bound_lambda_is_still_polymorphic` |
| read of a **frozen** fill skipped (this rework) | 135 / 148, set == dev | 277 / 12 |

The dynamic fill is the difference: it is a **per-apply open cell the argument fills**, so a
read of it must keep its own undecided verdict — `g`'s block-bound parameter is exactly
that shape. A frozen cell is immutable, so its read can never decide, and the structure is
provable without waiting.

**The WIP origin-following verdict (`72a670df`) is reverted** to `is_wildcard_template`:
the recorded M10 marker duality stands (a per-apply clone of a fill is an ordinary open
cell, not a requirement), and measured it changed no test on either side.

### Oracles (verbatim)

```text
perspective test:  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 283 filtered out
lichen_compute:    test result: FAILED. 135 passed; 148 failed; 1 ignored; 0 measured — 0 hangs,
                   failing set identical to dev's 148
language:          test result: FAILED. 277 passed; 12 failed  (the documented surface)
compute_gpu:       test result: ok. 94 passed; 0 failed
highlevel:         test result: ok. 136 passed; 0 failed
lowlevel basic:    test result: ok. 167 passed; 0 failed; 1 ignored
language lib:      test result: ok. 69 passed; 0 failed
```

Acceptance probes through `lichen-compiler` (the language front end is the faithful
runner): `6: Int`, `2: Int`, `(2, 2): Tuple`, `(2, 0.5): Tuple`,
`((1, 2), (1.5, 2.5)): Tuple`, `(43, 44): Tuple`; `1 + 2` → `3: Int`, `"a" + "b"` →
`the refinement's predicate did not answer 1` (M11 still enforced).

The nested form (probe 5) is unchanged by this work: the spelling used here,
`f _{.a (A){…}, .b (B){…}, .w Int}`, is refused with the struct-vs-type-function mismatch
both before and after, which is the [ideal-api-review](ideal-api-review.md) nested-instance
gap, not this rule.
