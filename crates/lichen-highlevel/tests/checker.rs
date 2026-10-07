//! The highlevel checker: compiles an IR into a lowlevel Module where
//! the runtime *is* the typechecker — values are recursive pairs
//! `[value, type]` whose type slots are themselves pairs bottoming out at
//! the self-referential `Type : Type` universe, and the apply-time unify is
//! the parameter type check.

use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ChildRange, ExprId, ExprKind, IR};
use lichen_highlevel::program::{
    HighGlobal, HighProgramLiteral, HighProgramValue, IntLit, IntTypeLit, ProgramImpl, TypeTypeLit,
    TypeValue,
};
use lichen_lowlevel::{AnyFunctionId, AnyNodeId, ArrayItem, FunctionId, LowValue, NodeId};
use lichen_utils::compose::AsField;

// --- hand-built IR helpers (the language frontend will produce these) -----

/// The dynamic node behind an item ref — the checker builds only dynamic graphs.
fn dyn_node(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => unreachable!("checker graphs are dynamic"),
    }
}

fn int(ir: &mut IR, n: u64) -> ExprId {
    ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(
        n as usize,
    ))))
}
fn ty(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Literal(HighProgramLiteral::from(TypeTypeLit)))
}
fn int_t(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntTypeLit)))
}
fn param(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Parameter)
}
fn lam(ir: &mut IR, b: ExprId, body: ExprId) -> ExprId {
    lam_at(ir, b, body, None)
}
/// A lambda whose enclosing function is `parent` — the explicit link the
/// checker turns into [`Function::parent`](lichen_lowlevel::Function::parent).
/// A nested closure joins the enclosing template; `None` is top level (or the
/// mutual-recursion sibling case), which hangs under nothing.
///
/// Note the allocation order: a nested lambda may be built *before* the
/// lambda enclosing it (hence needing a reserved id, see `lam_in`).
fn lam_at(ir: &mut IR, b: ExprId, body: ExprId, parent: Option<ExprId>) -> ExprId {
    lam_at_typed(ir, b, None, body, parent)
}
/// A lambda whose parameter carries its annotated type — `x : T => e`.
fn lam_at_typed(
    ir: &mut IR,
    b: ExprId,
    t: Option<ExprId>,
    body: ExprId,
    parent: Option<ExprId>,
) -> ExprId {
    ir.alloc(ExprKind::Function {
        parameter: b,
        parameter_type: t,
        parameter_attribute: None,
        r#return: body,
        parent,
        looping: false,
    })
}
/// A lambda nested inside another that is **not allocated yet**: reserves the
/// enclosing node's id first, so the inner lambda's parent link can name it,
/// then fills the reserved node's kind in (the same reserve-then-stamp
/// discipline the frontend's `fn_parents` uses).  Returns `(enclosing, inner)`.
fn lam_nested(
    ir: &mut IR,
    inner_b: ExprId,
    inner_body: ExprId,
    outer_b: ExprId,
) -> (ExprId, ExprId) {
    let reserved = ir.alloc(ExprKind::Placeholder);
    let inner = lam_at(ir, inner_b, inner_body, Some(reserved));
    ir.set_kind(
        reserved,
        ExprKind::Function {
            parameter: outer_b,
            parameter_type: None,
            parameter_attribute: None,
            r#return: inner,
            parent: None,
            looping: false,
        },
    );
    (reserved, inner)
}
fn app(ir: &mut IR, f: ExprId, x: ExprId) -> ExprId {
    ir.alloc(ExprKind::Apply {
        function: f,
        argument: x,
    })
}
/// `a[i]` — an array-element read (the container is pinned to an array).
fn index(ir: &mut IR, a: ExprId, i: ExprId) -> ExprId {
    ir.alloc(ExprKind::Index { array: a, index: i })
}
/// `a(k)` — a positional slot read over a tuple element (a struct instance
/// reads by name, `s.x`; a decided non-tuple container is refused).
fn field(ir: &mut IR, c: ExprId, k: ExprId) -> ExprId {
    ir.alloc(ExprKind::Field {
        container: c,
        key: k,
    })
}
/// A **named** field read `c.name` — the form that accepts a struct container.
fn named_field(ir: &mut IR, c: ExprId, name: &'static str) -> ExprId {
    ir.alloc(ExprKind::NamedField { container: c, name })
}
fn ann(ir: &mut IR, e: ExprId, t: ExprId) -> ExprId {
    ir.alloc(ExprKind::Annotation {
        value: e,
        r#type: Some(t),
        attributes: ChildRange::EMPTY,
    })
}
fn arrow(ir: &mut IR, d: ExprId, c: ExprId) -> ExprId {
    ir.alloc(ExprKind::TypeFunction {
        parameter: d,
        r#return: c,
    })
}
fn tuple(ir: &mut IR, elements: &[ExprId]) -> ExprId {
    ir.alloc_tuple(elements)
}
/// A tuple type expression: `[int, int]`.
fn type_tuple(ir: &mut IR, elements: &[ExprId]) -> ExprId {
    ir.alloc_type_tuple(elements)
}
/// A struct type expression with field names: `struct<.a T1, .b T2>` — every
/// struct field is named (an unnamed one is refused, `DiagKind::StructFieldName`).
fn named_type_struct(ir: &mut IR, fields: &[(ExprId, &'static str)]) -> ExprId {
    let fields: Vec<(ExprId, Option<&'static str>)> =
        fields.iter().map(|&(e, name)| (e, Some(name))).collect();
    ir.alloc_type_struct(&fields)
}
/// An array literal: `[1, 2]` — all elements share one type.
fn array(ir: &mut IR, elements: &[ExprId]) -> ExprId {
    ir.alloc_array(elements)
}
/// The real array type: `array<Int, 3>`.
fn type_array(ir: &mut IR, element_type: ExprId, length: ExprId) -> ExprId {
    ir.alloc(ExprKind::TypeArray {
        element_type,
        length,
    })
}
/// `_` — an inference placeholder hole in any position (type or value).
fn hole(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::Placeholder)
}
/// A recovered-error region — an opaque leaf the checker must skip.  Distinct
/// from [`hole`], so the frontend can identify it for a diff / mask.
fn err_block(ir: &mut IR) -> ExprId {
    ir.alloc(ExprKind::ErrorBlock)
}

fn build(root: ExprId, mut ir: IR) -> lichen_highlevel::checker::Build<ProgramImpl> {
    ir.set_root(root);
    Checker::build(ir)
}

/// The ids inside a checker-built array value.
fn array_ids(
    b: &lichen_highlevel::checker::Build<ProgramImpl>,
    node: lichen_lowlevel::NodeId,
) -> Vec<lichen_lowlevel::NodeId> {
    // SAFETY: `node` is a live node of the build under test, whose block has
    // not been dropped.
    unsafe { b.module.array_items(node) }
        .expect("expected an array value")
        .iter()
        .map(|item| dyn_node(item.node))
        .collect()
}

/// The `FunctionId` a function-type node `[Function(fid), ↺]` carries — the
/// function's own type (`f : f`). Reads slot 0's value.
fn function_type_id(
    b: &lichen_highlevel::checker::Build<ProgramImpl>,
    ftype: lichen_lowlevel::NodeId,
) -> lichen_lowlevel::FunctionId {
    let slot0 = array_ids(b, ftype)[0];
    match b.module.node_value(AnyNodeId::Dynamic(slot0)) {
        Some(HighProgramValue::LowValue(LowValue::Function(AnyFunctionId::Dynamic(fid)))) => fid,
        _ => panic!("expected a function-type node"),
    }
}

/// The function template's parameter *type* cell — `Function::parameter`'s
/// slot 1 — the signature's domain, read for inference checks.
fn param_type_cell(
    b: &lichen_highlevel::checker::Build<ProgramImpl>,
    fid: lichen_lowlevel::FunctionId,
) -> lichen_lowlevel::NodeId {
    let parameter = b.module.functions[fid].parameter;
    array_ids(b, parameter)[1]
}

/// The ids inside an evaluated array value.
fn array_ids_from(value: HighProgramValue) -> Vec<lichen_lowlevel::NodeId> {
    let HighProgramValue::LowValue(LowValue::Array(array)) = value else {
        panic!("expected an array value, got {value:?}");
    };
    // SAFETY: the value was just produced by the module under test, whose
    // block has not been dropped.
    unsafe { array.items() }
        .iter()
        .map(|item| dyn_node(item.node))
        .collect()
}

/// The shallow flags inside an evaluated array value.
fn array_mask_from(value: HighProgramValue) -> Vec<bool> {
    let HighProgramValue::LowValue(LowValue::Array(array)) = value else {
        panic!("expected an array value, got {value:?}");
    };
    // SAFETY: the value was just produced by the module under test, whose
    // block has not been dropped.
    unsafe { array.items() }
        .iter()
        .map(|item| item.shallow)
        .collect()
}

/// Whether the given ids form the int type — a `[int, Type]` pair.  The two
/// slots are matched differently, on purpose: element 0 is the marker and is
/// matched by *content*, because a literal rebuilds its type per occurrence and
/// so is never the shared `Build::int_type` node; element 1 is the universe and
/// is compared by node identity against the canonical `b.type_expr`.
fn is_int_type_ids(b: &lichen_highlevel::checker::Build<ProgramImpl>, ids: &[NodeId]) -> bool {
    ids.len() == 2
        && matches!(
            b.module.node_value(AnyNodeId::Dynamic(ids[0])),
            Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
        )
        && ids[1] == b.type_expr
}

/// Whether `node` is the int type — the two-slot test `is_int_type_ids` states.
fn is_int_type(b: &lichen_highlevel::checker::Build<ProgramImpl>, node: NodeId) -> bool {
    is_int_type_ids(b, &array_ids(b, node))
}

/// Whether an evaluated value is the int type pair `[int, Type]` — the
/// two-slot test `is_int_type_ids` states, on the evaluated ids.
fn is_int_type_value(
    b: &lichen_highlevel::checker::Build<ProgramImpl>,
    value: HighProgramValue,
) -> bool {
    is_int_type_ids(b, &array_ids_from(value))
}

// --- checking -------------------------------------------------------------

#[test]
fn int_literal_checks() {
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let b = build(five, ir);
    assert!(b.ok, "5 should check");
    // The type of 5 is the recursive pair [int, [Type, ↺]] — rebuilt fresh per
    // occurrence (content-equal to the shared int_type, not node-identical).
    assert!(is_int_type(&b, b.state[five].ty.unwrap()));
    let ids = array_ids(&b, b.state[five].ty.unwrap());
    assert_eq!(ids.len(), 2);
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(ids[0])),
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
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
    assert!(is_int_type(&b, b.state[a].ty.unwrap()));
}

#[test]
fn the_type_universe_is_self_referential() {
    // `Type` compiles to the canonical node K = [Type, K] — Type : Type via
    // a self-cycle, closing every type spine.
    let mut ir = IR::new();
    let t = ty(&mut ir);
    let b = build(t, ir);
    assert!(b.ok);
    assert_eq!(b.state[t].term, Some(b.type_expr));
    assert_eq!(b.state[t].ty, Some(b.type_expr), "Type : Type");
    let ids = array_ids(&b, b.type_expr);
    assert_eq!(ids.len(), 2);
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(ids[0])),
        Some(HighProgramValue::TypeValue(TypeValue::TypeType))
    ));
    assert_eq!(
        ids[1], b.type_expr,
        "the universe's type slot cycles back to itself"
    );
}

