//! The runtime is the typechecker; see docs/notes/lowlevel-vm.md and
//! docs/language-spec.md.

use lichen_highlevel::NoAttr;
use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{BinOp, ExprId, ExprKind};
use lichen_highlevel::program::{
    AnyLit, HighProgramLiteral, HighProgramValue, ProgramImpl, StructTemplateLit, TypeValue,
};
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

/// The wildcard `@any` — a value that unifies with everything and constrains
/// nothing.
///
/// # Invariant
/// The literal is written under a name, so a template's element is as
/// readable as the rest of the program.
fn any(w: &Writer, name: &str) -> ExprId {
    let literal = w.hole(name);
    w.fill(literal, ExprKind::Literal(HighProgramLiteral::from(AnyLit)));
    literal
}

/// The wildcard bound to `ty` — the `@any : T` a template element spells.
///
/// # Invariant
/// The wildcard's own type cell is fresh per occurrence, so one template's
/// requirement never binds another's.
fn any_of(w: &Writer, name: &str, ty: ExprId) -> ExprId {
    let literal = any(w, &format!("{name}_value"));
    w.annotate(name, literal, ty)
}

/// The layout marker a type node carries — the `Tuple`, `Array` or `Struct`
/// constant its value slot holds.
fn layout_marker(b: &lichen_highlevel::checker::Build<ProgramImpl>, ty: NodeId) -> TypeValue {
    let ids = array_ids(b, ty);
    match b.module.node_value(AnyNodeId::Dynamic(ids[0])) {
        Some(HighProgramValue::TypeValue(marker)) => marker,
        other => panic!("expected a layout marker, got {other:?}"),
    }
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

/// The `FunctionId` a function's **value** carries — the identity is never in
/// the type.
fn function_id(
    b: &lichen_highlevel::checker::Build<ProgramImpl>,
    expr: lichen_lowlevel::NodeId,
) -> lichen_lowlevel::FunctionId {
    let slot0 = array_ids(b, expr)[0];
    match b.module.node_value(AnyNodeId::Dynamic(slot0)) {
        Some(HighProgramValue::LowValue(LowValue::Function(AnyFunctionId::Dynamic(fid)))) => fid,
        _ => panic!("expected a function expr"),
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

/// Whether the given ids form the int type — a `[int, Type]` expr.
///
/// # Invariant
/// The two slots are matched differently on purpose: element 0 is the marker,
/// matched by *content*, because a literal rebuilds its type per occurrence and
/// so is never the shared `Build::int_type` node; element 1 is the universe,
/// compared by node identity against the canonical `b.type_expr`.
fn is_int_type_ids(b: &lichen_highlevel::checker::Build<ProgramImpl>, ids: &[NodeId]) -> bool {
    ids.len() >= 2
        && matches!(
            b.module.node_value(AnyNodeId::Dynamic(ids[0])),
            Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
        )
        && ids[1] == b.type_expr
}

/// An expr's width in the checked program: the value and type slots plus one
/// per attribute.
fn width() -> usize {
    lichen_highlevel::shape::slot_width::<ProgramImpl>()
}

/// Whether `node` is the int type — the two-slot test `is_int_type_ids` states.
fn is_int_type(b: &lichen_highlevel::checker::Build<ProgramImpl>, node: NodeId) -> bool {
    is_int_type_ids(b, &array_ids(b, node))
}

/// Whether an evaluated value is the int type expr `[int, Type]`.
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
    assert_eq!(ids.len(), width());
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
    assert_eq!(ids.len(), width());
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
    // A recovered-error region is an expr of fresh cells: it carries no
    // grammar, so nothing inside it is checked.
    let w = Writer::new();
    let e = w.error_block("e");
    let b = build(e, w);
    assert!(b.ok, "a masked error region is not a check failure");
    // The skip path records the expr's slots, so downstream reads still work.
    assert!(b.state[e].term.is_some(), "the skip path records the expr");
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
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 2));
    let b = build(w.array_inst("array", template, &[e, five]), w);
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
        "the embedded error block still records its expr"
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
    // The lambda's type is the shared `Function` type: the `[FunctionType, Type]`
    // expr, which names no function.
    let ftype = b.state[l].ty.unwrap();
    let ids = array_ids(&b, ftype);
    assert_eq!(
        ids.len(),
        width(),
        "the Function type is an expr [marker, universe]"
    );
    assert_ne!(
        ids[1], ftype,
        "the Function type is not self-referential: the chain never cycles at a function"
    );
    assert_eq!(
        b.module.node_value(AnyNodeId::Dynamic(ids[0])),
        Some(HighProgramValue::TypeValue(TypeValue::TypeFunction)),
        "the value slot holds the FunctionType marker"
    );
}

#[test]
fn lambda_checks_against_the_function_type_annotation() {
    // (\x. x) : int → int
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let t = w.function_type("t");
    let a = w.annotate("a", l, t);
    let b = build(a, w);
    assert!(b.ok, "(\\x. x) : int → int should check");
}

#[test]
fn annotating_a_lambda_with_a_tuple_template_fails() {
    // (\x. x) : (@any : int, @any : int) — a tuple template is a value, not a
    // function type: the two must not agree.
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let d = w.int_type("d");
    let c = w.int_type("c");
    let t = w.tuple("t", &[any_of(&w, "d_any", d), any_of(&w, "c_any", c)]);
    let a = w.annotate("a", l, t);
    let b = build(a, w);
    assert!(
        !b.ok,
        "(\\x. x) : (@any : int, @any : int) must fail (a lambda is not a tuple)"
    );
}

