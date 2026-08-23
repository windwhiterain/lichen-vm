//! The highlevel checker: compiles an ExprTable into a lowlevel Module where
//! the runtime *is* the typechecker — values are recursive pairs
//! `[value, type]` whose type slots are themselves pairs bottoming out at
//! the self-referential `Type : Type` universe, and the apply-time unify is
//! the parameter type check.

use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprId, ExprKind, IR};
use lichen_highlevel::program::HighProgramValue;

// --- hand-built IR helpers (the language frontend will produce these) -----

fn int(ir: &mut IR, n: u64) -> ExprId {
    ir.alloc(
        ExprKind::Constant(HighProgramValue::USize(n as usize)),
        None,
    )
}
fn ty(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Constant(HighProgramValue::TypeType), None)
}
fn int_t(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Constant(HighProgramValue::TypeInt), None)
}
fn param(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Parameter, None)
}
fn lam(ir: &mut IR, b: ExprId, body: ExprId) -> ExprId {
    lam_at(ir, b, body, 0)
}
/// A lambda at a given lexical depth — the count of enclosing function
/// scopes (0 = top-level).  The checker uses this to absorb a nested closure
/// into its parent's template while keeping siblings' templates disjoint.
fn lam_at(ir: &mut IR, b: ExprId, body: ExprId, depth: u32) -> ExprId {
    ir.alloc(
        ExprKind::Function {
            parameter: b,
            r#return: body,
            depth,
        },
        None,
    )
}
fn app(ir: &mut IR, f: ExprId, x: ExprId) -> ExprId {
    ir.alloc(
        ExprKind::Apply {
            function: f,
            argument: x,
        },
        None,
    )
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
/// A struct type expression: `struct<T1, ..., Tn>` — positional fields.
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
/// `_` — an inference placeholder in type position.
fn hole(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Placeholder, None)
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
    b.module
        .array_ids(node)
        .expect("expected an array value")
        .to_vec()
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
        Some(HighProgramValue::TypeInt)
    ));
    assert_eq!(
        ids[1], b.type_expr,
        "the type of int must be the Type universe"
    );
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
        Some(HighProgramValue::TypeType)
    ));
    assert_eq!(
        ids[1], b.type_expr,
        "the universe's type slot cycles back to itself"
    );
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
        Some(HighProgramValue::TypeFunction)
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
    assert!(
        !b.ok,
        "(\\x. x) : [int, int] must fail (a lambda is not a tuple)"
    );
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
        Some(HighProgramValue::TypeTuple)
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
        Some(HighProgramValue::TypeArray)
    ));
    assert_eq!(kind_ids[1], b.type_expr);
    // The value is the instance [type, length].
    let shape = b.val[arr].unwrap();
    let shape_ids = array_ids(&b, shape);
    assert_eq!(shape_ids.len(), 2);
    assert_eq!(shape_ids[0], b.int_type, "instance[0] is the element type");
    assert!(
        matches!(
            b.module.nodes[shape_ids[1]].value,
            Some(HighProgramValue::USize(3))
        ),
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
    assert_eq!(diags[0].value_a, Some(HighProgramValue::TypeArray));
    assert_eq!(diags[0].value_b, Some(HighProgramValue::TypeType));
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
    assert_eq!(
        diags.len(),
        1,
        "the array kinding passes; only the shape clashes"
    );
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(diags[0].value_b, Some(HighProgramValue::USize(3)));
    // the found side is the int type expression `[int, Type]`, unified into
    // the shared parameter cell of the identity lambda
    let found = array_ids(&b, diags[0].a);
    assert_eq!(found.len(), 2);
    assert!(matches!(
        b.module.nodes[found[0]].value,
        Some(HighProgramValue::TypeInt)
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
        matches!(
            b.module.nodes[shape_ids[1]].value,
            Some(HighProgramValue::USize(2))
        ),
        "instance[1] is the element count"
    );
    let kind_ids = array_ids(&b, ids[1]);
    assert_eq!(kind_ids.len(), 2);
    assert!(matches!(
        b.module.nodes[kind_ids[0]].value,
        Some(HighProgramValue::TypeArray)
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
    assert_eq!(diags[0].value_a, Some(HighProgramValue::USize(2)));
    assert_eq!(diags[0].value_b, Some(HighProgramValue::USize(3)));
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
    assert!(
        !b.ok,
        "[1, int] must fail (the elements must share one type)"
    );
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::ArrayElement);
    assert_eq!(diags[0].value_a, Some(HighProgramValue::TypeType));
    assert_eq!(diags[0].value_b, Some(HighProgramValue::TypeInt));
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
    let inner_lam = lam_at(&mut ir, b2, body2, 1);
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
    let l = lam_at(&mut ir, bx, body, 1);
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
    assert!(matches!(value, HighProgramValue::USize(5)));
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
    assert!(matches!(value, HighProgramValue::USize(5)));
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
    assert!(matches!(value, HighProgramValue::USize(5)));
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
    assert_eq!(diags[0].value_a, Some(HighProgramValue::TypeInt));
    assert_eq!(diags[0].value_b, Some(HighProgramValue::TypeType));
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
    assert_eq!(diags[1].value_a, Some(HighProgramValue::TypeInt));
    assert_eq!(diags[1].value_b, Some(HighProgramValue::USize(3)));
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
    assert_eq!(diags[0].value_a, Some(HighProgramValue::TypeInt));
    // the expected side is the synthesized function type the guard built
    assert!(matches!(diags[0].value_b, Some(HighProgramValue::Array(_))));
}

#[test]
fn indexing_a_function_reports_expected_tuple_or_array() {
    // (\x. x)[0] — the index-target guard, mirroring the apply guard: a
    // concretely-known function type is not indexable, reported statically
    // instead of a runtime panic.
    let mut ir = IR::new();
    let x = param(&mut ir);
    let l = lam(&mut ir, x, x);
    let zero = int(&mut ir, 0);
    let idx = index(&mut ir, l, zero);
    ir.expr[idx.0 as usize].span = Some((3, 7));
    let b = build(idx, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexTarget);
    assert_eq!(diags[0].span, Some((3, 7)));
    assert_eq!(diags[0].a, b.ty[l].unwrap());
}

#[test]
fn indexing_an_int_reports_expected_tuple_or_array() {
    // 5[0] — an atomic type is not indexable either.
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let zero = int(&mut ir, 0);
    let idx = index(&mut ir, five, zero);
    let b = build(idx, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexTarget);
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
    assert_eq!(diags[0].value_a, Some(HighProgramValue::TypeType));
    assert_eq!(diags[0].value_b, Some(HighProgramValue::TypeInt));
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
    assert_eq!(diags[0].value_a, Some(HighProgramValue::TypeType));
    assert_eq!(diags[0].value_b, Some(HighProgramValue::TypeInt));
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
    assert_eq!(diags[0].value_b, Some(HighProgramValue::TypeType));
    assert!(
        b.arrows.contains(&diags[0].a),
        "the found side is the arrow shape"
    );
    assert!(matches!(diags[0].value_a, Some(HighProgramValue::Array(_))));
}

#[test]
fn an_unannotated_call_syncs_its_root_type_to_the_return_type() {
    // (\id. id 5) (\x. x) — the call's result type cell is a lazy record
    // (the runtime apply never fills it), so the apply's evaluation syncs
    // the cell with the return pair: the program evaluates to 5, whose type
    // is int.
    let mut ir = IR::new();
    let x = param(&mut ir);
    let id = lam(&mut ir, x, x);
    let b1 = param(&mut ir);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, b1, five);
    let outer = lam(&mut ir, b1, call);
    let whole = app(&mut ir, outer, id);
    let mut b = build(whole, ir);
    assert!(b.ok);
    assert!(b.diagnostics().is_empty());
    assert_eq!(
        b.module.equality_representative(b.root_ty),
        b.module.equality_representative(b.int_type),
        "the root type is synced to the return type (int)"
    );
}

#[test]
fn a_tuples_unbound_element_types_sync_from_the_return_types() {
    // a = \x. (1, Int)[x]; (a 0, a 1) — the tuple's element types are the
    // calls' lazy result cells.  Each apply's evaluation syncs its cell with
    // its return pair: element 0's cell binds to int, element 1's to the
    // universe (the `Int` constant's type is `Type`).
    let mut ir = IR::new();
    let x = param(&mut ir);
    let one = int(&mut ir, 1);
    let tval = ty(&mut ir);
    let tup = tuple(&mut ir, &[one, tval]);
    let idx = index(&mut ir, tup, x);
    let a = lam(&mut ir, x, idx);
    let zero = int(&mut ir, 0);
    let c0 = app(&mut ir, a, zero);
    let one1 = int(&mut ir, 1);
    let c1 = app(&mut ir, a, one1);
    let whole = tuple(&mut ir, &[c0, c1]);
    let b = build(whole, ir);
    assert!(b.ok);
    assert!(b.diagnostics().is_empty());
    // The resolved root type is the tuple type `<Int, Type>`.
    let pair = array_ids(&b, b.root_ty);
    assert_eq!(pair.len(), 2);
    let shape = array_ids(&b, pair[0]);
    assert_eq!(
        b.module.nodes[shape[0]].value, b.module.nodes[b.int_type].value,
        "element 0's cell syncs to int"
    );
    assert_eq!(
        b.module.nodes[shape[1]].value, b.module.nodes[b.type_expr].value,
        "element 1's cell syncs to Type's"
    );
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
fn a_call_result_annotation_is_checked_against_the_return_type() {
    // (\f. (f 5 : int)) (\x. Type) — f actually returns Type.  The
    // annotation binds the result cell at check time; the apply's runtime
    // evaluation then syncs the return pair against the apply's pair, and
    // the mismatch between the annotation's int and the real return type is
    // a reported error — the annotation is checked, not silently bound.
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
    assert!(!b.ok, "(f 5 : int) must fail: the annotation is checked");
    assert!(!b.module.unify_errors.is_empty());
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime);
}

#[test]
fn a_direct_call_result_annotation_is_checked_against_the_return_type() {
    // (\x. x) 5 : Type — the identity applied to 5 actually returns int.
    // The apply's evaluation syncs the result cell with the return pair, so
    // the annotation's Type conflicts with the real return type: the
    // annotation is checked, not silently bound.
    let mut ir = IR::new();
    let x = param(&mut ir);
    // The return expression uses the parameter's id directly.
    let l = lam(&mut ir, x, x);
    let five = int(&mut ir, 5);
    let call = app(&mut ir, l, five);
    let want = ty(&mut ir);
    let a = ann(&mut ir, call, want);
    let b = build(a, ir);
    assert!(
        !b.ok,
        "(\\x. x) 5 : Type must fail: the annotation is checked"
    );
    assert!(!b.module.unify_errors.is_empty());
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime);
}

