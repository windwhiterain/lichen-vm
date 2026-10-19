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