#[test]
fn a_tuple_value_is_typed_by_the_shared_tuple_template() {
    // (1, 2) — the value holds the elements' own terms and the type is the
    // shared `Tuple` template.
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let tup = w.tuple("tup", &[e1, e2]);
    let b = build(tup, w);
    assert!(b.ok, "(1, 2) should check");
    let ty = b.state[tup].ty.unwrap();
    let ty_ids = array_ids(&b, ty);
    assert_eq!(
        ty_ids.len(),
        width(),
        "a type expression is an expr [value, type, attrs…]"
    );
    assert_eq!(
        layout_marker(&b, ty),
        TypeValue::TypeTuple,
        "the tuple's type is the shared Tuple template"
    );
    assert_eq!(ty_ids[1], b.type_expr);
    // The value holds the elements' own terms: each is a complete slot, so the
    // element's type is its slot's type position.
    let value_ids = array_ids(&b, b.state[tup].val.unwrap());
    assert_eq!(value_ids.len(), 2, "the tuple's value is its two elements");
    for element in value_ids {
        let slot = array_ids(&b, element);
        assert_eq!(slot.len(), width());
        assert!(is_int_type(&b, slot[1]), "the element's type is Int");
    }
}

#[test]
fn an_array_template_holds_its_element_template_and_its_holes() {
    // [@any : int, 3] — the value is the element template and three holes; the
    // type is the shared `Array` template.
    let w = Writer::new();
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let n = w.int("n", 3);
    let arr = w.array_template("arr", element, n);
    let b = build(arr, w);
    assert!(b.ok, "[@any : int, 3] should check");
    // The type of the template is the shared `Array` template.
    let ty = b.state[arr].ty.unwrap();
    let kind_ids = array_ids(&b, ty);
    assert_eq!(kind_ids.len(), width());
    assert_eq!(layout_marker(&b, ty), TypeValue::TypeArray);
    assert_eq!(kind_ids[1], b.type_expr);
    // The value is the element template and the holes the length states.
    let value_ids = array_ids(&b, b.state[arr].val.unwrap());
    assert_eq!(value_ids.len(), 4, "the template and its three holes");
    let template_slot = array_ids(&b, value_ids[0]);
    assert!(
        is_int_type(&b, template_slot[1]),
        "the element template states Int"
    );
    for hole in &value_ids[1..] {
        let hole_ids = array_ids(&b, *hole);
        assert!(
            b.module
                .node_value(AnyNodeId::Dynamic(hole_ids[0]))
                .is_none(),
            "a hole's value slot is undecided"
        );
    }
}

#[test]
fn an_array_template_is_not_itself_a_type() {
    // [@any : int, 3] : Type — the array template's own type is the `Array`
    // layout, not the universe.
    let w = Writer::new();
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let n = w.int("n", 3);
    let arr = w.array_template("arr", element, n);
    let type_val = w.universe("type_val");
    let a = w.annotate("a", arr, type_val);
    let b = build(a, w);
    assert!(!b.ok, "an array template is not itself a Type");
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
fn a_lambda_against_an_array_template_conflicts() {
    // (\x. x) : [@any : int, 3] — a function's type is not an array's.
    let w = Writer::new();
    let x = w.param("x");
    let l = w.lam("l", x, x);
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let n = w.int("n", 3);
    let arr_ty = w.array_template("arr_ty", element, n);
    let a = w.annotate("a", l, arr_ty);
    let b = build(a, w);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(
        diags.len(),
        1,
        "one conflict: a function-type vs an array layout"
    );
    assert_eq!(diags[0].kind, DiagKind::Annotation);
}

#[test]
fn an_array_instance_is_typed_by_the_shared_array_template() {
    // [@any : int, 2][1, 2] — the type is the shared `Array` template; the
    // value is the element template and the elements.
    let w = Writer::new();
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 2));
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let arr = w.array_inst("arr", template, &[e1, e2]);
    let b = build(arr, w);
    assert!(b.ok, "[1, 2] as an array instance should check");
    let ty = b.state[arr].ty.unwrap();
    let ids = array_ids(&b, ty);
    assert_eq!(
        ids.len(),
        width(),
        "a type expression is an expr [value, type, attrs…]"
    );
    assert_eq!(layout_marker(&b, ty), TypeValue::TypeArray);
    assert_eq!(ids[1], b.type_expr);
    // The value holds the element template and then the elements.
    let value_ids = array_ids(&b, b.state[arr].val.unwrap());
    assert_eq!(value_ids.len(), 3);
    let element_slot = array_ids(&b, value_ids[0]);
    assert!(
        is_int_type(&b, element_slot[1]),
        "the instance keeps the template's element template"
    );
    let first = array_ids(&b, value_ids[1]);
    assert!(is_int_type(&b, first[1]), "the first element's type is Int");
}

#[test]
fn an_array_instance_checks_against_its_template() {
    // [@any : int, 2][1, 2] — every element is matched against its own clone
    // of the template.
    let w = Writer::new();
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 2));
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let arr = w.array_inst("arr", template, &[e1, e2]);
    let b = build(arr, w);
    assert!(b.ok, "[1, 2] against its template should check");
    assert!(b.module.unify_errors.is_empty());
}

