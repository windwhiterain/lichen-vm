//! The highlevel checker: compiles an ExprTable into a lowlevel Module where
//! the runtime *is* the typechecker — values are recursive pairs
//! `[value, type]` whose type slots are themselves pairs bottoming out at
//! the self-referential `Type : Type` universe, and the apply-time unify is
//! the parameter type check.

use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{Constant, ExprId, ExprKind, IR};
use lichen_highlevel::program::HighValue;
use lichen_lowlevel::Value;

// --- hand-built IR helpers (the language frontend will produce these) -----

fn int(ir: &mut IR, n: u64) -> ExprId {
    ir.alloc(ExprKind::Constant(Constant::USize(n as usize)), None)
}
fn ty(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Constant(Constant::TypeType), None)
}
fn int_t(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Constant(Constant::TypeInt), None)
}
fn param(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Parameter, None)
}
fn lam(ir: &mut IR, b: ExprId, body: ExprId) -> ExprId {
    ir.alloc(
        ExprKind::Function {
            parameter: b,
            r#return: body,
        },
        None,
    )
}
fn app(ir: &mut IR, f: ExprId, x: ExprId) -> ExprId {
    ir.alloc(ExprKind::Apply { function: f, argument: x }, None)
}
/// `a[i]` — element selection.
fn index(ir: &mut IR, a: ExprId, i: ExprId) -> ExprId {
    ir.alloc(ExprKind::Index { array: a, index: i }, None)
}
fn ann(ir: &mut IR, e: ExprId, t: ExprId) -> ExprId {
    ir.alloc(
        ExprKind::Annotation {
            value: e,
            r#type: t,
        },
        None,
    )
}
fn arrow(ir: &mut IR, d: ExprId, c: ExprId) -> ExprId {
    ir.alloc(
        ExprKind::TypeFunction {
            parameter: d,
            r#return: c,
        },
        None,
    )
}
fn tuple(ir: &mut IR, elements: &[ExprId]) -> ExprId {
    ir.alloc_tuple(elements, None)
}
/// A tuple type expression: `[int, int]`.
fn type_tuple(ir: &mut IR, elements: &[ExprId]) -> ExprId {
    ir.alloc_type_tuple(elements, None)
}
/// A struct type expression: `struct { T1, ..., Tn }` — positional fields.
fn type_struct(ir: &mut IR, fields: &[ExprId]) -> ExprId {
    ir.alloc_type_struct(fields, None)
}
/// An array literal: `[1, 2]` — all elements share one type.
fn array(ir: &mut IR, elements: &[ExprId]) -> ExprId {
    ir.alloc_array(elements, None)
}
/// The real array type: `Array(int, 3)` = `int[3]`.
fn type_array(ir: &mut IR, element_type: ExprId, length: ExprId) -> ExprId {
    ir.alloc(
        ExprKind::TypeArray {
            element_type,
            length,
        },
        None,
    )
}

fn build(root: ExprId, mut ir: IR) -> lichen_highlevel::checker::Build {
    ir.set_root(root);
    Checker::build(ir)
}

/// The ids inside a checker-built array value.
fn array_ids(
    b: &lichen_highlevel::checker::Build,
    node: lichen_lowlevel::NodeId,
) -> Vec<lichen_lowlevel::NodeId> {
    b.module.array_ids(node).expect("expected an array value").to_vec()
}

// --- checking -------------------------------------------------------------

#[test]
fn int_literal_checks() {
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let b = build(five, ir);
    assert!(b.ok, "5 should check");
    // The type of 5 is the recursive pair [int, [Type, ↺]].
    assert_eq!(b.ty[five], Some(b.int_type));
    let ids = array_ids(&b, b.int_type);
    assert_eq!(ids.len(), 2);
    assert!(matches!(
        b.module.nodes[ids[0]].value,
        Some(Value::Ext(HighValue::TypeInt))
    ));
    assert_eq!(ids[1], b.type_expr, "the type of int must be the Type universe");
}

#[test]
fn annotated_literal_checks() {
    // 5 : int
    let mut ir = IR::new();
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
    let mut ir = IR::new();
    let t = ty(&mut ir);
    let b = build(t, ir);
    assert!(b.ok);
    assert_eq!(b.term[t], Some(b.type_expr));
    assert_eq!(b.ty[t], Some(b.type_expr), "Type : Type");
    let ids = array_ids(&b, b.type_expr);
    assert_eq!(ids.len(), 2);
    assert!(matches!(
        b.module.nodes[ids[0]].value,
        Some(Value::Ext(HighValue::TypeType))
    ));
    assert_eq!(ids[1], b.type_expr, "the universe's type slot cycles back to itself");
}

