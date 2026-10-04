//! The host loop on **real programs**: a marked recursion in a plain lichen
//! program is run as a loop by the evaluator, and the answers are the unroll's —
//! which is the half that must not be assumed.
//!
//! The shape is the acceptance case's, minus the buffer read: a `@loop`
//! reduction over a tuple state with a **literal** count, so the host can see
//! the whole state and the only thing between it and a value is how the
//! recursion is run.
//!
//! **What the loop is and is not, in the budget's terms.** An iteration is one
//! application, exactly as an unwound level is, so the *work* budget bounds both
//! the same way: a large trip count is refused for the loop too, and a host that
//! wants one raises that bound. What a loop never spends is **nesting** — and
//! that side needs a budget whose total is large and whose nesting bound is
//! small, which is a build-level knob (`lichen-highlevel`'s `loop_marker` pins
//! it there; the language entry point takes only the defaults).

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
fn a_marked_reduction_is_bounded_by_the_same_work_budget() {
    // The loop is not a way around the *work* budget: an iteration is an
    // application, so a trip count the expansion could not afford in work is
    // refused for the loop too. The bound a host raises for a large trip count
    // is therefore that one — the loop only removes the nesting.
    let report = compile(&sum_to_source(true, 3_000));
    assert!(!report.ok(), "3_000 iterations must exceed the work budget");
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
fn an_unmarked_reduction_is_refused_by_a_budget_of_the_same_family() {
    // The same shape without the marker, at the same count: refused as well.
    // Which of the two bounds a program meets is the loop's whole difference,
    // and it is the nesting one it stops meeting.
    let report = compile(&sum_to_source(false, 3_000));
    assert!(!report.ok(), "the unroll must not afford 3_000 levels");
    assert!(
        report.diagnostics.iter().any(|d| d
            .check
            .as_ref()
            .is_some_and(|c| c.kind == DiagKind::NonTerminating)),
        "expected a budget refusal, got {:?}",
        report.diagnostics
    );
}
