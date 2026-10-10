//! The host loop: a marked, convertible recursion runs as a loop instead of being expanded.

use super::*;
use lichen_lowlevel::BudgetExhausted;

/// Build a function, run the definition pass over it, then call it with `argument`:

/// the answer and the run's budget verdict. `tune` sets the budgets before the build.
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
    // An undecided answer (the loop's test did not decide, a budget refused)
    // is no value at all.
    let answer =
        value.and_then(|value| matches!(value, TestValue::U128(_)).then(|| u128_of(value)));
    (answer, m.budget_exhausted)
}

#[test]
fn the_depth_a_node_carries_is_the_levels_it_was_created_under() {
    // A node records how many apply levels it was created under, from the apply node
    // it is instantiated for.

    // An expansion deepens once per level; a converted loop instantiates the same
    // entering apply node every iteration.
    let deepest = |marked: bool, count: u128| {
        let mut m = Module::new();
        let (function, id, _, _, _) = countdown(&mut m, marked);
        m.evaluate_node_deep(function, None);
        m.evaluate_node_deep(m.functions[id].r#return, None);
        let root = m.add_block(None);
        let argument = u128_node(&mut m, root, count);
        let call = call_node(&mut m, root, function, argument);
        m.evaluate_node_deep(call, None);
        m.nodes
            .keys()
            .map(|node| m.node_depth(node))
            .max()
            .expect("a module has nodes")
    };

    assert!(
        deepest(false, 20) >= 19,
        "the expansion must deepen once per level, got {}",
        deepest(false, 20)
    );
    assert!(
        deepest(true, 20) <= 2,
        "the loop must stay at the entering apply node's depth, got {}",
        deepest(true, 20)
    );
    // And the loop's depth does not grow with the count, which is the claim
    // that matters: it is flat, not merely small.
    assert_eq!(deepest(true, 20), deepest(true, 400));
}

#[test]
fn a_marked_loop_answers_what_the_unroll_answers() {
    // The marked and unmarked runs must agree, on the value and on the budget verdict.
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
fn the_loop_spends_one_application_per_iteration() {
    // Each iteration is one application, the same unit an unwound level would have cost.

    // `count 17` therefore spends 18 applications; one fewer is refused, so the loop went round.
    let short = run(
        |m| m.apply_total_limit = 18,
        |m| {
            let (function, id, _, _, _) = countdown(m, true);
            (function, id)
        },
        17,
    );
    assert_eq!(
        short,
        (None, Some(BudgetExhausted::ApplyTotal { limit: 18 }))
    );
    let exact = run(
        |m| m.apply_total_limit = 19,
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
    // Both paths apply once per count, but the expansion nests and the loop does not.
    let count = 50u128;
    let unrolled = run(
        |m| m.apply_depth_limit = 8,
        |m| {
            let (function, id, _, _, _) = countdown(m, false);
            (function, id)
        },
        count,
    );
    assert_eq!(
        unrolled,
        (None, Some(BudgetExhausted::ApplyDepth { limit: 8 })),
        "the expansion must be refused by the nesting guard"
    );
    let looped = run(
        |m| m.apply_depth_limit = 8,
        |m| {
            let (function, id, _, _, _) = countdown(m, true);
            (function, id)
        },
        count,
    );
    assert_eq!(
        looped,
        (Some(0), None),
        "the loop must run at one level however many iterations it takes"
    );
}

#[test]
fn a_marked_loop_that_never_reaches_its_base_is_refused_by_the_work_budget() {
    // The work budget turns an endless loop into a diagnostic instead of a hang.

    // The expansion's nesting guard never fires here, which is why the total bound has to.
    let refused = run(|m| m.apply_total_limit = 5, |m| stuck_loop(m, true), 5);
    assert_eq!(
        refused,
        (None, Some(BudgetExhausted::ApplyTotal { limit: 5 }))
    );
}

#[test]
fn an_unconvertible_marked_recursion_still_expands() {
    // The marker is permission to convert, not a promise: a refused shape keeps the unroll.
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