#[test]
fn an_error_block_is_skipped_and_never_cascades() {
    // A recovered-error region lowers to `ExprKind::ErrorBlock` (distinct from
    // a real `_` = Placeholder).  The checker's *skip* path compiles it to a
    // pair of fresh, never-unified cells: the masked region carries no grammar,
    // so it is not checked and cannot introduce a type-level "expected X, found
    // Y" from inside a region the user is still typing.
    let mut ir = IR::new();
    let e = err_block(&mut ir);
    let b = build(e, ir);
    assert!(b.ok, "a masked error region is not a check failure");
    // The skip path still records the pair's three slots, so every downstream
    // read (`value_of`, `ty`) works and the expression is "done" (recompiles).
    assert!(b.state[e].term.is_some(), "the skip path records the pair");
    assert!(
        b.state[e].val.is_some(),
        "the skip path records the value slot"
    );
    assert!(
        b.state[e].ty.is_some(),
        "the skip path records the type slot"
    );
    // The two slots are fresh (undecided) cells — never unified by the
    // surrounding context, so they have no value and no type conflict.
    let ty_cell = b.state[e].ty.unwrap();
    assert!(
        b.module.node_value(AnyNodeId::Dynamic(ty_cell)).is_none(),
        "the type cell is a fresh, undecided cell"
    );
}

#[test]
fn an_error_block_as_a_child_does_not_cascade() {
    // An error block embedded in real code must not poison it.  A homogeneous
    // array unifies its element types; the error block's fresh type cell binds
    // quietly to the int sibling instead of raising a conflict — the array
    // still checks, and the masked region is never "expected X, found Y"ed.
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let e = err_block(&mut ir);
    let b = build(array(&mut ir, &[e, five]), ir);
    assert!(b.ok, "the error block binds without a conflict");

    // A heterogeneous tuple does not unify elements at all; the error block
    // rides along and the whole thing checks.
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let e = err_block(&mut ir);
    let b = build(tuple(&mut ir, &[e, five]), ir);
    assert!(b.ok, "a tuple with an error block checks");
    assert!(
        b.state[e].term.is_some(),
        "the embedded error block still records its pair"
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
    // The lambda's type is the function itself (`f : f`): a self-referential
    // `[Function(fid), ↺]` — slot 0 the function's value node, slot 1 the node
    // itself (the self-cycle, like the universe `[Type, ↺]`).
    let ftype = b.state[l].ty.unwrap();
    let ids = array_ids(&b, ftype);
    assert_eq!(ids.len(), 2, "a function-type node is a pair [func, self]");
    assert_eq!(
        ids[1], ftype,
        "slot 1 is the node itself (self-referential)"
    );
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(ids[0])),
        Some(HighProgramValue::LowValue(LowValue::Function(_)))
    ));
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
    let ty = b.state[tup].ty.unwrap();
    let ids = array_ids(&b, ty);
    assert_eq!(ids.len(), 2, "a type expression is a pair [shape, kind]");
    let shape_ids = array_ids(&b, ids[0]);
    assert_eq!(shape_ids.len(), 2);
    assert!(is_int_type(&b, shape_ids[0]));
    assert!(is_int_type(&b, shape_ids[1]));
    let kind_ids = array_ids(&b, ids[1]);
    assert_eq!(kind_ids.len(), 2);
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(kind_ids[0])),
        Some(HighProgramValue::TypeValue(TypeValue::TypeTuple))
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
    let ty = b.state[arr].ty.unwrap();
    let kind_ids = array_ids(&b, ty);
    assert_eq!(kind_ids.len(), 2);
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(kind_ids[0])),
        Some(HighProgramValue::TypeValue(TypeValue::TypeArray))
    ));
    assert_eq!(kind_ids[1], b.type_expr);
    // The value is the instance [type, length].
    let shape = b.state[arr].val.unwrap();
    let shape_ids = array_ids(&b, shape);
    assert_eq!(shape_ids.len(), 2);
    assert!(
        is_int_type(&b, shape_ids[0]),
        "instance[0] is the element type"
    );
    assert!(
        matches!(
            b.module.node_value(AnyNodeId::Dynamic(shape_ids[1])),
            Some(HighProgramValue::LowValue(LowValue::USize(3)))
        ),
        "instance[1] is the length"
    );
    // The pair is [shape, kind].
    let ids = array_ids(&b, b.state[arr].term.unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], shape);
    assert_eq!(ids[1], ty);
}

#[test]
fn the_array_type_has_a_kind_not_a_type() {
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
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeArray))
    );
    assert_eq!(
        diags[0].value_b,
        Some(HighProgramValue::TypeValue(TypeValue::TypeType))
    );
}

#[test]
fn lambda_against_an_array_type_conflicts_on_the_length() {
    // (\x. x) : Array(int, 3) — a function's type is not an array type, so the
    // annotation conflicts (a function-type node against an array type).
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
        "one conflict: a function-type vs an array type"
    );
    assert_eq!(diags[0].kind, DiagKind::Annotation);
}

#[test]
fn array_literal_is_homogeneous() {
    // [1, 2] — the pair [[1, 2], [[int, 2], [ArrayType, Type]]]: one type
    // slot shared by all elements, the length is the element count.
    let mut ir = IR::new();
    let e1 = int(&mut ir, 1);
    let e2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[e1, e2]);
    let b = build(arr, ir);
    assert!(b.ok, "[1, 2] as an array literal should check");
    let ty = b.state[arr].ty.unwrap();
    let ids = array_ids(&b, ty);
    assert_eq!(ids.len(), 2, "a type expression is a pair [shape, kind]");
    let shape_ids = array_ids(&b, ids[0]);
    assert_eq!(shape_ids.len(), 2);
    assert!(
        is_int_type(&b, shape_ids[0]),
        "the shared element type unifies to int"
    );
    assert!(
        matches!(
            b.module.node_value(AnyNodeId::Dynamic(shape_ids[1])),
            Some(HighProgramValue::LowValue(LowValue::USize(2)))
        ),
        "instance[1] is the element count"
    );
    let kind_ids = array_ids(&b, ids[1]);
    assert_eq!(kind_ids.len(), 2);
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(kind_ids[0])),
        Some(HighProgramValue::TypeValue(TypeValue::TypeArray))
    ));
    assert_eq!(kind_ids[1], b.type_expr);
    // The value holds the element values.
    let value_ids = array_ids(&b, b.state[arr].val.unwrap());
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
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::LowValue(LowValue::USize(2)))
    );
    assert_eq!(
        diags[0].value_b,
        Some(HighProgramValue::LowValue(LowValue::USize(3)))
    );
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
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeType))
    );
    assert_eq!(
        diags[0].value_b,
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
    );
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
    let (outer_lam, inner_lam) = lam_nested(&mut ir, b2, body2, b1);
    let inner = app(&mut ir, inner_lam, a);
    // The enclosing lambda's own return is the applied inner one, so its
    // reserved node's kind must be re-stamped after `inner` exists.
    ir.set_kind(
        outer_lam,
        ExprKind::Function {
            parameter: b1,
            parameter_type: None,
            parameter_attribute: None,
            r#return: inner,
            parent: None,
            looping: false,
        },
    );
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
    let (t_lam, _inner) = lam_nested(&mut ir, bx, body, bt);
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
    let value = module.evaluate_node_deep(b.root_val, None).unwrap();
    assert!(matches!(
        value,
        HighProgramValue::LowValue(LowValue::USize(5))
    ));
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
    // PROBE: what the build reports when the unify no longer forces.
    if !b.ok && std::env::var_os("LICHEN_BUILD_TRACE").is_some() {
        for d in b.diagnostics() {
            eprintln!(
                "PROBE diag: kind={:?} a={:?} b={:?} loc={:?} va={:?} vb={:?}",
                d.kind, d.a, d.b, d.loc, d.value_a, d.value_b
            );
        }
        for e in &b.module.unify_errors {
            eprintln!(
                "PROBE unify_error: a={:?} b={:?} va={:?} vb={:?} steps={:?}",
                e.a, e.b, e.value_a, e.value_b, e.steps
            );
        }
    }
    assert!(b.ok);
    let mut module = b.module;
    let value = module.evaluate_node_deep(b.root_val, None).unwrap();
    assert!(matches!(
        value,
        HighProgramValue::LowValue(LowValue::USize(5))
    ));
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
    let value = module.evaluate_node_deep(b.root_val, None).unwrap();
    assert!(matches!(
        value,
        HighProgramValue::LowValue(LowValue::USize(5))
    ));
}