#[test]
fn literal_against_type_fails() {
    // 5 : Type
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let t = ty(&mut ir);
    let a = ann(&mut ir, five, t);
    let b = build(a, ir);
    assert!(!b.ok, "5 : Type must fail");
    assert!(!b.module.unify_errors.is_empty());
}

#[test]
fn lambda_has_arrow_type() {
    let mut ir = IR::new();
    let x = param(&mut ir);
    // The return expression uses the parameter's id directly.
    let l = lam(&mut ir, x, x);
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
        Some(Value::Ext(HighValue::TypeFunction))
    ));
    assert_eq!(kind_ids[1], b.type_expr);
}

#[test]
fn lambda_checks_against_arrow_annotation() {
    // (\x. x) : int → int
    let mut ir = IR::new();
    let x = param(&mut ir);
    // The return expression uses the parameter's id directly.
    let l = lam(&mut ir, x, x);
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
    let mut ir = IR::new();
    let x = param(&mut ir);
    // The return expression uses the parameter's id directly.
    let l = lam(&mut ir, x, x);
    let d = int_t(&mut ir);
    let c = int_t(&mut ir);
    let t = type_tuple(&mut ir, &[d, c]);
    let a = ann(&mut ir, l, t);
    let b = build(a, ir);
    assert!(!b.ok, "(\\x. x) : [int, int] must fail (a lambda is not a tuple)");
}

#[test]
fn typed_tuple_is_a_kinded_tuple() {
    // [1, 2] — the pair [[1, 2], [[int, int], [TupleType, Type]]].
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let tup = tuple(&mut ir, &[e1, e2]);
    let b = build(tup, ir);
    assert!(b.ok, "[1, 2] should check");
    let ty = b.ty[tup].unwrap();
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
        Some(Value::Ext(HighValue::TypeTuple))
    ));
    assert_eq!(kind_ids[1], b.type_expr);
}

#[test]
fn real_array_type_is_type_and_length() {
    // Array(int, 3) — the pair [[int, 3], [ArrayType, Type]]: instance[0] is
    // the type shared by all elements, instance[1] the length.
    let mut ir = IR::new();
    let t = int_t(&mut ir);
    let n = int(&mut ir, 3);
    let arr = type_array(&mut ir, t, n);
    let b = build(arr, ir);
    assert!(b.ok, "Array(int, 3) should check");
    // The type of the array type is its kind [ArrayType, Type].
    let ty = b.ty[arr].unwrap();
    let kind_ids = array_ids(&b, ty);
    assert_eq!(kind_ids.len(), 2);
    assert!(matches!(
        b.module.nodes[kind_ids[0]].value,
        Some(Value::Ext(HighValue::TypeArray))
    ));
    assert_eq!(kind_ids[1], b.type_expr);
    // The value is the instance [type, length].
    let shape = b.val[arr].unwrap();
    let shape_ids = array_ids(&b, shape);
    assert_eq!(shape_ids.len(), 2);
    assert_eq!(shape_ids[0], b.int_type, "instance[0] is the element type");
    assert!(
        matches!(b.module.nodes[shape_ids[1]].value, Some(Value::USize(3))),
        "instance[1] is the length"
    );
    // The pair is [shape, kind].
    let ids = array_ids(&b, b.term[arr].unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], shape);
    assert_eq!(ids[1], ty);
}

#[test]
fn array_element_type_must_be_a_type() {
    // Array(5, 3) — 5 in type position is not a Type: a kinding error.
    let mut ir = IR::new();
    let bad = int(&mut ir, 5);
    let n = int(&mut ir, 3);
    let arr = type_array(&mut ir, bad, n);
    let b = build(arr, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Kinding);
    assert_eq!(
        diags[0].a, b.int_type,
        "the found side is the literal's type expression"
    );
    assert_eq!(
        diags[0].b, b.type_expr,
        "kinding compares against the Type universe"
    );
}

#[test]
fn the_array_type_is_kinded_not_a_type() {
    // Array(int, 3) : Type — the array type's own type is the kind
    // [ArrayType, Type], not the universe.
    let mut ir = IR::new();
    let t = int_t(&mut ir);
    let n = int(&mut ir, 3);
    let arr = type_array(&mut ir, t, n);
    let type_val = ty(&mut ir);
    let a = ann(&mut ir, arr, type_val);
    let b = build(a, ir);
    assert!(!b.ok, "an array type is not itself a Type");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(diags[0].value_a, Some(Value::Ext(HighValue::TypeArray)));
    assert_eq!(diags[0].value_b, Some(Value::Ext(HighValue::TypeType)));
}

