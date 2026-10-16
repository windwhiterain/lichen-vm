//! The runtime is the typechecker; see docs/notes/lowlevel-vm.md and
//! docs/language-spec.md.

use lichen_highlevel::NoAttr;
use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{BinOp, ExprId, ExprKind};
use lichen_highlevel::program::{HighProgramValue, ProgramImpl, TypeValue};
use lichen_lowlevel::{AnyFunctionId, AnyNodeId, ArrayItem, FunctionId, LowValue, NodeId};
use lichen_testkit::{Writer, dyn_node};

/// Check the program `w` built, rooted at `root`.
///
/// # Invariant
/// A refusing build is still returned: the refusal is what most of these tests
/// assert, so the caller reads `ok` and the diagnostics itself.
fn build(root: ExprId, w: Writer<NoAttr>) -> Build<ProgramImpl> {
    Checker::build(w.finish(root).ir)
}

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

/// The `FunctionId` a function-type node `[Function(fid), ↺]` carries (`f : f`).
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

/// The function template's parameter type cell — `Function::parameter` slot 1.
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

/// Whether the given ids form the int type — a `[int, Type]` pair.
///
/// # Invariant
/// The two slots are matched differently on purpose: element 0 is the marker,
/// matched by *content*, because a literal rebuilds its type per occurrence and
/// so is never the shared `Build::int_type` node; element 1 is the universe,
/// compared by node identity against the canonical `b.type_expr`.
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

/// Whether an evaluated value is the int type pair `[int, Type]`.
fn is_int_type_value(
    b: &lichen_highlevel::checker::Build<ProgramImpl>,
    value: HighProgramValue,
) -> bool {
    is_int_type_ids(b, &array_ids_from(value))
}

// --- checking -------------------------------------------------------------

#[test]
fn int_literal_checks() {
    let w = Writer::new();
    let five = w.int("five", 5);
    let b = build(five, w);
    assert!(b.ok, "5 should check");
    // The type of 5 is [int, [Type, ↺]], rebuilt fresh per occurrence.
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
    let w = Writer::new();
    let five = w.int("five", 5);
    let t = w.int_type("t");
    let a = w.annotate("a", five, t);
    let b = build(a, w);
    assert!(b.ok, "5 : int should check");
    assert!(is_int_type(&b, b.state[a].ty.unwrap()));
}

#[test]
fn the_type_universe_is_self_referential() {
    // `Type` compiles to the canonical node K = [Type, K] — Type : Type via
    // a self-cycle, closing every type spine.
    let w = Writer::new();
    let t = w.universe("t");
    let b = build(t, w);
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
    // A recovered-error region is a pair of fresh cells: it carries no
    // grammar, so nothing inside it is checked.
    let w = Writer::new();
    let e = w.error_block("e");
    let b = build(e, w);
    assert!(b.ok, "a masked error region is not a check failure");
    // The skip path records the pair's slots, so downstream reads still work.
    assert!(b.state[e].term.is_some(), "the skip path records the pair");
    assert!(
        b.state[e].val.is_some(),
        "the skip path records the value slot"
    );
    assert!(
        b.state[e].ty.is_some(),
        "the skip path records the type slot"
    );
    // The two slots are fresh, undecided cells, never unified by the context.
    let ty_cell = b.state[e].ty.unwrap();
    assert!(
        b.module.node_value(AnyNodeId::Dynamic(ty_cell)).is_none(),
        "the type cell is a fresh, undecided cell"
    );
}

#[test]
fn an_error_block_as_a_child_does_not_cascade() {
    // An error block in real code must not poison it: its fresh type cell
    // binds quietly to the int sibling.
    let w = Writer::new();
    let five = w.int("five", 5);
    let e = w.error_block("e");
    let b = build(w.array("array", &[e, five]), w);
    assert!(b.ok, "the error block binds without a conflict");

    // A heterogeneous tuple does not unify elements at all; the error block
    // rides along and the whole thing checks.
    let w = Writer::new();
    let five = w.int("five", 5);
    let e = w.error_block("e");
    let b = build(w.tuple("tuple", &[e, five]), w);
    assert!(b.ok, "a tuple with an error block checks");
    assert!(
        b.state[e].term.is_some(),
        "the embedded error block still records its pair"
    );
}

#[test]
fn literal_against_type_fails() {
    // 5 : Type
    let w = Writer::new();
    let five = w.int("five", 5);
    let t = w.universe("t");
    let a = w.annotate("a", five, t);
    let b = build(a, w);
    assert!(!b.ok, "5 : Type must fail");
    assert!(!b.module.unify_errors.is_empty());
}

#[test]
fn lambda_has_arrow_type() {
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let b = build(l, w);
    assert!(b.ok, "\\x. x should check");
    // The lambda's type is the function itself: a self-referential
    // `[Function(fid), ↺]`, slot 1 being the node itself.
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
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let d = w.int_type("d");
    let c = w.int_type("c");
    let t = w.type_function("t", d, c);
    let a = w.annotate("a", l, t);
    let b = build(a, w);
    assert!(b.ok, "(\\x. x) : int → int should check");
}

#[test]
fn annotating_a_lambda_with_a_tuple_type_fails() {
    // (\x. x) : [int, int] — a tuple type, not a function type: the kind
    // markers now distinguish the two, so this must fail.
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let d = w.int_type("d");
    let c = w.int_type("c");
    let t = w.type_tuple("t", &[d, c]);
    let a = w.annotate("a", l, t);
    let b = build(a, w);
    assert!(
        !b.ok,
        "(\\x. x) : [int, int] must fail (a lambda is not a tuple)"
    );
}