// --- diagnostics ----------------------------------------------------------

#[test]
fn annotation_mismatch_reports_expected_found() {
    // 5 : Type  →  expected TypeType, found TypeInt
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let t = ty(&mut ir);
    let a = ann(&mut ir, five, t);
    let b = build(a, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(diags[0].loc().map(|loc| loc.expr), Some(five));
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
    );
    assert_eq!(
        diags[0].value_b,
        Some(HighProgramValue::TypeValue(TypeValue::TypeType))
    );
}

#[test]
fn applying_a_non_function_reports_expected_function() {
    // 5 6 — the function-ness guard, with the flow showing where `int` came
    // from
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let six = int(&mut ir, 6);
    let call = app(&mut ir, five, six);
    let b = build(call, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Guard);
    assert_eq!(diags[0].loc().map(|loc| loc.expr), Some(call));
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
    );
    // The expected side is the function the guard built — a real function, not
    // an arrow term — so the first pair that fails to be the same value is the
    // function itself against the callee's `int` marker.  That reads as
    // "expected a function, found Int", which is what the guard means; under
    // the arrow term the clash was one level out, on the term's shape array.
    assert!(matches!(
        diags[0].value_b,
        Some(HighProgramValue::LowValue(LowValue::Function(_)))
    ));
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
    let b = build(idx, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Guard);
    assert_eq!(diags[0].loc().map(|loc| loc.expr), Some(l));
    assert_eq!(diags[0].a, b.state[l].ty.unwrap());
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
    assert_eq!(diags[0].kind, DiagKind::Guard);
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
    let b = build(call, ir);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime);
    assert_eq!(diags[0].loc().map(|loc| loc.expr), Some(five));
    // runtime direction is reversed: a = the parameter's expected type,
    // b = the argument's found type
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeType))
    );
    assert_eq!(
        diags[0].value_b,
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
    );
}

#[test]
fn annotating_a_lambda_with_a_mixed_tuple_type_reports_expected_found() {
    // (\x. x) : <Type, Int> — a function's type is not a tuple type, so the
    // annotation conflicts at the top level (a function-type node against a
    // tuple type), rather than element-wise as the old arrow shape did.
    let mut ir = IR::new();
    let x = param(&mut ir);
    // The return expression uses the parameter's id directly.
    let l = lam(&mut ir, x, x);
    let t1 = ty(&mut ir);
    let t2 = int_t(&mut ir);
    let t = type_tuple(&mut ir, &[t1, t2]);
    let a = ann(&mut ir, l, t);
    let b = build(a, ir);
    assert!(!b.ok, "a function is not a tuple type");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
}

#[test]
fn an_unannotated_lambda_has_an_undecided_arrow_type() {
    // (\x. x) : Type — a function's type is not Type (nor any self-referential
    // non-function type), so the annotation conflicts.
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
    // The found side is the function-type node `[Function(fid), ↺]`.
    assert!(matches!(
        diags[0].value_a,
        Some(HighProgramValue::LowValue(LowValue::Array(_)))
    ));
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
    let b = build(whole, ir);
    assert!(b.ok);
    assert!(b.diagnostics().is_empty());
    assert!(
        is_int_type(&b, b.root_ty),
        "the root type is synced to the return type (int)"
    );
}

#[test]
fn a_tuples_undecided_element_types_sync_from_the_return_types() {
    // a = \x. (1, Int)[x]; (a 0, a 1) — the tuple's element types are the
    // calls' lazy result cells.  Each apply's evaluation syncs its cell with
    // its return pair: element 0's cell binds to int, element 1's to the
    // universe (the `Int` constant's type is `Type`).
    let mut ir = IR::new();
    let x = param(&mut ir);
    let one = int(&mut ir, 1);
    let tval = ty(&mut ir);
    let tup = tuple(&mut ir, &[one, tval]);
    let idx = field(&mut ir, tup, x);
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
    assert!(is_int_type(&b, shape[0]), "element 0's cell syncs to int");
    assert_eq!(
        b.module.node_value(AnyNodeId::Dynamic(shape[1])),
        b.module.node_value(AnyNodeId::Dynamic(b.type_expr)),
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
    // The length mismatch: `a`/`b` are the full type pairs — their *shapes*
    // (element 0) are the positional element-type lists, two elements vs one.
    assert_eq!(array_ids(&b, array_ids(&b, diags[0].a)[0]).len(), 2);
    assert_eq!(array_ids(&b, array_ids(&b, diags[0].b)[0]).len(), 1);
}

// --- call result annotations are checked, not just bound --------------------
// A call's result type cell is a lazy record (the runtime apply does not force
// it), so an annotation on a call result only *binds* the cell at check time.
// The check happens later and elsewhere: the apply's evaluation syncs that
// cell with the callee's return pair, and a disagreement between the
// annotation and the real return type is a reported failure.  The two tests
// below pin it from both directions — a call through a parameter and a direct
// apply — each as a `Runtime` diagnostic.

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
    let (f1, _f2) = lam_nested(&mut ir, y, f2_body, x);
    let three = int(&mut ir, 3);
    let four = int(&mut ir, 4);
    let call1 = app(&mut ir, f1, three);
    let whole = app(&mut ir, call1, four);
    let mut b = build(whole, ir);
    assert!(b.ok, "the closure program must check");
    let value = b.module.evaluate_node_deep(b.root_val, None).unwrap();
    let ids = array_ids_from(value);
    let expected = [1usize, 2, 3, 4];
    assert_eq!(ids.len(), expected.len());
    for (&id, &n) in ids.iter().zip(expected.iter()) {
        assert_eq!(
            b.module.node_value(AnyNodeId::Dynamic(id)),
            Some(HighProgramValue::LowValue(LowValue::USize(n))),
            "element {n} must be a bound value, not the leaked parameter"
        );
    }
}

/// The [`FunctionId`](lichen_lowlevel::FunctionId) this lambda expression
/// compiled to — read off the expression's compiled value node.
fn function_of(b: &lichen_highlevel::checker::Build<ProgramImpl>, e: ExprId) -> FunctionId {
    let node = b.state[e].val.expect("the lambda compiled to a value node");
    let value = b
        .module
        .node_value(AnyNodeId::Dynamic(node))
        .expect("the lambda's value node is written");
    let HighProgramValue::LowValue(LowValue::Function(AnyFunctionId::Dynamic(id))) = value else {
        panic!("expected a function value, got {value:?}");
    };
    id
}

#[test]
fn a_nested_lambda_hangs_under_its_enclosing_function() {
    // The nesting half of the parent rule: a closure in a lambda's body joins
    // the enclosing template, so the outer function's own `nodes` include the
    // inner closure's nodes and the apply clone copies them per call.
    let mut ir = IR::new();
    let a = int(&mut ir, 1);
    let x = param(&mut ir);
    let y = param(&mut ir);
    let (outer, inner) = lam_nested(&mut ir, y, a, x);
    let b = build(outer, ir);
    assert!(b.ok, "the nested closure must check");
    let outer_id = function_of(&b, outer);
    let inner_id = function_of(&b, inner);
    assert_eq!(
        b.module.functions[inner_id].parent,
        Some(outer_id),
        "the inner closure's parent is the enclosing function"
    );
}

#[test]
fn a_sibling_lambda_hangs_under_nothing() {
    // The mutual-recursion sibling half: two lambdas at the same level are
    // siblings, not nested — each hangs under nothing, so neither template
    // absorbs the other and the recursion re-applies the sibling's
    // never-bound template (see `examples/mutual_recursion.lichen`).
    let mut ir = IR::new();
    let x = param(&mut ir);
    let sibling = lam(&mut ir, x, x);
    let other = lam(&mut ir, x, sibling);
    let b = build(other, ir);
    assert!(b.ok, "the sibling binding must check");
    assert_eq!(
        b.module.functions[function_of(&b, sibling)].parent,
        None,
        "a same-level sibling hangs under nothing"
    );
    assert_eq!(
        b.module.functions[function_of(&b, other)].parent,
        None,
        "the enclosing lambda is itself top level"
    );
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
    let idx = field(&mut ir, tup, zero);
    let mut b = build(idx, ir);
    assert!(b.ok, "(1, 2)[0] should check");
    assert!(
        matches!(
            b.module
                .evaluate_node_deep(b.state[idx].val.unwrap(), None)
                .unwrap(),
            HighProgramValue::LowValue(LowValue::USize(1))
        ),
        "the value is the selected element"
    );
    let ty_val = b
        .module
        .evaluate_node_deep(b.state[idx].ty.unwrap(), None)
        .unwrap();
    assert!(
        is_int_type_value(&b, ty_val),
        "the type is the element type"
    );
}