#[test]
fn lambda_against_an_array_type_conflicts_on_the_length() {
    // (\x. x) : Array(int, 3) — the identity's shared parameter type is
    // fixed to int by instance[0], then conflicts with the length 3.
    let mut ir = IR::new();
    let x = param(&mut ir);
    let l = lam(&mut ir, x, x);
    let t = int_t(&mut ir);
    let n = int(&mut ir, 3);
    let arr_ty = type_array(&mut ir, t, n);
    let a = ann(&mut ir, l, arr_ty);
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1, "the array kinding passes; only the shape clashes");
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(diags[0].value_b, Some(Value::USize(3)));
    // the found side is the int type expression `[int, Type]`, unified into
    // the shared parameter cell of the identity lambda
    let found = array_ids(&b, diags[0].a);
    assert_eq!(found.len(), 2);
    assert!(matches!(
        b.module.nodes[found[0]].value,
        Some(Value::Ext(HighValue::TypeInt))
    ));
    assert_eq!(found[1], b.type_expr);
}

#[test]
fn array_literal_is_homogeneous() {
    // [1, 2] — the pair [[1, 2], [[int, 2], [ArrayType, Type]]]: one type
    // slot shared by all elements, the length is the element count.
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[e1, e2]);
    let mut b = build(arr, ir);
    assert!(b.ok, "[1, 2] as an array literal should check");
    let ty = b.ty[arr].unwrap();
    let ids = array_ids(&b, ty);
    assert_eq!(ids.len(), 2, "a type expression is a pair [shape, kind]");
    let shape_ids = array_ids(&b, ids[0]);
    assert_eq!(shape_ids.len(), 2);
    assert_eq!(
        b.module.equality_representative(shape_ids[0]),
        b.module.equality_representative(b.int_type),
        "the shared element type unifies to int"
    );
    assert!(
        matches!(b.module.nodes[shape_ids[1]].value, Some(Value::USize(2))),
        "instance[1] is the element count"
    );
    let kind_ids = array_ids(&b, ids[1]);
    assert_eq!(kind_ids.len(), 2);
    assert!(matches!(
        b.module.nodes[kind_ids[0]].value,
        Some(Value::Ext(HighValue::TypeArray))
    ));
    assert_eq!(kind_ids[1], b.type_expr);
    // The value holds the element values.
    let value_ids = array_ids(&b, b.val[arr].unwrap());
    assert_eq!(value_ids.len(), 2);
}

#[test]
fn array_literal_typechecks_against_an_array_type() {
    // [1, 2] : Array(int, 2)
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[e1, e2]);
    let t = int_t(&mut ir);
    let n = int(&mut ir, 2);
    let arr_ty = type_array(&mut ir, t, n);
    let a = ann(&mut ir, arr, arr_ty);
    let b = build(a, ir);
    assert!(b.ok, "[1, 2] : int[2] should check");
    assert!(b.module.unify_errors.is_empty());
}

#[test]
fn array_literal_length_mismatch_fails() {
    // [1, 2] : Array(int, 3)
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[e1, e2]);
    let t = int_t(&mut ir);
    let n = int(&mut ir, 3);
    let arr_ty = type_array(&mut ir, t, n);
    let a = ann(&mut ir, arr, arr_ty);
    let b = build(a, ir);
    assert!(!b.ok, "[1, 2] : int[3] must fail (length mismatch)");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(diags[0].value_a, Some(Value::USize(2)));
    assert_eq!(diags[0].value_b, Some(Value::USize(3)));
}

#[test]
fn heterogeneous_array_literal_fails() {
    // [1, int] — the second element's type is Type; the first fixed the
    // shared element type to int.
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int_t(&mut ir); // `int` as a value — its type is Type
    let arr = array(&mut ir, &[e1, e2]);
    let b = build(arr, ir);
    assert!(!b.ok, "[1, int] must fail (the elements must share one type)");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::ArrayElement);
    assert_eq!(diags[0].value_a, Some(Value::Ext(HighValue::TypeType)));
    assert_eq!(diags[0].value_b, Some(Value::Ext(HighValue::TypeInt)));
}