#[test]
fn typed_tuple_is_a_kinded_tuple() {
    // [1, 2] — the pair [[1, 2], [[int, int], [TupleType, Type]]].
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let tup = w.tuple("tup", &[e1, e2]);
    let b = build(tup, w);
    assert!(b.ok, "[1, 2] should check");
    let ty = b.state[tup].ty.unwrap();
    let ids = array_ids(&b, ty);
    assert_eq!(ids.len(), 2, "a type expression is a pair [shape, kind]");
    let shape_ids = array_ids(&b, ids[0]);
    assert_eq!(shape_ids.len(), 2);
    // Each shape element is an element slot `[value, type, attrs…]`, so the
    // element's type is its slot's type position.
    for slot in shape_ids {
        let slot_ids = array_ids(&b, slot);
        assert_eq!(slot_ids.len(), 3, "the value, the type, one attribute");
        assert!(is_int_type(&b, slot_ids[1]));
        assert!(matches!(
            b.module.node_value(AnyNodeId::Dynamic(slot_ids[0])),
            Some(HighProgramValue::LowValue(LowValue::Any))
        ));
    }
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
    // Array(int, 3) is [[int, 3], [ArrayType, Type]]: the element type and the
    // length.
    let w = Writer::new();
    let t = w.int_type("t");
    let n = w.int("n", 3);
    let arr = w.type_array("arr", t, n);
    let b = build(arr, w);
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
    // The value is the instance [element slot, length].
    let shape = b.state[arr].val.unwrap();
    let shape_ids = array_ids(&b, shape);
    assert_eq!(shape_ids.len(), 2);
    // The element is a slot, so its type is the slot's type position.
    let element_slot = array_ids(&b, shape_ids[0]);
    assert!(
        is_int_type(&b, element_slot[1]),
        "instance[0] is the element slot, whose type is Int"
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
    let w = Writer::new();
    let t = w.int_type("t");
    let n = w.int("n", 3);
    let arr = w.type_array("arr", t, n);
    let type_val = w.universe("type_val");
    let a = w.annotate("a", arr, type_val);
    let b = build(a, w);
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
    // (\x. x) : Array(int, 3) — a function's type is not an array type.
    let w = Writer::new();
    let x = w.param("x");
    let l = w.lam("l", x, x);
    let t = w.int_type("t");
    let n = w.int("n", 3);
    let arr_ty = w.type_array("arr_ty", t, n);
    let a = w.annotate("a", l, arr_ty);
    let b = build(a, w);
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
    // [1, 2] — one type slot shared by all elements, the length the element
    // count.
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let arr = w.array("arr", &[e1, e2]);
    let b = build(arr, w);
    assert!(b.ok, "[1, 2] as an array literal should check");
    let ty = b.state[arr].ty.unwrap();
    let ids = array_ids(&b, ty);
    assert_eq!(ids.len(), 2, "a type expression is a pair [shape, kind]");
    let shape_ids = array_ids(&b, ids[0]);
    assert_eq!(shape_ids.len(), 2);
    // The element position is a slot: the shared element type unifies to int.
    let element_slot = array_ids(&b, shape_ids[0]);
    assert!(
        is_int_type(&b, element_slot[1]),
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
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let arr = w.array("arr", &[e1, e2]);
    let t = w.int_type("t");
    let n = w.int("n", 2);
    let arr_ty = w.type_array("arr_ty", t, n);
    let a = w.annotate("a", arr, arr_ty);
    let b = build(a, w);
    assert!(b.ok, "[1, 2] : int[2] should check");
    assert!(b.module.unify_errors.is_empty());
}

#[test]
fn array_literal_length_mismatch_fails() {
    // [1, 2] : Array(int, 3)
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let arr = w.array("arr", &[e1, e2]);
    let t = w.int_type("t");
    let n = w.int("n", 3);
    let arr_ty = w.type_array("arr_ty", t, n);
    let a = w.annotate("a", arr, arr_ty);
    let b = build(a, w);
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
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int_type("e2"); // `int` as a value — its type is Type
    let arr = w.array("arr", &[e1, e2]);
    let b = build(arr, w);
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
    // `let` is desugared by the frontend into an applied binding; polymorphism
    // is the apply's fresh clones.
    let w = Writer::new();
    let x = w.param("x");
    let id = w.lam("id", x, x);
    let b1 = w.param("b1");
    let five = w.int("five", 5);
    let call1 = w.app("call1", b1, five);
    let t1 = w.int_type("t1");
    let a = w.annotate("a", call1, t1);
    let b2 = w.param("b2");
    let type_val = w.universe("type_val");
    let call2 = w.app("call2", b1, type_val); // a second use of id
    let t2 = w.universe("t2");
    let body2 = w.annotate("body2", call2, t2);
    let (outer_lam, inner_lam) = w.lam_nested(("inner_lam", b2, body2), ("outer_lam", b1));
    let inner = w.app("inner", inner_lam, a);
    // The enclosing lambda's return is the applied inner one, so its kind is
    // re-stamped.
    w.fill(
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
    let whole = w.app("whole", outer_lam, id);
    let b = build(whole, w);
    assert!(
        b.ok,
        "two uses of id at different types must both check (polymorphism via the apply's fresh clones)"
    );
}

#[test]
fn applying_a_non_function_fails() {
    let w = Writer::new();
    let five = w.int("five", 5);
    let six = w.int("six", 6);
    let a = w.app("a", five, six);
    let b = build(a, w);
    assert!(!b.ok, "applying an int must fail (the function-ness guard)");
    assert!(!b.module.unify_errors.is_empty());
}

#[test]
fn tuple_typechecks_elementwise() {
    // [1, 2] : [int, int]
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let tup = w.tuple("tup", &[e1, e2]);
    let d = w.int_type("d");
    let c = w.int_type("c");
    let t = w.type_tuple("t", &[d, c]);
    let a = w.annotate("a", tup, t);
    let b = build(a, w);
    assert!(b.ok, "[1, 2] : [int, int] should check");
}

#[test]
fn tuple_length_mismatch_fails() {
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let tup = w.tuple("tup", &[e1, e2]);
    let d = w.int_type("d");
    let t = w.type_tuple("t", &[d]);
    let a = w.annotate("a", tup, t);
    let b = build(a, w);
    assert!(!b.ok, "[1, 2] : [int] must fail (length mismatch)");
}

#[test]
fn types_are_first_class() {
    // let T = int in \x. (x : T) → (\T. \x. (x : T)) int
    let w = Writer::new();
    let bt = w.param("bt");
    let tval = w.int_type("tval");
    let bx = w.param("bx");
    let body = w.annotate("body", bx, bt); // (x : T) — both uses are parameter ids
    let (t_lam, _inner) = w.lam_nested(("inner_lam", bx, body), ("outer_lam", bt));
    let whole = w.app("whole", t_lam, tval);
    let b = build(whole, w);
    assert!(b.ok, "(\\T. \\x. (x : T)) int should check");
}

// --- check-then-run round-trips -------------------------------------------

#[test]
fn built_program_runs_to_a_value() {
    // (\id. id 5) (\x. x)  ==  5
    let w = Writer::new();
    let x = w.param("x");
    let id = w.lam("id", x, x);
    let b1 = w.param("b1");
    let five = w.int("five", 5);
    let call = w.app("call", b1, five);
    let outer = w.lam("outer", b1, call);
    let whole = w.app("whole", outer, id);
    let b = build(whole, w);
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
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let five = w.int("five", 5);
    let call = w.app("call", l, five);
    let b = build(call, w);
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
    let w = Writer::new();
    let x = w.param("x");
    let id = w.lam("id", x, x);
    let b1 = w.param("b1");
    let five = w.int("five", 5);
    let inner = w.app("inner", b1, five);
    let call = w.app("call", b1, inner); // the same parameter id, nested
    let outer = w.lam("outer", b1, call);
    let whole = w.app("whole", outer, id);
    let b = build(whole, w);
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
    let w = Writer::new();
    let five = w.int("five", 5);
    let t = w.universe("t");
    let a = w.annotate("a", five, t);
    let b = build(a, w);
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
    let w = Writer::new();
    let five = w.int("five", 5);
    let six = w.int("six", 6);
    let call = w.app("call", five, six);
    let b = build(call, w);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Guard);
    assert_eq!(diags[0].loc().map(|loc| loc.expr), Some(call));
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
    );
    // The expected side is the function the guard built, not an arrow term:
    // it clashes with the callee's `int` marker.
    assert!(matches!(
        diags[0].value_b,
        Some(HighProgramValue::LowValue(LowValue::Function(_)))
    ));
}

#[test]
fn indexing_a_function_reports_expected_tuple_or_array() {
    // (\x. x)[0] — the index-target guard: a function type is not indexable.
    let w = Writer::new();
    let x = w.param("x");
    let l = w.lam("l", x, x);
    let zero = w.int("zero", 0);
    let idx = w.index("idx", l, zero);
    let b = build(idx, w);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Guard);
    assert_eq!(diags[0].loc().map(|loc| loc.expr), Some(l));
    // The requirement names the container's **kind**: the expected side is the
    // array kind, not the container's own type.
    let requirement = array_ids(&b, diags[0].b);
    assert_eq!(requirement.len(), 2, "a kind is a [marker, K] pair");
    assert!(
        matches!(
            b.module.node_value(AnyNodeId::Dynamic(requirement[0])),
            Some(HighProgramValue::TypeValue(TypeValue::TypeArray))
        ),
        "the expected side is the array kind"
    );
}

