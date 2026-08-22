//! The highlevel checker: compiles an ExprTable into a lowlevel Module where
//! the runtime *is* the typechecker — values are recursive pairs
//! `[value, type]` whose type slots are themselves pairs bottoming out at
//! the self-referential `Type : Type` universe, and the apply-time unify is
//! the parameter type check.

use lichen_highlevel::checker::Checker;
use lichen_highlevel::expr::{ExprId, ExprKind, ExprTable};
use lichen_highlevel::program::HighValue;
use lichen_lowlevel::Value;

// --- hand-built IR helpers (the language frontend will produce these) -----

fn int(ir: &mut ExprTable, n: u64) -> ExprId {
    ir.alloc(ExprKind::Int(n), None)
}
fn ty(ir: &mut ExprTable) -> ExprId {
    ir.alloc(ExprKind::Type, None)
}
fn int_t(ir: &mut ExprTable) -> ExprId {
    ir.alloc(ExprKind::Const(HighValue::Int), None)
}
fn binder(ir: &mut ExprTable) -> ExprId {
    ir.alloc(ExprKind::Binder, None)
}
fn var(ir: &mut ExprTable, b: ExprId) -> ExprId {
    ir.alloc(ExprKind::Var(b), None)
}
fn lam(ir: &mut ExprTable, b: ExprId, body: ExprId) -> ExprId {
    ir.alloc(ExprKind::Lam(b, body), None)
}
fn app(ir: &mut ExprTable, f: ExprId, x: ExprId) -> ExprId {
    ir.alloc(ExprKind::App(f, x), None)
}
fn let_(ir: &mut ExprTable, b: ExprId, v: ExprId, body: ExprId) -> ExprId {
    ir.alloc(ExprKind::Let(b, v, body), None)
}
fn ann(ir: &mut ExprTable, e: ExprId, t: ExprId) -> ExprId {
    ir.alloc(ExprKind::Ann(e, t), None)
}
fn arrow(ir: &mut ExprTable, d: ExprId, c: ExprId) -> ExprId {
    ir.alloc(ExprKind::Arrow(d, c), None)
}
fn array(ir: &mut ExprTable, elements: &[ExprId]) -> ExprId {
    ir.alloc_array(elements, None)
}

fn build(root: ExprId, mut ir: ExprTable) -> lichen_highlevel::checker::Build {
    ir.set_root(root);
    Checker::build(ir)
}

/// The ids inside a checker-built array value.
fn array_ids(
    b: &lichen_highlevel::checker::Build,
    node: lichen_lowlevel::NodeId,
) -> Vec<lichen_lowlevel::NodeId> {
    let Some(Value::Array(ptr)) = b.module.nodes[node].value else {
        panic!("expected an array value")
    };
    unsafe { &*ptr }.to_vec()
}

// --- checking -------------------------------------------------------------

#[test]
fn int_literal_checks() {
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let b = build(five, ir);
    assert!(b.ok, "5 should check");
    // The type of 5 is the recursive pair [int, [Type, ↺]].
    assert_eq!(b.ty[five], Some(b.int_type));
    let ids = array_ids(&b, b.int_type);
    assert_eq!(ids.len(), 2);
    assert!(matches!(
        b.module.nodes[ids[0]].value,
        Some(Value::Ext(HighValue::Int))
    ));
    assert_eq!(ids[1], b.type_expr, "the type of int must be the Type universe");
}

#[test]
fn annotated_literal_checks() {
    // 5 : int
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let t = int_t(&mut ir);
    let a = ann(&mut ir, five, t);
    let b = build(a, ir);
    assert!(b.ok, "5 : int should check");
    assert_eq!(b.ty[a], Some(b.int_type));
}