#[test]
fn a_nested_function_value_captures_the_applied_outer_parameter() {
    // a = 1; f1 = x => { b = 2; f2 = y => [a, b, x, y]; f2 }; f1 3 4 — the
    // returned closure captures f1's parameter: applying it to 4 yields
    // [1, 2, 3, 4], not a leaked template parameter.
    let mut ir = IR::new();
    let a = int(&mut ir, 1);
    let b = int(&mut ir, 2);
    let x = param(&mut ir);
    let y = param(&mut ir);
    let f2_body = array(&mut ir, &[a, b, x, y]);
    let f2 = lam_at(&mut ir, y, f2_body, 1);
    let f1 = lam(&mut ir, x, f2);
    let three = int(&mut ir, 3);
    let four = int(&mut ir, 4);
    let call1 = app(&mut ir, f1, three);
    let whole = app(&mut ir, call1, four);
    let mut b = build(whole, ir);
    assert!(b.ok, "the closure program must check");
    let value = b.module.evaluate_node_deep(b.root_val, None);
    let HighProgramValue::Array(ptr) = value else {
        panic!("expected an array result, got {value:?}");
    };
    let ids = unsafe { &*ptr };
    let expected = [1usize, 2, 3, 4];
    assert_eq!(ids.len(), expected.len());
    for (&id, &n) in ids.iter().zip(expected.iter()) {
        assert_eq!(
            b.module.nodes[id].value,
            Some(HighProgramValue::USize(n)),
            "element {n} must be a bound value, not the leaked parameter"
        );
    }
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
            HighProgramValue::USize(1)
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
            HighProgramValue::USize(2)
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
    assert_eq!(diags[0].value_a, Some(HighProgramValue::USize(5)));
    assert_eq!(diags[0].value_b, Some(HighProgramValue::USize(2)));
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
    assert_eq!(diags[0].value_a, Some(HighProgramValue::USize(5)));
    assert_eq!(diags[0].value_b, Some(HighProgramValue::USize(3)));
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
// A struct type is the pair [[TypeId(n), field types], [TypeStruct, Type]]:
// like an array type (shape [element type, length]), the shape bundles a
// *fresh nominal* id with the positional field-type list, and the kind slot
// holds the fixed TypeStruct marker.  Equal ids unify, different ids never
// do, and a struct never unifies with a same-shape tuple — nominal identity.

#[test]
fn struct_type_is_kinded_and_carries_a_fresh_type_id() {
    // struct { Int, Type } — the pair [[TypeId(0), [int, Type]], [TypeStruct, Type]].
    let mut ir = IR::new();
    let t1 = int_t(&mut ir);
    let t2 = ty(&mut ir);
    let s = type_struct(&mut ir, &[t1, t2]);
    let b = build(s, ir);
    assert!(b.ok, "struct {{ Int, Type }} should kind");
    assert!(b.module.unify_errors.is_empty());
    // the shape bundles the nominal id with the field-type list [int, Type]
    let shape = b.val[s].unwrap();
    let shape_ids = array_ids(&b, shape);
    assert_eq!(shape_ids.len(), 2);
    assert!(matches!(
        b.module.nodes[shape_ids[0]].value,
        Some(HighProgramValue::TypeId(0))
    ));
    let fields = array_ids(&b, shape_ids[1]);
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0], b.int_type);
    // the kind slot is [TypeStruct, K]
    let kind = b.ty[s].unwrap();
    let kind_ids = array_ids(&b, kind);
    assert_eq!(kind_ids.len(), 2);
    assert_eq!(kind_ids[1], b.type_expr);
    assert_eq!(
        b.module.nodes[kind_ids[0]].value,
        Some(HighProgramValue::TypeStruct)
    );
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
    let id1 = array_ids(&b, b.val[s1].unwrap())[0];
    let id2 = array_ids(&b, b.val[s2].unwrap())[0];
    assert!(matches!(
        b.module.nodes[id1].value,
        Some(HighProgramValue::TypeId(0))
    ));
    assert!(matches!(
        b.module.nodes[id2].value,
        Some(HighProgramValue::TypeId(1))
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
    assert!(matches!(err.value_a, Some(HighProgramValue::TypeId(0))));
    assert!(matches!(err.value_b, Some(HighProgramValue::TypeId(1))));
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
    // The struct shape is [TypeId, field list] (2 elements) while the tuple
    // shape is the field list itself (1 element) — the arity clash at the
    // shape level is what keeps a struct from ever unifying with a tuple.
    let err = module.unify_errors[0];
    assert!(matches!(err.value_a, Some(HighProgramValue::Array(_))));
    assert!(matches!(err.value_b, Some(HighProgramValue::Array(_))));
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
    assert_eq!(diags[0].value_a, Some(HighProgramValue::TypeInt));
    assert!(
        diags[0].message.starts_with("expected ["),
        "the struct's shape renders as the expected side: {}",
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
        diags[0].message.contains("TypeId("),
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
    assert!(matches!(diags[0].value_a, Some(HighProgramValue::Array(_))));
    assert_eq!(diags[0].value_b, Some(HighProgramValue::TypeInt));
}

#[test]
fn a_shared_expression_compiles_once_with_one_nominal_id() {
    // The IR is a graph: the same expression may be referenced from several
    // parents (statement bindings pre-resolve every use of a name to the
    // value's own id).  Compiling it once keeps the single fresh nominal id,
    // so an array holding the same bound struct type twice is homogeneous —
    // recompiling per use would allocate a second id and the element check
    // would conflict.
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s = type_struct(&mut ir, &[f]);
    let a = array(&mut ir, &[s, s]);
    let b = build(a, ir);
    assert!(b.ok, "one shared occurrence is one nominal type");
    assert!(b.module.unify_errors.is_empty());
    assert_eq!(
        b.module.global_ext.type_id_counter, 1,
        "one expression compiles to exactly one Fresh call"
    );
}

/// Struct instantiation: `s(1, 2)` — the struct type applied to a tuple.
fn instantiate(ir: &mut IR, type_expr: ExprId, value: ExprId) -> ExprId {
    ir.alloc_instantiate(type_expr, value, None)
}

// --- struct instantiation ----------------------------------------------------
// `s(1, 2)` wraps the positional tuple in the struct type: the element-type
// list is checked against the field list, and the expression's type is the
// struct type itself.

#[test]
fn a_tuple_instantiated_with_a_struct_type_is_an_instance() {
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let f1 = int_t(&mut ir);
    let f2 = int_t(&mut ir);
    let s = type_struct(&mut ir, &[f1, f2]);
    let inst = instantiate(&mut ir, s, v);
    let b = build(inst, ir);
    assert!(b.ok, "s(1, 2) must check");
    assert!(b.module.unify_errors.is_empty());
    // the instance's type is the struct type, not the tuple type
    assert_eq!(
        b.ty[inst], b.term[s],
        "the instance's type is the struct type"
    );
}

#[test]
fn a_struct_instantiation_checks_its_fields() {
    // arity: two fields, one value
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let f = int_t(&mut ir);
    let s = type_struct(&mut ir, &[f]);
    let inst = instantiate(&mut ir, s, v);
    let b = build(inst, ir);
    assert!(
        !b.ok,
        "s(1, 2) against a one-field struct must fail (arity)"
    );
    // field type: the tuple's Ints are not Type
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let t1 = ty(&mut ir);
    let t2 = ty(&mut ir);
    let s = type_struct(&mut ir, &[t1, t2]);
    let inst = instantiate(&mut ir, s, v);
    let b = build(inst, ir);
    assert!(
        !b.ok,
        "s(1, 2) against struct {{ Type, Type }} must fail (fields)"
    );
    // a literal is not a positional value
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let f = int_t(&mut ir);
    let s = type_struct(&mut ir, &[f]);
    let inst = instantiate(&mut ir, s, five);
    let b = build(inst, ir);
    assert!(!b.ok, "s(5) must fail — a literal is not a struct value");
}

#[test]
fn instances_of_different_struct_occurrences_conflict() {
    // [s1(1, 2), s2(1, 2)] — each instance carries its own nominal id, so
    // the array element check reports the conflict (same fields, different
    // types).
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let f1 = int_t(&mut ir);
    let f2 = int_t(&mut ir);
    let s1 = type_struct(&mut ir, &[f1, f2]);
    let i1 = instantiate(&mut ir, s1, v);
    let f3 = int_t(&mut ir);
    let f4 = int_t(&mut ir);
    let s2 = type_struct(&mut ir, &[f3, f4]);
    let i2 = instantiate(&mut ir, s2, v);
    let a = array(&mut ir, &[i1, i2]);
    let b = build(a, ir);
    assert!(
        !b.ok,
        "instances of different struct occurrences must conflict"
    );
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("TypeId("), "{}", diags[0].message);
}

// --- the `_` placeholder ----------------------------------------------------

#[test]
fn an_underscore_annotation_infers_the_type() {
    // 5 : _ — the placeholder's value slot binds to the int marker, its
    // kind slot to the universe.
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let h = hole(&mut ir);
    let a = ann(&mut ir, five, h);
    let mut b = build(a, ir);
    assert!(b.ok, "5 : _ should check");
    let ids = array_ids(&b, b.ty[a].unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        b.module.equality_representative(ids[0]),
        b.module.equality_representative(b.int_marker),
        "the placeholder's value slot binds to the int marker"
    );
    assert_eq!(
        b.module.equality_representative(ids[1]),
        b.module.equality_representative(b.type_expr),
        "the placeholder's kind slot binds to the universe"
    );
}

#[test]
fn an_underscore_annotation_binds_a_function_type() {
    // (\x. x) : _ — the placeholder's kind slot is a cell too, so it binds
    // to the identity's kind expression instead of clashing with the
    // universe; the parameter type stays unbound (inference does not guess).
    let mut ir = IR::new();
    let x = param(&mut ir);
    let l = lam(&mut ir, x, x);
    let h = hole(&mut ir);
    let a = ann(&mut ir, l, h);
    let mut b = build(a, ir);
    assert!(b.ok, "(\\x. x) : _ should check");
    // The placeholder's value slot binds to the arrow shape.
    let ann_ids = array_ids(&b, b.ty[a].unwrap());
    let shape = array_ids(&b, b.ty[l].unwrap())[0];
    assert_eq!(
        b.module.equality_representative(ann_ids[0]),
        b.module.equality_representative(shape),
        "the placeholder binds to the arrow shape"
    );
    // The parameter type cell stays unbound.
    let shape_ids = array_ids(&b, shape);
    assert!(
        lichen_lowlevel::is_unbound(b.module.nodes[shape_ids[0]].value),
        "the parameter type must not be guessed"
    );
}

#[test]
fn partial_inference_in_an_arrow_type() {
    // (\x. x) : (Int -> _) — the parameter side fixes the input to int, the
    // placeholder return binds to the output (int, for the identity).
    let mut ir = IR::new();
    let x = param(&mut ir);
    let l = lam(&mut ir, x, x);
    let it = int_t(&mut ir);
    let h = hole(&mut ir);
    let t = arrow(&mut ir, it, h);
    let a = ann(&mut ir, l, t);
    let mut b = build(a, ir);
    assert!(b.ok, "the identity fits Int -> _");
    let arrow = array_ids(&b, b.ty[l].unwrap());
    let shape_ids = array_ids(&b, arrow[0]);
    assert_eq!(
        b.module.equality_representative(shape_ids[0]),
        b.module.equality_representative(b.int_type),
        "the parameter type unifies with int"
    );
}

#[test]
fn an_underscore_in_the_array_length_position() {
    // [1, 2, 3] : Int<_> — the placeholder length binds to the element
    // count.
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let e3 = int(&mut ir, 3);
    let arr = array(&mut ir, &[e1, e2, e3]);
    let it = int_t(&mut ir);
    let h = hole(&mut ir);
    let t = type_array(&mut ir, it, h);
    let a = ann(&mut ir, arr, t);
    let mut b = build(a, ir);
    assert!(b.ok, "[1, 2, 3] : Int<_> should check");
    // The annotated type's length slot unifies with the literal's length 3.
    let ann_shape = array_ids(&b, b.ty[a].unwrap())[0];
    let length_slot = array_ids(&b, ann_shape)[1];
    let arr_shape = array_ids(&b, b.ty[arr].unwrap())[0];
    let len3 = array_ids(&b, arr_shape)[1];
    assert!(matches!(
        b.module.nodes[len3].value,
        Some(HighProgramValue::USize(3))
    ));
    assert_eq!(
        b.module.equality_representative(length_slot),
        b.module.equality_representative(len3),
        "the placeholder length binds to the element count"
    );
}

#[test]
fn a_mismatch_against_a_partial_type_is_still_an_error() {
    // 5 : (Int -> _) — the placeholder does not mask the mismatch between
    // the literal and the function type.
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let it = int_t(&mut ir);
    let h = hole(&mut ir);
    let t = arrow(&mut ir, it, h);
    let a = ann(&mut ir, five, t);
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
}