#[test]
fn indexing_an_int_reports_expected_tuple_or_array() {
    // 5[0] — an atomic type is not indexable either.
    let w = Writer::new();
    let five = w.int("five", 5);
    let zero = w.int("zero", 0);
    let idx = w.index("idx", five, zero);
    let b = build(idx, w);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Guard);
}

#[test]
fn runtime_apply_mismatch_is_attributed_to_the_argument() {
    // (\x. (x : Type)) 5 — the parameter's type is Type, the argument's int.
    let w = Writer::new();
    let x = w.param("x");
    let t = w.universe("t");
    let body = w.annotate("body", x, t);
    let l = w.lam("l", x, body);
    let five = w.int("five", 5);
    let call = w.app("call", l, five);
    let b = build(call, w);
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
    // (\x. x) : tuple{Type, Int} — a function's type is not a tuple type.
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let t1 = w.universe("t1");
    let t2 = w.int_type("t2");
    let t = w.type_tuple("t", &[t1, t2]);
    let a = w.annotate("a", l, t);
    let b = build(a, w);
    assert!(!b.ok, "a function is not a tuple type");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
}

#[test]
fn an_unannotated_lambda_has_an_undecided_arrow_type() {
    // (\x. x) : Type — a function's type is not Type, so the annotation
    // conflicts.
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let t = w.universe("t");
    let a = w.annotate("a", l, t);
    let b = build(a, w);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    // The found side is the function's leaf: the arm compares functions by their cells.
    assert!(matches!(
        diags[0].value_a,
        Some(HighProgramValue::LowValue(LowValue::Function(_)))
    ));
}

#[test]
fn an_unannotated_call_syncs_its_root_type_to_the_return_type() {
    // (\id. id 5) (\x. x) — the call's result cell is a lazy record, synced
    // to the return pair by the apply's evaluation.
    let w = Writer::new();
    let x = w.param("x");
    let id = w.lam("id", x, x);
    let b1 = w.param("b1");
    let five = w.int("five", 5);
    let call = w.app("call", b1, five);
    let outer = w.lam("outer", b1, call);
    let whole = w.app("whole", outer, id);
    let b = build(whole, w);
    assert!(b.ok);
    assert!(b.diagnostics().is_empty());
    assert!(
        is_int_type(&b, b.root_ty),
        "the root type is synced to the return type (int)"
    );
}

#[test]
fn a_tuples_undecided_element_types_sync_from_the_return_types() {
    // a = \x. (1, Int)[x]; (a 0, a 1) — the element types are the calls'
    // lazy result cells, synced with their return pairs.
    let w = Writer::new();
    let x = w.param("x");
    let one = w.int("one", 1);
    let tval = w.universe("tval");
    let tup = w.tuple("tup", &[one, tval]);
    let idx = w.field("idx", tup, x);
    let a = w.lam("a", x, idx);
    let zero = w.int("zero", 0);
    let c0 = w.app("c0", a, zero);
    let one1 = w.int("one1", 1);
    let c1 = w.app("c1", a, one1);
    let whole = w.tuple("whole", &[c0, c1]);
    let b = build(whole, w);
    assert!(b.ok);
    assert!(b.diagnostics().is_empty());
    // The resolved root type is the tuple type `tuple{Int, Type}`; each element's
    // type is its slot's type position.
    let pair = array_ids(&b, b.root_ty);
    assert_eq!(pair.len(), 2);
    let shape = array_ids(&b, pair[0]);
    let e0 = array_ids(&b, shape[0]);
    assert!(is_int_type(&b, e0[1]), "element 0's cell syncs to int");
    let e1 = array_ids(&b, shape[1]);
    assert_eq!(
        b.module.node_value(AnyNodeId::Dynamic(e1[1])),
        b.module.node_value(AnyNodeId::Dynamic(b.type_expr)),
        "element 1's cell syncs to Type's"
    );
}

#[test]
fn tuple_length_mismatch_reports_both_sides() {
    // [1, 2] : [int]
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let tup = w.tuple("tup", &[e1, e2]);
    let d = w.int_type("d");
    let t = w.type_tuple("t", &[d]);
    let a = w.annotate("a", tup, t);
    let b = build(a, w);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    // The length mismatch shows in the shapes: two element types against one.
    assert_eq!(array_ids(&b, array_ids(&b, diags[0].a)[0]).len(), 2);
    assert_eq!(array_ids(&b, array_ids(&b, diags[0].b)[0]).len(), 1);
}

