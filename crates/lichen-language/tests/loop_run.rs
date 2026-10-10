//! A marked recursion runs as a loop, so a trip count the expansion cannot
//! afford is answered. See loop-conversion.md.

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
    // This compares the loop's values with the expansion's, not with the loop's own
    // idea of the answer.
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
    // 600 is past the nesting bound and inside the work bound: the expansion
    // deepens once per count, the loop never does.
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
    // The loop is not exempt from work: an iteration is one application.
    assert_eq!(
        refusal(&sum_to_source(true, 3_000)),
        BudgetExhausted::ApplyTotal { limit: 2_000 },
    );
}