#[test]
fn the_type_universe_is_self_referential() {
    // `Type` compiles to the canonical node K = [Type, K] — Type : Type via
    // a self-cycle, closing every type spine.
    let mut ir = ExprTable::new();
    let t = ty(&mut ir);
    let b = build(t, ir);
    assert!(b.ok);
    assert_eq!(b.term[t], Some(b.type_expr));
    assert_eq!(b.ty[t], Some(b.type_expr), "Type : Type");
    let ids = array_ids(&b, b.type_expr);
    assert_eq!(ids.len(), 2);
    assert!(matches!(
        b.module.nodes[ids[0]].value,
        Some(Value::Ext(HighValue::Type))
    ));
    assert_eq!(ids[1], b.type_expr, "the universe's type slot cycles back to itself");
}

#[test]
fn literal_against_type_fails() {
    // 5 : Type
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let t = ty(&mut ir);
    let a = ann(&mut ir, five, t);
    let b = build(a, ir);
    assert!(!b.ok, "5 : Type must fail");
    assert!(!b.module.unify_errors.is_empty());
}

#[test]
fn lambda_has_arrow_type() {
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let l = lam(&mut ir, x, body);
    let b = build(l, ir);
    assert!(b.ok, "\\x. x should check");
    // The lambda's type is the kinded arrow [[?a, ?a], [FunctionType, Type]].
    let arrow = b.ty[l].unwrap();
    let ids = array_ids(&b, arrow);
    assert_eq!(ids.len(), 2, "a type expression is a pair [shape, kind]");
    let kind_ids = array_ids(&b, ids[1]);
    assert_eq!(kind_ids.len(), 2);
    assert!(matches!(
        b.module.nodes[kind_ids[0]].value,
        Some(Value::Ext(HighValue::FunctionType))
    ));
    assert_eq!(kind_ids[1], b.type_expr);
}

#[test]
fn lambda_checks_against_arrow_annotation() {
    // (\x. x) : int → int
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let l = lam(&mut ir, x, body);
    let d = int_t(&mut ir);
    let c = int_t(&mut ir);
    let t = arrow(&mut ir, d, c);
    let a = ann(&mut ir, l, t);
    let b = build(a, ir);
    assert!(b.ok, "(\\x. x) : int → int should check");
}

#[test]
fn annotating_a_lambda_with_a_tuple_type_fails() {
    // (\x. x) : [int, int] — a tuple type, not a function type: the kind
    // markers now distinguish the two, so this must fail.
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let l = lam(&mut ir, x, body);
    let d = int_t(&mut ir);
    let c = int_t(&mut ir);
    let t = array(&mut ir, &[d, c]);
    let a = ann(&mut ir, l, t);
    let b = build(a, ir);
    assert!(!b.ok, "(\\x. x) : [int, int] must fail (a lambda is not a tuple)");
}

#[test]
fn typed_array_is_a_kinded_tuple() {
    // [1, 2] — the pair [[1, 2], [[int, int], [ArrayType, Type]]].
    let mut ir = ExprTable::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[e1, e2]);
    let b = build(arr, ir);
    assert!(b.ok, "[1, 2] should check");
    let ty = b.ty[arr].unwrap();
    let ids = array_ids(&b, ty);
    assert_eq!(ids.len(), 2, "a type expression is a pair [shape, kind]");
    let shape_ids = array_ids(&b, ids[0]);
    assert_eq!(shape_ids.len(), 2);
    assert_eq!(shape_ids[0], b.int_type);
    assert_eq!(shape_ids[1], b.int_type);
    let kind_ids = array_ids(&b, ids[1]);
    assert_eq!(kind_ids.len(), 2);
    assert!(matches!(
        b.module.nodes[kind_ids[0]].value,
        Some(Value::Ext(HighValue::ArrayType))
    ));
    assert_eq!(kind_ids[1], b.type_expr);
}

