# The apply's type side runs at check

> Status: **current.** Points at: `crates/lichen-highlevel/src/checker/lambda.rs`
> (`check_app`), `crates/lichen-lowlevel/src/function.rs`
> (`apply_function_type`, `ParameterCheck`), `crates/lichen-lowlevel/src/lib.rs`
> and `module.rs` (the reporting flag).
>
> Companion: [eval-before-unify](eval-before-unify.md) §7.1 records the
> runtime half — an apply of an undecided callee applies its type — and
> [undecided-reads](undecided-reads.md) records the strict read rule this
> sits under.
>
> The `Check` mode itself (every expression's type slot decided, no body run)
> lives on the colleague's `feature/check-mode` branch; this note records the
> apply side it needs.

## 1. The problem

`check_app` built the apply's pair as a value apply plus a **fresh result
cell** `c`: `apply_type` bound `c` to the call's own codomain when it ran, and
the value side bound it too. Until then `c` was an undecided cell, and
`Check` — which decides every expression's type slot and runs no body — read
the cell and learned nothing. `f = (x : Int) => x; f "s"` therefore reported
`expected Int, found string` in `Evaluate` (the value side's parameter check)
and **nothing** in `Check`: the conflict was only ever seen by the value
apply, and a body never ran.

## 2. The ruling and the landed shape

A call's type is the **type-slot function's application**, run at check. The
landed form of that:

- `check_app` builds the type-level apply with a **type-only argument** — an
  undecided value slot, the argument's type, and its attribute — so the type
  side's parameter check never evaluates the argument's value. The value apply
  keeps the real `[value, type, attr]` pair.
- For a decided function type whose function is not under check, `check_app`
  calls `Module::apply_function_type` **at check**: the per-call signature
  clone binds `c` to the call's codomain and optionally runs the parameter
  check. A function under check is skipped, so a recursive call's result stays
  connected (the call-result shape of [undecided-reads](undecided-reads.md) §4
  depends on it).
- The type-level apply node registers the same `ApplyEdge` as the value apply,
  so a failed parameter check is attributed to the argument's cursor
  (`mismatch` in `diagnostic.rs` reads the edge).
- `ParameterCheck::Report` is the fourth mode: run the check and **keep** its
  errors and their `Loc`; `Type` still truncates, `Value` reports, `Skip`
  clones the signature only.

`Module::report_type_side_failures` selects which check the type side runs:
`apply_function_type(..., report)` chooses `Skip` when it is off (`Evaluate`:
the signature-only clone, the behaviour the call-result propagation already
had) and `Report` when it is on (`Check`). The flag is set once per build; the
colleague's `Checker::build_with` sets it from `Mode::Check` before the check
runs, so the lowlevel never learns a mode.

## 3. Measurements

`Evaluate` keeps today's answers: `f = (x : Int) => x; f "s"` reports **one**
`expected Int, found string` with the caret on `"s"`; `n = 2 + 3;
array{Int, n}` prints `array{Int, 5}` and two different computed lengths print
`(array{Int, 5}, array{Int, 6})`. Language 265/0, highlevel 136/0, compute
245/0 + 2 ignored, all 48 example files, `fmt` and `comments-check` clean.

`Check` is not runnable on this branch (the `Mode` is not on `dev`); the
colleague runs the two-mode table on the merge tip. The design makes the
conflict a check diagnostic because the check-time call records the kept
`unify_errors` entry, and `check_failed` stops the build before the sweep.

## 4. A failed shallow call runs its type side once

A `Check` build that fails keeps its diagnostics and stops before the sweep, so
an ordinary failing call is recorded once. The `@shallow` pass still walks every
registered type-apply node unconditionally; without a settle the same failed
call would run its type side a second time there — the second parameter-check
error is kept again, and `apply_error_nodes`' per-node de-duplication leaves it
without an apply attribution.

`apply_type_with` now answers whether the type side applied, and `check_app`
settles a failed `Check` type side by writing `LowValue::Error` into the
type-apply node: the second evaluation finds a decided node and runs nothing, so
one failure is one diagnostic. `Evaluate` is untouched — its type side is the
signature-only clone and the value side reports.

## 5. The check mode that consumes it

`Checker::Mode::Check` gates the definition pass and the top-level statement evaluation, and
sets `Module::set_type_side_reporting(true)` so the type-apply reports instead of truncating;
its own pass then decides every expression's type slot. Measured on the merge tip: `f = (x :
Int) => x; f "s"` reports `expected Int, found string` once in both modes, the caret on the
argument, and the computed-type programs (`array{Int, 2 + 3}`) read the same in both.
`BufferSession::set_mode` exposes the choice; a `Check` session loads and writes no cells.
`incremental-evaluation.md` Q9 records why the LSP has not switched yet (hover's contract for
an undecided value).

## 6. A `Check` build evaluates the asserts it can decide

The author's ruling: a `Check` build reports a builtin's refinement failure — the editor's
green must mean "the refinement holds", not "nobody asked". Four changes carry it, and the
scope of the forcing is the part that had to be measured:

- **A frozen callee's type side carries the argument's value.** `check_app` hands
  `apply_function_type` the value-bearing pair for a static callee and the type-only pair
  for a dynamic one. The value slot used to be a fresh cell in both modes ("its check never
  evaluates the argument's value"), so a refinement predicate that reads the argument —
  `add ["a", "b"]` — had nothing to decide and stayed pending per `assert.rs`'s invariant.
  The scope is not cosmetic: handing the value to **every** call made the editor hang on a
  legal lazy program. `a_lazy_binding_reports_type_but_no_value`'s Church-encoded `paradox`
  expands through the applications its coercions guard, and the type side was forcing each
  one at check time; with dynamic callees left type-only the same build finishes in 0.05 s.
- **A reporting check registers the instantiated asserts.** `instantiate`'s
  `assert_count * usize::from(check == ParameterCheck::Value)` becomes `check != Skip`: a
  `Check` build runs no value side, so the type side is the only place the per-call clone
  can be registered. `static_function_type_apply` registers them too, walking the frozen
  asserts through the signature copies' remap so a condition references the parameter cells
  the check bound.
- **An unapplied body assert is not drained.** `register_assert` leaves it out of the
  module worklist in a reporting build: its condition reads an unbound parameter, and
  forcing it hands the editor a lazy expansion rather than an answer. The function still
  owns the condition, so every applied call's clone is checked.
- **A depth-budget stop is a diagnostic.** `check_asserts` stops the drain when
  `budget_exhausted` is set (the module's `apply_depth_limit` guard — a stack bound, not a
  work bound), and the build records `NonTerminating` instead of a silent pass.

Measured on the merge tip with the shipping vocabulary, `Mode::Check`: `x = add ["a", "b"]`
reports `the refinement's predicate did not answer 1` and `does not satisfy {Int, Float}`,
attributed to `core.lichen`'s own source (the prelude's file) — `add [1, 2]` is clean;
`f = (x : Int) => x; f "s"` reports its one diagnostic in both modes; the two-existing
per-call assert tests read one error each, not two; the lazy program's Check build answers
in 0.05 s, with `paradox` still `None`.