// --- call result annotations are checked, not just bound --------------------

// A call's result type cell is a lazy record, so an annotation on a call
// result only *binds* the cell at check time.

// The apply's evaluation later syncs the cell with the callee's return pair,
// and a disagreement is a reported failure.

#[test]
fn a_call_result_annotation_is_checked_against_the_return_type() {
    // (\f. (f 5 : int)) (\x. Type) — f returns Type, so the annotation's int
    // is refused when the apply syncs the cell.
    let w = Writer::new();
    let x = w.param("x");
    let tval = w.universe("tval");
    let f = w.lam("f", x, tval);
    let b1 = w.param("b1");
    let five = w.int("five", 5);
    let call = w.app("call", b1, five);
    let want = w.int_type("want");
    let a = w.annotate("a", call, want);
    let f_lam = w.lam("f_lam", b1, a);
    let whole = w.app("whole", f_lam, f);
    let b = build(whole, w);
    assert!(!b.ok, "(f 5 : int) must fail: the annotation is checked");
    assert!(!b.module.unify_errors.is_empty());
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime);
}

#[test]
fn a_direct_call_result_annotation_is_checked_against_the_return_type() {
    // (\x. x) 5 : Type — the identity returns int, so the annotation is
    // refused when the apply syncs the result cell.
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let five = w.int("five", 5);
    let call = w.app("call", l, five);
    let want = w.universe("want");
    let a = w.annotate("a", call, want);
    let b = build(a, w);
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
    // A closure capturing its enclosing parameter: `f1 3 4` yields
    // [1, 2, 3, 4], not the leaked template.
    let w = Writer::new();
    let a = w.int("a", 1);
    let b = w.int("b", 2);
    let x = w.param("x");
    let y = w.param("y");
    let f2_body = w.array("f2_body", &[a, b, x, y]);
    let (f1, _f2) = w.lam_nested(("inner_lam", y, f2_body), ("outer_lam", x));
    let three = w.int("three", 3);
    let four = w.int("four", 4);
    let call1 = w.app("call1", f1, three);
    let whole = w.app("whole", call1, four);
    let mut b = build(whole, w);
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

/// The `FunctionId` this lambda expression compiled to.
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
    // The nesting half of the parent rule: a body closure joins the enclosing
    // template and clones with it.
    let w = Writer::new();
    let a = w.int("a", 1);
    let x = w.param("x");
    let y = w.param("y");
    let (outer, inner) = w.lam_nested(("inner_lam", y, a), ("outer_lam", x));
    let b = build(outer, w);
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
    // The sibling half: two same-level lambdas hang under nothing, so neither
    // template absorbs the other.
    let w = Writer::new();
    let x = w.param("x");
    let sibling = w.lam("sibling", x, x);
    let other = w.lam("other", x, sibling);
    let b = build(other, w);
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
    // (1, 2)[0] — the tuple's element-type list is structural, a plain Index.
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let tup = w.tuple("tup", &[one, two]);
    let zero = w.int("zero", 0);
    let idx = w.field("idx", tup, zero);
    let mut b = build(idx, w);
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
    // [1, 2, 3][1] — the shape holds the length as data, so the type
    // evaluation dispatches on the kind.
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let e3 = w.int("e3", 3);
    let arr = w.array("arr", &[e1, e2, e3]);
    let one = w.int("one", 1);
    let idx = w.index("idx", arr, one);
    let mut b = build(idx, w);
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
    // (1, 2)[5] — both evaluations hit the bounds check, collapsing to one
    // diagnostic.
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let tup = w.tuple("tup", &[one, two]);
    let five = w.int("five", 5);
    let idx = w.field("idx", tup, five);
    let b = build(idx, w);
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
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let e3 = w.int("e3", 3);
    let arr = w.array("arr", &[e1, e2, e3]);
    let five = w.int("five", 5);
    let idx = w.index("idx", arr, five);
    let b = build(idx, w);
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
    // (\x. x[3])([1, 2]) — the type is known only once the apply binds it.
    let w = Writer::new();
    let x = w.param("x");
    let three = w.int("three", 3);
    let body = w.index("body", x, three);
    let l = w.lam("l", x, body);
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let arr = w.array("arr", &[e1, e2]);
    let whole = w.app("whole", l, arr);
    let b = build(whole, w);
    assert!(!b.ok, "(\\x. x[3])([1, 2]) must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexOutOfBounds);
    assert_eq!(diags[0].index, Some(3));
    assert_eq!(diags[0].length, Some(2));
}

// --- struct types ----------------------------------------------------------

// A struct type is [[field types], [marker, Type]] with a nominal id.

// The payload is [TypeId(n), names, names_in_order]; see `lichen_highlevel::shape`.

#[test]
fn struct_type_has_a_kind_and_carries_a_fresh_type_id() {
    // struct{.f Int, .t Type} — the pair [[int, Type], [[TypeId(0), names,
    // names_in_order], TypeStruct]].
    let w = Writer::new();
    let t1 = w.int_type("t1");
    let t2 = w.universe("t2");
    let s = w.struct_type("s", &[(t1, Some("f")), (t2, Some("t"))]);
    let mut b = build(s, w);
    assert!(b.ok, "struct{{.f Int, .t Type}} should kind");
    assert!(b.module.unify_errors.is_empty());
    // the shape is the positional field-**slot** list [slot(Int), slot(Type)] —
    // the nominal id does not ride in the shape
    let struct_shape = b.state[s].val.unwrap();
    let shape_ids = array_ids(&b, struct_shape);
    assert_eq!(shape_ids.len(), 2);
    // A field's type is its slot's type position.
    let f_slot = array_ids(&b, shape_ids[0]);
    assert!(is_int_type(&b, f_slot[1]), "the `.f` field's type is Int");
    let t_slot = array_ids(&b, shape_ids[1]);
    assert_eq!(
        t_slot[1], b.type_expr,
        "the `.t` field's type is the universe"
    );
    // the kind slot is a standard [marker, K] pair; see `lichen_highlevel::shape`.
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
    assert!(
        matches!(
            b.module.node_value(AnyNodeId::Dynamic(payload_ids[0])),
            Some(HighProgramValue::TypeValue(TypeValue::TypeId(n))) if n == s.0 as usize
        ),
        "the id is the written occurrence itself"
    );
    // a named struct's payload carries a name table in its names slot.
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(payload_ids[1])),
        Some(HighProgramValue::LowValue(LowValue::Table(_)))
    ));
    // The tag is what makes it a struct marker: a same-payload pair whose
    // type slot is the universe is not one.
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
    // The identity is unaffected by the missing tag: it is the occurrence.
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(payload_ids[0])),
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(n))) if n == s.0 as usize
    ));
}

