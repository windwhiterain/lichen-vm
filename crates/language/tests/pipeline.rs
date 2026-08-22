//! End-to-end tests: source text → `language::compile` → checked build →
//! evaluation, and the diagnostics (frontend + checker) with their spans.

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::program::{HighProgram, HighValue};
use lichen_lowlevel::{Module, NodeId, Value};

use language::diag::Stage;
use language::{compile, frontend};

/// Compile and run a program, asserting it checks; returns the module and the
/// root value node.
fn run(source: &str) -> (Module<HighProgram>, NodeId) {
    let report = compile(source);
    assert!(
        report.ok(),
        "expected {source:?} to check, got: {:?}",
        report.diagnostics
    );
    let build = report.build.unwrap();
    let root = build.root_val;
    (build.module, root)
}

fn evaluate(source: &str) -> Value<HighProgram> {
    let (mut module, root) = run(source);
    module.evaluate_node_deep(root, None)
}

/// The node ids of an array value.
fn array_ids(value: Value<HighProgram>) -> Vec<NodeId> {
    let Value::Array(ptr) = value else {
        panic!("expected an array value, got {value:?}");
    };
    unsafe { &*ptr }.to_vec()
}

fn usize_of(value: &Value<HighProgram>) -> usize {
    let Value::USize(n) = value else {
        panic!("expected a usize value, got {value:?}");
    };
    *n
}

/// The rendered diagnostics of a failing program.
fn diags(source: &str) -> Vec<language::Diag> {
    let report = compile(source);
    assert!(!report.diagnostics.is_empty(), "{source:?} should fail");
    report.diagnostics
}

// --- well-typed programs ----------------------------------------------------

#[test]
fn an_int_literal_checks_and_evaluates() {
    assert_eq!(usize_of(&evaluate("5")), 5);
}

#[test]
fn an_annotated_int_checks() {
    assert_eq!(usize_of(&evaluate("5 : Int")), 5);
}

#[test]
fn applying_a_lambda_checks_and_evaluates() {
    assert_eq!(usize_of(&evaluate("(x => x) 5 : Int")), 5);
}

#[test]
fn a_binder_used_once_checks() {
    // The root apply's result cell is lazy, so the whole program is annotated
    // to anchor its type.
    assert_eq!(
        usize_of(&evaluate("(((id => (id 5 : Int)) (x => x)) : Int)")),
        5
    );
}

#[test]
fn the_polymorphic_identity_checks() {
    // One binder used at Int and at Type — every lambda is automatically
    // let-polymorphic.
    let (module, root) = run("(((id => ((id 5 : Int), (id Type : Type))) (x => x)) : <Int, Type>)");
    let mut module = module;
    let value = module.evaluate_node_deep(root, None);
    let ids = array_ids(value);
    assert_eq!(ids.len(), 2, "the tuple has two elements");
    assert_eq!(usize_of(module.nodes[ids[0]].value.as_ref().unwrap()), 5);
    assert!(
        matches!(
            module.nodes[ids[1]].value,
            Some(Value::Ext(HighValue::TypeType))
        ),
        "the second element is the Type constant"
    );
}

#[test]
fn a_heterogeneous_tuple_checks() {
    run("(1, (x => x))");
}

#[test]
fn an_array_literal_checks_against_its_array_type() {
    assert_eq!(array_ids(evaluate("([1, 2, 3] : Int<3>)")).len(), 3);
}

#[test]
fn a_homogeneous_array_of_lambdas_checks() {
    // [x => x, x => x] — each lambda has its own fresh unbound arrow type;
    // the element check unifies the two shapes (`?a → ?a` with `?b → ?b`),
    // so the array is homogeneous.  Different binder names are the same
    // shape.  The root type is a determined array-of-arrow, so there is no
    // ambiguity diagnostic.
    assert_eq!(array_ids(evaluate("[x => x, x => x]")).len(), 2);
    assert_eq!(array_ids(evaluate("[y => y, x => x]")).len(), 2);
}

#[test]
fn an_index_selects_an_element() {
    // ([1, 2, 3])[1] — a literal array indexed by a literal; the type side
    // indexes the element-type list structurally, so it checks and selects.
    assert_eq!(usize_of(&evaluate("([1, 2, 3])[1]")), 2);
}

#[test]
fn an_index_with_a_runtime_index_selects() {
    // (i => [10, 20][i]) 1 — the index is a parameter, so the check cannot
    // know it; the length check and the selection happen at runtime (the
    // lowlevel Index operator).  The root apply is annotated to anchor its
    // lazy result cell.
    assert_eq!(usize_of(&evaluate("((i => [10, 20][i]) 1 : Int)")), 20);
}

