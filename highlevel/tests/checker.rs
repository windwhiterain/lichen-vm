//! The highlevel checker: builds a typed lowlevel Module from an ExprTable.

use highlevel::checker::Checker;
use highlevel::expr::{ExprId, ExprKind, ExprTable};
use highlevel::program::HighValue;
use lichen_vm::lowlevel::Value;

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
fn array(ir: &mut ExprTable, elements: &[ExprId]) -> ExprId {
    ir.alloc_array(elements, None)
}

fn build(root: ExprId, mut ir: ExprTable) -> highlevel::checker::Build {
    ir.set_root(root);
    Checker::build(ir)
}

#[test]
fn int_literal_has_type_int() {
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let b = build(five, ir);
    assert!(b.ok, "5 should check");
    assert_eq!(b.ty[five], Some(b.int_ty));
    assert!(matches!(
        b.module.nodes[b.root_term].value,
        Some(Value::USize(5))
    ));
}

#[test]
fn annotated_literal_checks() {
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let t = int_t(&mut ir);
    let a = ann(&mut ir, five, t);
    let b = build(a, ir);
    assert!(b.ok, "5 : int should check");
    assert_eq!(b.ty[a], Some(t));
}

#[test]
fn literal_against_type_fails() {
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let t = ty(&mut ir);
    let a = ann(&mut ir, five, t);
    let b = build(a, ir);
    assert!(!b.ok, "5 : Type must fail");
    assert_eq!(b.module.unify_errors.len(), 1);
}

#[test]
fn lambda_has_arrow_type() {
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let l = lam(&mut ir, x, body);
    let b = build(l, ir);
    assert!(b.ok, "\\x. x should check");
    let arrow = b.ty[l].unwrap();
    match b.ir.expr[arrow.0 as usize].kind {
        ExprKind::Array(range) => {
            assert_eq!(
                b.ir.children[range.start as usize..range.end as usize].len(),
                2,
                "the lambda type must be an arrow (a 2-element array type)"
            );
        }
        _ => panic!("the lambda type must be an arrow (a 2-element array type)"),
    }
}

#[test]
fn lambda_checks_against_arrow_annotation() {
    // (\x. x) : [int, int]
    let mut ir = ExprTable::new();
    let x = binder(&mut ir);
    let body = var(&mut ir, x);
    let l = lam(&mut ir, x, body);
    let d = int_t(&mut ir);
    let c = int_t(&mut ir);
    let t = array(&mut ir, &[d, c]);
    let a = ann(&mut ir, l, t);
    let b = build(a, ir);
    assert!(b.ok, "(\\x. x) : [int, int] should check");
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
    assert!(b.ok, "two uses of id at different types require polymorphism");
}

#[test]
fn applying_a_non_function_fails() {
    let mut ir = ExprTable::new();
    let five = int(&mut ir, 5);
    let six = int(&mut ir, 6);
    let a = app(&mut ir, five, six);
    let b = build(a, ir);
    assert!(!b.ok, "applying an int must fail");
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

// --- check-then-run round-trips (Milestone C) -----------------------------

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
    let value = module.evaluate_node_deep(b.root_term, None);
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
    let value = module.evaluate_node_deep(b.root_term, None);
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
    let value = module.evaluate_node_deep(b.root_term, None);
    assert!(matches!(value, Value::USize(5)));
}