#[test]
fn an_array_instance_whose_length_differs_from_its_template_fails() {
    // [@any : int, 3][1, 2] — the template's length is the instance's.
    let w = Writer::new();
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 3));
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let arr = w.array_inst("arr", template, &[e1, e2]);
    let b = build(arr, w);
    assert!(!b.ok, "[1, 2] against a three-element template must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::ArrayElement);
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
fn an_array_instance_with_a_heterogeneous_element_fails() {
    // [@any : int, 2][1, int] — the second element's type is Type; the
    // template's element states Int.
    let w = Writer::new();
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 2));
    let e1 = w.int("e1", 1);
    let e2 = w.int_type("e2"); // `int` as a value — its type is Type
    let arr = w.array_inst("arr", template, &[e1, e2]);
    let b = build(arr, w);
    assert!(
        !b.ok,
        "[1, int] must fail (the elements must share the template's type)"
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
fn a_tuple_matches_a_tuple_template_elementwise() {
    // (1, 2) = (@any : int, @any : int) — both sides are evaluated values.
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let tup = w.tuple("tup", &[e1, e2]);
    let d = w.int_type("d");
    let c = w.int_type("c");
    let template = w.tuple(
        "template",
        &[any_of(&w, "d_any", d), any_of(&w, "c_any", c)],
    );
    let a = w.unify("a", tup, template);
    let b = build(a, w);
    assert!(b.ok, "(1, 2) against (@any : int, @any : int) should check");
    assert!(b.module.unify_errors.is_empty());
}

#[test]
fn a_tuple_whose_length_differs_from_its_template_fails() {
    // (1, 2) <: (@any : int) — the template states one element.
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let tup = w.tuple("tup", &[e1, e2]);
    let d = w.int_type("d");
    let template = w.tuple("template", &[any_of(&w, "d_any", d)]);
    let a = w.unify("a", tup, template);
    let b = build(a, w);
    assert!(
        !b.ok,
        "(1, 2) against (@any : int) must fail (length mismatch)"
    );
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
    // The expected side is the shared `Function` type, not an arrow term: it
    // clashes with the callee's `int` marker.
    assert_eq!(
        diags[0].value_b,
        Some(HighProgramValue::TypeValue(TypeValue::TypeFunction)),
        "the expected side is the FunctionType marker"
    );
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
    assert_eq!(requirement.len(), width(), "a kind is a [marker, K] expr");
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
fn annotating_a_lambda_with_a_mixed_tuple_template_reports_expected_found() {
    // (\x. x) : (@any : Type, @any : Int) — a function's type is not a tuple's.
    let w = Writer::new();
    let x = w.param("x");
    // The return expression uses the parameter's id directly.
    let l = w.lam("l", x, x);
    let t1 = w.universe("t1");
    let t2 = w.int_type("t2");
    let template = w.tuple(
        "template",
        &[any_of(&w, "t1_any", t1), any_of(&w, "t2_any", t2)],
    );
    let a = w.annotate("a", l, template);
    let b = build(a, w);
    assert!(!b.ok, "a function is not a tuple template");
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
    // The found side is the shared `Function` type: a function's type is the
    // marker, and the marker is not `Type`.
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeFunction)),
        "the found side is the FunctionType marker"
    );
}

#[test]
fn an_unannotated_call_syncs_its_root_type_to_the_return_type() {
    // (\id. id 5) (\x. x) — the call's result cell is a lazy record, synced
    // to the return expr by the apply's evaluation.
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
    // lazy result cells, synced with their return exprs.
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
    // The element types are the calls' lazy result cells, synced with their
    // return exprs; the value holds the elements.
    let e0 = b.state[c0].ty.unwrap();
    assert!(is_int_type(&b, e0), "element 0's cell syncs to int");
    let e1 = b.state[c1].ty.unwrap();
    assert_eq!(
        b.module.node_value(AnyNodeId::Dynamic(e1)),
        b.module.node_value(AnyNodeId::Dynamic(b.type_expr)),
        "element 1's cell syncs to Type's"
    );
}

#[test]
fn tuple_length_mismatch_reports_both_sides() {
    // (1, 2) <: (@any : int)
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let tup = w.tuple("tup", &[e1, e2]);
    let d = w.int_type("d");
    let template = w.tuple("template", &[any_of(&w, "d_any", d)]);
    let a = w.unify("a", tup, template);
    let b = build(a, w);
    assert!(!b.ok);
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    // The length mismatch shows in the values: two elements against one.
    let a_value = array_ids(&b, array_ids(&b, diags[0].a)[0]);
    let b_value = array_ids(&b, array_ids(&b, diags[0].b)[0]);
    assert_eq!(a_value.len(), 2);
    assert_eq!(b_value.len(), 1);
}

// --- call result annotations are checked, not just bound --------------------

// A call's result type cell is a lazy record, so an annotation on a call
// result only *binds* the cell at check time.

// The apply's evaluation later syncs the cell with the callee's return expr,
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
    let element = any(&w, "element");
    let template = w.array_template("template", element, w.int("len", 4));
    let f2_body = w.array_inst("f2_body", template, &[a, b, x, y]);
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
    assert_eq!(
        ids.len(),
        expected.len() + 1,
        "the instance's value is the element template and the elements"
    );
    for (&id, &n) in ids[1..].iter().zip(expected.iter()) {
        // An element is a complete slot: the bound value is its first position.
        let element = b
            .module
            .element_value_node(AnyNodeId::Dynamic(id))
            .unwrap_or(AnyNodeId::Dynamic(id));
        assert_eq!(
            b.module.node_value(element),
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
        b.module.function_parent(inner_id),
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
        b.module.function_parent(function_of(&b, sibling)),
        None,
        "a same-level sibling hangs under nothing"
    );
    assert_eq!(
        b.module.function_parent(function_of(&b, other)),
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
    // [@any : int, 3][1, 2, 3][1] — the element sits one slot past the
    // element template the value carries.
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let e3 = w.int("e3", 3);
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 3));
    let arr = w.array_inst("arr", template, &[e1, e2, e3]);
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
    // [1, 2, 3][5] — the value's own length is the bound.
    let w = Writer::new();
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let e3 = w.int("e3", 3);
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 3));
    let arr = w.array_inst("arr", template, &[e1, e2, e3]);
    let five = w.int("five", 5);
    let idx = w.index("idx", arr, five);
    let b = build(idx, w);
    assert!(!b.ok, "[1, 2, 3][5] must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexOutOfBounds);
    // The diagnostic is source-level: the subscript the program wrote and the
    // element count, not the value's own slots.
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
    // The diagnostic is located at the subscript the program wrote.
    assert_eq!(diags[0].loc().map(|loc| loc.expr), Some(five));
}

#[test]
fn array_index_out_of_bounds_against_a_bound_length() {
    // (\x. x[3])([1, 2]) — the length is the value's own, so it is known only
    // once the apply binds the parameter.
    let w = Writer::new();
    let x = w.param("x");
    let three = w.int("three", 3);
    let read = w.index("read", x, three);
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 2));
    let matched = w.unify("matched", x, template);
    let body = w.tuple("body", &[matched, read]);
    let l = w.lam("l", x, body);
    let e1 = w.int("e1", 1);
    let e2 = w.int("e2", 2);
    let arr = w.array_inst("arr", template, &[e1, e2]);
    let whole = w.app("whole", l, arr);
    let b = build(whole, w);
    assert!(!b.ok, "(\\x. x[3])([1, 2]) must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::IndexOutOfBounds);
    // The diagnostic is source-level: the subscript the program wrote and the
    // element count, not the value's own slots.
    assert_eq!(diags[0].index, Some(3));
    assert_eq!(diags[0].length, Some(2));
}