#[test]
fn a_named_struct_carries_a_name_to_index_table() {
    // struct{.a Int, .b Type} — the names slot maps each name to its index.
    let w = Writer::new();
    let t1 = w.int_type("t1");
    let t2 = w.universe("t2");
    let s = w.struct_type("s", &[(t1, Some("a")), (t2, Some("b"))]);
    let b = build(s, w);
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
    // the payload's slot 2 repeats the names in definition order (the table's
    // inverse).
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
    let w = Writer::new();
    let f = w.int_type("f");
    let s1 = w.struct_type("s1", &[(f, Some("f"))]);
    let s2 = w.struct_type("s2", &[(f, Some("f"))]);
    let pair = w.tuple("pair", &[s1, s2]);
    let b = build(pair, w);
    assert!(b.ok);
    // the nominal id is the marker payload's slot 0; see `lichen_highlevel::shape`.
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
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(n))) if n == s1.0 as usize
    ));
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(id2)),
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(n))) if n == s2.0 as usize
    ));
}

#[test]
fn two_struct_type_occurrences_do_not_unify() {
    // same fields, different occurrences → different ids → nominal conflict
    let w = Writer::new();
    let f = w.int_type("f");
    let s1 = w.struct_type("s1", &[(f, Some("f"))]);
    let s2 = w.struct_type("s2", &[(f, Some("f"))]);
    let pair = w.tuple("pair", &[s1, s2]);
    let b = build(pair, w);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.state[s1].term.unwrap(), b.state[s2].term.unwrap());
    assert_eq!(module.unify_errors.len(), 1);
    let err = module.unify_errors[0].clone();
    assert!(matches!(
        err.value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(n))) if n == s1.0 as usize
    ));
    assert!(matches!(
        err.value_b,
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(n))) if n == s2.0 as usize
    ));
}

#[test]
fn a_struct_type_does_not_unify_with_a_same_shape_tuple_type() {
    let w = Writer::new();
    let f = w.int_type("f");
    let s = w.struct_type("s", &[(f, Some("f"))]);
    let t = w.type_tuple("t", &[f]);
    let pair = w.tuple("pair", &[s, t]);
    let b = build(pair, w);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.state[s].term.unwrap(), b.state[t].term.unwrap());
    assert_eq!(module.unify_errors.len(), 1);
    // Both shapes are the same field-**slot** list, so the nominal distinction
    // lives at the kind's marker slot.
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
    let w = Writer::new();
    let f = w.int_type("f");
    let s = w.struct_type("s", &[(f, Some("f"))]);
    let b = build(s, w);
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
    // 5 : struct{.f Int} — the literal's int type conflicts with the struct type.
    let w = Writer::new();
    let five = w.int("five", 5);
    let f = w.int_type("f");
    let s = w.struct_type("s", &[(f, Some("f"))]);
    let a = w.annotate("a", five, s);
    let b = build(a, w);
    assert!(!b.ok, "5 : struct{{.f Int}} must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
    );
    // The struct's shape is the structured expected side; the language layer
    // spells it.
    assert!(
        diags[0].loc().is_some(),
        "the annotation conflict is located"
    );
}

#[test]
fn two_struct_types_conflict_reports_the_nominal_ids() {
    // The argument's struct type has a different fresh id than the annotation's.
    let w = Writer::new();
    let f1 = w.int_type("f1");
    let s1 = w.struct_type("s1", &[(f1, Some("f"))]);
    let f2 = w.int_type("f2");
    let s2 = w.struct_type("s2", &[(f2, Some("f"))]);
    let x = w.param("x");
    let body = w.annotate("body", x, s1);
    let l = w.lam("l", x, body);
    let whole = w.app("whole", l, s2);
    let b = build(whole, w);
    assert!(!b.ok, "two distinct struct types must not unify");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Runtime);
    // The conflict is on the nominal id; the wording is the language layer's job.
    assert!(diags[0].loc().is_some(), "the runtime conflict is located");
}

#[test]
fn an_annotation_with_a_struct_type_rejects_a_literal_at_apply_time() {
    // (\x. (x : struct{.f Int})) 5 — the apply-time unify checks the argument.
    let w = Writer::new();
    let f = w.int_type("f");
    let s = w.struct_type("s", &[(f, Some("f"))]);
    let x = w.param("x");
    let body = w.annotate("body", x, s);
    let l = w.lam("l", x, body);
    let five = w.int("five", 5);
    let whole = w.app("whole", l, five);
    let b = build(whole, w);
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
    // The IR is a graph: the same expression may have several parents, so it
    // compiles once, keeping one nominal id.
    let w = Writer::new();
    let f = w.int_type("f");
    let s = w.struct_type("s", &[(f, Some("f"))]);
    let a = w.array("a", &[s, s]);
    let b = build(a, w);
    assert!(b.ok, "one shared occurrence is one nominal type");
    assert!(b.module.unify_errors.is_empty());
    // The id is the written occurrence, so sharing the expression shares the type.
    let id = array_ids(
        &b,
        array_ids(&b, array_ids(&b, b.state[s].ty.unwrap())[0])[0],
    )[0];
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(id)),
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(n))) if n == s.0 as usize
    ));
}

// --- struct instantiation ----------------------------------------------------

// `s(1, 2)` wraps the positional tuple in the struct type: the element-type
// list is checked against the field list.

#[test]
fn a_tuple_instantiated_with_a_struct_type_is_an_instance() {
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let f1 = w.int_type("f1");
    let f2 = w.int_type("f2");
    let s = w.struct_type("s", &[(f1, Some("f")), (f2, Some("g"))]);
    let inst = w.instantiate("inst", s, v, &[]);
    let b = build(inst, w);
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
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let f = w.int_type("f");
    let s = w.struct_type("s", &[(f, Some("f"))]);
    let inst = w.instantiate("inst", s, v, &[]);
    let b = build(inst, w);
    assert!(
        !b.ok,
        "s(1, 2) against a one-field struct must fail (arity)"
    );
    // field type: the tuple's Ints are not Type
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let t1 = w.universe("t1");
    let t2 = w.universe("t2");
    let s = w.struct_type("s", &[(t1, Some("f")), (t2, Some("t"))]);
    let inst = w.instantiate("inst", s, v, &[]);
    let b = build(inst, w);
    assert!(
        !b.ok,
        "s(1, 2) against struct{{.f Type, .t Type}} must fail (fields)"
    );
    // a literal is not a positional value
    let w = Writer::new();
    let five = w.int("five", 5);
    let f = w.int_type("f");
    let s = w.struct_type("s", &[(f, Some("f"))]);
    let inst = w.instantiate("inst", s, five, &[]);
    let b = build(inst, w);
    assert!(!b.ok, "s(5) must fail — a literal is not a struct value");
}

