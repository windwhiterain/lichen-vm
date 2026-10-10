//! The `@loop` marker's whole effect on a build: the frontend stamping it, the
//! conversion answering about the shape it marks, and the marked recursion the
//! unroll already handles staying untouched.
//!
//! **The build no longer refuses an undecided marked site**, and that is a
//! property of the feature rather than a gap: whether a loop is *emitted* is a
//! reader's fact, so the host loop and the kernel reader report the sites they
//! decline (`docs/notes/loop-conversion.md` §8.6). What the checker owns is the
//! shape answer, which is [`Module::loop_conversion`](lichen_lowlevel::Module::loop_conversion)
//! and is pinned below and in `crates/lichen-lowlevel/tests/basic/loop_conversion.rs`.
//!
//! The end-to-end evidence is the probe
//! (`crates/lichen-language/examples/recursion.rs`); this pins the decision
//! itself, with no compute dependency — the facts that must not drift are that
//! a convertible shape yields the conversion the JIT will read, that an
//! unconvertible one names its rule, and that the unmarked program is
//! untouched.

use lichen_highlevel::checker::{Build, Checker, WorkBudget};
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
fn a_convertible_marked_recursion_is_not_refused_by_the_build() {
    // The shape converts (it is a tail recursion with a base and a scalar
    // state) and nothing in the build refuses it: the decision about whether a
    // loop is *emitted* belongs to the reader that meets the site, not to the
    // checker.
    let found = kinds(countdown(true, None));
    assert!(
        !found.contains(&DiagKind::LoopNotEmitted) && !found.contains(&DiagKind::LoopNotRecorded),
        "a convertible `@loop` recursion must reach a reader, got {found:?}"
    );
}

#[test]
fn an_unconvertible_marked_recursion_names_its_rule() {
    // The refusal is the conversion's own answer, and this is where the rule it
    // names is pinned — `report_open_loop_sites` used to carry it into a
    // diagnostic, and the conversion is the single source either way.
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
    assert_eq!(conversion.exits.len(), 1);
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

/// **A marked site the checker does not refuse is a build that succeeds**, and
/// that is the whole of what moved: the reader that meets the site is the layer
/// that decides whether a loop is emitted (`docs/notes/loop-conversion.md` §8.6).
/// An undecided state is not a program error — it is a trip count the host
/// cannot see, which is exactly what a converted loop is for.
#[test]
fn an_undecided_marked_site_leaves_the_build_ok() {
    let build = build(countdown(true, None));
    assert!(
        build.ok,
        "an undecided marked site is the reader's question, not the build's: {:?}",
        build.diagnostics()
    );
}

/// **The loop's whole advantage, in the budget's terms, is nesting.** A host
/// that wants a large trip count states a large work total — the bound both
/// paths spend one application per count against — and then what a converted
/// loop never touches is the *nesting* guard. This pins the work side of that
/// statement: with the total raised, the loop answers a count the default total
/// refuses, and it does so without the markers of a run that nested.
#[test]
fn a_host_that_wants_a_large_trip_count_raises_the_work_bound() {
    // Under the default budget, the count is bounded by the *work* it costs —
    // one application per count — for the loop exactly as for the unroll.
    let default: Build<ProgramImpl> = build(countdown(true, Some(3_000)));
    assert!(
        !default.ok,
        "3_000 applications must exceed the checker's default work bound"
    );

    // Raised, the loop runs it. (The unroll answers it too, and that is the
    // honest half of the statement: in this lazy graph the expansion's applies
    // stay shallow, so it is not the *host* where nesting binds — it is a strict
    // recursion's depth guard (`tests/basic/host_loop.rs`), the emitter's
    // expression-nesting ceiling, and the kernel path.)
    let budget = WorkBudget {
        apply_depth_limit: 500,
        apply_total_limit: 100_000,
    };
    let (_root, ir) = countdown(true, Some(3_000));
    let looped: Build<ProgramImpl> = Checker::build_with_budget(ir, budget);
    assert!(
        looped.ok,
        "the raised work bound must let the loop run: {:?}",
        looped.diagnostics()
    );
    assert_eq!(
        looped.module.budget_exhausted, None,
        "and nothing refused along the way"
    );
}