#[test]
fn let_bound_functions_are_polymorphic() {
    // `let` is desugared by the frontend: `let id = \x. x in b` becomes
    // `(\id. b) (\x. x)`, and the inner `let a = (id 5 : int) in ...`
    // becomes `(\a. (id Type : Type)) (id 5 : int)`.  Both uses of id are
    // the parameter's id itself — polymorphism via the apply's fresh clones.
    let mut ir = IR::new();
    let x = param(&mut ir);
    let id = lam(&mut ir, x, x);
    let b1 = param(&mut ir);
    let five = int(&mut ir, 5);
    let call1 = app(&mut ir, b1, five);
    let t1 = int_t(&mut ir);
    let a = ann(&mut ir, call1, t1);
    let b2 = param(&mut ir);
    let type_val = ty(&mut ir);
    let call2 = app(&mut ir, b1, type_val); // a second use of id
    let t2 = ty(&mut ir);
    let body2 = ann(&mut ir, call2, t2);
    let inner_lam = lam(&mut ir, b2, body2);
    let inner = app(&mut ir, inner_lam, a);
    let outer_lam = lam(&mut ir, b1, inner);
    let whole = app(&mut ir, outer_lam, id);
    let b = build(whole, ir);
    assert!(
        b.ok,
        "two uses of id at different types must both check (polymorphism via the apply's fresh clones)"
    );
}

#[test]
fn applying_a_non_function_fails() {
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let six = int(&mut ir, 6);
    let a = app(&mut ir, five, six);
    let b = build(a, ir);
    assert!(!b.ok, "applying an int must fail (the function-ness guard)");
    assert!(!b.module.unify_errors.is_empty());
}

#[test]
fn tuple_typechecks_elementwise() {
    // [1, 2] : [int, int]
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let tup = tuple(&mut ir, &[e1, e2]);
    let d = int_t(&mut ir);
    let c = int_t(&mut ir);
    let t = type_tuple(&mut ir, &[d, c]);
    let a = ann(&mut ir, tup, t);
    let b = build(a, ir);
    assert!(b.ok, "[1, 2] : [int, int] should check");
}

#[test]
fn tuple_length_mismatch_fails() {
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let tup = tuple(&mut ir, &[e1, e2]);
    let d = int_t(&mut ir);
    let t = type_tuple(&mut ir, &[d]);
    let a = ann(&mut ir, tup, t);
    let b = build(a, ir);
    assert!(!b.ok, "[1, 2] : [int] must fail (length mismatch)");
}

#[test]
fn types_are_first_class() {
    // let T = int in \x. (x : T) → (\T. \x. (x : T)) int
    let mut ir = IR::new();
    let bt = param(&mut ir);
    let tval = int_t(&mut ir);
    let bx = param(&mut ir);
    let body = ann(&mut ir, bx, bt); // (x : T) — both uses are parameter ids
    let l = lam(&mut ir, bx, body);
    let t_lam = lam(&mut ir, bt, l);
    let whole = app(&mut ir, t_lam, tval);
    let b = build(whole, ir);
    assert!(b.ok, "(\\T. \\x. (x : T)) int should check");
}

// --- check-then-run round-trips -------------------------------------------

#[test]
fn built_program_runs_to_a_value() {
    // (\id. id 5) (\x. x)  ==  5
    let mut ir = IR::new();
    let x = param(&mut ir);
    let id = lam(&mut ir, x, x);
    let b1 = param(&mut ir);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, b1, five);
    let outer = lam(&mut ir, b1, call);
    let whole = app(&mut ir, outer, id);
    let b = build(whole, ir);
    assert!(b.ok);
    let mut module = b.module;
    let value = module.evaluate_node_deep(b.root_val, None);
    assert!(matches!(value, Value::USize(5)));
}

#[test]
fn inline_lambda_applies() {
    // (\x. x) 5  ==  5
    let mut ir = IR::new();
    let x = param(&mut ir);
    // The return expression uses the parameter's id directly.
    let l = lam(&mut ir, x, x);
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
    // (\id. id (id 5)) (\x. x)  ==  5
    let mut ir = IR::new();
    let x = param(&mut ir);
    let id = lam(&mut ir, x, x);
    let b1 = param(&mut ir);
    let five = int(&mut ir, 5);
    let inner = app(&mut ir, b1, five);
    let call = app(&mut ir, b1, inner); // the same parameter id, nested
    let outer = lam(&mut ir, b1, call);
    let whole = app(&mut ir, outer, id);
    let b = build(whole, ir);
    assert!(b.ok);
    let mut module = b.module;
    let value = module.evaluate_node_deep(b.root_val, None);
    assert!(matches!(value, Value::USize(5)));
}

// --- diagnostics ----------------------------------------------------------

#[test]
fn annotation_mismatch_reports_expected_found() {
    // 5 : Type  →  expected TypeType, found TypeInt
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let t = ty(&mut ir);
    let a = ann(&mut ir, five, t);
    ir.expr[a.0 as usize].span = Some((3, 7));
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(diags[0].span, Some((3, 7)));
    assert_eq!(diags[0].value_a, Some(Value::Ext(HighValue::TypeInt)));
    assert_eq!(diags[0].value_b, Some(Value::Ext(HighValue::TypeType)));
}

