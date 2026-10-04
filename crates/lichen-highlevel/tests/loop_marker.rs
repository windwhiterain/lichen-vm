//! The `@loop` marker's whole effect on a build: a marked recursion whose
//! state the evaluator cannot decide is refused **by name**, with the
//! conversion's verdict deciding which name — a shape rule, or a convertible
//! loop no backend emits yet — and a marked one the unroll already handles is
//! not refused at all.
//!
//! The end-to-end evidence is the probe
//! (`crates/lichen-language/examples/recursion.rs`); this pins the decision
//! itself, with no compute dependency — the facts that must not drift are that
//! the refusal names its cause, that a convertible shape yields the conversion
//! the JIT will read, and that the unmarked program is untouched.

use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::diagnostic::{Diag, DiagKind};
use lichen_highlevel::ir::{BinOp, ExprId, ExprKind, IR};
use lichen_highlevel::program::{HighProgramLiteral, IntLit, ProgramImpl};

/// `count n = if n == 0 then 0 else count (n - 1)`, applied to `argument`.
///
/// `marked` is the `@loop` the frontend stamps on a binding's function, and
/// `argument` is `None` for the `_` hole — a cell nothing ever decides, which
/// is what a run-time count looks like to the checker.
fn countdown(marked: bool, argument: Option<usize>) -> (ExprId, IR) {
    let mut ir = IR::new();
    // A block-wide binding reserves its own id before its value compiles, so the
    // recursive call in the body resolves to it (the frontend's block-root
    // discipline).
    let count = ir.alloc(ExprKind::Placeholder);
    ir.block_roots.insert(count);
    let n = ir.alloc(ExprKind::Parameter);
    let zero = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(0))));
    let condition = ir.alloc(ExprKind::BinOp {
        operator: BinOp::Eq,
        left: n,
        right: zero,
    });
    let one = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(1))));
    let decrement = ir.alloc(ExprKind::BinOp {
        operator: BinOp::Sub,
        left: n,
        right: one,
    });
    let recursive = ir.alloc(ExprKind::Apply {
        function: count,
        argument: decrement,
    });
    let base = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(0))));
    let branches = ir.alloc_tuple(&[recursive, base]);
    let body = ir.alloc(ExprKind::Field {
        container: branches,
        key: condition,
    });
    ir.set_kind(
        count,
        ExprKind::Function {
            parameter: n,
            parameter_type: None,
            parameter_attribute: None,
            r#return: body,
            parent: None,
            looping: marked,
        },
    );
    let argument = match argument {
        Some(value) => ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(value)))),
        None => ir.alloc(ExprKind::Placeholder),
    };
    let root = ir.alloc(ExprKind::Apply {
        function: count,
        argument,
    });
    ir.set_root(root);
    (root, ir)
}

/// `count n = if n == 0 then 0 else count (n - 1) + 1`: the recursive call is
/// an operand of the `Add`, not a branch of the return spine.
fn non_tail(marked: bool) -> (ExprId, IR) {
    let mut ir = IR::new();
    let count = ir.alloc(ExprKind::Placeholder);
    ir.block_roots.insert(count);
    let n = ir.alloc(ExprKind::Parameter);
    let zero = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(0))));
    let condition = ir.alloc(ExprKind::BinOp {
        operator: BinOp::Eq,
        left: n,
        right: zero,
    });
    let one = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(1))));
    let decrement = ir.alloc(ExprKind::BinOp {
        operator: BinOp::Sub,
        left: n,
        right: one,
    });
    let recursive = ir.alloc(ExprKind::Apply {
        function: count,
        argument: decrement,
    });
    let stepped = ir.alloc(ExprKind::BinOp {
        operator: BinOp::Add,
        left: recursive,
        right: one,
    });
    let base = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(0))));
    let branches = ir.alloc_tuple(&[stepped, base]);
    let body = ir.alloc(ExprKind::Field {
        container: branches,
        key: condition,
    });
    ir.set_kind(
        count,
        ExprKind::Function {
            parameter: n,
            parameter_type: None,
            parameter_attribute: None,
            r#return: body,
            parent: None,
            looping: marked,
        },
    );
    let argument = ir.alloc(ExprKind::Placeholder);
    let root = ir.alloc(ExprKind::Apply {
        function: count,
        argument,
    });
    ir.set_root(root);
    (root, ir)
}