// --- struct values ----------------------------------------------------------

// A struct value is [name table, nominal id, the fields' terms…]; see
// `lichen_highlevel::shape`.

#[test]
fn a_struct_value_carries_a_name_table_and_a_fresh_nominal_id() {
    // {.f @any : int, .t @any : Type} — the value holds the name table, the
    // nominal id and the fields' own terms.
    let w = Writer::new();
    let t1 = any_of(&w, "t1", w.int_type("int_ty"));
    let t2 = any_of(&w, "t2", w.universe("type_ty"));
    let s = w.struct_value("s", &[(t1, Some("f")), (t2, Some("t"))]);
    let mut b = build(s, w);
    assert!(b.ok, "{{.f @any : int, .t @any : Type}} should check");
    assert!(b.module.unify_errors.is_empty());
    // The value is [name table, nominal id, the fields…].
    let struct_value = b.state[s].val.unwrap();
    let value_ids = array_ids(&b, struct_value);
    assert_eq!(value_ids.len(), 4);
    // The name table is a constant table.
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(value_ids[0])),
        Some(HighProgramValue::LowValue(LowValue::Table(_)))
    ));
    // The nominal id is the written occurrence itself.
    assert!(
        matches!(
            b.module.node_value(AnyNodeId::Dynamic(value_ids[1])),
            Some(HighProgramValue::TypeValue(TypeValue::TypeId(n))) if n == s.0 as usize
        ),
        "the id is the written occurrence itself"
    );
    // A field's type is its own slot's type position.
    let f_slot = array_ids(&b, value_ids[2]);
    assert!(is_int_type(&b, f_slot[1]), "the `.f` field's type is Int");
    let t_slot = array_ids(&b, value_ids[3]);
    assert_eq!(
        t_slot[1], b.type_expr,
        "the `.t` field's type is the universe"
    );
    // The type is the shared `Struct` template, which closes on itself.
    let kind = b.state[s].ty.unwrap();
    let kind_ids = array_ids(&b, kind);
    assert_eq!(kind_ids.len(), width());
    assert_eq!(layout_marker(&b, kind), TypeValue::TypeStruct);
    assert_eq!(kind_ids[1], kind, "the Struct template is self-referential");
    assert!(
        lichen_highlevel::shape::is_struct_type_any(
            &mut b.module,
            b.type_expr,
            AnyNodeId::Dynamic(kind)
        ),
        "the shared Struct template is the struct layout"
    );
    // A type whose kind's marker is not the struct marker is not a struct
    // layout: the tag is what names it.
    let block = b.module.node_block(kind);
    let tagless_items = b.module.alloc_array(
        &[
            ArrayItem::new(AnyNodeId::Dynamic(b.markers.tuple_type_marker)),
            ArrayItem::new(AnyNodeId::Dynamic(b.type_expr)),
        ],
        block,
    );
    let tagless = b.module.add_node(
        block,
        None,
        Some(HighProgramValue::from(LowValue::Array(
            tagless_items.into(),
        ))),
    );
    assert!(
        !lichen_highlevel::shape::is_struct_type_any(
            &mut b.module,
            b.type_expr,
            AnyNodeId::Dynamic(tagless)
        ),
        "a type whose kind's marker is not the struct marker is not a struct type"
    );
}

#[test]
fn a_named_struct_carries_a_name_to_index_table() {
    // {.a @any : int, .b @any : Type} — the value's first slot maps each name
    // to its field index.
    let w = Writer::new();
    let t1 = any_of(&w, "t1", w.int_type("int_ty"));
    let t2 = any_of(&w, "t2", w.universe("type_ty"));
    let s = w.struct_value("s", &[(t1, Some("a")), (t2, Some("b"))]);
    let b = build(s, w);
    assert!(b.ok);
    // the names slot (the value's slot 0) is a constant table: "a" -> 0,
    // "b" -> 1.
    let names_node = array_ids(&b, b.state[s].val.unwrap())[0];
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
    // The fields follow in the definition's order, so the table's index is the
    // field's own position.
    let value_ids = array_ids(&b, b.state[s].val.unwrap());
    assert_eq!(
        value_ids.len(),
        4,
        "the name table, the id and the two fields"
    );
    assert_eq!(value_ids[2], b.state[t1].term.unwrap(), "the first field");
    assert_eq!(value_ids[3], b.state[t2].term.unwrap(), "the second field");
}

#[test]
fn each_struct_value_occurrence_allocates_a_distinct_id() {
    let w = Writer::new();
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s1 = w.struct_value("s1", &[(f, Some("f"))]);
    let s2 = w.struct_value("s2", &[(f, Some("f"))]);
    let expr = w.tuple("expr", &[s1, s2]);
    let b = build(expr, w);
    assert!(b.ok);
    // the nominal id is the value's slot 1; see `lichen_highlevel::shape`.
    let id1 = array_ids(&b, b.state[s1].val.unwrap())[1];
    let id2 = array_ids(&b, b.state[s2].val.unwrap())[1];
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
fn two_struct_value_occurrences_do_not_unify() {
    // same fields, different occurrences → different name tables and ids → a
    // nominal conflict
    let w = Writer::new();
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s1 = w.struct_value("s1", &[(f, Some("f"))]);
    let s2 = w.struct_value("s2", &[(f, Some("f"))]);
    let expr = w.tuple("expr", &[s1, s2]);
    let b = build(expr, w);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.state[s1].term.unwrap(), b.state[s2].term.unwrap());
    assert_eq!(module.unify_errors.len(), 1);
    // The conflict is inside the value layout: the two occurrences do not name
    // one struct.
    let err = module.unify_errors[0].clone();
    assert!(
        err.value_a.is_some() && err.value_b.is_some(),
        "the conflict names both sides: {err:?}"
    );
    assert_ne!(
        err.value_a, err.value_b,
        "the two occurrences are distinct: {err:?}"
    );
}