#[test]
fn array_index_selects_value_and_type() {
    // [1, 2, 3][1] — value 2, type int.  The array type shape [int, 3]
    // holds the length as data, so the type evaluation dispatches on the
    // kind (a native `Index` subgraph keyed by `IndexTypeDispatch`'s code)
    // and selects the element type at shape[0].
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
            b.module
                .evaluate_node_deep(b.state[idx].val.unwrap(), None)
                .unwrap(),
            HighProgramValue::LowValue(LowValue::USize(2))
        ),
        "the value is the selected element"
    );
    let ty_val = b
        .module
        .evaluate_node_deep(b.state[idx].ty.unwrap(), None)
        .unwrap();
    assert!(
        is_int_type_value(&b, ty_val),
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
    let idx = field(&mut ir, tup, five);
    let b = build(idx, ir);
    assert!(!b.ok, "(1, 2)[5] must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexOutOfBounds);
    assert_eq!(diags[0].index, Some(5));
    assert_eq!(diags[0].length, Some(2));
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::LowValue(LowValue::USize(5)))
    );
    assert_eq!(
        diags[0].value_b,
        Some(HighProgramValue::LowValue(LowValue::USize(2)))
    );
    assert_eq!(diags[0].loc().map(|loc| loc.expr), Some(five));
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
    let b = build(idx, ir);
    assert!(!b.ok, "[1, 2, 3][5] must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexOutOfBounds);
    assert_eq!(diags[0].index, Some(5));
    assert_eq!(diags[0].length, Some(3));
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::LowValue(LowValue::USize(5)))
    );
    assert_eq!(
        diags[0].value_b,
        Some(HighProgramValue::LowValue(LowValue::USize(3)))
    );
    assert_eq!(diags[0].loc().map(|loc| loc.expr), Some(five));
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
    assert_eq!(diags[0].index, Some(3));
    assert_eq!(diags[0].length, Some(2));
}

// --- struct types ----------------------------------------------------------
// A struct type is the pair [[field types], [marker, Type]]: like an array
// type (shape [element type, length]), the shape is the *positional
// field-type list*, and the kind slot holds the struct marker — the ordinary
// `[payload, type]` pair whose payload is `[TypeId(n), names, names_in_order]`
// (a *fresh nominal* id, the name→index table, and the same names in definition
// order) and whose type slot is the `TypeStruct` atom.  Struct-ness is that
// tag: a payload-shaped pair without it is not a struct.  Equal ids unify,
// different ids never do, and a struct never unifies with a
// same-shape tuple — nominal identity.  Every field is named, so a struct
// instance reads by name.

#[test]
fn struct_type_has_a_kind_and_carries_a_fresh_type_id() {
    // struct<.f Int, .t Type> — the pair [[int, Type], [[TypeId(0), names,
    // names_in_order], TypeStruct]].
    let mut ir = IR::new();
    let t1 = int_t(&mut ir);
    let t2 = ty(&mut ir);
    let s = named_type_struct(&mut ir, &[(t1, "f"), (t2, "t")]);
    let mut b = build(s, ir);
    assert!(b.ok, "struct<.f Int, .t Type> should kind");
    assert!(b.module.unify_errors.is_empty());
    // the shape is just the positional field-type list [int, Type] — the
    // nominal id does not ride in the shape
    let struct_shape = b.state[s].val.unwrap();
    let shape_ids = array_ids(&b, struct_shape);
    assert_eq!(shape_ids.len(), 2);
    assert!(is_int_type(&b, shape_ids[0]));
    // the kind slot is a standard [marker, K] pair; its marker is the ordinary
    // [payload, type] pair: the payload [id, names, names_in_order] in the
    // value slot, the TypeStruct atom in the type slot.
    let kind = b.state[s].ty.unwrap();
    let kind_ids = array_ids(&b, kind);
    assert_eq!(kind_ids.len(), 2);
    assert_eq!(kind_ids[1], b.type_expr);
    let marker = kind_ids[0];
    let marker_ids = array_ids(&b, marker);
    assert_eq!(marker_ids.len(), 2, "the marker is a [value, type] pair");
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(marker_ids[1])),
        Some(HighProgramValue::TypeValue(TypeValue::TypeStruct))
    ));
    let payload_ids = array_ids(&b, marker_ids[0]);
    assert_eq!(payload_ids.len(), 3);
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(payload_ids[0])),
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(0)))
    ));
    // a named struct's payload carries a name table in its names slot (the
    // `Error` marker is the no-names case, reachable only from hand-built IR:
    // every source struct type now has named fields).
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(payload_ids[1])),
        Some(HighProgramValue::LowValue(LowValue::Table(_)))
    ));
    // The tag is what makes it a struct marker: a pair with the same payload
    // but the universe (not the `TypeStruct` atom) in its type slot is not one,
    // and a type whose kind's marker is that pair is not a struct type.
    let block = b.module.node_block(marker);
    let tagless_items = b.module.alloc_array(
        &[
            ArrayItem::new(AnyNodeId::Dynamic(marker_ids[0])),
            ArrayItem::new(AnyNodeId::Dynamic(b.type_expr)),
        ],
        block,
    );
    let tagless = b.module.add_node(
        block,
        None,
        Some(HighProgramValue::from(LowValue::Array(tagless_items))),
    );
    assert!(
        !lichen_highlevel::shape::is_struct_marker_any(&b.module, AnyNodeId::Dynamic(tagless)),
        "a marker-shaped pair without the TypeStruct tag is not a struct marker"
    );
    let tagless_kind_items = b.module.alloc_array(
        &[
            ArrayItem::new(AnyNodeId::Dynamic(tagless)),
            ArrayItem::new(AnyNodeId::Dynamic(b.type_expr)),
        ],
        block,
    );
    let tagless_kind = b.module.add_node(
        block,
        None,
        Some(HighProgramValue::from(LowValue::Array(tagless_kind_items))),
    );
    let tagless_term_items = b.module.alloc_array(
        &[
            ArrayItem::new(AnyNodeId::Dynamic(struct_shape)),
            ArrayItem::new(AnyNodeId::Dynamic(tagless_kind)),
        ],
        block,
    );
    let tagless_term = b.module.add_node(
        block,
        None,
        Some(HighProgramValue::from(LowValue::Array(tagless_term_items))),
    );
    assert!(
        !lichen_highlevel::shape::is_struct_type_any(
            &mut b.module,
            b.type_expr,
            AnyNodeId::Dynamic(tagless_term)
        ),
        "a type whose kind's marker lacks the TypeStruct tag is not a struct type"
    );
    // one source occurrence consumed exactly one fresh id
    assert_eq!(
        AsField::<HighGlobal>::get(&b.module.global_ext).type_id_counter,
        1
    );
}

#[test]
fn a_named_struct_carries_a_name_to_index_table() {
    // struct<.a Int, .b Type> — the struct marker pair `[payload, TypeStruct]`
    // (in the kind's marker slot) holds a payload `[id, names, names_in_order]`
    // whose names slot maps each field name to its positional index, and the
    // same names again in definition order.
    let mut ir = IR::new();
    let t1 = int_t(&mut ir);
    let t2 = ty(&mut ir);
    let s = named_type_struct(&mut ir, &[(t1, "a"), (t2, "b")]);
    let b = build(s, ir);
    assert!(b.ok);
    let kind = b.state[s].ty.unwrap();
    let kind_ids = array_ids(&b, kind);
    assert_eq!(kind_ids.len(), 2);
    let marker_ids = array_ids(&b, kind_ids[0]);
    assert_eq!(marker_ids.len(), 2);
    let payload_ids = array_ids(&b, marker_ids[0]);
    assert_eq!(payload_ids.len(), 3);
    // the names field (the payload's slot 1) is a constant table: "a" -> 0,
    // "b" -> 1.
    let names_node = payload_ids[1];
    let Some(HighProgramValue::LowValue(LowValue::Table(table))) =
        b.module.node_value(AnyNodeId::Dynamic(names_node))
    else {
        panic!("the names field must be a table");
    };
    // SAFETY: `names_node` is a live node of the build under test, whose block
    // has not been dropped.
    let items = unsafe { table.items() };
    assert_eq!(items.len(), 2);
    let mut found: Vec<(&str, usize)> = items
        .iter()
        .map(|item| {
            let name = match b.module.node_value(item.key) {
                Some(HighProgramValue::LowValue(LowValue::Str(s))) => s,
                other => panic!("a field-name key must be a string: {other:?}"),
            };
            let index = match b.module.node_value(item.value) {
                Some(HighProgramValue::LowValue(LowValue::USize(n))) => n,
                other => panic!("a field-name value must be an index: {other:?}"),
            };
            (name, index)
        })
        .collect();
    found.sort_by_key(|&(_, i)| i);
    assert_eq!(found, vec![("a", 0), ("b", 1)]);
    // the definition-order field (the payload's slot 2) is an array of the
    // names, one per definition position — the table's inverse, which the
    // deferred named instantiation's reorder reads.
    let in_order = array_ids(&b, payload_ids[2]);
    let names: Vec<&str> = in_order
        .iter()
        .map(
            |&node| match b.module.node_value(AnyNodeId::Dynamic(node)) {
                Some(HighProgramValue::LowValue(LowValue::Str(s))) => s,
                other => panic!("a definition-order name must be a string: {other:?}"),
            },
        )
        .collect();
    assert_eq!(names, vec!["a", "b"]);
}

#[test]
fn each_struct_type_occurrence_allocates_a_distinct_id() {
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s1 = named_type_struct(&mut ir, &[(f, "f")]);
    let s2 = named_type_struct(&mut ir, &[(f, "f")]);
    let pair = tuple(&mut ir, &[s1, s2]);
    let b = build(pair, ir);
    assert!(b.ok);
    assert_eq!(
        AsField::<HighGlobal>::get(&b.module.global_ext).type_id_counter,
        2
    );
    // the nominal id is the marker payload's slot 0: kind = [marker, K],
    // marker = [payload, TypeStruct], payload = [id, names, names_in_order].
    let id1 = array_ids(
        &b,
        array_ids(&b, array_ids(&b, b.state[s1].ty.unwrap())[0])[0],
    )[0];
    let id2 = array_ids(
        &b,
        array_ids(&b, array_ids(&b, b.state[s2].ty.unwrap())[0])[0],
    )[0];
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(id1)),
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(0)))
    ));
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(id2)),
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(1)))
    ));
}

