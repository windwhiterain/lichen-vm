//! The `@loop` marker's effect on a build; see docs/notes/loop-conversion.md.
//!
//! # Invariant
//! The checker owns only the shape answer (`Module::loop_conversion`): a
//! marked site it cannot decide is not a build failure, because whether a
//! loop is emitted is the reader's fact, and the reader reports the sites it
//! declines.

use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::diagnostic::{Diag, DiagKind};
use lichen_highlevel::ir::{BinOp, ExprKind};
use lichen_highlevel::program::{HighProgramLiteral, ProgramImpl, TupleTemplateLit};
use lichen_testkit::{Program, Writer};

/// The recursion under test, written under `count` and applied to `argument`.
fn countdown(marked: bool, argument: Option<usize>) -> Program {
    let w = Writer::new();
    // A block-wide binding reserves its id before its value compiles, so the
    // recursive call resolves to it.
    let count = w.binding("count");
    let n = w.param("n");
    let zero = w.int("zero", 0);
    let condition = w.binop("condition", BinOp::Eq, n, zero);
    let one = w.int("one", 1);
    let decrement = w.binop("decrement", BinOp::Sub, n, one);
    let recursive = w.app("recursive", count, decrement);
    let base = w.int("base", 0);
    let branches = w.tuple("branches", &[recursive, base]);
    let body = w.field("body", branches, condition);
    let count = w.close_binding(count, n, body, marked);
    let argument = match argument {
        Some(value) => w.int("argument", value),
        None => w.hole("argument"),
    };
    let root = w.app("root", count, argument);
    w.finish(root)
}

/// `count n = if n == 0 then 0 else count (n - 1) + 1`: the call sits in an
/// operand, not in the return spine.
fn non_tail(marked: bool) -> Program {
    let w = Writer::new();
    let count = w.binding("count");
    let n = w.param("n");
    let zero = w.int("zero", 0);
    let condition = w.binop("condition", BinOp::Eq, n, zero);
    let one = w.int("one", 1);
    let decrement = w.binop("decrement", BinOp::Sub, n, one);
    let recursive = w.app("recursive", count, decrement);
    let stepped = w.binop("stepped", BinOp::Add, recursive, one);
    let base = w.int("base", 0);
    let branches = w.tuple("branches", &[stepped, base]);
    let body = w.field("body", branches, condition);
    let count = w.close_binding(count, n, body, marked);
    let argument = w.hole("argument");
    let root = w.app("root", count, argument);
    w.finish(root)
}

/// `sum_to s = if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)`: a
/// tuple carried state, one slot per element.
fn tuple_state(marked: bool) -> Program {
    let w = Writer::new();
    let sum_to = w.binding("sum_to");
    let s = w.param("s");
    let zero = w.int("zero", 0);
    let one = w.int("one", 1);
    let first = w.field("first", s, zero);
    let second = w.field("second", s, one);
    let condition = w.binop("condition", BinOp::Eq, first, zero);
    let decrement = w.binop("decrement", BinOp::Sub, first, one);
    let increment = w.binop("increment", BinOp::Add, second, one);
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
    let argument = w.hole("argument");
    let root = w.app("root", sum_to, argument);
    w.finish(root)
}

fn build(program: Program) -> Build<ProgramImpl> {
    Checker::build(program.ir)
}

fn diagnostics(program: Program) -> Vec<Diag<ProgramImpl>> {
    build(program).diagnostics()
}

fn kinds(program: Program) -> Vec<DiagKind> {
    diagnostics(program).into_iter().map(|d| d.kind).collect()
}

/// The one marked function a marked build produced, which the conversion is
/// asked about.
fn marked_function(build: &Build<ProgramImpl>) -> lichen_lowlevel::FunctionId {
    build
        .module
        .functions
        .keys()
        .find(|&function| build.module.function_is_looping(function))
        .expect("the harness marks exactly one function")
}

#[test]
fn a_convertible_marked_recursion_is_not_refused_by_the_build() {
    let found = kinds(countdown(true, None));
    assert!(
        !found.contains(&DiagKind::LoopNotEmitted) && !found.contains(&DiagKind::LoopNotRecorded),
        "a convertible `@loop` recursion must reach a reader, got {found:?}"
    );
}

#[test]
fn an_unconvertible_marked_recursion_names_its_rule() {
    // The refusal is the conversion's own answer; the conversion is its single
    // source.
    let build = build(non_tail(true));
    let refusal = build
        .module
        .loop_conversion(marked_function(&build))
        .expect_err("a call outside tail position is not convertible");
    assert_eq!(
        refusal.name(),
        "a recursive call not in tail position",
        "the refusal must name the shape rule"
    );
}

#[test]
fn a_marked_scalar_recursion_converts_to_one_carried_slot() {
    let build = build(countdown(true, None));
    let conversion = build
        .module
        .loop_conversion(marked_function(&build))
        .expect("the countdown shape converts");
    assert_eq!(conversion.state, vec![Vec::<usize>::new()]);
    assert_eq!(conversion.tests.len(), 1);
    assert_eq!(conversion.steps.len(), 1);
    assert_eq!(conversion.exits.len(), 1);
    assert_eq!(conversion.steps[0].next.len(), 1);
    assert_eq!(conversion.exits.len(), 1);
}

#[test]
fn a_marked_tuple_state_converts_to_one_slot_per_element() {
    let build = build(tuple_state(true));
    let conversion = build
        .module
        .loop_conversion(marked_function(&build))
        .expect("the sum_to shape converts");
    assert_eq!(conversion.state, vec![vec![0], vec![1]]);
    assert_eq!(conversion.steps.len(), 1);
    assert_eq!(
        conversion.steps[0].next.len(),
        2,
        "both slots are handed to the recursive call"
    );
    assert_eq!(conversion.exits.len(), 1);
    assert_eq!(conversion.exits.len(), 1);
}

#[test]
fn a_marked_recursion_the_unroll_handles_is_not_refused() {
    // The marker is permission, not a command: a decided trip count is expanded
    // as if unmarked.
    let found = kinds(countdown(true, Some(3)));
    assert!(
        !found.contains(&DiagKind::LoopNotRecorded) && !found.contains(&DiagKind::LoopNotEmitted),
        "a decided state must still be expanded, got {found:?}"
    );
}

#[test]
fn an_unmarked_recursion_is_never_refused() {
    // No marker means the unroll, for both an undecided and a decided count.
    for argument in [None, Some(3)] {
        let found = kinds(countdown(false, argument));
        assert!(
            !found.contains(&DiagKind::LoopNotRecorded)
                && !found.contains(&DiagKind::LoopNotEmitted),
            "no `@loop` means no loop refusal, got {found:?}"
        );
    }
}

/// An undecided marked site is a trip count the host cannot see, not a build
/// failure.
#[test]
fn an_undecided_marked_site_leaves_the_build_ok() {
    let build = build(countdown(true, None));
    assert!(
        build.ok,
        "an undecided marked site is the reader's question, not the build's: {:?}",
        build.diagnostics()
    );
}
