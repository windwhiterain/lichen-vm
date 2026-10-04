//! The host loop on **real programs**: a marked recursion in a plain lichen
//! program is run as a loop by the evaluator, so a trip count the *expansion*
//! cannot afford is answered — and the answers are the unroll's, which is the
//! half that must not be assumed.
//!
//! The shape is the acceptance case's, minus the buffer read: a `@loop`
//! reduction over a tuple state with a **literal** count, so the host can see
//! the whole state and the only thing between it and a value is how the
//! recursion is run.
//!
//! **The separation is nesting.** A node records the number of apply levels it
//! was created under (`Module::node_depth`), which is a fact of the graph and
//! not of the pass that built it, so an expansion reaches the nesting bound at
//! its trip count even though the lazy deep pass walks it at depth one. A loop
//! instantiates the *same* entering apply node every iteration, so it never
//! deepens and is bounded by work alone. Measured here, under the default
//! budgets: the expansion answers counts up to 499, the loop up to 1_998.

mod common;

use common::{evaluate, usize_of};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_language::compile;
use lichen_lowlevel::BudgetExhausted;

/// `sum_to (n, 0)` = `n`, iterated: the reduction whose count is the answer.
fn sum_to_source(marked: bool, count: usize) -> String {
    let marker = if marked { "@loop " } else { "" };
    format!(
        "{marker}sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)\n\
         sum_to ({count}, 0)"
    )
}

/// The budget a refused program was stopped by.
fn refusal(source: &str) -> BudgetExhausted {
    let report = compile(source);
    assert!(!report.ok(), "expected a refusal for {source:?}");
    report
        .diagnostics
        .iter()
        .find_map(|diagnostic| {
            let check = diagnostic.check.as_ref()?;
            (check.kind == DiagKind::NonTerminating)
                .then_some(check.budget)
                .flatten()
        })
        .unwrap_or_else(|| panic!("no budget refusal in {:?}", report.diagnostics))
}

#[test]
fn the_loop_and_the_unroll_answer_the_same_value() {
    // Both paths run at a count they can both afford, so this compares the
    // loop's values with the expansion's rather than with the loop's own idea
    // of the answer.
    for count in [0usize, 1, 2, 7, 64, 200] {
        let (_module, looped, _ty) = evaluate(&sum_to_source(true, count));
        let (_module, unrolled, _ty) = evaluate(&sum_to_source(false, count));
        assert_eq!(
            usize_of(&looped),
            usize_of(&unrolled),
            "the loop and the unroll must agree at count {count}"
        );
        assert_eq!(usize_of(&looped), count);
    }
}

#[test]
fn a_marked_reduction_runs_a_count_the_expansion_cannot_afford() {
    // 600 is past the checker's nesting bound (500) and well inside its work
    // bound (2_000), so it is exactly the window the conversion opens: the
    // expansion deepens once per count, the loop does not deepen at all.
    let (_module, value, _ty) = evaluate(&sum_to_source(true, 600));
    assert_eq!(
        usize_of(&value),
        600,
        "the loop must answer a count the expansion cannot afford"
    );
    assert_eq!(
        refusal(&sum_to_source(false, 600)),
        BudgetExhausted::ApplyDepth { limit: 500 },
        "the same program unmarked must be refused by the *nesting* guard"
    );
}

#[test]
fn a_marked_reduction_past_the_work_bound_is_refused_too() {
    // The loop is not exempt from work: an iteration is one application, so a
    // trip count past the work bound is refused for it as well — by that
    // bound, never by nesting.
    assert_eq!(
        refusal(&sum_to_source(true, 3_000)),
        BudgetExhausted::ApplyTotal { limit: 2_000 },
    );
}