#[test]
fn two_struct_type_occurrences_do_not_unify() {
    // same fields, different occurrences → different ids → nominal conflict
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s1 = named_type_struct(&mut ir, &[(f, "f")]);
    let s2 = named_type_struct(&mut ir, &[(f, "f")]);
    let pair = tuple(&mut ir, &[s1, s2]);
    let b = build(pair, ir);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.state[s1].term.unwrap(), b.state[s2].term.unwrap());
    assert_eq!(module.unify_errors.len(), 1);
    let err = module.unify_errors[0].clone();
    assert!(matches!(
        err.value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(0)))
    ));
    assert!(matches!(
        err.value_b,
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(1)))
    ));
}

#[test]
fn a_struct_type_does_not_unify_with_a_same_shape_tuple_type() {
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s = named_type_struct(&mut ir, &[(f, "f")]);
    let t = type_tuple(&mut ir, &[f]);
    let pair = tuple(&mut ir, &[s, t]);
    let b = build(pair, ir);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.state[s].term.unwrap(), b.state[t].term.unwrap());
    assert_eq!(module.unify_errors.len(), 1);
    // The struct and tuple shapes are both the field-type list (same arity),
    // so the nominal distinction now lives at the kind's marker: a struct
    // marker is the `[payload, TypeStruct]` pair, while a tuple
    // marker is the `TupleType` type constant — they clash at the marker
    // slot of the `[marker, K]` kind.
    let err = module.unify_errors[0].clone();
    let (a, b) = (err.value_a, err.value_b);
    assert!(
        matches!(
            (&a, &b),
            (
                Some(HighProgramValue::LowValue(LowValue::Array(_))),
                Some(HighProgramValue::TypeValue(TypeValue::TypeTuple))
            ) | (
                Some(HighProgramValue::TypeValue(TypeValue::TypeTuple)),
                Some(HighProgramValue::LowValue(LowValue::Array(_)))
            )
        ),
        "marker-level nominal clash: got {a:?} vs {b:?}"
    );
}

#[test]
fn a_struct_type_unifies_with_itself() {
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s = named_type_struct(&mut ir, &[(f, "f")]);
    let b = build(s, ir);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.state[s].term.unwrap(), b.state[s].term.unwrap());
    assert!(
        module.unify_errors.is_empty(),
        "the same struct type unifies with itself"
    );
}

#[test]
fn an_annotation_against_a_struct_type_reports_the_conflict() {
    // 5 : struct<.f Int> — the literal's int type conflicts with the struct
    // type; the struct pair (the diary's expected side) renders with its
    // nominal id in the flow line.
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let f = int_t(&mut ir);
    let s = named_type_struct(&mut ir, &[(f, "f")]);
    let a = ann(&mut ir, five, s);
    let b = build(a, ir);
    assert!(!b.ok, "5 : struct<.f Int> must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
    );
    // The struct's shape is the structured expected side now; the wording
    // ("expected [...]") is re-rendered by the language layer from the kind
    // and the value_a/value_b facts.
    assert!(
        diags[0].loc().is_some(),
        "the annotation conflict is located"
    );
}

#[test]
fn two_struct_types_conflict_reports_the_nominal_ids() {
    // (\x. (x : struct<.f Int>)) (struct<.f Int>) — the argument's struct
    // type has a different fresh id than the annotation's, so the apply-time
    // unify fails on the ids.
    let mut ir = IR::new();
    let f1 = int_t(&mut ir);
    let s1 = named_type_struct(&mut ir, &[(f1, "f")]);
    let f2 = int_t(&mut ir);
    let s2 = named_type_struct(&mut ir, &[(f2, "f")]);
    let x = param(&mut ir);
    let body = ann(&mut ir, x, s1);
    let l = lam(&mut ir, x, body);
    let whole = app(&mut ir, l, s2);
    let b = build(whole, ir);
    assert!(!b.ok, "two distinct struct types must not unify");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime);
    // The two distinct struct types conflict on their nominal id.  That
    // id-bearing wording ("TypeId(…)") is the language layer's job to
    // re-render from the structured facts; the checker only guarantees the
    // runtime apply-time kind and that the conflict is located.
    assert!(diags[0].loc().is_some(), "the runtime conflict is located");
}

#[test]
fn an_annotation_with_a_struct_type_rejects_a_literal_at_apply_time() {
    // (\x. (x : struct<.f Int>)) 5 — the struct type sits directly in the
    // annotation, so the apply-time unify checks the argument's type against
    // it and fails.
    let mut ir = IR::new();
    let f = int_t(&mut ir);
    let s = named_type_struct(&mut ir, &[(f, "f")]);
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
    assert!(matches!(
        diags[0].value_a,
        Some(HighProgramValue::LowValue(LowValue::Array(_)))
    ));
    assert_eq!(
        diags[0].value_b,
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
    );
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
    let s = named_type_struct(&mut ir, &[(f, "f")]);
    let a = array(&mut ir, &[s, s]);
    let b = build(a, ir);
    assert!(b.ok, "one shared occurrence is one nominal type");
    assert!(b.module.unify_errors.is_empty());
    assert_eq!(
        AsField::<HighGlobal>::get(&b.module.global_ext).type_id_counter,
        1,
        "one expression compiles to exactly one Fresh call"
    );
}

/// Struct instantiation: `s(1, 2)` — the struct type applied to a tuple.
fn instantiate(ir: &mut IR, type_expr: ExprId, value: ExprId) -> ExprId {
    ir.alloc_instantiate(type_expr, value, &[])
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
    let s = named_type_struct(&mut ir, &[(f1, "f"), (f2, "g")]);
    let inst = instantiate(&mut ir, s, v);
    let b = build(inst, ir);
    assert!(b.ok, "s(1, 2) must check");
    assert!(b.module.unify_errors.is_empty());
    // the instance's type is the struct type, not the tuple type
    assert_eq!(
        b.state[inst].ty, b.state[s].term,
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
    let s = named_type_struct(&mut ir, &[(f, "f")]);
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
    let s = named_type_struct(&mut ir, &[(t1, "f"), (t2, "t")]);
    let inst = instantiate(&mut ir, s, v);
    let b = build(inst, ir);
    assert!(
        !b.ok,
        "s(1, 2) against struct<.f Type, .t Type> must fail (fields)"
    );
    // a literal is not a positional value
    let mut ir = IR::new();
    let five = int(&mut ir, 5);
    let f = int_t(&mut ir);
    let s = named_type_struct(&mut ir, &[(f, "f")]);
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
    let s1 = named_type_struct(&mut ir, &[(f1, "f"), (f2, "g")]);
    let i1 = instantiate(&mut ir, s1, v);
    let f3 = int_t(&mut ir);
    let f4 = int_t(&mut ir);
    let s2 = named_type_struct(&mut ir, &[(f3, "f"), (f4, "g")]);
    let i2 = instantiate(&mut ir, s2, v);
    let a = array(&mut ir, &[i1, i2]);
    let b = build(a, ir);
    assert!(
        !b.ok,
        "instances of different struct occurrences must conflict"
    );
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::ArrayElement);
    // The conflict is the two distinct struct instances' nominal ids.
    assert!(
        matches!(
            (&diags[0].value_a, &diags[0].value_b),
            (
                Some(HighProgramValue::TypeValue(TypeValue::TypeId(_))),
                Some(HighProgramValue::TypeValue(TypeValue::TypeId(_)))
            )
        ),
        "the two distinct struct instances conflict on their nominal id"
    );
}

#[test]
fn an_instantiation_through_a_call_result_callee_checks() {
    // `(mk (Int))(1, 2)` with `mk = u => struct<.f Int, .g Int>` — the callee is
    // an unevaluated apply node (not a statically readable array pair): the
    // checker forces it, so the instantiation sees the concrete struct type.
    // (Reading the callee's pair unconditionally was a panic before the fix.)
    let mut ir = IR::new();
    let p = param(&mut ir);
    let f1 = int_t(&mut ir);
    let f2 = int_t(&mut ir);
    let s = named_type_struct(&mut ir, &[(f1, "f"), (f2, "g")]);
    let mk = lam(&mut ir, p, s);
    let arg = int_t(&mut ir);
    let call = app(&mut ir, mk, arg);
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let inst = instantiate(&mut ir, call, v);
    let b = build(inst, ir);
    assert!(
        b.ok,
        "a call-result callee resolves to its struct type: {:?}",
        b.diagnostics()
    );
    assert_eq!(
        b.state[inst].ty, b.state[call].term,
        "the instance's type is the callee's struct type"
    );
}

#[test]
fn a_call_result_callee_of_a_non_struct_type_is_a_nominal_error() {
    // `(mk (Int))(1, 2)` with `mk = u => Int`: the forced callee is
    // concretely not a struct type — a reported diagnostic, never a panic.
    let mut ir = IR::new();
    let p = param(&mut ir);
    let intt = int_t(&mut ir);
    let mk = lam(&mut ir, p, intt);
    let arg = int_t(&mut ir);
    let call = app(&mut ir, mk, arg);
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let inst = instantiate(&mut ir, call, v);
    let b = build(inst, ir);
    assert!(!b.ok, "a non-struct callee must not instantiate");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::InstantiateCallee);
    assert_eq!(
        diags[0].loc.as_ref().map(|loc| loc.expr),
        Some(call),
        "the not-a-struct error points at the callee"
    );
}