// --- statements and bindings -------------------------------------------------

#[test]
fn statement_bindings_check_and_evaluate() {
    // `a = [1, 2]; b = 0; a[b]` — each binding compiles its value once into
    // the IR graph and every use of the name is that node; the root is the
    // final expression itself (no desugared application), so the program
    // checks and evaluates.
    assert_eq!(usize_of(&evaluate("a = [1, 2]; b = 0; a[b]")), 1);
    assert_eq!(usize_of(&evaluate("a = 5; a")), 5);
}

#[test]
fn a_binding_can_shadow_an_earlier_one() {
    assert_eq!(usize_of(&evaluate("a = 1; a = 2; a")), 2);
}

#[test]
fn a_binding_used_twice_shares_one_node() {
    // `a = 5; (a, a)` — the two uses are the same compiled node; the tuple
    // holds two fives.
    let (module, root) = run("a = 5; (a, a)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
    assert_eq!(ids.len(), 2);
    assert_eq!(usize_of(module.nodes[ids[0]].value.as_ref().unwrap()), 5);
    assert_eq!(usize_of(module.nodes[ids[1]].value.as_ref().unwrap()), 5);
}

#[test]
fn a_bound_lambda_is_still_polymorphic() {
    // The shared function node keeps per-apply fresh clones, so one binding
    // used at Int and at Type still checks — graph sharing does not
    // monomorphize functions.
    let (module, root) = run("a = x => x; ((a 5 : Int), (a Type : Type))");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
    assert_eq!(ids.len(), 2);
    assert_eq!(usize_of(module.nodes[ids[0]].value.as_ref().unwrap()), 5);
    assert!(
        matches!(
            module.nodes[ids[1]].value,
            Some(Value::Ext(HighValue::TypeType))
        ),
        "the second element is the Type constant"
    );
}

#[test]
fn a_binding_value_can_reference_an_earlier_binding() {
    // `a = [1, 2]; b = a[0]; b` — the later binding's value reads the
    // earlier one through the graph.
    assert_eq!(usize_of(&evaluate("a = [1, 2]; b = a[0]; b")), 1);
}

#[test]
fn a_statement_program_with_an_out_of_bounds_index_is_rejected() {
    let d = diags("a = [1, 2]; a[5]");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::IndexOutOfBounds);
    assert_eq!(check.value_a, Some(Value::USize(5)), "the index");
    assert_eq!(check.value_b, Some(Value::USize(2)), "the length");
}

#[test]
fn an_unresolved_name_in_a_statement_program_is_reported() {
    let d = diags("a = 5; y");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Resolve);
    assert_eq!(d[0].message, "unresolved name 'y'");
    assert_eq!(d[0].span, Some((1, 8)));
}

#[test]
fn a_function_type_is_a_first_class_value() {
    // `Int -> Int` checks in term position; the identity is such a function.
    run("(x => x) : (Int -> Int)");
}

#[test]
fn a_dependent_array_length_pins_the_parameter() {
    // `Int<n>` with a bound `n`: the check resolves the length read to a pure
    // reference of `n`'s cell and pins it to the literal's length — the
    // parameter is monomorphized, and applying the pinned length checks and
    // runs.  (The root apply is annotated to anchor its lazy result cell.)
    assert_eq!(
        array_ids(evaluate("(((n => ([1, 2, 3] : Int<n>)) 3) : Int<3>)")).len(),
        3
    );
}

#[test]
fn a_dependent_array_length_rejects_other_lengths() {
    // `n` is pinned to 3 by the annotation; applying 5 clashes at the apply
    // (a runtime failure — the parameter's expected value against the
    // argument).
    let d = diags("((n => ([1, 2, 3] : Int<n>)) 5)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Runtime);
    assert_eq!(check.value_a, Some(Value::USize(3)), "the pinned length");
    assert_eq!(check.value_b, Some(Value::USize(5)), "the argument");
}

// --- ill-typed programs -----------------------------------------------------

#[test]
fn an_unresolved_name_is_reported() {
    let d = diags("x => y");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Resolve);
    assert_eq!(d[0].message, "unresolved name 'y'");
    assert_eq!(d[0].span, Some((1, 6)));
    assert!(
        compile("x => y").build.is_none(),
        "the frontend failed first"
    );
}