#[test]
fn instances_of_different_struct_occurrences_conflict() {
    // [s1(1, 2), s2(1, 2)] — each instance carries its own nominal id.
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let f1 = w.int_type("f1");
    let f2 = w.int_type("f2");
    let s1 = w.struct_type("s1", &[(f1, Some("f")), (f2, Some("g"))]);
    let i1 = w.instantiate("i1", s1, v, &[]);
    let f3 = w.int_type("f3");
    let f4 = w.int_type("f4");
    let s2 = w.struct_type("s2", &[(f3, Some("f")), (f4, Some("g"))]);
    let i2 = w.instantiate("i2", s2, v, &[]);
    let a = w.array("a", &[i1, i2]);
    let b = build(a, w);
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
    // `(mk (Int))(1, 2)` with `mk = u => struct{...}` — the callee is an
    // unevaluated apply the checker forces first.
    let w = Writer::new();
    let p = w.param("p");
    let f1 = w.int_type("f1");
    let f2 = w.int_type("f2");
    let s = w.struct_type("s", &[(f1, Some("f")), (f2, Some("g"))]);
    let mk = w.lam("mk", p, s);
    let arg = w.int_type("arg");
    let call = w.app("call", mk, arg);
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let inst = w.instantiate("inst", call, v, &[]);
    let b = build(inst, w);
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
    // `(mk (Int))(1, 2)` with `mk = u => Int`: the forced callee is not a struct.
    let w = Writer::new();
    let p = w.param("p");
    let intt = w.int_type("intt");
    let mk = w.lam("mk", p, intt);
    let arg = w.int_type("arg");
    let call = w.app("call", mk, arg);
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let inst = w.instantiate("inst", call, v, &[]);
    let b = build(inst, w);
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
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let f1 = w.int_type("f1");
    let f2 = w.int_type("f2");
    let t = w.type_tuple("t", &[f1, f2]);
    let inst = w.instantiate("inst", t, v, &[]);
    let b = build(inst, w);
    assert!(!b.ok, "tuple{{Int, Int}}(1, 2) must be rejected");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::InstantiateCallee);
    assert_eq!(
        diags[0].loc.as_ref().map(|loc| loc.expr),
        Some(t),
        "the not-a-struct error points at the callee"
    );
    // Nor can a function type.
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let d = w.int_type("d");
    let c = w.int_type("c");
    let f = w.type_function("f", d, c);
    let inst = w.instantiate("inst", f, v, &[]);
    let b = build(inst, w);
    assert!(!b.ok, "(Int -> Int)(1, 2) must be rejected");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::InstantiateCallee);
}