#[test]
fn a_struct_value_does_not_unify_with_a_same_shape_tuple_value() {
    let w = Writer::new();
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f, Some("f"))]);
    let t = w.tuple("t", &[f]);
    let expr = w.tuple("expr", &[s, t]);
    let b = build(expr, w);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.state[s].ty.unwrap(), b.state[t].ty.unwrap());
    assert_eq!(module.unify_errors.len(), 1);
    // The layouts are named by different markers, so a struct is not a tuple.
    let err = module.unify_errors[0].clone();
    let (a, b) = (err.value_a, err.value_b);
    assert!(
        matches!(
            (&a, &b),
            (
                Some(HighProgramValue::TypeValue(TypeValue::TypeStruct)),
                Some(HighProgramValue::TypeValue(TypeValue::TypeTuple))
            ) | (
                Some(HighProgramValue::TypeValue(TypeValue::TypeTuple)),
                Some(HighProgramValue::TypeValue(TypeValue::TypeStruct))
            )
        ),
        "marker-level nominal clash: got {a:?} vs {b:?}"
    );
}

#[test]
fn a_struct_value_unifies_with_itself() {
    let w = Writer::new();
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f, Some("f"))]);
    let b = build(s, w);
    assert!(b.ok);
    let mut module = b.module;
    module.unify(b.state[s].term.unwrap(), b.state[s].term.unwrap());
    assert!(
        module.unify_errors.is_empty(),
        "the same struct value unifies with itself"
    );
}

#[test]
fn an_annotation_against_a_struct_value_reports_the_conflict() {
    // 5 : {.f @any : int} — the literal's int type conflicts with the struct
    // layout.
    let w = Writer::new();
    let five = w.int("five", 5);
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f, Some("f"))]);
    let a = w.annotate("a", five, s);
    let b = build(a, w);
    assert!(!b.ok, "5 : {{.f @any : int}} must fail");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    assert_eq!(
        diags[0].value_a,
        Some(HighProgramValue::TypeValue(TypeValue::TypeInt))
    );
    // The struct's layout is the structured expected side; the language layer
    // spells it.
    assert!(
        diags[0].loc().is_some(),
        "the annotation conflict is located"
    );
}

#[test]
fn two_struct_values_conflict_on_their_name_tables() {
    // An instance of one struct occurrence against a template of another: the
    // two do not unify.
    let w = Writer::new();
    let f1 = any_of(&w, "f1", w.int_type("int_ty"));
    let s1 = w.struct_value("s1", &[(f1, Some("f"))]);
    let one = w.int("one", 1);
    let args = w.tuple("args", &[one]);
    let i1 = w.instantiate("i1", s1, args, &[Some("f")]);
    let f2 = any_of(&w, "f2", w.int_type("int_ty"));
    let s2 = w.struct_value("s2", &[(f2, Some("f"))]);
    let matched = w.unify("matched", i1, s2);
    let b = build(matched, w);
    assert!(!b.ok, "two distinct struct occurrences must not unify");
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::Annotation);
    // The conflict is inside the value layout, and the name table comes
    // first: two tables are never one handle.
    assert!(
        matches!(
            (&diags[0].value_a, &diags[0].value_b),
            (
                Some(HighProgramValue::LowValue(LowValue::Table(_))),
                Some(HighProgramValue::LowValue(LowValue::Table(_)))
            )
        ),
        "the two occurrences do not share a name table: {:?} vs {:?}",
        diags[0].value_a,
        diags[0].value_b
    );
    assert_ne!(
        diags[0].value_a, diags[0].value_b,
        "the two occurrences do not share a name table: {:?} vs {:?}",
        diags[0].value_a, diags[0].value_b
    );
}

#[test]
fn an_annotation_with_a_struct_value_rejects_a_literal_at_apply_time() {
    // (\x. (x : {.f @any : int})) 5 — the apply-time unify checks the argument.
    let w = Writer::new();
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f, Some("f"))]);
    let x = w.param("x");
    let body = w.annotate("body", x, s);
    let l = w.lam("l", x, body);
    let five = w.int("five", 5);
    let whole = w.app("whole", l, five);
    let b = build(whole, w);
    assert!(!b.ok, "a literal is not a struct value");
    let diags = b.diagnostics();
    // One diagnostic: the apply-time check reports the struct's layout against
    // the literal's int type.
    assert_eq!(diags.len(), 1, "one diagnostic: {diags:?}");
    assert_eq!(diags[0].kind, DiagKind::Runtime);
    // reversed direction: a = the parameter's expected type (the struct's
    // layout), b = the argument's found type (int)
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
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f, Some("f"))]);
    let a = w.tuple("a", &[s, s]);
    let b = build(a, w);
    assert!(b.ok, "one shared occurrence is one nominal type");
    assert!(b.module.unify_errors.is_empty());
    // The id is the written occurrence, so sharing the expression shares the
    // nominal id.
    let id = array_ids(&b, b.state[s].val.unwrap())[1];
    assert!(matches!(
        b.module.node_value(AnyNodeId::Dynamic(id)),
        Some(HighProgramValue::TypeValue(TypeValue::TypeId(n))) if n == s.0 as usize
    ));
}

// --- struct instantiation ----------------------------------------------------

// `S{.f 1, .g 2}` fills the clone of the struct value's own fields.

#[test]
fn a_tuple_instantiated_with_a_struct_value_is_an_instance() {
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let f1 = any_of(&w, "f1", w.int_type("int_ty"));
    let f2 = any_of(&w, "f2", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f1, Some("f")), (f2, Some("g"))]);
    let inst = w.instantiate("inst", s, v, &[Some("f"), Some("g")]);
    let b = build(inst, w);
    assert!(b.ok, "S{{.f 1, .g 2}} must check");
    assert!(b.module.unify_errors.is_empty());
    // the instance's type is the shared struct layout, not the tuple's
    assert_eq!(
        b.state[inst].ty, b.state[s].ty,
        "the instance's type is the struct layout"
    );
    assert_eq!(
        layout_marker(&b, b.state[inst].ty.unwrap()),
        TypeValue::TypeStruct
    );
}