#[test]
fn let_bound_functions_are_polymorphic() {
    // let id = \x. x in
    //   let a = (id 5 : int) in
    //     (id Type : Type)
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let id = lam(&mut ir, x, body);
    let b1 = binder(&mut ir);
    let use1 = var(&mut ir, b1);
    let five = int(&mut ir, 5);
    let call1 = app(&mut ir, use1, five);
    let t1 = int_t(&mut ir);
    let a = ann(&mut ir, call1, t1);
    let b2 = binder(&mut ir);
    let use2 = var(&mut ir, b1); // a second use of id, in the inner let's body
    let type_val = ty(&mut ir);
    let call2 = app(&mut ir, use2, type_val);
    let t2 = ty(&mut ir);
    let body2 = ann(&mut ir, call2, t2);
    let inner = let_(&mut ir, b2, a, body2);
    let whole = let_(&mut ir, b1, id, inner);
    let b = build(whole, ir);
    assert!(
        b.ok,
        "two uses of id at different types must both check (polymorphism via the apply's fresh clones)"
    );
}

#[test]
fn applying_a_non_function_fails() {
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let six = int(&mut ir, 6);
    let a = app(&mut ir, five, six);
    let b = build(a, ir);
    assert!(!b.ok, "applying an int must fail (the function-ness guard)");
    assert!(!b.module.unify_errors.is_empty());
}

#[test]
fn array_typechecks_elementwise() {
    // [1, 2] : [int, int]
    let mut ir = ExprTable::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[e1, e2]);
    let d = int_t(&mut ir);
    let c = int_t(&mut ir);
    let t = array(&mut ir, &[d, c]);
    let a = ann(&mut ir, arr, t);
    let b = build(a, ir);
    assert!(b.ok, "[1, 2] : [int, int] should check");
}

#[test]
fn array_length_mismatch_fails() {
    let mut ir = ExprTable::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[e1, e2]);
    let d = int_t(&mut ir);
    let t = array(&mut ir, &[d]);
    let a = ann(&mut ir, arr, t);
    let b = build(a, ir);
    assert!(!b.ok, "[1, 2] : [int] must fail (length mismatch)");
}

#[test]
fn types_are_first_class() {
    // let T = int in \x. (x : T)
    let mut ir = ExprTable::new();
    let bt = binder(&mut ir);
    let tval = int_t(&mut ir);
    let bx = binder(&mut ir);
    let use_x = var(&mut ir, bx);
    let use_t = var(&mut ir, bt);
    let body = ann(&mut ir, use_x, use_t);
    let l = lam(&mut ir, bx, body);
    let whole = let_(&mut ir, bt, tval, l);
    let b = build(whole, ir);
    assert!(b.ok, "let T = int in \\x. (x : T) should check");
}

// --- check-then-run round-trips -------------------------------------------

#[test]
fn built_program_runs_to_a_value() {
    // let id = \x. x in id 5  ==  5
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let id = lam(&mut ir, x, body);
    let b1 = binder(&mut ir);
    let use1 = var(&mut ir, b1);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, use1, five);
    let whole = let_(&mut ir, b1, id, call);
    let b = build(whole, ir);
    assert!(b.ok);
    let mut module = b.module;
    let value = module.evaluate_node_deep(b.root_val, None);
    assert!(matches!(value, Value::USize(5)));
}

#[test]
fn inline_lambda_applies() {
    // (\x. x) 5  ==  5
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let l = lam(&mut ir, x, body);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, l, five);
    let b = build(call, ir);
    assert!(b.ok);
    let mut module = b.module;
    let value = module.evaluate_node_deep(b.root_val, None);
    assert!(matches!(value, Value::USize(5)));
}

#[test]
fn nested_polymorphic_applies_run() {
    // let id = \x. x in id (id 5)  ==  5
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let id = lam(&mut ir, x, body);
    let b1 = binder(&mut ir);
    let use1 = var(&mut ir, b1);
    let five = int(&mut ir, 5);
    let inner = app(&mut ir, use1, five);
    let use2 = var(&mut ir, b1);
    let call = app(&mut ir, use2, inner);
    let whole = let_(&mut ir, b1, id, call);
    let b = build(whole, ir);
    assert!(b.ok);
    let mut module = b.module;
    let value = module.evaluate_node_deep(b.root_val, None);
    assert!(matches!(value, Value::USize(5)));
}