#[test]
fn an_instantiation_through_a_parameter_pins_the_callee_to_a_struct_kind() {
    // `f = s => s(1, 2); f (Int)` — the undecided callee is pinned to a struct
    // kind; a non-struct argument fails the check.
    let w = Writer::new();
    let p = w.param("p");
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let inst = w.instantiate("inst", p, v, &[]);
    let f = w.lam("f", p, inst);
    let arg = w.int_type("arg");
    let call = w.app("call", f, arg);
    let b = build(call, w);
    assert!(!b.ok, "applying f to a non-struct type must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    // Read per slot type, the requirement is stated here rather than through
    // the apply's argument channel.
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(
        diags[0].loc.as_ref().map(|loc| loc.expr),
        Some(inst),
        "the refusal names the instantiation whose callee is not a struct"
    );
    // The pin rejects a tuple type too (nominality through the deferral).
    let w = Writer::new();
    let p = w.param("p");
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let inst = w.instantiate("inst", p, v, &[]);
    let f = w.lam("f", p, inst);
    let f1 = w.int_type("f1");
    let f2 = w.int_type("f2");
    let arg = w.type_tuple("arg", &[f1, f2]);
    let call = w.app("call", f, arg);
    let b = build(call, w);
    assert!(
        !b.ok,
        "a tuple type must not instantiate through a parameter"
    );
}

#[test]
fn a_named_instantiation_through_a_parameter_defers_its_reorder() {
    // `f = s => s(.x 1, .y Int)` — the name table is not known through a
    // parameter, so the reorder stays a lazy read.
    let w = Writer::new();
    let p = w.param("p");
    let one = w.int("one", 1);
    let t = w.universe("t");
    let v = w.tuple("v", &[one, t]);
    let inst = w.instantiate("inst", p, v, &[Some("x"), Some("y")]);
    let f = w.lam("f", p, inst);
    let b = build(f, w);
    assert!(
        b.ok,
        "a named instantiation through a parameter is not refused: {:?}",
        b.diagnostics()
    );
    // The instance's value is the deferred reorder, one element per argument.
    let ids = array_ids(&b, b.state[inst].val.unwrap());
    assert_eq!(ids.len(), 2, "one instance position per named argument");
    // Each element is `Index(call_values, TableGet(supply, key))`: the lazy
    // gather, attributed through the template origin.
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
    let w = Writer::new();
    let five = w.int("five", 5);
    let h = w.hole("h");
    let a = w.annotate("a", five, h);
    let mut b = build(a, w);
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
    // (\x. x) : _ — the placeholder binds to the whole function-type node:
    // clone-on-unify never fires without a signature.
    let w = Writer::new();
    let x = w.param("x");
    let l = w.lam("l", x, x);
    let h = w.hole("h");
    let a = w.annotate("a", l, h);
    let mut b = build(a, w);
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
    // (\x. x) : (Int -> _) — the annotation binds the two functions' own
    // cells; nothing is cloned.

    // The identity is monomorphic at this type; the cost is deliberate
    // (`docs/notes/function-type-merge.md`).
    let w = Writer::new();
    let x = w.param("x");
    let l = w.lam("l", x, x);
    let it = w.int_type("it");
    let h = w.hole("h");
    let t = w.type_function("t", it, h);
    let a = w.annotate("a", l, t);
    let mut b = build(a, w);
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
    // [1, 2, 3] : array{Int, _} — the placeholder length binds to the
    // element count.
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let e3 = w.int("e3", 3);
    let arr = w.array("arr", &[e1, e2, e3]);
    let it = w.int_type("it");
    let h = w.hole("h");
    let t = w.type_array("t", it, h);
    let a = w.annotate("a", arr, t);
    let mut b = build(a, w);
    assert!(b.ok, "[1, 2, 3] : array{{Int, _}} should check");
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
    let w = Writer::new();
    let five = w.int("five", 5);
    let it = w.int_type("it");
    let h = w.hole("h");
    let t = w.type_function("t", it, h);
    let a = w.annotate("a", five, t);
    let b = build(a, w);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
}

#[test]
fn shallow_array_is_masked_and_typed_like_a_tuple() {
    // [1, @shallow(2)] — the marker marks position 1; the type is a tuple, not an
    // array type.
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let arr = w.shallow_array("arr", &[(one, 0), (two, usize::MAX)]);
    let mut b = build(arr, w);
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
    // [1, @shallow(x => x + 1) 5] — the apply sits at the bare-`@shallow` position, so the
    // deep pass skips it.
    let w = Writer::new();
    let one = w.int("one", 1);
    let five = w.int("five", 5);
    let x = w.param("x");
    let one2 = w.int("one2", 1);
    let add = w.binop("add", BinOp::Add, x, one2);
    let lam = w.lam("lam", x, add);
    let call = w.app("call", lam, five);
    let arr = w.shallow_array("arr", &[(one, 0), (call, usize::MAX)]);
    let b = build(arr, w);
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
    // [1, @shallow(x => x + 1) 5][1] — the read runs the apply that the deep pass
    // deliberately skipped.
    let w = Writer::new();
    let one = w.int("one", 1);
    let five = w.int("five", 5);
    let x = w.param("x");
    let one2 = w.int("one2", 1);
    let add = w.binop("add", BinOp::Add, x, one2);
    let lam = w.lam("lam", x, add);
    let call = w.app("call", lam, five);
    let arr = w.shallow_array("arr", &[(one, 0), (call, usize::MAX)]);
    let one_idx = w.int("one_idx", 1);
    let read = w.field("read", arr, one_idx);
    let mut b = build(read, w);
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

#[test]
fn top_level_assert_passes_when_the_condition_is_one() {
    // assert(1 == 1) — the condition resolves to 1 at check time.
    let w = Writer::new();
    let one = w.int("one", 1);
    let one2 = w.int("one2", 1);
    let cond = w.binop("cond", BinOp::Eq, one, one2);
    let asserted = w.assert("asserted", cond);
    let b = build(asserted, w);
    assert!(b.ok, "assert(1 == 1) should check");
    assert!(b.module.assert_errors.is_empty());
}

#[test]
fn top_level_assert_fails_when_the_condition_is_not_one() {
    // assert(1 == 2) — the condition resolves to 0: a failed assert, not a
    // unification failure.
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let cond = w.binop("cond", BinOp::Eq, one, two);
    let asserted = w.assert("asserted", cond);
    let b = build(asserted, w);
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
    // f = n => assert(n == 1); f 1 — the apply clones the undecided assert and
    // re-checks it.
    let w = Writer::new();
    let n = w.param("n");
    let one = w.int("one", 1);
    let cond = w.binop("cond", BinOp::Eq, n, one);
    let asserted = w.assert("asserted", cond);
    let f = w.lam("f", n, asserted);
    let one_arg = w.int("one_arg", 1);
    let root = w.app("root", f, one_arg);
    let b = build(root, w);
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
    let w = Writer::new();
    let n = w.param("n");
    let one = w.int("one", 1);
    let cond = w.binop("cond", BinOp::Eq, n, one);
    let asserted = w.assert("asserted", cond);
    let f = w.lam("f", n, asserted);
    let two_arg = w.int("two_arg", 2);
    let root = w.app("root", f, two_arg);
    let b = build(root, w);
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
    // f = n => assert(n == 1), never applied: the assert stays pending.
    let w = Writer::new();
    let n = w.param("n");
    let one = w.int("one", 1);
    let cond = w.binop("cond", BinOp::Eq, n, one);
    let asserted = w.assert("asserted", cond);
    let f = w.lam("f", n, asserted);
    let b = build(f, w);
    assert!(b.ok, "an untriggered assert is no failure");
    assert_eq!(b.module.asserts.len(), 1);
    assert!(b.module.assert_errors.is_empty());
}

#[test]
fn assert_on_a_literal_checks_the_value_itself() {
    // assert(3) — the condition is the literal's value: it resolves to 3,
    // not 1, so the assert fails.
    let w = Writer::new();
    let three = w.int("three", 3);
    let asserted = w.assert("asserted", three);
    let b = build(asserted, w);
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
    // [1, 2, 3][3] — the generated `i < len` constraint is what fails; the
    // read stays in-bounds for the runtime.
    let w = Writer::new();
    let a = w.int("a", 1);
    let b2 = w.int("b2", 2);
    let c = w.int("c", 3);
    let arr = w.array("arr", &[a, b2, c]);
    let idx = w.int("idx", 3);
    let e = w.index("e", arr, idx);
    let b = build(e, w);
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
    // [1, 2][1] — `1 < 2` holds, so the constraint is consumed like a passing
    // assert.
    let w = Writer::new();
    let a = w.int("a", 1);
    let b2 = w.int("b2", 2);
    let arr = w.array("arr", &[a, b2]);
    let idx = w.int("idx", 1);
    let e = w.index("e", arr, idx);
    let b = build(e, w);
    assert!(b.ok, "an in-range read should check");
    assert!(b.module.assert_errors.is_empty());
    assert!(
        b.module.asserts.is_empty(),
        "the satisfied constraint was consumed"
    );
}

#[test]
fn a_body_index_on_a_literal_stays_pending_and_rechecks_per_call() {
    // f = i => [7, 8, 9][i] — the body's `i < 3` constraint stays pending; it
    // is cloned and decided per apply.
    let w = Writer::new();
    let i = w.param("i");
    let a = w.int("a", 7);
    let b2 = w.int("b2", 8);
    let c = w.int("c", 9);
    let arr = w.array("arr", &[a, b2, c]);
    let read = w.index("read", arr, i);
    let f = w.lam("f", i, read);
    let two = w.int("two", 2);
    let good = w.app("good", f, two);
    let b = build(good, w);
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
    // f = i => [7, 8, 9][i]; f 3 — the generated `i < 3` constraint is not
    // reachable from the return; the clone fails.
    let w = Writer::new();
    let i = w.param("i");
    let a = w.int("a", 7);
    let b2 = w.int("b2", 8);
    let c = w.int("c", 9);
    let arr = w.array("arr", &[a, b2, c]);
    let read = w.index("read", arr, i);
    let f = w.lam("f", i, read);
    let three = w.int("three", 3);
    let bad = w.app("bad", f, three);
    let b = build(bad, w);
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
    let w = Writer::new();
    let i = w.param("i");
    let a = w.int("a", 7);
    let b2 = w.int("b2", 8);
    let c = w.int("c", 9);
    let arr = w.array("arr", &[a, b2, c]);
    let read = w.index("read", arr, i);
    let f = w.lam("f", i, read);
    let five = w.int("five", 5);
    let bad = w.app("bad", f, five);
    let b = build(bad, w);
    assert!(!b.ok, "f 5 must fail");
    assert_eq!(b.module.assert_errors.len(), 1);
    assert_eq!(
        b.module.assert_errors[0].value,
        HighProgramValue::LowValue(LowValue::USize(0))
    );
}

// --- a struct-domain function that reads a field ----------------------------

// A named field read registers a re-checkable kind assert when the container
// is undecided.

#[test]
fn an_annotated_struct_parameter_function_reads_a_named_field() {
    // `f = (p : struct{.x Int, .y Int} => p.x)`; `f (struct{.x 1, .y 2})`.
    let w = Writer::new();
    let p = w.param("p");
    let int_x = w.int_type("int_x");
    let int_y = w.int_type("int_y");
    let struct_ty = w.struct_type("struct_ty", &[(int_x, Some("x")), (int_y, Some("y"))]);
    let read = w.named_field("read", p, "x");
    let f = w.lam_typed("f", p, Some(struct_ty), read, false);
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let fields = w.tuple("fields", &[one, two]);
    let arg = w.instantiate("inst", struct_ty, fields, &[]);
    let call = w.app("call", f, arg);
    let b = build(call, w);
    assert!(b.ok, "f (struct{{.x 1, .y 2}}) must check");
}

/// Two **separately written** occurrences of `struct{.x Int}` are two distinct
/// nominal types.
///
/// # Invariant
/// A struct type expression mints a fresh nominal id per source occurrence
/// (`check_type_struct`), and unification compares that id; sharing one *named*
/// type expression is what makes an annotation and an instance agree.
#[test]
fn two_written_struct_types_are_distinct_nominal_types() {
    let w = Writer::new();
    let p = w.param("p");
    let int_a = w.int_type("int_a");
    let written_in_annotation = w.struct_type("written_in_annotation", &[(int_a, Some("x"))]);
    let read = w.named_field("read", p, "x");
    let f = w.lam_typed("f", p, Some(written_in_annotation), read, false);
    let int_b = w.int_type("int_b");
    let written_in_argument = w.struct_type("written_in_argument", &[(int_b, Some("x"))]);
    let one = w.int("one", 1);
    let fields = w.tuple("fields", &[one]);
    let arg = w.instantiate("inst", written_in_argument, fields, &[]);
    let call = w.app("call", f, arg);
    let b = build(call, w);
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
    // `F = T: KT T => T.I` with `KT = _x => struct{.I _, .O _}` — the
    // annotation names the fields, leaving their types open.

    // The annotated parameter's type is compiled and unified into its slot
    // before the body compiles (`checker/lambda.rs`).

    // So the body's `T.I` reads a field the annotation already named, and
    // applying `F` to a struct type closes it.
    let w = Writer::new();
    let t = w.param("t");
    let open_i = w.hole("open_i");
    let open_o = w.hole("open_o");
    // **One** named struct type for both sides: two written `struct{…}` are
    // distinct.
    let open_struct = w.struct_type("open_struct", &[(open_i, Some("I")), (open_o, Some("O"))]);
    let read = w.named_field("read", t, "I");
    let f = w.lam_typed("f", t, Some(open_struct), read, false);
    // The argument is an **instance** of that struct whose field values are the
    // *types* `Int` — the struct contains a type.
    let int_i = w.int_type("int_i");
    let int_o = w.int_type("int_o");
    let fields = w.tuple("fields", &[int_i, int_o]);
    let arg = w.instantiate("inst", open_struct, fields, &[]);
    let call = w.app("call", f, arg);
    let b = build(call, w);
    // The checker's own per-expression view, not mutated by the unify.
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
    // What each function's parameter type slot holds after the build.
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
    // The regression `f = p : tuple{Int, Int} => p(0)`; `f (1, 2)` failed with
    // `expected raw[?a, Int], found raw[?a, Int]`.
    let w = Writer::new();
    let p = w.param("p");
    let int1 = w.int_type("int1");
    let int2 = w.int_type("int2");
    let tuple_ty = w.type_tuple("tuple_ty", &[int1, int2]);
    let zero = w.int("zero", 0);
    let read = w.field("read", p, zero);
    let f = w.lam_typed("f", p, Some(tuple_ty), read, false);
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let fields = w.tuple("fields", &[one, two]);
    let call = w.app("call", f, fields);
    let b = build(call, w);
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

// The regression target: a wrapper `g = (a => f a)` gets its parameter type
// cell from normalizing g.

// This is the language-level shape of the compute wrapper: the kernel's
// parameter class is undecided at the same cell.

#[test]
fn a_wrappers_parameter_type_is_inferred_from_a_body_call() {
    // `f = (p : struct{.x Int, .y Int} => p)`; `g = (a => f a)`.
    let w = Writer::new();
    let p = w.param("p");
    let int_x = w.int_type("int_x");
    let int_y = w.int_type("int_y");
    let struct_ty = w.struct_type("struct_ty", &[(int_x, Some("x")), (int_y, Some("y"))]);
    let f = w.lam_typed("f", p, Some(struct_ty), p, false);

    let a = w.param("a");
    let call = w.app("call", f, a);
    let g = w.lam("g", a, call);

    let b = build(g, w);
    assert!(b.ok, "the wrapper must check");
    let gtype = b.state[g].ty.expect("a lambda has a type");
    let gid = function_type_id(&b, gtype);
    let cell = param_type_cell(&b, gid);
    assert!(
        b.module.node_value(AnyNodeId::Dynamic(cell)).is_some(),
        "the wrapper's parameter type must be inferred from the body call"
    );
}
