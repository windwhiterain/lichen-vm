//! The `@loop` marker's whole effect on a build: a marked recursion whose
//! state the evaluator cannot decide is refused **by name**, and a marked one
//! the unroll already handles is not refused at all.
//!
//! The end-to-end evidence is the probe
//! (`crates/lichen-language/examples/recursion.rs`); this pins the decision
//! itself, with no compute dependency — the two facts that must not drift are
//! that the refusal names its cause, and that the unmarked program is untouched.

use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::diagnostic::DiagKind;
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

fn kinds(marked: bool, argument: Option<usize>) -> Vec<DiagKind> {
    let (_root, ir) = countdown(marked, argument);
    let build: Build<ProgramImpl> = Checker::build(ir);
    build.diagnostics().into_iter().map(|d| d.kind).collect()
}

#[test]
fn a_marked_recursion_with_an_undecided_state_is_refused_by_name() {
    let found = kinds(true, None);
    assert!(
        found.contains(&DiagKind::LoopNotRecorded),
        "the marker's refusal must be a named one, got {found:?}"
    );
}

#[test]
fn a_marked_recursion_the_unroll_handles_is_not_refused() {
    // The marker is permission, not a command: `count 3` is a trip count the
    // definition pass decides, so it is expanded exactly as an unmarked one is.
    let found = kinds(true, Some(3));
    assert!(
        !found.contains(&DiagKind::LoopNotRecorded),
        "a decided state must still be expanded, got {found:?}"
    );
}

#[test]
fn an_unmarked_recursion_is_never_refused() {
    // Both states, marked or not, must reach the same answer with no marker —
    // the default is the unroll and it does not move.
    for argument in [None, Some(3)] {
        let found = kinds(false, argument);
        assert!(
            !found.contains(&DiagKind::LoopNotRecorded),
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
    let (_root, ir) = countdown(true, None);
    let build: Build<ProgramImpl> = Checker::build(ir);
    assert!(
        !build.ok,
        "a refused `@loop` recursion must reject the build"
    );
}