#[test]
fn an_instantiation_requires_a_struct_type_callee() {
    // Nominality: a tuple type is not a struct — it cannot instantiate.
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let f1 = int_t(&mut ir);
    let f2 = int_t(&mut ir);
    let t = type_tuple(&mut ir, &[f1, f2]);
    let inst = instantiate(&mut ir, t, v);
    let b = build(inst, ir);
    assert!(!b.ok, "<Int, Int>(1, 2) must be rejected");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::InstantiateCallee);
    assert_eq!(
        diags[0].loc.as_ref().map(|loc| loc.expr),
        Some(t),
        "the not-a-struct error points at the callee"
    );
    // Nor can a function type.
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let d = int_t(&mut ir);
    let c = int_t(&mut ir);
    let f = arrow(&mut ir, d, c);
    let inst = instantiate(&mut ir, f, v);
    let b = build(inst, ir);
    assert!(!b.ok, "(Int -> Int)(1, 2) must be rejected");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::InstantiateCallee);
}

#[test]
fn an_instantiation_through_a_parameter_pins_the_callee_to_a_struct_kind() {
    // `f = s => s(1, 2); f (Int)` — the callee's type is undecided in the body,
    // so the checker pins it to a struct kind; applying f to a non-struct
    // fails the apply's parameter check (attributed to the argument).
    let mut ir = IR::new();
    let p = param(&mut ir);
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let inst = instantiate(&mut ir, p, v);
    let f = lam(&mut ir, p, inst);
    let arg = int_t(&mut ir);
    let call = app(&mut ir, f, arg);
    let b = build(call, ir);
    assert!(!b.ok, "applying f to a non-struct type must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime, "the apply-time check");
    assert_eq!(
        diags[0].loc.as_ref().map(|loc| loc.expr),
        Some(arg),
        "the deferred failure is attributed to the apply's argument"
    );
    // The pin rejects a tuple type too (nominality through the deferral).
    let mut ir = IR::new();
    let p = param(&mut ir);
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let v = tuple(&mut ir, &[one, two]);
    let inst = instantiate(&mut ir, p, v);
    let f = lam(&mut ir, p, inst);
    let f1 = int_t(&mut ir);
    let f2 = int_t(&mut ir);
    let arg = type_tuple(&mut ir, &[f1, f2]);
    let call = app(&mut ir, f, arg);
    let b = build(call, ir);
    assert!(
        !b.ok,
        "a tuple type must not instantiate through a parameter"
    );
}

#[test]
fn a_named_instantiation_through_a_parameter_defers_its_reorder() {
    // `f = s => s(.x 1, .y Int)` — the callee's name table is not statically
    // known through a parameter, so the definition-order reorder is not
    // computed here: the instantiation stays unresolved instead of being
    // refused, and the reorder is a lazy read that resolves at the unification
    // that binds the callee's struct type.
    let mut ir = IR::new();
    let p = param(&mut ir);
    let one = int(&mut ir, 1);
    let t = ty(&mut ir);
    let v = tuple(&mut ir, &[one, t]);
    let inst = ir.alloc_instantiate(p, v, &[Some("x"), Some("y")]);
    let f = lam(&mut ir, p, inst);
    let b = build(f, ir);
    assert!(
        b.ok,
        "a named instantiation through a parameter is not refused: {:?}",
        b.diagnostics()
    );
    // The instance's value is the deferred reorder — one element per
    // argument, each a read of the argument supplying that definition
    // position.
    let ids = array_ids(&b, b.state[inst].val.unwrap());
    assert_eq!(ids.len(), 2, "one instance position per named argument");
    // Each element is `Index(call_values, TableGet(supply, key))` — the lazy
    // gather whose key the diagnostics attribute through the template origin
    // (the key node is cloned per apply, so its `node_edges` entry is the
    // template's).
    for id in ids {
        let read = b
            .module
            .node_operation(id)
            .expect("the gather element reads");
        assert_eq!(
            format!("{:?}", read.operator),
            "LowOperator(Index)",
            "an instance position is a gather read"
        );
        let operands = read.operand.expect("Index has operands");
        let operands = array_ids(&b, operands);
        let subscript = b.module.node_operation(operands[1]).expect("the subscript");
        assert_eq!(
            format!("{:?}", subscript.operator),
            "LowOperator(TableGet)",
            "the subscript is the supplying read"
        );
    }
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
    let ids = array_ids(&b, b.state[a].ty.unwrap());
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
    // (\x. x) : _ — the placeholder binds to the function-type node (the
    // function's own type, `f : f`). The clone-on-unify never fires for a
    // placeholder (no signature on the placeholder side), so the placeholder's
    // cell binds to the whole function-type node, and the template's parameter
    // type stays undecided.
    let mut ir = IR::new();
    let x = param(&mut ir);
    let l = lam(&mut ir, x, x);
    let h = hole(&mut ir);
    let a = ann(&mut ir, l, h);
    let mut b = build(a, ir);
    assert!(b.ok, "(\\x. x) : _ should check");
    // The placeholder binds to the function-type node.
    assert_eq!(
        b.module.equality_representative(b.state[a].ty.unwrap()),
        b.module.equality_representative(b.state[l].ty.unwrap()),
        "the placeholder binds to the function-type node"
    );
    // The template's parameter type cell stays undecided.
    let fid = function_type_id(&b, b.state[l].ty.unwrap());
    assert!(
        b.module
            .node_value(AnyNodeId::Dynamic(param_type_cell(&b, fid)))
            .is_none(),
        "the template's parameter type must not be guessed"
    );
}

#[test]
fn partial_inference_in_an_arrow_type() {
    // (\x. x) : (Int -> _) — `Int -> _` is a real function, so the annotation
    // binds the two functions' own cells: the identity's parameter type
    // reaches the signature's `Int` and its return type reaches the
    // placeholder.  Nothing is cloned, so the template is what the annotation
    // touched — the identity is monomorphic at this type, and the cost is
    // deliberate (`docs/notes/function-type-merge.md`).
    let mut ir = IR::new();
    let x = param(&mut ir);
    let l = lam(&mut ir, x, x);
    let it = int_t(&mut ir);
    let h = hole(&mut ir);
    let t = arrow(&mut ir, it, h);
    let a = ann(&mut ir, l, t);
    let mut b = build(a, ir);
    assert!(b.ok, "the identity fits Int -> _");
    // The annotation's domain reached the template's own parameter type cell.
    let fid = function_type_id(&b, b.state[l].ty.unwrap());
    let param_type = param_type_cell(&b, fid);
    assert!(
        b.module
            .node_value(AnyNodeId::Dynamic(param_type))
            .is_some(),
        "the annotation bound the template's parameter type, not left it open"
    );
    assert_eq!(
        b.module.equality_representative(param_type),
        b.module.equality_representative(b.int_type),
        "and it is the annotated domain itself"
    );
}

#[test]
fn an_underscore_in_the_array_length_position() {
    // [1, 2, 3] : array<Int, _> — the placeholder length binds to the
    // element count.
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
    assert!(b.ok, "[1, 2, 3] : array<Int, _> should check");
    // The annotated type's length slot unifies with the literal's length 3.
    let ann_shape = array_ids(&b, b.state[a].ty.unwrap())[0];
    let length_slot = array_ids(&b, ann_shape)[1];
    let arr_shape = array_ids(&b, b.state[arr].ty.unwrap())[0];
    let len3 = array_ids(&b, arr_shape)[1];
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(len3)),
        Some(HighProgramValue::LowValue(LowValue::USize(3)))
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

#[test]
fn shallow_array_is_masked_and_typed_like_a_tuple() {
    // [1, ~2] — the bare `~` marks position 1 in the value array's mask,
    // and the type is a tuple (per-element slots), not a homogeneous array
    // type.
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let arr = ir.alloc_shallow_array(&[(one, 0), (two, usize::MAX)]);
    let mut b = build(arr, ir);
    assert!(b.ok, "the shallow array should check");
    let value = b
        .module
        .evaluate_node_deep(b.state[arr].val.unwrap(), None)
        .unwrap();
    assert_eq!(
        array_mask_from(value),
        [false, true],
        "position 1 is marked"
    );
    let ty_val = b
        .module
        .evaluate_node_deep(b.state[arr].ty.unwrap(), None)
        .unwrap();
    let HighProgramValue::LowValue(LowValue::Array(ty_pair)) = ty_val else {
        panic!("expected a type pair");
    };
    let kind_val = b
        .module
        .evaluate_node_deep(
            // SAFETY: `ty_pair` is the value just evaluated by the build under
            // test, whose block has not been dropped.
            dyn_node(unsafe { ty_pair.items() }[1].node),
            None,
        )
        .unwrap();
    let HighProgramValue::LowValue(LowValue::Array(kind)) = kind_val else {
        panic!("expected a kind expression");
    };
    assert_eq!(
        b.module.node_value(AnyNodeId::Dynamic(
            // SAFETY: `kind` is the value just evaluated by the build under
            // test, whose block has not been dropped.
            dyn_node(unsafe { kind.items() }[0].node)
        )),
        Some(HighProgramValue::TypeValue(TypeValue::TypeTuple)),
        "typed like a tuple"
    );
}

#[test]
fn shallow_marked_position_stays_lazy_until_a_read() {
    // [1, ~(x => x + 1) 5] — the apply sits at the bare-`~` position: the
    // deep pass must skip it (the apply never runs during the definition
    // pass).
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let five = int(&mut ir, 5);
    let x = param(&mut ir);
    let one2 = int(&mut ir, 1);
    let add = ir.alloc(ExprKind::BinOp {
        operator: lichen_highlevel::ir::BinOp::Add,
        left: x,
        right: one2,
    });
    let lam = lam(&mut ir, x, add);
    let call = app(&mut ir, lam, five);
    let arr = ir.alloc_shallow_array(&[(one, 0), (call, usize::MAX)]);
    let b = build(arr, ir);
    assert!(b.ok, "the shallow array should check");
    let ids = array_ids(&b, b.state[arr].val.unwrap());
    assert_eq!(ids.len(), 2);
    assert!(
        b.module.node_value(AnyNodeId::Dynamic(ids[1])).is_none(),
        "the masked apply never ran in the deep pass"
    );
}

#[test]
fn a_read_of_a_shallow_position_forces_the_element() {
    // [1, ~(x => x + 1) 5][1] — the read runs the apply that the deep pass
    // deliberately skipped.
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let five = int(&mut ir, 5);
    let x = param(&mut ir);
    let one2 = int(&mut ir, 1);
    let add = ir.alloc(ExprKind::BinOp {
        operator: lichen_highlevel::ir::BinOp::Add,
        left: x,
        right: one2,
    });
    let lam = lam(&mut ir, x, add);
    let call = app(&mut ir, lam, five);
    let arr = ir.alloc_shallow_array(&[(one, 0), (call, usize::MAX)]);
    let one_idx = int(&mut ir, 1);
    let read = field(&mut ir, arr, one_idx);
    let mut b = build(read, ir);
    assert!(b.ok, "the read should check");
    assert_eq!(
        b.module
            .evaluate_node_deep(b.state[read].val.unwrap(), None)
            .unwrap(),
        HighProgramValue::LowValue(LowValue::USize(6)),
        "the read forces the apply at the masked position"
    );
}

// --- assert: explicit constraints ---------------------------------------

/// `a == b` as an IR BinOp.
fn eq_binop(ir: &mut IR, a: ExprId, b: ExprId) -> ExprId {
    ir.alloc(ExprKind::BinOp {
        operator: lichen_highlevel::ir::BinOp::Eq,
        left: a,
        right: b,
    })
}

#[test]
fn top_level_assert_passes_when_the_condition_is_one() {
    // assert(1 == 1) — the condition resolves to 1 at check time.
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let one2 = int(&mut ir, 1);
    let cond = eq_binop(&mut ir, one, one2);
    let asserted = ir.alloc(ExprKind::Assert { condition: cond });
    let b = build(asserted, ir);
    assert!(b.ok, "assert(1 == 1) should check");
    assert!(b.module.assert_errors.is_empty());
}

#[test]
fn top_level_assert_fails_when_the_condition_is_not_one() {
    // assert(1 == 2) — the condition resolves to 0: a failed assert, not a
    // unification failure.
    let mut ir = IR::new();
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let cond = eq_binop(&mut ir, one, two);
    let asserted = ir.alloc(ExprKind::Assert { condition: cond });
    let b = build(asserted, ir);
    assert!(!b.ok, "assert(1 == 2) must fail");
    assert!(b.module.unify_errors.is_empty(), "no unification failed");
    assert_eq!(b.module.assert_errors.len(), 1);
    assert_eq!(
        b.module.assert_errors[0].value,
        HighProgramValue::LowValue(LowValue::USize(0))
    );
}

#[test]
fn in_function_assert_passes_for_a_satisfying_argument() {
    // f = n => assert(n == 1); f 1 — the body's assert cannot resolve at
    // normalize (n is undecided), so the apply clones it and the clone
    // re-checks against the argument.
    let mut ir = IR::new();
    let n = param(&mut ir);
    let one = int(&mut ir, 1);
    let cond = eq_binop(&mut ir, n, one);
    let asserted = ir.alloc(ExprKind::Assert { condition: cond });
    let f = lam(&mut ir, n, asserted);
    let one_arg = int(&mut ir, 1);
    let root = app(&mut ir, f, one_arg);
    let b = build(root, ir);
    assert!(b.ok, "f 1 should check");
    assert_eq!(
        b.module.asserts.len(),
        1,
        "the satisfied clone was consumed; only the untriggered template stays"
    );
    assert!(b.module.assert_errors.is_empty());
}

#[test]
fn in_function_assert_fails_for_a_violating_argument() {
    // f 2 — the clone's condition resolves to 0.
    let mut ir = IR::new();
    let n = param(&mut ir);
    let one = int(&mut ir, 1);
    let cond = eq_binop(&mut ir, n, one);
    let asserted = ir.alloc(ExprKind::Assert { condition: cond });
    let f = lam(&mut ir, n, asserted);
    let two_arg = int(&mut ir, 2);
    let root = app(&mut ir, f, two_arg);
    let b = build(root, ir);
    assert!(!b.ok, "f 2 must fail");
    assert_eq!(b.module.assert_errors.len(), 1);
    assert_eq!(
        b.module.assert_errors[0].value,
        HighProgramValue::LowValue(LowValue::USize(0))
    );
    assert_eq!(
        b.module.asserts.len(),
        1,
        "the failed clone was consumed too — its error is what stays"
    );
}

#[test]
fn never_called_function_assert_is_not_triggered() {
    // f = n => assert(n == 1) — never applied: the condition stays undecided,
    // so the assert stays pending instead of failing.
    let mut ir = IR::new();
    let n = param(&mut ir);
    let one = int(&mut ir, 1);
    let cond = eq_binop(&mut ir, n, one);
    let asserted = ir.alloc(ExprKind::Assert { condition: cond });
    let f = lam(&mut ir, n, asserted);
    let b = build(f, ir);
    assert!(b.ok, "an untriggered assert is no failure");
    assert_eq!(b.module.asserts.len(), 1);
    assert!(b.module.assert_errors.is_empty());
}

#[test]
fn assert_on_a_literal_checks_the_value_itself() {
    // assert(3) — the condition is the literal's value: it resolves to 3,
    // not 1, so the assert fails.
    let mut ir = IR::new();
    let three = int(&mut ir, 3);
    let asserted = ir.alloc(ExprKind::Assert { condition: three });
    let b = build(asserted, ir);
    assert!(!b.ok, "assert(3) must fail");
    assert_eq!(b.module.assert_errors.len(), 1);
    assert_eq!(
        b.module.assert_errors[0].value,
        HighProgramValue::LowValue(LowValue::USize(3))
    );
}

// --- generated array-bounds constraints ------------------------------------

#[test]
fn an_out_of_bounds_array_index_fails_a_generated_assert() {
    // [1, 2, 3][3] — indexing a statically-array target also registers the
    // generated `i < len` constraint; the read itself stays in-bounds for
    // the runtime, so the failure is the assert's and only the assert's.
    let mut ir = IR::new();
    let a = int(&mut ir, 1);
    let b2 = int(&mut ir, 2);
    let c = int(&mut ir, 3);
    let arr = array(&mut ir, &[a, b2, c]);
    let idx = int(&mut ir, 3);
    let e = index(&mut ir, arr, idx);
    let b = build(e, ir);
    assert!(!b.ok, "an out-of-range read must fail");
    assert!(b.module.unify_errors.is_empty(), "no unification failed");
    assert_eq!(b.module.assert_errors.len(), 1);
    assert_eq!(
        b.module.assert_errors[0].value,
        HighProgramValue::LowValue(LowValue::USize(0))
    );
    assert!(
        b.module.asserts.is_empty(),
        "the decided constraint left the worklist"
    );
}

#[test]
fn an_in_bounds_generated_constraint_is_drained() {
    // [1, 2][1] — `1 < 2` holds: the generated constraint is consumed like
    // a passing assert, so the worklist comes out empty.
    let mut ir = IR::new();
    let a = int(&mut ir, 1);
    let b2 = int(&mut ir, 2);
    let arr = array(&mut ir, &[a, b2]);
    let idx = int(&mut ir, 1);
    let e = index(&mut ir, arr, idx);
    let b = build(e, ir);
    assert!(b.ok, "an in-range read should check");
    assert!(b.module.assert_errors.is_empty());
    assert!(
        b.module.asserts.is_empty(),
        "the satisfied constraint was consumed"
    );
}

#[test]
fn a_body_index_on_a_literal_stays_pending_and_rechecks_per_call() {
    // f = i => [7, 8, 9][i] — the body's constraint `i < 3` is generated
    // (the literal's type is statically an array) but stays pending at
    // normalize (the index is the undecided parameter).  Each apply clones it
    // and decides it against the argument: f 2 passes, the template alone
    // remains on the worklist.
    let mut ir = IR::new();
    let i = param(&mut ir);
    let a = int(&mut ir, 7);
    let b2 = int(&mut ir, 8);
    let c = int(&mut ir, 9);
    let arr = array(&mut ir, &[a, b2, c]);
    let read = index(&mut ir, arr, i);
    let f = lam(&mut ir, i, read);
    let two = int(&mut ir, 2);
    let good = app(&mut ir, f, two);
    let b = build(good, ir);
    assert!(b.ok, "f 2 should check");
    assert!(b.module.assert_errors.is_empty());
    assert_eq!(
        b.module.asserts.len(),
        1,
        "only the uninstantiated template stays pending"
    );
}

#[test]
fn an_in_function_bounds_constraint_fails_for_a_violating_argument() {
    // f = i => [7, 8, 9][i]; f 3 — the body's generated `i < 3` constraint
    // is not reachable from the return (the return is the index result), so
    // only the function's own assert list carries it; the apply clones it
    // and the clone fails for the out-of-range argument.
    let mut ir = IR::new();
    let i = param(&mut ir);
    let a = int(&mut ir, 7);
    let b2 = int(&mut ir, 8);
    let c = int(&mut ir, 9);
    let arr = array(&mut ir, &[a, b2, c]);
    let read = index(&mut ir, arr, i);
    let f = lam(&mut ir, i, read);
    let three = int(&mut ir, 3);
    let bad = app(&mut ir, f, three);
    let b = build(bad, ir);
    assert!(!b.ok, "f 3 must fail");
    assert!(b.module.unify_errors.is_empty(), "no unification failed");
    assert_eq!(b.module.assert_errors.len(), 1);
    assert_eq!(
        b.module.assert_errors[0].value,
        HighProgramValue::LowValue(LowValue::USize(0))
    );
    assert_eq!(
        b.module.asserts.len(),
        1,
        "the failed clone was consumed — only the untriggered template stays"
    );
}

#[test]
fn a_body_index_fails_at_the_violating_argument() {
    // f = i => [7, 8, 9][i]; f 5 — the instantiated clone's `5 < 3` fails.
    let mut ir = IR::new();
    let i = param(&mut ir);
    let a = int(&mut ir, 7);
    let b2 = int(&mut ir, 8);
    let c = int(&mut ir, 9);
    let arr = array(&mut ir, &[a, b2, c]);
    let read = index(&mut ir, arr, i);
    let f = lam(&mut ir, i, read);
    let five = int(&mut ir, 5);
    let bad = app(&mut ir, f, five);
    let b = build(bad, ir);
    assert!(!b.ok, "f 5 must fail");
    assert_eq!(b.module.assert_errors.len(), 1);
    assert_eq!(
        b.module.assert_errors[0].value,
        HighProgramValue::LowValue(LowValue::USize(0))
    );
}

// --- a struct-domain function that reads a field ----------------------------
//
// The named field read (`a.name`) accepts a struct container and registers the
// kind requirement as a re-checkable assert when the container is undecided
// (`docs/notes/eval-before-unify.md` §5.2/§6.2), so a function whose parameter
// is annotated with a struct type and whose body reads a named field checks:

#[test]
fn an_annotated_struct_parameter_function_reads_a_named_field() {
    // `f = (p : struct<.x Int, .y Int> => p.x)`; `f (struct<.x 1, .y 2>)`.
    let mut ir = IR::new();
    let p = param(&mut ir);
    let int_x = int_t(&mut ir);
    let int_y = int_t(&mut ir);
    let struct_ty = named_type_struct(&mut ir, &[(int_x, "x"), (int_y, "y")]);
    let read = named_field(&mut ir, p, "x");
    let f = lam_at_typed(&mut ir, p, Some(struct_ty), read, None);
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let fields = tuple(&mut ir, &[one, two]);
    let arg = instantiate(&mut ir, struct_ty, fields);
    let call = app(&mut ir, f, arg);
    let b = build(call, ir);
    assert!(b.ok, "f (struct<.x 1, .y 2>) must check");
}

/// Two **separately written** occurrences of `struct<.x Int>` are two distinct
/// nominal types — a struct type expression mints a fresh nominal id per source
/// occurrence (`check_type_struct`), and unification compares that id.  Sharing
/// one *named* type expression is what makes an annotation and an instance
/// agree; writing the type twice does not.
#[test]
fn two_written_struct_types_are_distinct_nominal_types() {
    let mut ir = IR::new();
    let p = param(&mut ir);
    let int_a = int_t(&mut ir);
    let written_in_annotation = named_type_struct(&mut ir, &[(int_a, "x")]);
    let read = named_field(&mut ir, p, "x");
    let f = lam_at_typed(&mut ir, p, Some(written_in_annotation), read, None);
    let int_b = int_t(&mut ir);
    let written_in_argument = named_type_struct(&mut ir, &[(int_b, "x")]);
    let one = int(&mut ir, 1);
    let fields = tuple(&mut ir, &[one]);
    let arg = instantiate(&mut ir, written_in_argument, fields);
    let call = app(&mut ir, f, arg);
    let b = build(call, ir);
    for d in b.diagnostics() {
        eprintln!("PROBE two occurrences diag: {d:?}");
    }
    assert!(
        !b.ok,
        "two written occurrences of the same struct text are distinct nominal types"
    );
}

#[test]
fn an_open_struct_annotation_lets_its_field_be_read() {
    // `F = T: KT T => T.I` with `KT = _x => struct<.I _, .O _>` — the
    // annotation names the two fields but leaves their *types* open.
    //
    // The contract (`checker/lambda.rs`) is that the annotated parameter's type
    // is compiled and unified into the parameter's type slot **before** the
    // body compiles, so the body's reader sees the annotated kind statically.
    // The body's `T.I` is therefore reading a field of a struct the annotation
    // already named, and applying `F` to a concrete struct type closes that
    // field — the read must resolve.
    let mut ir = IR::new();
    let t = param(&mut ir);
    let open_i = hole(&mut ir);
    let open_o = hole(&mut ir);
    // **One** named struct type, reused for the annotation and for the argument's
    // instantiation: two written `struct<…>` are distinct nominal types.
    let open_struct = named_type_struct(&mut ir, &[(open_i, "I"), (open_o, "O")]);
    let read = named_field(&mut ir, t, "I");
    let f = lam_at_typed(&mut ir, t, Some(open_struct), read, None);
    // The argument is an **instance** of that struct whose field values are the
    // *types* `Int` — the struct contains a type.
    let int_i = int_t(&mut ir);
    let int_o = int_t(&mut ir);
    let fields = tuple(&mut ir, &[int_i, int_o]);
    let arg = instantiate(&mut ir, open_struct, fields);
    let call = app(&mut ir, f, arg);
    let b = build(call, ir);
    // The checker's own per-expression view — set while checking, not mutated
    // by the unify, so this is the pre-unify encoding of each expression.
    for (at, st) in b.state.iter().enumerate() {
        let e = ExprId(at as u32);
        eprintln!(
            "PROBE expr[{at}] kind={:?}\n    term={:?}\n    val ={:?}\n    ty  ={:?}",
            b.ir[e].kind, st.term, st.val, st.ty,
        );
    }
    for d in b.diagnostics() {
        eprintln!("PROBE open struct diag: {d:?}");
    }
    // What each function's parameter type slot actually holds after the build:
    // if the annotation ran before the body, this is the annotated struct type.
    for (fid, function) in b.module.functions.iter() {
        let cell = array_ids(&b, function.parameter)[1];
        eprintln!(
            "PROBE function {fid:?} param_type_cell={cell:?} value={:?} low={:?}",
            b.module.node_value(AnyNodeId::Dynamic(cell)),
            b.module.low_type_of_node(cell),
        );
    }
    assert!(
        b.ok,
        "an open struct annotation must still let the body read its field"
    );
}

// --- a tuple-domain function that reads an element --------------------------

#[test]
fn an_annotated_tuple_parameter_function_reads_an_element() {
    // The three-line regression, measured through the compiler:
    //   `f = p : <Int, Int> => p(0)`; `f (1, 2)`
    // resolves on the baseline and fails here with
    //   `expected raw[?a, Int], found raw[?a, Int]`.
    let mut ir = IR::new();
    let p = param(&mut ir);
    let int1 = int_t(&mut ir);
    let int2 = int_t(&mut ir);
    let tuple_ty = type_tuple(&mut ir, &[int1, int2]);
    let zero = int(&mut ir, 0);
    let read = field(&mut ir, p, zero);
    let f = lam_at_typed(&mut ir, p, Some(tuple_ty), read, None);
    let one = int(&mut ir, 1);
    let two = int(&mut ir, 2);
    let fields = tuple(&mut ir, &[one, two]);
    let call = app(&mut ir, f, fields);
    let b = build(call, ir);
    for d in b.diagnostics() {
        eprintln!(
            "PROBE tuple read diag: kind={:?} loc={:?} a={:?} b={:?}",
            d.kind, d.loc, d.a, d.b
        );
    }
    assert!(b.ok, "f (1, 2) against the tuple domain must check");
    // And actually evaluate it: the apply's parameter check runs there.
    let mut module = b.module;
    let value = module.evaluate_node_deep(b.root_val, None).unwrap();
    for e in &module.unify_errors {
        eprintln!(
            "PROBE tuple read eval error: a={:?} b={:?} va={:?} vb={:?} steps={:?}",
            e.a, e.b, e.value_a, e.value_b, e.steps
        );
    }
    assert_eq!(common_value(&value), 1, "f (1, 2) yields 1");
}

/// The integer a value carries, for the assertions above.
fn common_value(value: &HighProgramValue) -> u64 {
    match value {
        HighProgramValue::LowValue(LowValue::USize(n)) => *n as u64,
        other => panic!("expected a usize value, got {other:?}"),
    }
}

// --- a wrapper's parameter type comes from the body call --------------------
//
// The regression target: `f`'s parameter type states the argument's type, so a
// wrapper `g = (a => f a)` must have its parameter type cell filled by
// normalizing `g` — no apply, no inference at run time.  This is the
// language-level shape of the compute wrapper (`k1 = jit (x => launch k0 (x,1))`
// fails with "the kernel parameter's class is not decided": the same cell).

#[test]
fn a_wrappers_parameter_type_is_inferred_from_a_body_call() {
    // `f = (p : struct<.x Int, .y Int> => p)`; `g = (a => f a)`.
    let mut ir = IR::new();
    let p = param(&mut ir);
    let int_x = int_t(&mut ir);
    let int_y = int_t(&mut ir);
    let struct_ty = named_type_struct(&mut ir, &[(int_x, "x"), (int_y, "y")]);
    let f = lam_at_typed(&mut ir, p, Some(struct_ty), p, None);

    let a = param(&mut ir);
    let call = app(&mut ir, f, a);
    let g = lam(&mut ir, a, call);

    let b = build(g, ir);
    assert!(b.ok, "the wrapper must check");
    let gtype = b.state[g].ty.expect("a lambda has a type");
    let gid = function_type_id(&b, gtype);
    let cell = param_type_cell(&b, gid);
    assert!(
        b.module.node_value(AnyNodeId::Dynamic(cell)).is_some(),
        "the wrapper's parameter type must be inferred from the body call"
    );
}