#[test]
fn a_struct_instantiation_checks_its_fields() {
    // arity: one field, two values
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f, Some("f"))]);
    let inst = w.instantiate("inst", s, v, &[]);
    let b = build(inst, w);
    assert!(!b.ok, "S{{.f 1}} against two values must fail (arity)");
    // field type: the template's fields are Type, the values' are Int
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let t1 = any_of(&w, "t1", w.universe("type_ty"));
    let t2 = any_of(&w, "t2", w.universe("type_ty"));
    let s = w.struct_value("s", &[(t1, Some("f")), (t2, Some("t"))]);
    let inst = w.instantiate("inst", s, v, &[Some("f"), Some("t")]);
    let b = build(inst, w);
    assert!(
        !b.ok,
        "S{{.f 1, .t 2}} against struct{{.f Type, .t Type}} must fail (fields)"
    );
    // a literal is not a positional value
    let w = Writer::new();
    let five = w.int("five", 5);
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f, Some("f"))]);
    let inst = w.instantiate("inst", s, five, &[]);
    let b = build(inst, w);
    assert!(!b.ok, "S(5) must fail — a literal is not a struct value");
    // an unknown field name is refused
    let w = Writer::new();
    let one = w.int("one", 1);
    let v = w.tuple("v", &[one]);
    let f = any_of(&w, "f", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f, Some("f"))]);
    let inst = w.instantiate("inst", s, v, &[Some("g")]);
    let b = build(inst, w);
    assert!(!b.ok, "S{{.g 1}} against a template with no `.g` must fail");
    let diags = b.diagnostics();
    assert!(
        diags
            .iter()
            .any(|diag| diag.kind == DiagKind::StructUnknownField),
        "the unknown field is named: {diags:?}"
    );
}

#[test]
fn instances_of_different_struct_occurrences_conflict() {
    // [i1, i2] against a template of the first occurrence — each instance
    // carries its own nominal id.
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let f1 = any_of(&w, "f1", w.int_type("int_ty"));
    let f2 = any_of(&w, "f2", w.int_type("int_ty"));
    let s1 = w.struct_value("s1", &[(f1, Some("f")), (f2, Some("g"))]);
    let i1 = w.instantiate("i1", s1, v, &[Some("f"), Some("g")]);
    let f3 = any_of(&w, "f3", w.int_type("int_ty"));
    let f4 = any_of(&w, "f4", w.int_type("int_ty"));
    let s2 = w.struct_value("s2", &[(f3, Some("f")), (f4, Some("g"))]);
    let i2 = w.instantiate("i2", s2, v, &[Some("f"), Some("g")]);
    let template = w.array_template("template", s1, w.int("len", 2));
    let a = w.array_inst("a", template, &[i1, i2]);
    let b = build(a, w);
    assert!(
        !b.ok,
        "instances of different struct occurrences must conflict"
    );
    let diags = b.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagKind::ArrayElement);
    // The conflict is inside the value layout, and the name table comes
    // first: two tables are never one handle.
    assert!(
        matches!(
            (&diags[0].value_a, &diags[0].value_b),
            (
                Some(HighProgramValue::LowValue(LowValue::Table(_))),
                Some(HighProgramValue::LowValue(LowValue::Table(_)))
            )
        ),
        "the two occurrences do not share a name table: {:?} vs {:?}",
        diags[0].value_a,
        diags[0].value_b
    );
    assert_ne!(
        diags[0].value_a, diags[0].value_b,
        "the two occurrences do not share a name table: {:?} vs {:?}",
        diags[0].value_a, diags[0].value_b
    );
}

#[test]
fn an_instantiation_through_a_call_result_callee_checks() {
    // `(mk (Int)){.f 1, .g 2}` with `mk = u => {...}` — the callee is an
    // unevaluated apply the checker forces first.
    let w = Writer::new();
    let p = w.param("p");
    let f1 = any_of(&w, "f1", w.int_type("int_ty"));
    let f2 = any_of(&w, "f2", w.int_type("int_ty"));
    let s = w.struct_value("s", &[(f1, Some("f")), (f2, Some("g"))]);
    let mk = w.lam("mk", p, s);
    let arg = w.int_type("arg");
    let call = w.app("call", mk, arg);
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let inst = w.instantiate("inst", call, v, &[Some("f"), Some("g")]);
    let b = build(inst, w);
    assert!(
        b.ok,
        "a call-result callee resolves to its struct layout: {:?}",
        b.diagnostics()
    );
    // The instance's type is the struct layout the callee returns: the
    // shared `Struct` template.
    assert_eq!(
        b.state[inst].ty, b.state[s].ty,
        "the instance's type is the callee's struct layout"
    );
}

