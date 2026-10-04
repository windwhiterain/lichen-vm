//! The host loop on **real programs**: a marked recursion in a plain lichen
//! program is run as a loop by the evaluator, so a trip count the expansion
//! cannot afford is answered rather than refused — and the answers are the
//! unroll's, which is the half that must not be assumed.
//!
//! The shape is the acceptance case's, minus the buffer read: a `@loop`
//! reduction over a tuple state with a **literal** count, so the host can see
//! the whole state and the only thing between it and a value is how the
//! recursion is run.

mod common;

use common::{evaluate, usize_of};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_language::compile;

/// `sum_to (n, 0)` = `n`, iterated: the reduction whose count is the answer.
fn sum_to_source(marked: bool, count: usize) -> String {
    let marker = if marked { "@loop " } else { "" };
    format!(
        "{marker}sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)\n\
         sum_to ({count}, 0)"
    )
}

#[test]
fn a_marked_reduction_runs_a_count_the_expansion_cannot_afford() {
    // Above the checker's apply budget (2_000 by default): the unroll would
    // nest 3_000 levels and be refused as non-terminating, while the loop pays
    // one instantiation per iteration and no nesting.
    let (_module, value, _ty) = evaluate(&sum_to_source(true, 3_000));
    assert_eq!(usize_of(&value), 3_000);
}

#[test]
fn the_same_reduction_without_the_marker_is_still_refused() {
    // The marker is the whole difference: an unmarked recursion of the same
    // shape keeps the unroll, which the work budget refuses.
    let report = compile(&sum_to_source(false, 3_000));
    assert!(!report.ok(), "the unroll must not afford 10_000 levels");
    assert!(
        report.diagnostics.iter().any(|d| d
            .check
            .as_ref()
            .is_some_and(|c| c.kind == DiagKind::NonTerminating)),
        "expected the work budget's refusal, got {:?}",
        report.diagnostics
    );
}

#[test]
fn the_loop_and_the_unroll_answer_the_same_value() {
    // Both paths run at a count they can both afford, so this compares the
    // loop's values with the expansion's rather than with the loop's own idea
    // of the answer.
    for count in [0usize, 1, 2, 7, 64] {
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