#[test]
fn kinding_mismatch_reports_expected_type() {
    // 1 : 3 — the type expression `3` is an int literal, not a Type
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let three = int(&mut ir, 3);
    let a = ann(&mut ir, one, three);
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 2);
    assert_eq!(diags[0].kind, DiagKind::Kinding);
    assert_eq!(diags[0].a, b.int_type);
    assert_eq!(diags[1].kind, DiagKind::Annotation);
    assert_eq!(diags[1].value_a, Some(Value::Ext(HighValue::TypeInt)));
    assert_eq!(diags[1].value_b, Some(Value::USize(3)));
}

#[test]
fn applying_a_non_function_reports_expected_function() {
    // 5 6 — the function-ness guard, with the flow showing where `int` came
    // from
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let six = int(&mut ir, 6);
    let call = app(&mut ir, five, six);
    ir.expr[five.0 as usize].span = Some((2, 1));
    ir.expr[call.0 as usize].span = Some((2, 3));
    let b = build(call, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Guard);
    assert_eq!(diags[0].span, Some((2, 3)));
    assert_eq!(diags[0].value_a, Some(Value::Ext(HighValue::TypeInt)));
    // the expected side is the synthesized function type the guard built
    assert!(matches!(diags[0].value_b, Some(Value::Array(_))));
}

#[test]
fn runtime_apply_mismatch_is_attributed_to_the_argument() {
    // (\x. (x : Type)) 5 — the parameter's type is Type, the argument's int:
    // a runtime apply-time failure, attributed to the argument's span.
    let mut ir = IR::new();
    let x = param(&mut ir);
    let t = ty(&mut ir);
    let body = ann(&mut ir, x, t);
    let l = lam(&mut ir, x, body);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, l, five);
    ir.expr[five.0 as usize].span = Some((5, 17));
    let b = build(call, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime);
    assert_eq!(diags[0].span, Some((5, 17)));
    // runtime direction is reversed: a = the parameter's expected type,
    // b = the argument's found type
    assert_eq!(diags[0].value_a, Some(Value::Ext(HighValue::TypeType)));
    assert_eq!(diags[0].value_b, Some(Value::Ext(HighValue::TypeInt)));
}

#[test]
fn annotating_a_lambda_with_a_mixed_tuple_type_reports_expected_found() {
    // (\x. x) : [Type, int] — the identity's shared parameter type is fixed
    // to Type by the annotation's first element, then conflicts with int
    let mut ir = IR::new();
    let x = param(&mut ir);
    // The return expression uses the parameter's id directly.
    let l = lam(&mut ir, x, x);
    let t1 = ty(&mut ir);
    let t2 = int_t(&mut ir);
    let t = type_tuple(&mut ir, &[t1, t2]);
    let a = ann(&mut ir, l, t);
    ir.expr[t1.0 as usize].span = Some((4, 9));
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(diags[0].value_a, Some(Value::Ext(HighValue::TypeType)));
    assert_eq!(diags[0].value_b, Some(Value::Ext(HighValue::TypeInt)));
}

#[test]
fn an_unannotated_lambda_has_an_unbound_arrow_type() {
    // (\x. x) : Type — the found side is the identity's arrow shape
    // `?a → ?a`: unbound components, but the arrow shape is determined.
    let mut ir = IR::new();
    let x = param(&mut ir);
    // The return expression uses the parameter's id directly.
    let l = lam(&mut ir, x, x);
    let t = ty(&mut ir);
    let a = ann(&mut ir, l, t);
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(diags[0].value_b, Some(Value::Ext(HighValue::TypeType)));
    assert!(
        b.arrows.contains(&diags[0].a),
        "the found side is the arrow shape"
    );
    assert!(matches!(diags[0].value_a, Some(Value::Array(_))));
}

#[test]
fn an_unannotated_call_reports_an_ambiguous_type() {
    // (\id. id 5) (\x. x) — the result type is never anchored: the call's
    // type cell is lazy (the runtime does not force a polymorphic template's
    // result — evaluating it yields the parameterized marker), so the
    // program's type is ambiguous.
    let mut ir = IR::new();
    let x = param(&mut ir);
    let id = lam(&mut ir, x, x);
    let b1 = param(&mut ir);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, b1, five);
    let outer = lam(&mut ir, b1, call);
    let whole = app(&mut ir, outer, id);
    let b = build(whole, ir);
    assert!(b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Ambiguity);
    assert!(diags[0].value_a.is_none(), "the root type is unbound");
}