#[test]
fn a_call_result_callee_of_a_non_struct_type_is_a_nominal_error() {
    // `(mk (Int)){.f 1, .g 2}` with `mk = u => Int`: the forced callee is not a
    // struct.
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
fn an_instantiation_requires_a_struct_value_callee() {
    // Nominality: a tuple is not a struct — it cannot instantiate.
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let v = w.tuple("v", &[one, two]);
    let f1 = any_of(&w, "f1", w.int_type("int_ty"));
    let f2 = any_of(&w, "f2", w.int_type("int_ty"));
    let t = w.tuple("t", &[f1, f2]);
    let inst = w.instantiate("inst", t, v, &[]);
    let b = build(inst, w);
    assert!(
        !b.ok,
        "(@any : int, @any : int){{.f 1, .g 2}} must be rejected"
    );
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
    assert_eq!(ids.len(), width());
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
    let fid = function_id(&b, b.state[l].term.unwrap());
    assert!(
        b.module
            .node_value(AnyNodeId::Dynamic(param_type_cell(&b, fid)))
            .is_none(),
        "the template's parameter type must not be guessed"
    );
}

#[test]
fn partial_inference_in_a_function_template() {
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
    let a = w.unify("a", l, t);
    let b = build(a, w);
    assert!(b.ok, "the identity fits Int -> _");
    // The annotation's domain reached the template's own parameter type cell.
    let fid = function_id(&b, b.state[l].term.unwrap());
    let param_type = param_type_cell(&b, fid);
    assert!(
        b.module
            .node_value(AnyNodeId::Dynamic(param_type))
            .is_none(),
        "the signature unify leaves the parameter type open"
    );
    let return_type = b.module.functions[fid].return_type;
    assert!(
        b.module
            .node_value(AnyNodeId::Dynamic(return_type))
            .is_none(),
        "and leaves the codomain type open"
    );
}

#[test]
fn an_undecidable_array_template_length_is_guarded() {
    // [@any : int, _][1] — the length is a value now, so a placeholder
    // leaves the template undecidable.
    let w = Writer::new();
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.hole("len"));
    let e1 = w.int("e1", 1);
    let arr = w.array_inst("arr", template, &[e1]);
    let b = build(arr, w);
    assert!(!b.ok, "an undecidable template length must be refused");
    let diags = b.diagnostics();
    assert!(
        diags.iter().any(|diag| diag.kind == DiagKind::Guard),
        "the undecidable template is guarded: {diags:?}"
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
fn a_shallow_array_element_is_masked() {
    // [@any : int, 2][1, @shallow(1) 2] — the marker masks the element's
    // value slot; the type is the `Array` template.
    let w = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 2));
    let arr = w.array_inst_depths("arr", template, &[(one, 0), (two, 1)]);
    let b = build(arr, w);
    assert!(b.ok, "the shallow array should check");
    // The instance's value is the element template and the two elements.
    let value_ids = array_ids(&b, b.state[arr].val.unwrap());
    assert_eq!(value_ids.len(), 3);
    // The marked element's value slot carries the mask.
    let marked = array_ids(&b, value_ids[2]);
    let masked = b
        .module
        .node_value(AnyNodeId::Dynamic(marked[0]))
        .expect("the marked element's value slot is written");
    let mask = array_mask_from(masked);
    assert!(mask[0], "the marked element's value slot is masked");
    assert!(!mask[1], "its type slot is not masked");
    // The type is the shared `Array` template.
    assert_eq!(
        layout_marker(&b, b.state[arr].ty.unwrap()),
        TypeValue::TypeArray
    );
}

#[test]
fn shallow_marked_position_stays_lazy_until_a_read() {
    // [@any : int, 2][1, @shallow(1) (x => x + 1) 5] — the apply sits at the
    // marked position, so the deep pass skips it.
    let w = Writer::new();
    let one = w.int("one", 1);
    let five = w.int("five", 5);
    let x = w.param("x");
    let one2 = w.int("one2", 1);
    let add = w.binop("add", BinOp::Add, x, one2);
    let lam = w.lam("lam", x, add);
    let call = w.app("call", lam, five);
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 2));
    let arr = w.array_inst_depths("arr", template, &[(one, 0), (call, 1)]);
    let b = build(arr, w);
    assert!(b.ok, "the shallow array should check");
    let ids = array_ids(&b, b.state[arr].val.unwrap());
    assert_eq!(ids.len(), 3, "the element template and the two elements");
    // The unmarked element's value slot is its own value.
    let plain = array_ids(&b, ids[1]);
    assert_eq!(
        b.module.node_value(AnyNodeId::Dynamic(plain[0])),
        Some(HighProgramValue::LowValue(LowValue::USize(1))),
        "the unmarked element's value is its own"
    );
    // The marked element's value slot is the masked slot, and the element's
    // own value is under the mask.
    let marked = array_ids(&b, ids[2]);
    let masked = array_ids(&b, marked[0]);
    assert_eq!(
        masked[0],
        b.state[call].term.unwrap(),
        "the element's own value is under the mask"
    );
    // The marked position is **lazy**: the apply under the mask has not run,
    // so its answer is undecided.
    let answer = b
        .module
        .element_value_node(AnyNodeId::Dynamic(masked[0]))
        .unwrap_or(AnyNodeId::Dynamic(masked[0]));
    assert!(
        b.module
            .has_no_result_yet(answer.dynamic().expect("the masked apply is dynamic")),
        "the masked apply still owes its own run"
    );
}

#[test]
fn a_read_of_a_shallow_position_forces_the_element() {
    // [@any : int, 2][1, @shallow(1) (x => x + 1) 5][1] — the read runs the
    // apply that the deep pass deliberately skipped.
    let w = Writer::new();
    let one = w.int("one", 1);
    let five = w.int("five", 5);
    let x = w.param("x");
    let one2 = w.int("one2", 1);
    let add = w.binop("add", BinOp::Add, x, one2);
    let lam = w.lam("lam", x, add);
    let call = w.app("call", lam, five);
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 2));
    let arr = w.array_inst_depths("arr", template, &[(one, 0), (call, 1)]);
    let one_idx = w.int("one_idx", 1);
    let read = w.index("read", arr, one_idx);
    let mut b = build(read, w);
    assert!(b.ok, "the read should check");
    let value = b
        .module
        .evaluate_node_deep(b.state[read].val.unwrap(), None)
        .unwrap();
    let items = array_ids_from(value);
    assert_eq!(items.len(), width(), "the read's value is the masked slot");
    let answer = b
        .module
        .element_value_node(AnyNodeId::Dynamic(items[0]))
        .unwrap_or(AnyNodeId::Dynamic(items[0]));
    assert_eq!(
        b.module.node_value(answer),
        Some(HighProgramValue::LowValue(LowValue::USize(6))),
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
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 3));
    let arr = w.array_inst("arr", template, &[a, b2, c]);
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
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 2));
    let arr = w.array_inst("arr", template, &[a, b2]);
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
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 3));
    let arr = w.array_inst("arr", template, &[a, b2, c]);
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
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 3));
    let arr = w.array_inst("arr", template, &[a, b2, c]);
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
    let element = any_of(&w, "element", w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 3));
    let arr = w.array_inst("arr", template, &[a, b2, c]);
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
fn a_struct_template_parameter_function_reads_a_named_field() {
    // `f = (p <: A => p.x)`; `f (A{.x 1, .y 2})`.
    let w = Writer::new();
    let p = w.param("p");
    let int_x = any_of(&w, "int_x", w.int_type("int_ty"));
    let int_y = any_of(&w, "int_y", w.int_type("int_ty"));
    let template = w.struct_value("template", &[(int_x, Some("x")), (int_y, Some("y"))]);
    let constructor_parameter = w.param("constructor_parameter");
    let constructor = w.lam("constructor", constructor_parameter, template);
    let requirement = w.app("requirement", constructor, w.hole("requirement_hole"));
    let matched = w.unify("matched", p, requirement);
    let read = w.named_field("read", p, "x");
    let body = w.tuple("body", &[matched, read]);
    let f = w.lam("f", p, body);
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let fields = w.tuple("fields", &[one, two]);
    let instance = w.app("instance", constructor, w.hole("instance_hole"));
    let arg = w.instantiate("arg", instance, fields, &[Some("x"), Some("y")]);
    let call = w.app("call", f, arg);
    let b = build(call, w);
    assert!(b.ok, "f (A{{.x 1, .y 2}}) must check");
}