// --- diagnostics ----------------------------------------------------------

#[test]
fn annotation_mismatch_reports_expected_found() {
    // 5 : Type  →  expected Type, found int
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let t = ty(&mut ir);
    let a = ann(&mut ir, five, t);
    ir.expr[a.0 as usize].span = Some((3, 7));
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].span, Some((3, 7)));
    // the flow points at the annotation itself, which fixed the expected
    // side to Type
    assert_eq!(
        diags[0].message,
        "expected Type, found int\n  ?b is fixed to Type at line 3"
    );
}

#[test]
fn kinding_mismatch_reports_expected_type() {
    // 1 : 3 — the type expression `3` is an int literal, not a Type
    let mut ir = ExprTable::new();
    let one = int(&mut ir, 1);
    let three = int(&mut ir, 3);
    let a = ann(&mut ir, one, three);
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 2);
    assert_eq!(diags[0].message, "expected Type, found int");
    assert_eq!(diags[1].message, "expected 3, found int");
}

#[test]
fn applying_a_non_function_reports_expected_function() {
    // 5 6 — the function-ness guard, with the flow showing where `int` came
    // from
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let six = int(&mut ir, 6);
    let call = app(&mut ir, five, six);
    ir.expr[five.0 as usize].span = Some((2, 1));
    ir.expr[call.0 as usize].span = Some((2, 3));
    let b = build(call, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].span, Some((2, 3)));
    assert_eq!(
        diags[0].message,
        "expected a function, found int\n  ?a is fixed to int at line 2"
    );
}

#[test]
fn runtime_apply_mismatch_is_attributed_to_the_argument() {
    // (\x. (x : Type)) 5 — the parameter's type is Type, the argument's int:
    // a runtime apply-time failure, attributed to the argument's span.
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let use_x = var(&mut ir, x);
    let t = ty(&mut ir);
    let body = ann(&mut ir, use_x, t);
    let l = lam(&mut ir, x, body);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, l, five);
    ir.expr[five.0 as usize].span = Some((5, 17));
    let b = build(call, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].span, Some((5, 17)));
    assert_eq!(
        diags[0].message,
        "expected Type, found int\n  ?b is fixed to int at line 5"
    );
}

#[test]
fn flow_shows_what_fixed_the_type() {
    // (\x. x) : [Type, int] — the identity's shared parameter type is fixed
    // to Type by the annotation's first element, then conflicts with int
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let l = lam(&mut ir, x, body);
    let t1 = ty(&mut ir);
    let t2 = int_t(&mut ir);
    let t = array(&mut ir, &[t1, t2]);
    let a = ann(&mut ir, l, t);
    ir.expr[t1.0 as usize].span = Some((4, 9));
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "expected int, found Type\n  ?a is fixed to Type at line 4"
    );
}

#[test]
fn an_unannotated_lambda_prints_with_stable_names() {
    // (\x. x) : Type — the lambda's own type is the arrow ?a → ?a
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let l = lam(&mut ir, x, body);
    let t = ty(&mut ir);
    let a = ann(&mut ir, l, t);
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "expected Type, found ?a → ?a");
}

#[test]
fn an_unannotated_call_reports_an_ambiguous_type() {
    // let id = \x. x in id 5 — the result type is never anchored
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let id = lam(&mut ir, x, body);
    let b1 = binder(&mut ir);
    let use1 = var(&mut ir, b1);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, use1, five);
    let whole = let_(&mut ir, b1, id, call);
    let b = build(whole, ir);
    assert!(b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "cannot determine the type of the program: ?a is ambiguous"
    );
}

#[test]
fn array_length_mismatch_reports_both_sides() {
    // [1, 2] : [int]
    let mut ir = ExprTable::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[e1, e2]);
    let d = int_t(&mut ir);
    let t = array(&mut ir, &[d]);
    let a = ann(&mut ir, arr, t);
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "expected [int], found [int, int]");
}
