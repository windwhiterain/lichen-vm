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
