//! A marked recursion runs as a loop; see loop-conversion.md.
//!
//! # Invariant
//! The mark is **permission, not a command**: marked and unmarked must answer
//! the same value, differing only in what the expansion can afford.

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::BinOp;
use lichen_highlevel::program::ProgramImpl;
use lichen_lowlevel::BudgetExhausted;
use lichen_testkit::{Probe, Writer};

/// `sum_to (n, 0) = n`, iterated: the reduction whose count is the answer.
///
/// # Invariant
/// `if c then t else e` lowers to `[e, t][c]`, so the branch is an array read.
fn sum_to(marked: bool, count: usize) -> Probe<ProgramImpl> {
    let w = Writer::new();
    let sum_to = w.binding("sum_to");
    let s = w.param("s");
    let zero = w.int("zero", 0);
    let first = w.field("first", s, zero);
    let second = w.field("second", s, w.int("one", 1));
    let condition = w.binop("condition", BinOp::Eq, first, zero);
    let decrement = w.binop("decrement", BinOp::Sub, first, w.int("step", 1));
    let increment = w.binop("increment", BinOp::Add, second, w.int("step2", 1));
    let next = w.tuple("next", &[decrement, increment]);
    let recursive = w.app("recursive", sum_to, next);
    let branches = w.tuple("branches", &[recursive, second]);
    let body = w.field("body", branches, condition);
    let int_ty = w.int_type("int_ty");
    let pair_ty = w.type_tuple("pair_ty", &[int_ty, int_ty]);
    let sum_to = w.close_binding_typed(sum_to, s, pair_ty, body, marked);
    let argument = w.tuple("argument", &[w.int("count", count), w.int("acc", 0)]);
    let root = w.app("root", sum_to, argument);
    Probe::build(w.finish(root))
}

/// The budget a refused program was stopped by.
fn refusal(probe: &Probe<ProgramImpl>) -> BudgetExhausted {
    probe
        .checked()
        .diagnostics()
        .into_iter()
        .find(|d| d.kind == DiagKind::NonTerminating)
        .and_then(|d| d.budget)
        .unwrap_or_else(|| panic!("no budget refusal in {:?}", probe.diagnostics()))
}

#[test]
fn the_loop_and_the_unroll_answer_the_same_value() {
    // This compares the loop's values with the expansion's, not with the loop's
    // own idea of the answer.
    for count in [0usize, 1, 2, 7, 64, 200] {
        let mut looped = sum_to(true, count);
        let mut unrolled = sum_to(false, count);
        assert_eq!(
            looped.usize_of("root"),
            unrolled.usize_of("root"),
            "the loop and the unroll must agree at count {count}"
        );
        assert_eq!(looped.usize_of("root"), count);
    }
}

#[test]
fn a_marked_reduction_runs_a_count_the_expansion_cannot_afford() {
    // 600 is past the nesting bound and inside the work bound: the expansion
    // deepens once per count, the loop never does.
    let mut looped = sum_to(true, 600);
    assert_eq!(
        looped.usize_of("root"),
        600,
        "the loop must answer a count the expansion cannot afford"
    );
    let unrolled = sum_to(false, 600);
    assert_eq!(
        refusal(&unrolled),
        BudgetExhausted::ApplyDepth { limit: 500 },
        "the same program unmarked must be refused by the *nesting* guard"
    );
}