#[test]
fn tuple_length_mismatch_reports_both_sides() {
    // [1, 2] : [int]
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let tup = tuple(&mut ir, &[e1, e2]);
    let d = int_t(&mut ir);
    let t = type_tuple(&mut ir, &[d]);
    let a = ann(&mut ir, tup, t);
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    // the length mismatch: the found tuple type has two elements, the
    // expected one
    assert_eq!(array_ids(&b, diags[0].a).len(), 2);
    assert_eq!(array_ids(&b, diags[0].b).len(), 1);
}

// --- call result annotations are lazy --------------------------------------
// A call's result type cell is a lazy record: the runtime apply does not
// force it (evaluating a polymorphic template yields the parameterized
// marker), so an annotation on a call result simply binds the cell at check
// time — it is never compared against the runtime result in v1.

#[test]
fn a_call_result_annotation_binds_lazily() {
    // (\f. (f 5 : int)) (\x. Type) — f actually returns Type, but the
    // annotation anchors the lazy result cell of `f 5` without a runtime
    // check, so the call checks and its type is int.  The desugared let's
    // own type is the outer apply's lazy cell — unanchored, so the root is
    // ambiguous (the let no longer propagates the body's type).
    let mut ir = IR::new();
    let x = param(&mut ir);
    let tval = ty(&mut ir);
    let f = lam(&mut ir, x, tval);
    let b1 = param(&mut ir);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, b1, five);
    let want = int_t(&mut ir);
    let a = ann(&mut ir, call, want);
    let f_lam = lam(&mut ir, b1, a);
    let whole = app(&mut ir, f_lam, f);
    let b = build(whole, ir);
    assert!(b.ok, "(f 5 : int) should check lazily");
    assert!(b.module.unify_errors.is_empty());
    // the annotated call's type is int
    let rt = b.ty[a].unwrap();
    let ids = array_ids(&b, rt);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], b.int_marker);
    assert_eq!(ids[1], b.type_expr);
    // the root (the desugared let's apply) is ambiguous
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Ambiguity);
    assert!(diags[0].value_a.is_none(), "the root type is unbound");
}

#[test]
fn a_direct_call_result_annotation_binds_lazily() {
    // (\x. x) 5 : Type — the identity applied to 5 actually returns int, but
    // the annotation binds the lazy result cell without a runtime check.
    let mut ir = IR::new();
    let x = param(&mut ir);
    // The return expression uses the parameter's id directly.
    let l = lam(&mut ir, x, x);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, l, five);
    let want = ty(&mut ir);
    let a = ann(&mut ir, call, want);
    let b = build(a, ir);
    assert!(b.ok, "(\\x. x) 5 : Type should check lazily");
    assert!(b.module.unify_errors.is_empty());
}

// --- indexing -------------------------------------------------------------

#[test]
fn tuple_index_selects_value_and_type() {
    // (1, 2)[0] — value 1, type int.  The tuple's element-type list is
    // structural, so the type evaluation is a plain Index over it.
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let tup = tuple(&mut ir, &[one, two]);
    let zero = int(&mut ir, 0);
    let idx = index(&mut ir, tup, zero);
    let mut b = build(idx, ir);
    assert!(b.ok, "(1, 2)[0] should check");
    assert!(
        matches!(
            b.module.evaluate_node_deep(b.val[idx].unwrap(), None),
            Value::USize(1)
        ),
        "the value is the selected element"
    );
    let ty_val = b.module.evaluate_node_deep(b.ty[idx].unwrap(), None);
    assert_eq!(
        ty_val,
        b.module.nodes[b.int_type].value.unwrap(),
        "the type is the element type"
    );
}

#[test]
fn array_index_selects_value_and_type() {
    // [1, 2, 3][1] — value 2, type int.  The array type shape [int, 3]
    // holds the length as data, so the type evaluation runs in the custom
    // IndexType operator.
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let e3 = int(&mut ir, 3);
    let arr = array(&mut ir, &[e1, e2, e3]);
    let one = int(&mut ir, 1);
    let idx = index(&mut ir, arr, one);
    let mut b = build(idx, ir);
    assert!(b.ok, "[1, 2, 3][1] should check");
    assert!(
        matches!(
            b.module.evaluate_node_deep(b.val[idx].unwrap(), None),
            Value::USize(2)
        ),
        "the value is the selected element"
    );
    let ty_val = b.module.evaluate_node_deep(b.ty[idx].unwrap(), None);
    assert_eq!(
        ty_val,
        b.module.nodes[b.int_type].value.unwrap(),
        "the type is the element type"
    );
}