#[test]
fn an_annotation_mismatch_reports_expected_and_found() {
    let d = diags("5 : Int -> Int");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Check);
    assert_eq!(d[0].span, Some((1, 1)));
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
    assert_eq!(check.value_a, Some(Value::Ext(HighValue::TypeInt)));
    // the expected side is the arrow type — an array of two elements
    assert_eq!(
        array_ids(check.value_b.expect("the expected arrow type")).len(),
        2
    );
}

#[test]
fn a_literal_in_type_position_is_a_kinding_error() {
    // 5 : 5 — the type expression `5` is not a kind; the annotation also
    // fails with its own mismatch.
    let d = diags("5 : 5");
    assert_eq!(d.len(), 2);
    // kinding: the type expression `5` is not a kind — its type expression
    // pair `[int, Type]` is the found side
    let first = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(first.kind, DiagKind::Kinding);
    assert_eq!(
        array_ids(first.value_a.expect("the found type expression")).len(),
        2
    );
    // the annotation against the literal type expression
    let second = d[1].check.as_ref().expect("a checker diagnostic");
    assert_eq!(second.kind, DiagKind::Annotation);
    assert_eq!(second.value_a, Some(Value::Ext(HighValue::TypeInt)));
    assert_eq!(second.value_b, Some(Value::USize(5)));
}

#[test]
fn applying_a_non_function_is_a_guard_error() {
    let d = diags("(5 3)");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].span, Some((1, 2)), "the apply starts at the `5`");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Guard);
    assert_eq!(check.value_a, Some(Value::Ext(HighValue::TypeInt)));
}

#[test]
fn a_bare_lambda_checks() {
    // The root type is the arrow `?a → ?a` — unbound components, but the
    // arrow shape is determined, so there is no ambiguity diagnostic.
    let report = compile("x => x");
    assert!(report.ok(), "bare lambdas check: {:?}", report.diagnostics);
}

#[test]
fn an_unannotated_call_reports_ambiguity() {
    let d = diags("((id => id 5) (x => x))");
    assert_eq!(d.len(), 1);
    assert_eq!(
        d[0].check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::Ambiguity
    );
}

#[test]
fn a_heterogeneous_array_is_rejected() {
    let d = diags("[1, x => x]");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::ArrayElement);
    assert_eq!(check.value_b, Some(Value::Ext(HighValue::TypeInt)));
    // the found side is the lambda's arrow shape — an array of two cells
    assert_eq!(
        array_ids(check.value_a.expect("the found arrow shape")).len(),
        2
    );
}

#[test]
fn an_array_of_the_wrong_length_is_rejected() {
    let d = diags("([1, 2] : Int<3>)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
    assert_eq!(check.value_a, Some(Value::USize(2)));
    assert_eq!(check.value_b, Some(Value::USize(3)));
}

#[test]
fn an_out_of_bounds_index_is_rejected() {
    // The type side indexes the element-type list structurally, so the
    // bounds check fires at check time; the diagnostic carries the index and
    // the length, at the index's span.
    let d = diags("([1, 2, 3])[5]");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::IndexOutOfBounds);
    assert_eq!(check.value_a, Some(Value::USize(5)), "the index");
    assert_eq!(check.value_b, Some(Value::USize(3)), "the length");
    assert_eq!(d[0].span, Some((1, 13)), "the index's span");
}

// --- the frontend -----------------------------------------------------------

#[test]
fn the_frontend_builds_a_rooted_table() {
    let ir = frontend("(x => x) 5").unwrap();
    assert_eq!(
        ir.root,
        lichen_highlevel::ir::ExprId(ir.expr.len() as u32 - 1)
    );
}

// --- garbage never panics ---------------------------------------------------

#[test]
fn garbage_input_never_panics() {
    for source in [
        "", "(", "x =>", "3 :", "\\", "@", "x )", "f x => e", "(,)", "[ ]", "<", "<Int>", "->",
        "5 : ",
    ] {
        let report = compile(source);
        assert!(
            !report.diagnostics.is_empty(),
            "{source:?} must produce a diagnostic"
        );
        assert!(
            report.build.is_none(),
            "the frontend must fail before the checker for {source:?}"
        );
    }
}

// --- rendering --------------------------------------------------------------

#[test]
fn diagnostics_render_with_carets() {
    let d = diags("x => y");
    let out = language::render::render("x => y", &d[0]);
    assert_eq!(
        out,
        "error: unresolved name 'y'\n  --> 1:6\n   |\n 1 | x => y\n   |      ^\n"
    );
}
