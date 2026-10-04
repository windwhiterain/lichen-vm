//! The host loop: a marked, convertible recursion **runs as a loop** instead of
//! being expanded — same values, and a trip count the expansion could not
//! afford. The equivalence half is what matters most: the loop and the unroll
//! are two code paths for one program, so every shape the conversion accepts
//! must answer what the unroll answers, and what they may differ in is what
//! they can afford.

use super::*;
use lichen_lowlevel::BudgetExhausted;

/// Build a function, run the **definition pass** over it (its own value and its
/// body, before any apply — what a host does), call it with `argument` in a
/// fresh root block, and deep-evaluate the call: the answer and the run's budget
/// verdict. `tune` runs before the build, so a test can set the budgets the run
/// is measured against.
fn run(
    tune: impl FnOnce(&mut Module<TestProgram>),
    build: impl FnOnce(&mut Module<TestProgram>) -> (NodeId, FunctionId),
    argument: u128,
) -> (Option<u128>, Option<BudgetExhausted>) {
    let mut m = Module::new();
    tune(&mut m);
    let (function, id) = build(&mut m);
    m.evaluate_node_deep(function, None);
    m.evaluate_node_deep(m.functions[id].r#return, None);
    let root = m.add_block(None);
    let argument = u128_node(&mut m, root, argument);
    let call = call_node(&mut m, root, function, argument);
    let value = m.evaluate_node_deep(call, None);
    let answer = matches!(value, TestValue::U128(_)).then(|| u128_of(value));
    (answer, m.budget_exhausted)
}

#[test]
fn a_marked_loop_answers_what_the_unroll_answers() {
    // The same program, marked and unmarked: the marked one takes the loop and
    // the unmarked one the unroll, and the two must not disagree — on the value
    // *or* on the budget verdict.
    for count in [0u128, 1, 2, 17] {
        let looped = run(
            |_| {},
            |m| {
                let (function, id, _, _, _) = countdown(m, true);
                (function, id)
            },
            count,
        );
        let unrolled = run(
            |_| {},
            |m| {
                let (function, id, _, _, _) = countdown(m, false);
                (function, id)
            },
            count,
        );
        assert_eq!(
            looped, unrolled,
            "the loop and the unroll must agree at count {count}"
        );
    }
}

#[test]
fn the_loop_iterates_once_per_count() {
    // The count *is* the iteration count: `count 17` tests 17 states that fail
    // the base test and one that passes it. A limit one below that is refused
    // and a limit that fits answers, which is what says the loop really went
    // round rather than reaching the base some other way.
    let short = run(
        |m| m.loop_work_limit = 17,
        |m| {
            let (function, id, _, _, _) = countdown(m, true);
            (function, id)
        },
        17,
    );
    assert_eq!(short, (None, Some(BudgetExhausted::LoopWork { limit: 17 })));
    let exact = run(
        |m| m.loop_work_limit = 18,
        |m| {
            let (function, id, _, _, _) = countdown(m, true);
            (function, id)
        },
        17,
    );
    assert_eq!(exact, (Some(0), None));
}

#[test]
fn a_marked_loop_runs_a_count_the_unroll_cannot_afford() {
    // The expansion pays one application per level, so a count above the apply
    // budget is refused as non-terminating; the loop pays one instantiation per
    // iteration and no nesting, which is the whole point of the conversion.
    let count = 5_000u128;
    let unrolled = run(
        |m| m.apply_total_limit = 100,
        |m| {
            let (function, id, _, _, _) = countdown(m, false);
            (function, id)
        },
        count,
    );
    assert_eq!(
        unrolled,
        (None, Some(BudgetExhausted::ApplyTotal { limit: 100 })),
        "the expansion must be refused by the apply budget"
    );
    let looped = run(
        |m| m.apply_total_limit = 100,
        |m| {
            let (function, id, _, _, _) = countdown(m, true);
            (function, id)
        },
        count,
    );
    assert_eq!(
        looped,
        (Some(0), None),
        "the loop must answer without spending the apply budget"
    );
}

#[test]
fn a_marked_loop_that_never_reaches_its_base_is_refused_by_the_loop_budget() {
    // `stuck 5` tests `5 < 1`, fails, and recurses with the same state forever:
    // a *convertible* shape, so the loop runs it — and the loop's own budget is
    // what turns that into a diagnostic instead of a hang.
    let refused = run(|m| m.loop_work_limit = 5, |m| stuck_loop(m, true), 5);
    assert_eq!(
        refused,
        (None, Some(BudgetExhausted::LoopWork { limit: 5 }))
    );
}

#[test]
fn an_unconvertible_marked_recursion_still_expands() {
    // The marker is permission to convert, not a promise: a shape the
    // conversion refuses (both calls are operands of the `Add`) keeps the
    // unroll, which is why marking it changes nothing.
    let marked = run(
        |_| {},
        |m| {
            let (function, id) = fibonacci(m);
            m.mark_looping(id);
            (function, id)
        },
        6,
    );
    assert_eq!(
        marked,
        (Some(8), None),
        "the marked non-loop keeps the unroll's answer"
    );
}