#[test]
fn tuple_index_out_of_bounds_renders_a_diagnostic() {
    // (1, 2)[5] — the value and the type evaluation both hit the structural
    // bounds check; the identical facts collapse to one diagnostic.
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let tup = tuple(&mut ir, &[one, two]);
    let five = int(&mut ir, 5);
    let idx = index(&mut ir, tup, five);
    ir.expr[five.0 as usize].span = Some((3, 9));
    let b = build(idx, ir);
    assert!(!b.ok, "(1, 2)[5] must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexOutOfBounds);
    assert_eq!(diags[0].message, "index 5 out of bounds (array length 2)");
    assert_eq!(diags[0].value_a, Some(Value::USize(5)));
    assert_eq!(diags[0].value_b, Some(Value::USize(2)));
    assert_eq!(diags[0].span, Some((3, 9)));
}

#[test]
fn array_index_out_of_bounds_renders_a_diagnostic() {
    // [1, 2, 3][5] — the type side checks the index against the ArrayType's
    // length (3), not the shape's structural size (2).
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let e3 = int(&mut ir, 3);
    let arr = array(&mut ir, &[e1, e2, e3]);
    let five = int(&mut ir, 5);
    let idx = index(&mut ir, arr, five);
    ir.expr[five.0 as usize].span = Some((4, 11));
    let b = build(idx, ir);
    assert!(!b.ok, "[1, 2, 3][5] must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexOutOfBounds);
    assert_eq!(diags[0].message, "index 5 out of bounds (array length 3)");
    assert_eq!(diags[0].value_a, Some(Value::USize(5)));
    assert_eq!(diags[0].value_b, Some(Value::USize(3)));
    assert_eq!(diags[0].span, Some((4, 11)));
}

#[test]
fn array_index_out_of_bounds_against_a_bound_length() {
    // (\x. x[3])([1, 2]) — x's type is only known once the apply binds it;
    // the definition pass forces the type evaluation against the applied
    // array's length 2.
    let mut ir = IR::new();
    let x = param(&mut ir);
    let three = int(&mut ir, 3);
    let body = index(&mut ir, x, three);
    let l = lam(&mut ir, x, body);
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[e1, e2]);
    let whole = app(&mut ir, l, arr);
    let b = build(whole, ir);
    assert!(!b.ok, "(\\x. x[3])([1, 2]) must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexOutOfBounds);
    assert_eq!(diags[0].message, "index 3 out of bounds (array length 2)");
}

// --- struct types ----------------------------------------------------------
// A struct type is the pair [field types, [TypeId(n), Type]]: like a tuple
// type, but the kind slot holds a *fresh nominal* id (from the Fresh
// operator) instead of the fixed TupleType marker.  Equal ids unify,
// different ids never do, and a struct never unifies with a same-shape
// tuple — nominal identity.

#[test]
fn struct_type_is_kinded_and_carries_a_fresh_type_id() {
    // struct { Int, Type } — the pair [[int, Type], [TypeId(0), Type]].
    let mut ir = IR::new();
    let t1 = int_t(&mut ir);
    let t2 = ty(&mut ir);
    let s = type_struct(&mut ir, &[t1, t2]);
    let b = build(s, ir);
    assert!(b.ok, "struct {{ Int, Type }} should kind");
    assert!(b.module.unify_errors.is_empty());
    // the shape is the field-type list [int, Type]
    let shape = b.val[s].unwrap();
    let shape_ids = array_ids(&b, shape);
    assert_eq!(shape_ids.len(), 2);
    assert_eq!(shape_ids[0], b.int_type);
    // the kind slot is [TypeId(0), K]
    let kind = b.ty[s].unwrap();
    let kind_ids = array_ids(&b, kind);
    assert_eq!(kind_ids.len(), 2);
    assert_eq!(kind_ids[1], b.type_expr);
    assert!(matches!(
        b.module.nodes[kind_ids[0]].value,
        Some(Value::Ext(HighValue::TypeId(0)))
    ));
    // one source occurrence consumed exactly one fresh id
    assert_eq!(b.module.global_ext.type_id_counter, 1);
}

#[test]
fn each_struct_type_occurrence_allocates_a_distinct_id() {
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s1 = type_struct(&mut ir, &[f]);
    let s2 = type_struct(&mut ir, &[f]);
    let pair = tuple(&mut ir, &[s1, s2]);
    let b = build(pair, ir);
    assert!(b.ok);
    assert_eq!(b.module.global_ext.type_id_counter, 2);
    let id1 = array_ids(&b, b.ty[s1].unwrap())[0];
    let id2 = array_ids(&b, b.ty[s2].unwrap())[0];
    assert!(matches!(
        b.module.nodes[id1].value,
        Some(Value::Ext(HighValue::TypeId(0)))
    ));
    assert!(matches!(
        b.module.nodes[id2].value,
        Some(Value::Ext(HighValue::TypeId(1)))
    ));
}

#[test]
fn two_struct_type_occurrences_do_not_unify() {
    // same fields, different occurrences → different ids → nominal conflict
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s1 = type_struct(&mut ir, &[f]);
    let s2 = type_struct(&mut ir, &[f]);
    let pair = tuple(&mut ir, &[s1, s2]);
    let b = build(pair, ir);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.term[s1].unwrap(), b.term[s2].unwrap());
    assert_eq!(module.unify_errors.len(), 1);
    let err = module.unify_errors[0];
    assert!(matches!(err.value_a, Some(Value::Ext(HighValue::TypeId(0)))));
    assert!(matches!(err.value_b, Some(Value::Ext(HighValue::TypeId(1)))));
}

#[test]
fn a_struct_type_does_not_unify_with_a_same_shape_tuple_type() {
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s = type_struct(&mut ir, &[f]);
    let t = type_tuple(&mut ir, &[f]);
    let pair = tuple(&mut ir, &[s, t]);
    let b = build(pair, ir);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.term[s].unwrap(), b.term[t].unwrap());
    assert_eq!(module.unify_errors.len(), 1);
    let err = module.unify_errors[0];
    // the kind slot clashed: the nominal id vs the structural marker
    assert!(matches!(err.value_a, Some(Value::Ext(HighValue::TypeId(0)))));
    assert!(matches!(err.value_b, Some(Value::Ext(HighValue::TypeTuple))));
}

