//! A marked recursion runs as a loop; see loop-conversion.md.
//!
//! # Invariant
//! The mark is **permission, not a command**: marked and unmarked must answer
//! the same value, differing only in what the expansion can afford.

use lichen_highlevel::ir::{BinOp, ExprKind};
use lichen_highlevel::program::{HighProgramLiteral, ProgramImpl, TupleTemplateLit};
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
    // The parameter's domain is the shared `Tuple` template: the layout, not
    // a type that carries the elements.
    let expr_ty = w.hole("expr_ty");
    w.fill(
        expr_ty,
        ExprKind::Literal(HighProgramLiteral::from(TupleTemplateLit)),
    );
    let sum_to = w.close_binding_typed(sum_to, s, expr_ty, body, marked);
    let argument = w.tuple("argument", &[w.int("count", count), w.int("acc", 0)]);
    let root = w.app("root", sum_to, argument);
    Probe::build(w.finish(root))
}

#[test]
fn the_loop_and_the_unroll_answer_the_same_value() {
    // This compares the loop's values with the expansion's, not with the loop's
    // own idea of the answer.
    //
    // The counts stop where the expansion is affordable: the unmarked
    // expansion's cost grows sharply with the trip count in the value-template
    // encoding, so the two answers are compared over the small counts only.
    for count in [0usize, 1, 2, 7] {
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
}