/// Two **separately written** occurrences of `{.x @any : int}` are two distinct
/// nominal types.
///
/// # Invariant
/// A struct value mints a fresh nominal id per source occurrence
/// (`check_struct_value`), and unification compares that id; sharing one *named*
/// struct value is what makes a template and an instance agree.
#[test]
fn two_written_struct_values_are_distinct_nominal_types() {
    let w = Writer::new();
    let int_a = any_of(&w, "int_a", w.int_type("int_ty"));
    let written_in_annotation = w.struct_value("written_in_annotation", &[(int_a, Some("x"))]);
    let int_b = any_of(&w, "int_b", w.int_type("int_ty"));
    let written_in_argument = w.struct_value("written_in_argument", &[(int_b, Some("x"))]);
    let one = w.int("one", 1);
    let fields = w.tuple("fields", &[one]);
    let arg = w.instantiate("arg", written_in_argument, fields, &[Some("x")]);
    let matched = w.unify("matched", arg, written_in_annotation);
    let b = build(matched, w);
    assert!(
        !b.ok,
        "two written occurrences of the same struct text are distinct nominal types"
    );
}

#[test]
fn an_open_struct_template_lets_its_field_be_read() {
    // `F = t => { t <: A; t.I }` with `A = {.I _, .O _}` — the template names
    // the fields, leaving their types open.

    // The parameter and argument use the same constructor's returned requirement.
    let w = Writer::new();
    let t = w.param("t");
    let open_i = w.hole("open_i");
    let open_o = w.hole("open_o");
    // **One** named struct value for both sides: two written `{.…}` are
    // distinct.
    let open_struct = w.struct_value("open_struct", &[(open_i, Some("I")), (open_o, Some("O"))]);
    let constructor_parameter = w.param("constructor_parameter");
    let constructor = w.lam("constructor", constructor_parameter, open_struct);
    let requirement = w.app("requirement", constructor, w.hole("requirement_hole"));
    let matched = w.unify("matched", t, requirement);
    let read = w.named_field("read", t, "I");
    let body = w.tuple("body", &[matched, read]);
    let f = w.lam("f", t, body);
    // The argument is an **instance** of that struct whose field values are the
    // *types* `Int` — the struct contains a type.
    let int_i = w.int_type("int_i");
    let int_o = w.int_type("int_o");
    let fields = w.tuple("fields", &[int_i, int_o]);
    let instance = w.app("instance", constructor, w.hole("instance_hole"));
    let arg = w.instantiate("arg", instance, fields, &[Some("I"), Some("O")]);
    let call = w.app("call", f, arg);
    let b = build(call, w);
    assert!(
        b.ok,
        "an open struct template must still let the body read its field"
    );
}

// --- a tuple-domain function that reads an element --------------------------

#[test]
fn a_tuple_template_parameter_function_reads_an_element() {
    // The regression `f = p => { p <: (@any : int, @any : int); p(0) }`;
    // `f (1, 2)` failed with `expected raw[?a, Int]`.
    let w = Writer::new();
    let p = w.param("p");
    let int1 = any_of(&w, "int1", w.int_type("int_ty"));
    let int2 = any_of(&w, "int2", w.int_type("int_ty"));
    let tuple_ty = w.tuple("tuple_ty", &[int1, int2]);
    let matched = w.unify("matched", p, tuple_ty);
    let zero = w.int("zero", 0);
    let read = w.field("read", p, zero);
    let body = w.tuple("body", &[matched, read]);
    let f = w.lam("f", p, body);
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let fields = w.tuple("fields", &[one, two]);
    let call = w.app("call", f, fields);
    let b = build(call, w);
    assert!(b.ok, "f (1, 2) against the tuple template must check");
    // And actually evaluate it: the apply's parameter check runs there.
    let mut module = b.module;
    let value = module.evaluate_node_deep(b.root_val, None).unwrap();
    // The body's value is the match and the read; the read is the second.
    let ids = array_ids_from(value);
    assert_eq!(ids.len(), 2, "the body's value is the match and the read");
    let read_node = module
        .element_value_node(AnyNodeId::Dynamic(ids[1]))
        .unwrap_or(AnyNodeId::Dynamic(ids[1]))
        .dynamic()
        .expect("the read's value slot");
    let read_value = module.evaluate_node_deep(read_node, None).unwrap();
    assert_eq!(common_value(&read_value), 1, "f (1, 2) yields 1");
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
    // `f = (p : Struct => p)`; `g = (a => f a)`.
    let w = Writer::new();
    let p = w.param("p");
    // The parameter's domain is the shared `Struct` layout.
    let struct_ty = w.hole("struct_ty");
    w.fill(
        struct_ty,
        ExprKind::Literal(HighProgramLiteral::from(StructTemplateLit)),
    );
    let f = w.lam_typed("f", p, Some(struct_ty), p, false);

    let a = w.param("a");
    let call = w.app("call", f, a);
    let g = w.lam("g", a, call);

    let b = build(g, w);
    assert!(b.ok, "the wrapper must check");
    let gid = function_id(&b, b.state[g].term.unwrap());
    let cell = param_type_cell(&b, gid);
    assert!(
        b.module.node_value(AnyNodeId::Dynamic(cell)).is_some(),
        "the wrapper's parameter type must be inferred from the body call"
    );
}