#[test]
fn a_struct_type_unifies_with_itself() {
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s = type_struct(&mut ir, &[f]);
    let b = build(s, ir);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.term[s].unwrap(), b.term[s].unwrap());
    assert!(
        module.unify_errors.is_empty(),
        "the same struct type unifies with itself"
    );
}

#[test]
fn an_annotation_against_a_struct_type_reports_the_conflict() {
    // 5 : struct { Int } — the literal's int type conflicts with the struct
    // type; the struct pair (the diary's expected side) renders with its
    // nominal id in the flow line.
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let f = int_t(&mut ir);
    let s = type_struct(&mut ir, &[f]);
    let a = ann(&mut ir, five, s);
    ir.expr[s.0 as usize].span = Some((7, 8));
    let b = build(a, ir);
    assert!(!b.ok, "5 : struct {{ Int }} must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(diags[0].value_a, Some(Value::Ext(HighValue::TypeInt)));
    assert!(
        diags[0].message.contains("expected [TypeInt]"),
        "the struct's field list renders as the expected side: {}",
        diags[0].message
    );
}

#[test]
fn two_struct_types_conflict_reports_the_nominal_ids() {
    // (\x. (x : struct { Int })) (struct { Int }) — the argument's struct
    // type has a different fresh id than the annotation's, so the apply-time
    // unify fails on the ids.
    let mut ir = IR::new();
    let f1 = int_t(&mut ir);
    let s1 = type_struct(&mut ir, &[f1]);
    let f2 = int_t(&mut ir);
    let s2 = type_struct(&mut ir, &[f2]);
    let x = param(&mut ir);
    let body = ann(&mut ir, x, s1);
    let l = lam(&mut ir, x, body);
    let whole = app(&mut ir, l, s2);
    let b = build(whole, ir);
    assert!(!b.ok, "two distinct struct types must not unify");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime);
    assert!(
        diags[0].message.contains("TypeId(0)"),
        "the nominal ids render: {}",
        diags[0].message
    );
}

#[test]
fn an_annotation_with_a_struct_type_rejects_a_literal_at_apply_time() {
    // (\x. (x : struct { Int })) 5 — the struct type sits directly in the
    // annotation, so the apply-time unify checks the argument's type against
    // it and fails.
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s = type_struct(&mut ir, &[f]);
    let x = param(&mut ir);
    let body = ann(&mut ir, x, s);
    let l = lam(&mut ir, x, body);
    let five = int(&mut ir, 5);
    let whole = app(&mut ir, l, five);
    let b = build(whole, ir);
    assert!(!b.ok, "a literal is not a struct value");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime);
    // reversed direction: a = the parameter's expected type (the struct's
    // field list), b = the argument's found type (int)
    assert!(matches!(diags[0].value_a, Some(Value::Array(_))));
    assert_eq!(diags[0].value_b, Some(Value::Ext(HighValue::TypeInt)));
}