/// `sum_to s = if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)`: a
/// **tuple** carried state, one slot per element, both read by path and both
/// stepped. Applied to the undecided hole, like the scalar case.
fn tuple_state(marked: bool) -> (ExprId, IR) {
    let mut ir = IR::new();
    let sum_to = ir.alloc(ExprKind::Placeholder);
    ir.block_roots.insert(sum_to);
    let s = ir.alloc(ExprKind::Parameter);
    let zero = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(0))));
    let one = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(1))));
    let first = ir.alloc(ExprKind::Field {
        container: s,
        key: zero,
    });
    let second = ir.alloc(ExprKind::Field {
        container: s,
        key: one,
    });
    let condition = ir.alloc(ExprKind::BinOp {
        operator: BinOp::Eq,
        left: first,
        right: zero,
    });
    let decrement = ir.alloc(ExprKind::BinOp {
        operator: BinOp::Sub,
        left: first,
        right: one,
    });
    let increment = ir.alloc(ExprKind::BinOp {
        operator: BinOp::Add,
        left: second,
        right: one,
    });
    let next = ir.alloc_tuple(&[decrement, increment]);
    let recursive = ir.alloc(ExprKind::Apply {
        function: sum_to,
        argument: next,
    });
    let branches = ir.alloc_tuple(&[recursive, second]);
    let body = ir.alloc(ExprKind::Field {
        container: branches,
        key: condition,
    });
    ir.set_kind(
        sum_to,
        ExprKind::Function {
            parameter: s,
            parameter_type: None,
            parameter_attribute: None,
            r#return: body,
            parent: None,
            looping: marked,
        },
    );
    let argument = ir.alloc(ExprKind::Placeholder);
    let root = ir.alloc(ExprKind::Apply {
        function: sum_to,
        argument,
    });
    ir.set_root(root);
    (root, ir)
}

fn build(range: (ExprId, IR)) -> Build<ProgramImpl> {
    Checker::build(range.1)
}

fn diagnostics(range: (ExprId, IR)) -> Vec<Diag<ProgramImpl>> {
    build(range).diagnostics()
}

fn kinds(range: (ExprId, IR)) -> Vec<DiagKind> {
    diagnostics(range).into_iter().map(|d| d.kind).collect()
}

/// The compiled function a marked build produced — the one the conversion is
/// asked about. The checker builds one function per `Function` expression, and
/// these harnesses mark exactly one.
fn marked_function(build: &Build<ProgramImpl>) -> lichen_lowlevel::FunctionId {
    build
        .module
        .functions
        .keys()
        .find(|&function| build.module.functions[function].looping)
        .expect("the harness marks exactly one function")
}

#[test]
fn a_convertible_marked_recursion_is_refused_as_not_emitted() {
    // The shape converts (it is a tail recursion with a base and a scalar
    // state), so the missing piece is the backend — not the program.
    let found = kinds(countdown(true, None));
    assert!(
        found.contains(&DiagKind::LoopNotEmitted),
        "a convertible `@loop` recursion must be refused as not emitted, got {found:?}"
    );
    assert!(
        !found.contains(&DiagKind::LoopNotRecorded),
        "a convertible shape must not be refused as unconvertible, got {found:?}"
    );
}

#[test]
fn an_unconvertible_marked_recursion_names_its_rule() {
    let found = diagnostics(non_tail(true));
    let refusal = found
        .iter()
        .find(|d| d.kind == DiagKind::LoopNotRecorded)
        .expect("a call outside tail position is not convertible");
    assert_eq!(
        refusal.field.as_deref(),
        Some("a recursive call not in tail position"),
        "the refusal must name the shape rule, got {found:?}"
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
    assert_eq!(conversion.exits[0].values.len(), 1);
}

#[test]
fn a_marked_tuple_state_converts_to_one_slot_per_element() {
    // The state the checker composes from `s(0)`/`s(1)` reads is a two-element
    // tuple: one slot per path, in element order, each step handing both.
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
    assert_eq!(conversion.exits[0].values.len(), 1);
}

#[test]
fn a_marked_recursion_the_unroll_handles_is_not_refused() {
    // The marker is permission, not a command: `count 3` is a trip count the
    // definition pass decides, so it is expanded exactly as an unmarked one is.
    let found = kinds(countdown(true, Some(3)));
    assert!(
        !found.contains(&DiagKind::LoopNotRecorded) && !found.contains(&DiagKind::LoopNotEmitted),
        "a decided state must still be expanded, got {found:?}"
    );
}

#[test]
fn an_unmarked_recursion_is_never_refused() {
    // Both states, marked or not, must reach the same answer with no marker —
    // the default is the unroll and it does not move.
    for argument in [None, Some(3)] {
        let found = kinds(countdown(false, argument));
        assert!(
            !found.contains(&DiagKind::LoopNotRecorded)
                && !found.contains(&DiagKind::LoopNotEmitted),
            "no `@loop` means no loop refusal, got {found:?}"
        );
    }
}

/// The named refusal is what a reader gets instead of a `NodeId`, so it is
/// worth pinning that it is a **guard**: it records a diagnostic without
/// fabricating a unification error, which is the shape the build's own
/// `check_failed` reads.
#[test]
fn the_refusal_makes_the_build_not_ok() {
    let build = build(countdown(true, None));
    assert!(
        !build.ok,
        "a refused `@loop` recursion must reject the build"
    );
}
