//! End-to-end tests: source text → checked build → evaluation, plus the
//! diagnostics with their spans.

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::program::TypeValue;
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};

use lichen_language::diag::Stage;
use lichen_language::program::{LangProgram, LangValue};
use lichen_language::{compile, frontend};

mod common;

/// The dynamic node behind an item ref — the checker builds only dynamic graphs.
fn dyn_node(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => unreachable!("language graphs are dynamic"),
    }
}

/// Compile a program, asserting it checks; returns the module and the root
/// value node.
fn run(source: &str) -> (Module<LangProgram>, NodeId) {
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

fn evaluate(source: &str) -> LangValue {
    let (mut module, root) = run(source);
    module
        .evaluate_node_deep(root, None)
        .expect("the program's root value is undecided")
}

/// The node ids of an array value.
fn array_ids(value: LangValue) -> Vec<NodeId> {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("expected an array value, got {value:?}");
    };
    // SAFETY: the value was just produced by the module under test, whose
    // block has not been dropped.
    unsafe { array.items() }
        .iter()
        .map(|item| dyn_node(item.node))
        .collect()
}

fn usize_of(value: &LangValue) -> usize {
    let LangValue::LowValue(LowValue::USize(n)) = value else {
        panic!("expected a usize value, got {value:?}");
    };
    *n
}

/// The rendered diagnostics of a failing program.
fn diags(source: &str) -> Vec<lichen_language::Diag<lichen_language::program::LangProgram>> {
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
fn a_tuple_domain_parameter_read_checks_and_evaluates() {
    assert_eq!(
        usize_of(&evaluate("(p : <Int, Int> => p(0)) (1, 2) : Int")),
        1
    );
    // The same program written as a top-level binding + a final line — the form
    // that failed through the compiler.
    assert_eq!(
        usize_of(&evaluate("f = p : <Int, Int> => p(0)\nf (1, 2)")),
        1
    );
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
    let value = module.evaluate_node_deep(root, None).unwrap();
    let ids = array_ids(value);
    assert_eq!(ids.len(), 2, "the tuple has two elements");
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        5
    );
    assert!(
        matches!(
            module.node_value(AnyNodeId::Dynamic(ids[1])),
            Some(LangValue::TypeValue(TypeValue::TypeType))
        ),
        "the second element is the Type constant"
    );
}

#[test]
fn a_nested_function_captures_the_applied_outer_parameter() {
    // The returned closure captures the applied parameter's binding: a leak
    // would show as the undecided marker.
    let (mut module, root) = run("a = 1; f1 = x => { b = 2; f2 = y => [a, b, x, y]; f2 }; f1 3 4");
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    let expected = [1usize, 2, 3, 4];
    assert_eq!(ids.len(), expected.len());
    for (&id, &n) in ids.iter().zip(expected.iter()) {
        assert_eq!(
            module.node_value(AnyNodeId::Dynamic(id)),
            Some(LangValue::LowValue(LowValue::USize(n))),
            "element {n} must be a bound value, not the leaked parameter"
        );
    }
}

#[test]
fn an_array_literal_checks_against_its_array_type() {
    assert_eq!(array_ids(evaluate("([1, 2, 3] : array<Int, 3>)")).len(), 3);
}

#[test]
fn a_homogeneous_array_of_lambdas_checks() {
    // Each lambda has its own fresh undecided arrow type, and the element
    // check unifies the two shapes.

    // `?a → ?a` and `?b → ?b` unify, so the binder name is not part of the
    // shape.
    assert_eq!(array_ids(evaluate("[x => x, x => x]")).len(), 2);
    assert_eq!(array_ids(evaluate("[y => y, x => x]")).len(), 2);
}

#[test]
fn an_index_selects_an_element() {
    // The type side indexes the element-type list structurally, so a
    // literal index checks and selects.
    assert_eq!(usize_of(&evaluate("([1, 2, 3])[1]")), 2);
}

#[test]
fn an_index_with_a_runtime_index_selects() {
    // The index is a parameter, so the check cannot know it: the length check
    // and the selection happen at runtime.

    // The root apply is annotated to anchor its lazy result cell.
    assert_eq!(usize_of(&evaluate("((i => [10, 20][i]) 1 : Int)")), 20);
}

// --- statements and bindings -------------------------------------------------

#[test]
fn statement_bindings_check_and_evaluate() {
    // Each binding compiles its value once into the IR graph, and every use
    // of the name is that node.
    assert_eq!(usize_of(&evaluate("a = [1, 2]; b = 0; a[b]")), 1);
    assert_eq!(usize_of(&evaluate("a = 5; a")), 5);
}

#[test]
fn expression_statements_check_and_evaluate() {
    // The statements are wired into the root, so their type errors fire —
    // an annotation mismatch...
    assert_eq!(usize_of(&evaluate("5; 7")), 7);
    assert_eq!(usize_of(&evaluate("5; a = 1; a")), 1);
    let d = diags("5 : Type; 7");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Check);
    assert_eq!(d[0].check.as_ref().unwrap().kind, DiagKind::Annotation);
    // ...and an apply guard.
    let d = diags("(5 3); 7");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].check.as_ref().unwrap().kind, DiagKind::Guard);
    // The same inside a block.
    assert_eq!(usize_of(&evaluate("{5; 7}")), 7);
    let d = diags("f = x => {5 : Type; x}; (f 9 : Int)");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].check.as_ref().unwrap().kind, DiagKind::Annotation);
}

#[test]
fn a_nonterminating_binding_is_reported_an_error() {
    // The checker evaluates every top-level statement, so the
    // non-terminating `w` hits the VM depth guard.
    let report = compile("omega = x => x x\nw = omega omega\nif 0 then (w : Int) else 5");
    assert!(!report.ok(), "a non-terminating binding must not certify");
    let nonterminating: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.stage == Stage::Check
                && d.check
                    .as_ref()
                    .is_some_and(|c| c.kind == DiagKind::NonTerminating)
        })
        .collect();
    assert_eq!(nonterminating.len(), 1, "one NonTerminating diagnostic");
    assert!(
        nonterminating[0].message.contains("non-terminating"),
        "diagnostic message = {:?}",
        nonterminating[0].message
    );
}

#[test]
fn an_annotated_parameter_checks() {
    assert_eq!(usize_of(&evaluate("(x : Int => x) 5")), 5);
    assert_eq!(usize_of(&evaluate("(x : Int => x) 5 : Int")), 5);
    // Applying it at Type clashes at the apply (the parameter's pinned
    // type against the argument's).
    let d = diags("(x : Int => x) Type");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Runtime);
    assert_eq!(
        check.value_a,
        Some(LangValue::TypeValue(TypeValue::TypeInt))
    );
    // An annotated parameter in a bound function.
    assert_eq!(usize_of(&evaluate("f = x : Int => x; (f 5 : Int)")), 5);
    let d = diags("f = x : Int => x; f Type");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].check.as_ref().unwrap().kind, DiagKind::Runtime);
}

#[test]
fn an_annotated_parameter_prints_its_pinned_type() {
    // A `_` annotation renders the class it bound to (`Int`, not the raw
    // `[Int, Type]` pair).

    // The printer recognizes a cell unified into the universe class.
    assert_eq!(
        lichen_language::run::evaluate("x : Int => x").unwrap(),
        "Function: Int -> Int"
    );
    assert_eq!(lichen_language::run::evaluate("5 : _").unwrap(), "5: Int");
}

/// An annotation over an already-annotated value must render the **merged**
/// attribute set.
///
/// # Invariant
///
/// Only the runtime pair carries that width, so a renderer reading the
/// frontend's own IR stamp drops the preserved slot.
#[test]
fn a_root_annotation_renders_the_merged_attribute_tail() {
    let source = "Doc = struct<.tag Int>\nfive = 5 # 8 ? Doc(.tag 7)\nfive # 4";
    assert_eq!(
        lichen_language::run::evaluate(source).unwrap(),
        "5 # 4 ? tag = 7: Int"
    );
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
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        5
    );
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[1]))
                .as_ref()
                .unwrap()
        ),
        5
    );
}

#[test]
fn a_bound_lambda_is_still_polymorphic() {
    // The shared function node keeps per-apply fresh clones, so graph sharing
    // does not monomorphize a function.
    let (module, root) = run("a = x => x; ((a 5 : Int), (a Type : Type))");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        5
    );
    assert!(
        matches!(
            module.node_value(AnyNodeId::Dynamic(ids[1])),
            Some(LangValue::TypeValue(TypeValue::TypeType))
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
    assert_eq!(
        check.value_a,
        Some(LangValue::LowValue(LowValue::USize(5))),
        "the index"
    );
    assert_eq!(
        check.value_b,
        Some(LangValue::LowValue(LowValue::USize(2))),
        "the length"
    );
}

#[test]
fn an_unresolved_name_in_a_statement_program_is_reported() {
    let d = diags("a = 5; y");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Resolve);
    assert_eq!(d[0].message, "unresolved name 'y'");
    assert_eq!(d[0].span, Some((1, 8)));
}

// --- blocks ------------------------------------------------------------------

#[test]
fn a_block_body_checks_and_evaluates() {
    // The block is the lambda's body: its bindings resolve through the graph
    // and its final expression is the value.
    assert_eq!(usize_of(&evaluate("f = x => {y = x; y}; (f 5 : Int)")), 5);
    // The same with a block binding used by an index, the fib pattern.
    assert_eq!(
        usize_of(&evaluate("f = a => {i = 0; a[i]}; (f ([7, 8]) : Int)")),
        7
    );
}

#[test]
fn a_block_is_its_final_expression() {
    // The root is the final expression's own node — a concrete literal, so
    // no ambiguity and no extra form in the IR.
    assert_eq!(usize_of(&evaluate("{a = 5; a}")), 5);
    assert_eq!(usize_of(&evaluate("{a = 1; b = 2; a}")), 1);
}

#[test]
fn a_block_scopes_its_bindings() {
    // A block-bound name shadows an outer one inside the block, and is gone
    // after the `}`.
    assert_eq!(
        usize_of(&evaluate("a = 5; f = x => {a = x; a}; (f 9 : Int)")),
        9
    );
    assert_eq!(usize_of(&evaluate("a = 5; f = x => {a = x; a}; a")), 5);
}

#[test]
fn a_block_bound_lambda_is_still_polymorphic() {
    // `g` is one shared function node; each apply gets fresh clones, so it
    // checks at Int and at Type.
    let (module, root) =
        run("(((x => {g = y => y; ((g x : Int), (g Type : Type))}) 5) : <Int, Type>)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        5
    );
    assert!(
        matches!(
            module.node_value(AnyNodeId::Dynamic(ids[1])),
            Some(LangValue::TypeValue(TypeValue::TypeType))
        ),
        "the second element is the Type constant"
    );
}

#[test]
fn an_unresolved_name_inside_a_block_is_reported() {
    let d = diags("f = x => {a = 1; b}; 0");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Resolve);
    assert_eq!(d[0].message, "unresolved name 'b'");
    assert_eq!(d[0].span, Some((1, 18)));
}

// --- binary operators -------------------------------------------------------

#[test]
fn binary_operators_check_and_evaluate() {
    assert_eq!(usize_of(&evaluate("1 + 2")), 3);
    assert_eq!(usize_of(&evaluate("5 - 3")), 2);
    assert_eq!(usize_of(&evaluate("2 <= 1")), 0);
    assert_eq!(usize_of(&evaluate("2 <= 2")), 1);
    assert_eq!(usize_of(&evaluate("1 == 1")), 1);
    assert_eq!(usize_of(&evaluate("1 == 2")), 0);
    // A comparison's result drives a condition — there is no `Bool` value.
    assert_eq!(usize_of(&evaluate("(1 == 1) + (2 == 3)")), 1);
    // ...which is also what makes `& | ^` the language's `and`/`or`/`xor`.
    assert_eq!(usize_of(&evaluate("(1 == 1) & (2 == 3)")), 0);
    assert_eq!(usize_of(&evaluate("(1 == 1) | (2 == 3)")), 1);
    assert_eq!(usize_of(&evaluate("(1 == 1) ^ (2 == 2)")), 0);
}

/// The operators added so a program can compute something: `* / %`, the rest of
/// the comparisons, and the bitwise set.
#[test]
fn the_extended_operator_set_evaluates() {
    assert_eq!(usize_of(&evaluate("3 * 4")), 12);
    assert_eq!(usize_of(&evaluate("7 / 2")), 3);
    assert_eq!(usize_of(&evaluate("7 % 2")), 1);
    assert_eq!(usize_of(&evaluate("1 < 2")), 1);
    assert_eq!(usize_of(&evaluate("2 < 1")), 0);
    assert_eq!(usize_of(&evaluate("2 > 1")), 1);
    assert_eq!(usize_of(&evaluate("2 >= 3")), 0);
    assert_eq!(usize_of(&evaluate("1 != 2")), 1);
    assert_eq!(usize_of(&evaluate("1 != 1")), 0);
    assert_eq!(usize_of(&evaluate("6 & 3")), 2);
    assert_eq!(usize_of(&evaluate("4 | 1")), 5);
    assert_eq!(usize_of(&evaluate("5 ^ 1")), 4);
    // `!=` is the other face of the generalized equality: it works on the
    // values `==` does, two type values included.
    assert_eq!(usize_of(&evaluate("Int != string")), 1);
    assert_eq!(usize_of(&evaluate("Int != Int")), 0);
}

/// An `Int` is a machine-sized **unsigned** integer: the operators
/// that tell the two readings apart are unsigned.
///
/// # Invariant
///
/// `0 - 1` wraps, and every value from `2^63` up is reachable that way. A
/// signed `/` `%` `<` `<=` `>` `>=` — in the interpreter or in either JIT
/// backend — would agree here on every small program and disagree there.
#[test]
fn an_int_is_unsigned_where_the_two_readings_differ() {
    assert_eq!(usize_of(&evaluate("0 - 1 > 1")), 1);
    assert_eq!(usize_of(&evaluate("(0 - 1) / 2")), usize::MAX / 2);
    assert_eq!(usize_of(&evaluate("(0 - 1) % 2")), 1);
    assert_eq!(usize_of(&evaluate("(0 - 1) >= 0")), 1);
}

/// A zero divisor is refused with the lazy marker: an undecided result
/// plus the recorded reason.
///
/// # Invariant
///
/// Only the interpreter refuses. A jitted kernel has left this crate: wasm's
/// integer division traps and SPIR-V's is undefined, and a guard would cost a
/// branch on the GPU path (`docs/notes/operators.md`).
#[test]
fn a_zero_divisor_is_recorded_rather_than_answered() {
    for source in ["1 / 0", "1 % 0", "f = x => x / 0; f 5"] {
        let Err(diagnostics) = lichen_language::run::evaluate(source) else {
            panic!("{source:?} has no value");
        };
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.message.contains("operator.divide_by_zero")),
            "{source:?} should name its cause, got {diagnostics:?}"
        );
    }
    // A divisor that is merely *able* to be zero is fine: nothing is refused
    // until it is zero.
    assert_eq!(usize_of(&evaluate("(x => 10 / x) 2")), 5);
}

/// The two conversions each cross **one** way: `int2float` rounds past an
/// `f32`, `float2int` truncates toward zero.
///
/// # Invariant
///
/// A prefix operator binds tighter than `+`, so `int2float 1 + 2.0` converts
/// the `1` and not the sum (`docs/notes/floating-point.md` §4.2, §4.3).
#[test]
fn the_two_conversions_cross_in_the_direction_each_one_names() {
    for (source, expected) in [
        ("int2float 5", LangValue::LowValue(LowValue::Float(5.0))),
        (
            "int2float (1 + 2)",
            LangValue::LowValue(LowValue::Float(3.0)),
        ),
        // The conversion is the tighter level, so this is `(int2float 1) + 2.0`.
        (
            "int2float 1 + 2.0",
            LangValue::LowValue(LowValue::Float(3.0)),
        ),
        // Above 2^24 the destination cannot carry the source: the crossing
        // is a float's, and its own rounding answers it.
        (
            "int2float 16777217",
            LangValue::LowValue(LowValue::Float(16777216.0)),
        ),
        ("float2int 3.7", LangValue::LowValue(LowValue::USize(3))),
        (
            "float2int (7.0 / 2.0)",
            LangValue::LowValue(LowValue::USize(3)),
        ),
        (
            "float2int 16777216.0",
            LangValue::LowValue(LowValue::USize(16777216)),
        ),
        (
            "x = 3.7; float2int x",
            LangValue::LowValue(LowValue::USize(3)),
        ),
        (
            "(v => int2float v) 7",
            LangValue::LowValue(LowValue::Float(7.0)),
        ),
    ] {
        assert_eq!(
            evaluate(source),
            expected,
            "{source:?} answered with the wrong value"
        );
    }
}

/// Which way the conversion goes is the operator's, and the operand has to
/// agree.
///
/// # Invariant
///
/// The two classes never convert implicitly: the only way an `Int` reaches a
/// `Float` is the word that names it, and the refusal carries the same
/// expected/found shape as any other operator.
#[test]
fn a_conversion_applied_to_the_other_class_is_refused_by_name() {
    for (source, message) in [
        ("int2float 1.0", "expected Int, found Float"),
        ("float2int 5", "expected Float, found Int"),
    ] {
        let d = diags(source);
        assert_eq!(d.len(), 1, "{source:?} fails once: {d:?}");
        assert_eq!(d[0].stage, Stage::Check, "{source:?}: {d:?}");
        assert_eq!(d[0].message, message, "{source:?}: {d:?}");
        let check = d[0].check.as_ref().expect("a checker diagnostic");
        assert_eq!(check.kind, DiagKind::Conv, "{source:?}: {d:?}");
    }
}

/// A `float2int` with no `Int` to truncate toward is a **run-time** refusal,
/// recorded the way a zero divisor is.
///
/// # Invariant
///
/// Being in range is a fact about the value, not its type, so the checker
/// cannot see it: the three cases are a `NaN`, an infinity, and a
/// negative — the ones the unsigned `Int` cannot name (`floating-point.md`
/// §4.3).
///
/// Only the interpreter refuses. A jitted kernel has left this crate:
/// wasm's `i64.trunc_f32_u` traps and SPIR-V's `OpConvertFToU` is
/// undefined (`operators.md`).
#[test]
fn a_float_with_no_int_to_truncate_toward_is_recorded_rather_than_answered() {
    for source in [
        "float2int (0.0 / 0.0)",
        "float2int (1.0 / 0.0)",
        "float2int (0.0 - 3.7)",
    ] {
        let Err(diagnostics) = lichen_language::run::evaluate(source) else {
            panic!("{source:?} has no Int to truncate toward");
        };
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.message.contains("operator.out_of_range")),
            "{source:?} should name its cause, got {diagnostics:?}"
        );
    }
    // A float that merely *could* be out of range is fine: nothing is refused
    // until it is.
    assert_eq!(usize_of(&evaluate("(x => float2int x) 4.5")), 4);
}

/// Two tokens are both a bracket and a comparison; the grammar tells them
/// apart by the *shape* around them, not a mode.
#[test]
fn comparisons_share_their_tokens_with_the_angle_bracket_forms() {
    // An expression before the token, and an expression after it: a comparison.
    assert_eq!(usize_of(&evaluate("2 > 1")), 1);
    assert_eq!(usize_of(&evaluate("2 >= 2")), 1);
    assert_eq!(usize_of(&evaluate("1 < 2 > 0")), 1); // (1 < 2) > 0
    // An angle bracket over a *tuple type* wins: the application is the
    // tighter reading.
    assert_eq!(
        usize_of(&evaluate("f = x => x<1>; f <Int, string> == string")),
        1
    );
    // A `>` with no expression after it closes the bracket it is in, so every
    // angle-bracket form still parses.

    // The struct form states the *tuple* kind, so a struct type value is read
    // by name (`::a`) rather than positionally.
    assert_eq!(usize_of(&evaluate("<Int, string><1> == string")), 1);
    assert_eq!(
        usize_of(&evaluate("struct<.a Int, .b string>::a == Int")),
        1
    );
    assert_eq!(usize_of(&evaluate("a = <Int, string>; a<0> == Int")), 1);
    // The glued `(` of an instantiation belongs to the angle form, so the `>`
    // before it is still a closer.

    // The field is read by name `s.a`; the positional `s(0)` is the tuple read.
    assert_eq!(
        usize_of(&evaluate(
            "s = struct<.a Int, .b string>(1, \"a\"); s.a == 1"
        )),
        1
    );
    // The keyword-led array type's `>` closes as before.
    let (module, value, _) = common::evaluate("[1, 2] : array<Int, 2>");
    assert_eq!(common::usize_array(&module, &value), vec![1, 2]);
    // A `<` glued to the previous token is still the raw component read, so
    // a comparison needs a space before it.
    assert_eq!(
        usize_of(&evaluate("f = x => x; f (<Int, string>)<0> == Int")),
        1
    );
}

#[test]
fn operator_precedence_and_associativity() {
    // Arithmetic binds tighter than comparison; both are left-associative.

    assert_eq!(usize_of(&evaluate("1 + 2 <= 3")), 1); // (1 + 2) <= 3
    assert_eq!(usize_of(&evaluate("5 - 3 - 1")), 1); // (5 - 3) - 1
    // Application binds tighter than arithmetic: f x + 1 = (f x) + 1.

    assert_eq!(usize_of(&evaluate("f = x => x; f 5 + 1 : Int")), 6);
    // `->` keeps its place in the precedence ladder (looser than `+`).

    assert_eq!(
        lichen_language::run::evaluate("x => x + 1").unwrap(),
        "Function: Int -> Int"
    );
    // Each new level is checked against the reading that would come out
    // wrong in the wrong place.
    assert_eq!(usize_of(&evaluate("1 + 2 * 3")), 7);
    assert_eq!(usize_of(&evaluate("8 / 4 / 2")), 1);
    assert_eq!(usize_of(&evaluate("1 | 2 ^ 3 & 1")), 3); // 1 | (2 ^ (3 & 1))
    assert_eq!(usize_of(&evaluate("1 & 3 == 1")), 1); // (1 & 3) == 1
    assert_eq!(usize_of(&evaluate("1 < 2 == 1")), 1); // (1 < 2) == 1
}

#[test]
fn an_operator_operand_must_be_an_int() {
    // A concrete non-Int operand is a check error.
    let d = diags("1 + Int");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::BinOp);
    // A lambda is not an Int either.
    let d = diags("(x => x) <= 1");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::BinOp);
    // An undecided operand is pinned to Int, so applying at a non-Int is
    // a runtime failure, not a panic.
    let d = diags("f = x => x + 1; f Type");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Runtime);
}

// --- `if` --------------------------------------------------------------------

#[test]
fn if_selects_a_branch() {
    assert_eq!(usize_of(&evaluate("if 1 then 2 else 3")), 2);
    assert_eq!(usize_of(&evaluate("if 0 then 2 else 3")), 3);
    assert_eq!(usize_of(&evaluate("if 2 <= 1 then 2 else 3")), 3);
    assert_eq!(usize_of(&evaluate("if 1 <= 2 then 2 else 3")), 2);
    // The branches are one expression: if as an argument, as a lambda body.
    assert_eq!(usize_of(&evaluate("(x => if x then 1 else 0) 1")), 1);
    // An out-of-range condition is an out-of-bounds index at runtime.
    let d = diags("if 5 then 1 else 2");
    assert_eq!(d.len(), 1);
    assert_eq!(
        d[0].check.as_ref().unwrap().kind,
        DiagKind::IndexOutOfBounds
    );
}

#[test]
fn an_at_assert_prefix_asserts_and_evaluates() {
    // A passing assert keeps the condition's own value (it checks, not
    // replaces); the condition's type is the assert's type.
    assert_eq!(usize_of(&evaluate("@assert (1 == 1)")), 1);
    assert_eq!(usize_of(&evaluate("@assert (1 <= 2)")), 1);
    // `@assert` binds tighter than the comparison: `@assert 1 == 1` is `(@assert 1) == 1`.
    assert_eq!(usize_of(&evaluate("@assert 1 == 1")), 1);
}

#[test]
fn a_failed_assert_is_a_checker_diagnostic() {
    // `@assert (1 == 2)` — the condition resolves to 0, not 1: a failed assert, not
    // a unification failure, and not a panic.
    let d = diags("@assert (1 == 2)");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Check);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Assert);
    assert_eq!(
        check.assert_value,
        Some(LangValue::LowValue(LowValue::USize(0))),
        "the resolved condition value"
    );
    assert_eq!(d[0].message, "assertion failed: expected 1, found 0");
    assert_eq!(d[0].span, Some((1, 1)), "the caret is on the `!`");
}

#[test]
fn an_assert_on_a_non_one_value_fails() {
    // `!2` asserts that the literal's own value is `USize(1)` — it is 2.
    let d = diags("@assert 2");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Assert);
    assert_eq!(d[0].message, "assertion failed: expected 1, found 2");
}

#[test]
fn an_assert_on_a_failed_read_fails_with_none() {
    // `!([1, 2][5])` — the condition is a failed read, so its residue is the
    // computed-nothing value: the assert FAILS.
    let d = diags("@assert ([1, 2][5])");
    assert!(
        d.iter().any(|d| {
            d.check.as_ref().is_some_and(|c| c.kind == DiagKind::Assert)
                && d.message == "assertion failed: expected 1, found none"
        }),
        "the assert fails on the computed-nothing value: {d:?}"
    );
}

#[test]
fn an_assert_in_a_function_body_checks_per_call() {
    // The body's assert cannot resolve at normalize, so the apply clones
    // it and re-checks against the argument.
    let d = diags("f = x => @assert (x == 1); f 2");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Check);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Assert);
    assert_eq!(d[0].message, "assertion failed: expected 1, found 0");
    // The failure is inside the apply's clone of the body's assert, so
    // it is attributed through the clone's template.
    assert!(
        check.loc().is_some(),
        "the clone is attributed to its template's expression"
    );
    assert_eq!(
        d[0].span.map(|(line, _)| line),
        Some(1),
        "and the rendered caret is on the line the assert was written"
    );

    // A satisfying argument passes.
    assert!(
        lichen_language::run::evaluate("f = x => @assert (x == 1); f 1").is_ok(),
        "f 1 must satisfy the body assert"
    );
}

// --- recursion ---------------------------------------------------------------

#[test]
fn a_recursive_function_checks_and_evaluates() {
    // The countdown: f(n) = if n <= 0 then 0 else f(n-1).
    assert_eq!(
        usize_of(&evaluate("f = n => if n <= 0 then 0 else f (n - 1); f 5")),
        0
    );
    // Fibonacci: the recursion example.
    assert_eq!(
        usize_of(&evaluate(
            "fib = n => if n <= 1 then n else fib (n - 1) + fib (n - 2); fib 10"
        )),
        55
    );
}

#[test]
fn a_recursive_binding_parameter_can_be_annotated() {
    // `n : Int => e` desugars to `(n => e) : (Int -> _)`, and the `_`
    // codomain binds lazily.
    assert_eq!(
        usize_of(&evaluate(
            "f = n : Int => if n <= 0 then 0 else f (n - 1); f 5 : Int"
        )),
        0
    );
    // A wrong argument type is a runtime apply failure, not a panic.
    let d = diags("f = n => if n <= 0 then 0 else f (n - 1); f Int");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].check.as_ref().unwrap().kind, DiagKind::Runtime);
}

#[test]
fn a_recursive_binding_inside_a_block_recurses() {
    // g recurses without capturing the enclosing parameter.
    assert_eq!(
        usize_of(&evaluate(
            "f = y => {g = z => if z <= 0 then 0 else g (z - 1); g 3}; f 5"
        )),
        0
    );
}

#[test]
fn a_blockwide_binding_need_not_be_a_lambda() {
    // A block-wide binding may be any value, so `a = a` resolves `a` to
    // itself rather than to a "must be a lambda" error.
    let report = compile("a = a; a");
    assert!(
        report.ok(),
        "expected no diagnostic: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_non_terminating_recursive_function_is_reported_at_the_guard() {
    // No base case: the definition pass runs the recursion forever, and the
    // VM's application-depth guard refuses the walk.
    let report = compile("f = n => f n; f 3");
    let nonterminating: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.stage == Stage::Check
                && d.check
                    .as_ref()
                    .is_some_and(|c| c.kind == DiagKind::NonTerminating)
        })
        .collect();
    assert_eq!(nonterminating.len(), 1, "one NonTerminating diagnostic");
    assert!(
        nonterminating[0].message.contains("500"),
        "the message must name the exceeded budget's limit: {:?}",
        nonterminating[0].message
    );
}

// --- block-wide visibility --------------------------------------------------

#[test]
fn mutually_recursive_functions_check() {
    // Block-wide visibility lets either reference the other, and the
    // checker totalizes the cycle.

    // Sibling template scopes are disjoint, so the runtime descends in
    // place — see the evaluate test below.
    let report = compile(
        "f = n => if n <= 0 then 0 else g (n - 1);
         g = n => if n <= 0 then 0 else f (n - 1);
         f",
    );
    assert!(
        report.ok(),
        "expected mutual recursion to check: {:?}",
        report.diagnostics
    );
}

#[test]
fn mutually_recursive_functions_evaluate_in_place() {
    // Sibling template scopes are disjoint, so the apply clone
    // references the peer in place, not a clone per level.

    // That is what lets the recursion terminate with exactly two templates.
    let (mut module, root) = run("f = n => if n <= 0 then 0 else g (n - 1);
         g = n => if n <= 0 then 0 else f (n - 1);
         f 5");
    assert_eq!(usize_of(&module.evaluate_node_deep(root, None).unwrap()), 0);
    assert_eq!(module.functions.len(), 2, "peers are referenced in place");
}

#[test]
fn a_binding_can_forward_reference_a_later_block_wide_binding() {
    // Block-wide names are entered before any value compiles, so a forward
    // reference resolves and `a` aliases `b`'s node.
    assert_eq!(usize_of(&evaluate("a = b; b = [1, 2]; a[0]")), 1);
}

#[test]
fn a_let_binding_is_visible_only_to_later_statements() {
    // `let a = a` is restrictive: the value compiles before the name
    // enters scope, so `a` resolves to the block-wide `a`.
    assert_eq!(usize_of(&evaluate("a = 5; let a = a; a")), 5);
    // With no outer binding, `let a = a` is a resolve error (the name is not
    // visible to its own value).
    let d = diags("let a = a; a");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Resolve);
    assert_eq!(d[0].message, "unresolved name 'a'");
}

#[test]
fn a_self_referential_array_checks_without_overflow() {
    // A non-lambda self-reference: the checker cuts the cycle with a
    // skeleton pair, and the knot is benign.
    let report = compile("a = [a]; a");
    // It may report a diagnostic, but must never panic: the checker's
    // cycle cut totalizes the IR term.
    if let Some(s) = report.diagnostics.first() {
        assert_eq!(s.stage, Stage::Resolve, "{s:?}");
    }
}

#[test]
fn a_self_nested_struct_checks_without_overflow() {
    // The checker cuts the type-level cycle: a struct is a nominal type, not a
    // value, so its nominal id is allocated once.
    let report = compile("s = struct<.f s>; s");
    if let Some(s) = report.diagnostics.first() {
        assert_eq!(s.stage, Stage::Resolve, "{s:?}");
    }
}

#[test]
fn a_self_referential_field_read_checks_without_overflow() {
    // The frontend transplants the value's kind into the binding's
    // placeholder, so a block root may be any expression kind.

    // The checker's cycle cut gates on block-root membership alone.
    for source in ["a = a + 1; a", "a = a(0); a", "a = a.x; a", "a = a::x; a"] {
        let (mut module, root) = run(source);
        assert!(
            module.evaluate_node_deep(root, None).is_none(),
            "{source:?} must stay undecided like the control"
        );
    }
}

#[test]
fn a_self_referential_record_checks_without_overflow() {
    // A self-reference through a record block — the fourth kind the old skeleton
    // gate missed.

    // The record value is concrete, so the deep evaluation terminates on
    // the runtime cycle guard.
    let (mut module, root) = run("a = {x = a}; a");
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 1, "the record carries its one field");
    assert!(
        matches!(
            module.node_value(AnyNodeId::Dynamic(ids[0])),
            Some(LangValue::LowValue(LowValue::Array(_)))
        ),
        "the field is the record itself (the evaluated knot)"
    );
}

#[test]
fn a_deep_operator_chain_compiles_without_an_overflow() {
    // `1+1+…` is flat in the token stream but left-nested in the AST, so
    // the frontend's walks recurse once per term.

    // A bracket test cannot pin these walks: the parser recurses first,
    // on its own fixed-size worker.
    const TERMS: usize = 2000;
    let report = compile(&("1+".repeat(TERMS) + "1"));
    assert!(
        report.ok(),
        "expected the deep chain to check, got: {:?}",
        report.diagnostics
    );
}

// --- struct types ------------------------------------------------------------

#[test]
fn a_struct_type_kinds_and_evaluates() {
    // A bare struct type is a well-typed program: the pair
    // `[[Int, Int], [TypeId(n), Type]]`.
    run("struct<.f Int, .g Int>");
}

#[test]
fn a_bound_struct_type_is_reusable() {
    // The element check unifies the two uses, and they are the *same*
    // compiled node.

    // The checker compiles each expression once, so the nominal id survives.
    let (module, root) = run("s = struct<.f Int>; [s, s]");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    for id in ids {
        assert!(matches!(
            module.node_value(AnyNodeId::Dynamic(id)),
            Some(LangValue::LowValue(LowValue::Array(_)))
        ));
    }
}

#[test]
fn two_struct_type_occurrences_do_not_unify() {
    // Nominal identity is type-level: a struct type's kind is
    // `[[TypeId(n), names, names_in_order], TypeStruct]`.

    // Two distinct occurrences have different ids, so an array of two is
    // heterogeneous and rejected.
    let report = compile("[struct<.f Int>, struct<.f Int>]");
    assert!(
        !report.ok(),
        "two distinct struct occurrences are different nominal types: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_struct_type_in_a_function_body_is_shared_across_applications() {
    // The `Fresh` node does not read the parameter, so the deep pass
    // evaluates it to a concrete `TypeId`.

    // The apply clone therefore references the node in place: every
    // application of `f` shares one nominal id.
    let report = compile("f = t => struct<.f t>; [f (Int), f (Int)]");
    assert!(
        report.ok(),
        "a body-local struct must be one nominal type across applications: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_polymorphic_struct_constructor_shares_one_nominal_kind() {
    // The `Fresh` id is per *occurrence* and is shared, so all applications
    // of `Box` resolve to one nominal kind.

    // The id lives in the kind slot, the field types in the value
    // shape, so the same constructor still checks.
    let report = compile("Box = t => struct<.f t>; [Box (Int), Box (Int)]");
    assert!(
        report.ok(),
        "same constructor, same fields: {:?}",
        report.diagnostics
    );
    let report = compile("Box = t => struct<.f t>; [Box (Int), Box (Type)]");
    assert!(
        report.ok(),
        "same constructor (one nominal kind), fields differ only in the value: {:?}",
        report.diagnostics
    );
}

#[test]
fn an_applied_struct_constructor_keeps_the_occurrence_identity() {
    // A named-field struct type inside a function body; its identity
    // is decided when the occurrence is checked.

    // Both halves matter — the nullary `Fresh` node and the name table
    // (`function-type-merge.md`).
    let (module, root) = run("A = I => struct<.n Int, .I I>\n\
         In = struct<.x _, .y _>\n\
         S1 = A In\n\
         S2 = A In\n\
         x = S1(.n 3, .I In(.x 10, .y 20))\n\
         y = (x : S2)\n\
         y");
    let mut module = module;
    let instance = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(instance.len(), 2, "the instance wraps its two field values");
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(instance[0]))
                .as_ref()
                .unwrap()
        ),
        3
    );
    let inner = array_ids(module.evaluate_node_deep(instance[1], None).unwrap());
    assert_eq!(inner.len(), 2, "the inner struct wraps its own two fields");
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(inner[0]))
                .as_ref()
                .unwrap()
        ),
        10
    );
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(inner[1]))
                .as_ref()
                .unwrap()
        ),
        20
    );
    // The id is the *occurrence*, not the instantiation.
    let d = diags(
        "A = I => struct<.n Int, .I I>\n\
         S1 = A Int\n\
         S2 = A Float\n\
         x = S1(.n 3, .I 5)\n\
         y = (x : S2)\n\
         y",
    );
    assert_eq!(d.len(), 1, "different field types must not unify: {d:?}");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
    // The annotation sits in the callee's body, so the two types meet at
    // the apply-time parameter check.

    // A pinned id alone does not fix this row — the copied name table is
    // what conflicts.
    let report = compile(
        "A = I => struct<.n I>\n\
         S1 = A Int\n\
         S2 = A Int\n\
         f = v => (v : S2)\n\
         f S1(.n 3)",
    );
    assert!(
        report.ok(),
        "one occurrence, two evaluations, at apply time: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_named_struct_field_read_resolves_to_the_positional_index() {
    // `A = struct<.x Int, .y Type>` carries a name→index table; `a.x`
    // reads field `x` (index 0), `a.y` field `y` (index 1).
    let v = evaluate("A = struct<.x Int, .y Type>; a = A(1, Int); a.x");
    assert_eq!(v, LangValue::LowValue(LowValue::USize(1)));
    let v = evaluate("A = struct<.x Int, .y Type>; a = A(1, Int); a.y");
    assert_eq!(v, LangValue::TypeValue(TypeValue::TypeInt));
}

/// A field read's **class** is decided wherever its container's type is.
///
/// # Invariant
///
/// A class question is asked of a *cell*, which cannot see through an
/// unevaluated `Index`, so both the named form (a struct) and the positional
/// form (a tuple) resolve the field's type out of the container type's own
/// field list.
#[test]
fn a_field_reads_class_is_decided_where_the_container_type_is() {
    let named = evaluate(
        "A = struct<.n Int, .alpha Float>\n\
         f = (x : A) => x.alpha + x.alpha\n\
         f (A(.n 1, .alpha 0.5))",
    );
    assert_eq!(named, LangValue::LowValue(LowValue::Float(1.0)));
    let positional = evaluate(
        "f = (x : <Int, Float>) => x(1) + x(1)\n\
         f (1, 0.5)",
    );
    assert_eq!(positional, LangValue::LowValue(LowValue::Float(1.0)));
}

#[test]
fn a_named_field_read_on_a_missing_field_is_rejected() {
    // `a.z` on a struct that has no field `z` is a reported type error (a
    // named-field miss), not a runtime panic.
    let d = diags("A = struct<.x Int>; a = A(1,); a.z");
    let check = d[0]
        .check
        .as_ref()
        .expect("a named-field miss is a checker diagnostic");
    assert_eq!(check.kind, DiagKind::NamedField);
}

#[test]
fn a_named_field_miss_suggests_a_close_field() {
    // The message appends the actually-close field name, so the editor can
    // suggest a fix and power field completion.
    let d = diags("A = struct<.x Int, .sub Int>; a = A(1, 2); a.sux");
    let msg = &d[0].message;
    assert!(
        msg.contains("no field") && msg.contains("did you mean 'sub'?"),
        "a named-field miss should suggest the close field, got {msg}"
    );
}

#[test]
fn a_named_field_read_on_a_non_struct_is_rejected() {
    // The read states that the container's *kind* must be a struct kind, and
    // an atomic type's kind is `Type`.
    let d = diags("a = 1; a.b");
    let check = d[0]
        .check
        .as_ref()
        .expect("a named-field read on an int is a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Guard);
    assert_eq!(d[0].message, "expected TypeStruct, found Type");
}

#[test]
fn a_raw_named_read_requires_a_type_struct_container() {
    // `X::a` requires a TypeStruct container kind, the counterpart of
    // what `X<e>` states.
    let d = diags("a = 5; a::x");
    let check = d[0]
        .check
        .as_ref()
        .expect("a raw named read on a non-struct is a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Guard);
    assert_eq!(d[0].message, "expected TypeStruct, found Int");
}

#[test]
fn a_raw_read_of_a_type_value_reads_the_components_pair() {
    // `X<e>` reads a component of a *type-as-value*, whose elements are
    // themselves `[value, type]` pairs.

    // The positional read yields the element's value and its type.
    assert_eq!(
        evaluate("<Int, string><0>"),
        LangValue::TypeValue(TypeValue::TypeInt)
    );
    assert_eq!(usize_of(&evaluate("<Int, string><0> == Int")), 1);
    // A struct type value states the *tuple* kind for `X<e>`, so its
    // components are read by name (`X::a`).
    assert_eq!(
        usize_of(&evaluate("struct<.a Int, .b string>::b == string")),
        1
    );
    // The element's own *value* slot, which a `Type`-valued element carries.
    assert_eq!(usize_of(&evaluate("<Int, string><1> == string")), 1);
    // An undecided container stays lazy and resolves at the apply.
    assert_eq!(
        usize_of(&evaluate("f = k => k<0>; f <Int, string> == Int")),
        1
    );
}

#[test]
fn a_raw_read_of_a_non_tuple_container_is_refused_by_kind() {
    // `X<e>` reads a *type value* component, so the container's type
    // must be the tuple kind.
    let d = diags("[1, 2]<0>");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Guard);
    assert_eq!(d[0].message, "expected TypeTuple, found array<Int, 2>");
    // A span, too — the container is what the user wrote.
    assert_eq!(d[0].span, Some((1, 1)));
    // The same read through a bound name is refused at the name.
    let d = diags("x = [1, 2]; x<0>");
    assert_eq!(d[0].message, "expected TypeTuple, found array<Int, 2>");
    assert_eq!(d[0].span, Some((1, 5)));
    // A deferred parameter states the same requirement at the apply
    // that binds it.
    let d = diags("f = k => k<0>; f [1, 2]");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Runtime);
    assert_eq!(d[0].message, "expected TypeTuple, found array<Int, 2>");
    assert_eq!(d[0].span, Some((1, 18)));
    // A scalar is not a container at all, and gets the same kind refusal.
    let d = diags("5<0>");
    assert_eq!(d[0].message, "expected TypeTuple, found Int");
    assert_eq!(d[0].span, Some((1, 1)));
}

#[test]
fn a_raw_named_read_yields_the_field_type() {
    // `S::a` reads field `a`'s *type* as a value; `s.a` reads field `a`'s
    // *value* from the instance.
    assert_eq!(
        evaluate("S = struct<.a Int, .b string>; S::a"),
        LangValue::TypeValue(TypeValue::TypeInt)
    );
    assert_eq!(
        evaluate("S = struct<.a Int, .b string>; S::b"),
        LangValue::TypeValue(TypeValue::TypeString)
    );
    assert_eq!(
        usize_of(&evaluate(
            "S = struct<.a Int, .b string>; s = S(.a 1, .b \"h\"); s.a"
        )),
        1
    );
    // The instance read is an Int, so `s.a == 1` is a valid int comparison.
    assert_eq!(
        usize_of(&evaluate(
            "S = struct<.a Int, .b string>; s = S(.a 1, .b \"h\"); s.a == 1"
        )),
        1
    );
    // `==` is generalized: a `Type`-typed value compares against a type
    // constant by value.
    assert_eq!(
        usize_of(&evaluate("S = struct<.a Int, .b string>; S::a == Int")),
        1
    );
    assert_eq!(
        usize_of(&evaluate("S = struct<.a Int, .b string>; S::a == string")),
        0
    );
    // Comparing values of different types is still rejected.
    assert!(
        diags("S = struct<.a Int, .b string>; S::a == 1")
            .iter()
            .any(|d| d.check.as_ref().is_some_and(|c| c.kind == DiagKind::BinOp))
    );
}

#[test]
fn a_raw_read_of_a_deferred_non_tuple_is_refused_at_the_application() {
    // The container is a parameter, so the tuple-kind requirement
    // is deferred to the apply that binds it.
    let d = diags("f = s => s<0>; f (1)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Runtime);
    assert_eq!(d[0].message, "expected TypeTuple, found Int");
    assert_eq!(d[0].span, Some((1, 19)), "the caret is on the argument");
}

#[test]
fn a_raw_read_whose_subscript_is_not_an_index_reports_a_runtime_subscript_error() {
    // A string subscript is not a check-time diagnostic: the lowlevel
    // records it as `RuntimeIndexSubscript`.

    // The caret is on the subscript, the one node the raw read gives an edge.
    let d = diags("a = <Int, string>\ni = \"x\"\na<i>");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::RuntimeIndexSubscript);
    assert_eq!(d[0].span, Some((2, 5)), "the caret is on the subscript `i`");
}

#[test]
fn an_apply_of_a_deferred_non_function_reports_a_runtime_apply_target_error() {
    // The callee is a parameter, so the function-ness guard is skipped
    // and the lowlevel records `RuntimeApplyTarget`.
    let report = compile("f = g => g 1\nf 5");
    assert!(
        !report.ok(),
        "an apply of a non-function must not be accepted"
    );
    assert_eq!(report.diagnostics.len(), 1);
    let check = report.diagnostics[0]
        .check
        .as_ref()
        .expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::RuntimeApplyTarget);
    assert_eq!(
        report.diagnostics[0].message,
        "this value is not a function — it cannot be applied"
    );
}

#[test]
fn an_apply_of_a_deferred_struct_value_reports_a_runtime_apply_target_error() {
    // The instance is a `LowValue::Array` — a compute kernel's
    // `[native, sig]` pair looks the same here.

    // Only the program knows which values are callable
    // (`Program::is_callable`), so its answer refuses this one.
    let report = compile("S = struct<.a Int>\nf = g => g 1\nf S(.a 1)");
    assert!(
        !report.ok(),
        "an apply of a struct value must not be accepted: {:?}",
        report.diagnostics
    );
    assert_eq!(report.diagnostics.len(), 1);
    let check = report.diagnostics[0]
        .check
        .as_ref()
        .expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::RuntimeApplyTarget);
}

#[test]
fn struct_occurrences_in_distinct_bodies_keep_distinct_ids() {
    // Each body's `Fresh` node is its own, so the nominal ids stay
    // distinct and `[f (Int), g (Int)]` is heterogeneous.
    let report = compile("f = t => struct<.f t>; g = t => struct<.f t>; [f (Int), g (Int)]");
    assert!(
        !report.ok(),
        "distinct body-local structs must keep distinct nominal ids: {:?}",
        report.diagnostics
    );
}

#[test]
fn an_annotation_against_a_struct_type_conflicts() {
    // An annotation compares whole type expressions, so a literal's
    // int type is not the struct type; `s(1, 2)` instantiates.
    let d = diags("5 : struct<.f Int>");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
}

#[test]
fn a_struct_type_application_is_an_instance() {
    // Applied to a positional tuple, the struct type compiles to
    // Instantiate; the element types meet the field types.
    let (module, root) = run("struct<.f Int, .g Int>(1, 2)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    // a bound struct type instantiates the same way
    let (module, root) = run("s = struct<.f Int, .g Int>; s(1, 2)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
}

#[test]
fn a_struct_instance_with_mismatched_fields_is_rejected() {
    // arity: two fields, one value
    let d = diags("struct<.f Int>(1, 2)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
    // field types: the tuple's Ints are not Type
    let d = diags("struct<.f Type, .g Type>(1, 2)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
    // a different source occurrence is a different nominal type
    let d = diags("s1 = struct<.f Int, .g Int>; s2 = struct<.f Int, .g Int>; [s1(1, 2), s2(1, 2)]");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::ArrayElement);
    assert!(
        d[0].message.contains("struct<.f Int, .g Int>#"),
        "the two struct occurrences must keep distinct nominal ids: {}",
        d[0].message
    );
}

#[test]
fn a_struct_instance_reads_its_fields_by_name() {
    // A named read over an instance reads the wrapped tuple's elements,
    // each typed by the corresponding field type.
    let (module, root) = run("s = struct<.f Int, .t Type>; a = s(1, Int); (a.f, a.t)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        1
    );
    assert_eq!(
        module.node_value(AnyNodeId::Dynamic(ids[1])),
        Some(LangValue::TypeValue(TypeValue::TypeInt)),
        "the second field is the `Int` type constant"
    );
    // A named read through a parameter: its type is `Index(shape, k)`
    // over the container shape, indexed from the name table.

    // The argument is parenthesized: `f s(1, Int)` would parse as
    // `(f s)(1, Int)`.
    let (module, root) = run("f = a => a.f; s = struct<.f Int, .t Type>; f (s(1, Int)) : Int");
    let mut module = module;
    assert_eq!(usize_of(&module.evaluate_node_deep(root, None).unwrap()), 1);
}

#[test]
fn a_positional_read_of_a_struct_instance_is_refused() {
    // The paren read is the *tuple* read, so the positional form on a
    // struct instance is refused.

    // The requirement is the open tuple type the container would have to be,
    // and the caret is the container.
    let d = diags("s = struct<.f Int, .t Type>; a = s(1, Int); a(0)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Guard);
    assert!(
        d[0].message.contains("expected <?a, …>")
            && d[0].message.contains("struct<.f Int, .t Type>"),
        "the requirement is the open tuple type, the found side the struct: {}",
        d[0].message
    );
    assert_eq!(
        d[0].span,
        Some((1, 34)),
        "the caret lands on the container the read resolves to"
    );
}

#[test]
fn a_named_struct_instantiation_reorders_arguments() {
    // The named arguments are reordered to the definition's positional order.
    let (module, root) = run("S = struct<.x Int, .y Type>; a = S(.y Int, .x 1); (a.x, a.y)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        1,
        "a.x reads the reordered .x field"
    );
    assert_eq!(
        module.node_value(AnyNodeId::Dynamic(ids[1])),
        Some(LangValue::TypeValue(TypeValue::TypeInt)),
        "a.y reads the reordered .y field"
    );
}

#[test]
fn a_named_struct_instantiation_in_definition_order() {
    // S(.x 1, .y Int) — the named arguments in definition order evaluate to
    // the positional tuple (1, Int).
    let value = evaluate("S = struct<.x Int, .y Type>; S(.x 1, .y Int)");
    let ids = array_ids(value);
    assert_eq!(ids.len(), 2);
}

#[test]
fn a_named_struct_instantiation_mixes_positional_and_named() {
    // A bare positional argument fills the lowest-numbered unclaimed
    // definition position.
    let (module, root) = run("S = struct<.x Int, .y Type>; a = S(.y Int, 1); (a.x, a.y)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        1
    );
    assert_eq!(
        module.node_value(AnyNodeId::Dynamic(ids[1])),
        Some(LangValue::TypeValue(TypeValue::TypeInt))
    );
}

#[test]
fn a_named_struct_instantiation_against_an_unknown_field_is_rejected() {
    let d = diags("S = struct<.x Int, .y Type>; S(.z 1, .x 1, .y Int)");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::StructUnknownField);
    assert_eq!(check.field.as_deref(), Some("z"), "the unknown field name");
    assert!(
        d[0].message.contains("z"),
        "the message names the unknown field: {}",
        d[0].message
    );
}

#[test]
fn a_named_struct_instantiation_against_a_duplicate_field_is_rejected() {
    let d = diags("S = struct<.x Int, .y Type>; S(.x 1, .x 2, .y Int)");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::StructDuplicateField);
    assert_eq!(check.field.as_deref(), Some("x"));
}

#[test]
fn a_named_struct_instantiation_against_a_missing_field_is_rejected() {
    let d = diags("S = struct<.x Int, .y Type>; S(.x 1)");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::StructMissingField);
    assert_eq!(check.field.as_deref(), Some("y"), "the missing field name");
}

#[test]
fn a_named_struct_instantiation_against_an_anonymous_struct_is_rejected() {
    // Every struct field must carry a name, because the positional `a(k)` form
    // is the tuple read.
    let d = diags("S = struct<Int, Type>; S(.x 1, .y Int)");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::StructFieldName);
}

#[test]
fn a_named_struct_instantiation_with_a_wrong_field_type_is_rejected() {
    // After reordering, the element types are checked against the field
    // list, so a mismatch is an annotation failure.
    let d = diags("S = struct<.x Int, .y Type>; S(.x Type, .y Int)");
    assert_eq!(
        d[0].check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::Annotation
    );
}

#[test]
fn a_named_struct_instantiation_reads_through_a_parameter() {
    // Lazy resolution through a parameter: the field resolves through the struct's
    // name table at the call.
    let report = compile(
        "S = struct<.x Int, .y Type>; a = S(.y Int, .x 1); apply = s => s.y; apply (a) : Type",
    );
    assert!(
        report.ok(),
        "a named-instantiation field read through a parameter must check: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_lazy_named_read_over_an_anonymous_struct_is_refused() {
    // The positional struct is refused at its definition now, before the lazy
    // read ever resolves: its fields carry no names.

    // The refusal is a check diagnostic, never a panic and never a false
    // non-termination report from the read that follows.
    let d = diags("S = struct<Int, Type>\na = S(1, Int)\napply = s => s.x\napply (a)");
    assert!(
        d.iter().any(|d| d
            .check
            .as_ref()
            .is_some_and(|c| c.kind == DiagKind::StructFieldName)),
        "the unnamed struct field is the refusal: {d:?}"
    );
    assert!(
        !d.iter().any(|d| d
            .check
            .as_ref()
            .is_some_and(|c| c.kind == DiagKind::NonTerminating)),
        "no false non-termination report: {d:?}"
    );
}

#[test]
fn an_instantiation_through_a_call_result_checks() {
    // The checker forces the unevaluated call result and sees the concrete
    // struct type, where a panic used to be the answer.
    let (module, value, _) = common::evaluate("mk = u => struct<.f Int, .g Int>\n(mk (Int))(1, 2)");
    assert_eq!(common::usize_array(&module, &value), vec![1, 2]);
    let (module, value, _) =
        common::evaluate("mk = u => struct<.f Int, .g Int>\nt = mk (Int)\nt(1, 2)");
    assert_eq!(common::usize_array(&module, &value), vec![1, 2]);
}

#[test]
fn a_call_result_callee_of_a_non_struct_type_is_a_nominal_error() {
    // The forced callee is concretely not a struct type — a reported diagnostic,
    // never a panic.
    let d = diags("mk = u => Int\n(mk (Int))(1, 2)");
    assert_eq!(
        d[0].check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::InstantiateCallee
    );
    assert!(
        d[0].message
            .contains("the callee of an instantiation must be a struct type"),
        "{}",
        d[0].message
    );
}

#[test]
fn an_instantiation_of_a_non_struct_type_is_a_nominal_error() {
    // Structs are nominal: a tuple type and a function type cannot
    // instantiate, and the error points at the callee.
    let d = diags("(<Int, Int>)(1, 2)");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::InstantiateCallee);
    assert!(
        d[0].message
            .contains("the callee of an instantiation must be a struct type"),
        "{}",
        d[0].message
    );
    let d = diags("(Int -> Int)(1, 2)");
    assert_eq!(
        d[0].check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::InstantiateCallee
    );
}

#[test]
fn a_named_instantiation_through_a_parameter_reorders_when_the_type_resolves() {
    // The callee's name table is not statically known through the parameter, so
    // the instantiation is unresolved too.

    // The reorder is therefore a lazy read that resolves at the apply binding
    // `s` to `S`.
    let (module, root) =
        run("S = struct<.x Int, .y Type>\nf = s => s(.y Int, .x 1)\na = f (S)\n(a.x, a.y)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        1,
        "a.x reads the argument that named .x"
    );
    assert_eq!(
        module.node_value(AnyNodeId::Dynamic(ids[1])),
        Some(LangValue::TypeValue(TypeValue::TypeInt)),
        "a.y reads the argument that named .y"
    );
}

#[test]
fn a_named_instantiation_through_a_parameter_checks_the_field_types() {
    // The supplying lookup carries each argument's type beside its
    // name, so an unmatched field type is a miss there.
    let d = diags("S = struct<.x Int, .y Type>\nf = s => s(.x Int, .y 1)\nf (S)");
    assert_eq!(
        d[0].check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::TableMiss,
        "the supplying lookup misses on the mismatched field type: {d:?}"
    );
}

#[test]
fn a_deferred_instantiation_refusal_points_at_the_offending_argument() {
    // The refusal is recorded on a **per-apply clone** of the key, so
    // the caret comes from `Module::node_origin`.

    // That is the argument node the checker did attribute.
    let d = diags("S = struct<.y Int>\nf = s => s(.x 1)\nf (S)");
    assert_eq!(
        d[0].check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::TableMiss,
        "{d:?}"
    );
    assert_eq!(
        d[0].span,
        Some((2, 15)),
        "the caret is on the instantiation's offending argument: {d:?}"
    );
    // `f = s => s(.x 1)` — column 15 is the `.x 1` argument in the lambda
    // body, not the `f (S)` call site on the next line.
    let d = diags("S = struct<.x Int, .y Type>\nf = s => s(.x string, .y 1)\nf (S)");
    let spans: Vec<_> = d.iter().map(|diag| diag.span).collect();
    assert_eq!(
        spans,
        vec![Some((2, 15)), Some((2, 26))],
        "one caret per mismatched argument, both in the lambda body: {d:?}"
    );
}

#[test]
fn an_undecided_sibling_type_narrows_the_deferred_type_check() {
    // Known limit, pinned deliberately. The supplying key carries a type only
    // when every argument's type is decided.

    // One undecided argument drops the type from **every** key, so the
    // `string`-against-`Int` mismatch goes unchecked.

    // A per-argument key form would check it; that mechanism is not in place.
    let report = compile("S = struct<.x Int, .y Type>\nf = s => s(.x string, .y _)\nf (S)");
    assert!(
        report.ok(),
        "the undecided sibling suppresses the whole type check: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_deferred_instantiation_of_a_concrete_argument_type_is_accepted() {
    // A parameter annotated with the struct type resolves the callee at the call.
    let (module, root) = run("A = struct<.x Int, .y Int>\nf = x: A => x\nf (_(.x 1, .y 2))");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        1
    );
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[1]))
                .as_ref()
                .unwrap()
        ),
        2
    );
}

#[test]
fn a_mixed_positional_and_named_instantiation_reorders() {
    // A positional argument fills the lowest-numbered unclaimed definition
    // position, so `.y 2, 1` gives `(.x = 1, .y = 2)`.
    let (module, root) = run("A = struct<.x Int, .y Int>\nf = s => s(.y 2, 1)\nf (A)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        1,
        ".x takes the positional argument"
    );
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[1]))
                .as_ref()
                .unwrap()
        ),
        2,
        ".y takes the named argument"
    );
}

#[test]
fn an_instantiation_through_a_parameter_at_a_non_struct_fails_at_the_call() {
    // The body's callee is pinned to a struct kind, so the expected
    // side names that kind at the call's argument.
    let d = diags("f = s => s(1,2)\nf (Int)");
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].message.contains("TypeStruct"),
        "the expected side names the struct kind: {}",
        d[0].message
    );
    assert_eq!(d[0].span, Some((2, 4)), "the failing argument: {d:?}");
}

#[test]
fn an_alias_of_a_forward_used_binding_keeps_the_aliased_type() {
    // The use of `c` captured the reserved placeholder before `c = b` compiled;
    // the alias re-points it to `b`'s node.
    let (module, value, _) = common::evaluate("a = c(1, 2)\nb = struct<.f Int, .g Int>\nc = b\na");
    assert_eq!(common::usize_array(&module, &value), vec![1, 2]);
}

#[test]
fn a_block_without_a_tail_returns_an_anonymous_struct_instance() {
    // A block ending in a binding has no tail, so it returns a struct
    // instance of a fresh, unnamed type.
    let (module, root) = run("a = { x = 1; y = Int }; (a.x, a.y)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[0]))
                .as_ref()
                .unwrap()
        ),
        1,
        "a.x"
    );
    assert_eq!(
        module.node_value(AnyNodeId::Dynamic(ids[1])),
        Some(LangValue::TypeValue(TypeValue::TypeInt)),
        "a.y"
    );
}

#[test]
fn a_struct_block_with_pub_only_exposes_the_pub_fields() {
    // { pub x = 1; y = 2 } — a `pub` statement is a field; `y` is a
    // block-local, still compiled but not exposed.
    let (module, root) = run("a = { pub x = 1; y = 2 }; a.x");
    let mut module = module;
    let value = module.evaluate_node_deep(root, None).unwrap();
    assert_eq!(usize_of(&value), 1, "a.x reads the exposed pub field");
    // y is not exposed: reading it is a named-field error.
    let d = diags("a = { pub x = 1; y = 2 }; a.y");
    assert!(
        d.iter().any(|d| d
            .check
            .as_ref()
            .is_some_and(|c| c.kind == DiagKind::NamedField)),
        "a.y must be a named-field miss: {:?}",
        d
    );
}

#[test]
fn a_let_in_a_struct_block_is_a_local_not_a_field() {
    // { let x = 1; y = x + 1 } — `x` is a `let` local (never a field); `y` is
    // a field that references it.
    let (module, root) = run("a = { let x = 1; y = x + 1 }; a.y");
    let mut module = module;
    let value = module.evaluate_node_deep(root, None).unwrap();
    assert_eq!(usize_of(&value), 2, "a.y = x + 1");
    // x is not exposed: reading it is a named-field miss.
    let d = diags("a = { let x = 1; y = x + 1 }; a.x");
    assert!(
        d.iter().any(|d| d
            .check
            .as_ref()
            .is_some_and(|c| c.kind == DiagKind::NamedField)),
        "a.x must be a named-field miss: {:?}",
        d
    );
}

#[test]
fn a_bare_expression_in_a_block_is_a_statement_not_a_field() {
    // A bare expression is an ordinary statement: checked, its value discarded,
    // and never a field.

    // The block's record is its bindings alone, so the positional read `a(0)`
    // has nothing to read there.
    let (module, root) = run("a = { 1; x = 2 }; (a, a.x)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None).unwrap());
    assert_eq!(ids.len(), 2, "the record and the read of its field");
    let fields = array_ids(module.evaluate_node_deep(ids[0], None).unwrap());
    assert_eq!(fields.len(), 1, "only the binding is a field");
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(fields[0]))
                .as_ref()
                .unwrap()
        ),
        2,
        "the binding's value, not the discarded expression's"
    );
    assert_eq!(
        usize_of(
            module
                .node_value(AnyNodeId::Dynamic(ids[1]))
                .as_ref()
                .unwrap()
        ),
        2,
        "a.x reads it"
    );
}

#[test]
fn pub_let_is_a_parse_error() {
    // `pub` and `let` cannot co-exist.
    let d = diags("{ pub let x = 1; y = 2 }");
    assert!(
        d.iter()
            .any(|d| d.message.contains("pub cannot mark a let binding")),
        "expected a pub/let parse error: {:?}",
        d
    );
}

#[test]
fn a_block_with_a_trailing_expression_is_still_its_value() {
    // { x = 1; x } — a trailing bare expression (no separator after it) is the
    // block's value, not a struct field.
    assert_eq!(usize_of(&evaluate("a = { x = 1; x }; a")), 1);
}

#[test]
fn a_return_statement_enforces_the_block_value_anywhere() {
    // `return` designates the tail expression wherever it appears; the other
    // statements are block-locals.
    let value = evaluate("a = { x = 1; return 2; y = 3 }; a");
    assert_eq!(
        usize_of(&value),
        2,
        "the block's value is the return expression"
    );
}

#[test]
fn mutually_recursive_structs_check_and_evaluate() {
    // Two struct types that reference each other *as types*, plus a pair of
    // mutually-recursive instances.

    // The checker's skeleton cuts the IR cycle and the deep pass the value
    // cycle.
    let report = compile(
        "A = struct<.f Int, .g B>
         B = struct<.f Type, .g A>
         a = A(1, b)
         b = B(Int, a)
         (A, B, a, b)",
    );
    assert!(
        report.ok(),
        "expected mutually recursive structs to check: {:?}",
        report.diagnostics
    );
    let build = report.build.unwrap();
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None).unwrap();
    let ids = array_ids(value);
    assert_eq!(ids.len(), 4, "the final tuple holds A, B, a, b");
}

#[test]
fn a_function_type_is_a_first_class_value() {
    // `Int -> Int` checks in term position; the identity is such a function.
    run("(x => x) : (Int -> Int)");
}

#[test]
fn a_dependent_array_length_pins_the_parameter() {
    // The check resolves the length read to a pure reference of `n`'s cell and
    // pins it, so the parameter is monomorphized.
    assert_eq!(
        array_ids(evaluate(
            "(((n => ([1, 2, 3] : array<Int, n>)) 3) : array<Int, 3>)"
        ))
        .len(),
        3
    );
}

#[test]
fn a_dependent_array_length_rejects_other_lengths() {
    // `n` is pinned to 3 by the annotation; applying 5 clashes at the apply.
    let d = diags("((n => ([1, 2, 3] : array<Int, n>)) 5)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Runtime);
    assert_eq!(
        check.value_a,
        Some(LangValue::LowValue(LowValue::USize(3))),
        "the pinned length"
    );
    assert_eq!(
        check.value_b,
        Some(LangValue::LowValue(LowValue::USize(5))),
        "the argument"
    );
}

// --- ill-typed programs -----------------------------------------------------

#[test]
fn an_unresolved_name_is_reported() {
    let d = diags("x => y");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Resolve);
    assert_eq!(d[0].message, "unresolved name 'y'");
    assert_eq!(d[0].span, Some((1, 6)));
    // The resolve error is *absorbed* — it lowers to the same inert ErrorBlock
    // a parse error uses.
    assert!(
        compile("x => y").build.is_some(),
        "the resolve error is absorbed; the frontend no longer fails first"
    );
}

#[test]
fn calling_an_unregistered_native_operator_is_a_diagnostic() {
    // `$name` resolves against the compiling module's own private registry, which
    // is empty for an ordinary file.

    // The frontend compiles the call blind, so the checker's guard is the
    // first layer that sees the registry.
    let d = diags("x = $nosuchop(1)");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Check);
    assert_eq!(d[0].span, Some((1, 5)), "the caret is on the `$`");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::NativeOpUnresolved);
}

#[test]
fn an_annotation_mismatch_reports_expected_and_found() {
    let d = diags("5 : Int -> Int");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Check);
    assert_eq!(d[0].span, Some((1, 1)));
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
    assert_eq!(
        check.value_a,
        Some(LangValue::TypeValue(TypeValue::TypeInt))
    );
    // An arrow is a function now, so the expected side is that function.
    assert!(matches!(
        check.value_b.expect("the expected arrow"),
        LangValue::LowValue(LowValue::Function(_))
    ));
}

#[test]
fn applying_a_non_function_is_a_guard_error() {
    let d = diags("(5 3)");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].span, Some((1, 2)), "the apply starts at the `5`");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Guard);
    assert_eq!(
        check.value_a,
        Some(LangValue::TypeValue(TypeValue::TypeInt))
    );
}

#[test]
fn an_apply_argument_mismatch_reports_expected_and_found_at_the_argument() {
    // The lowlevel apply rejects the argument at the parameter check, and the
    // diagnostic points at the argument.

    // The highlevel `check.message` is raw (`[[TypeInt, TypeInt], TypeTuple]`).
    let d = diags("g = (x : <Int, Int>) => x(0)\ng (5)");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].span, Some((2, 4)), "the caret is on the argument `5`");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Runtime);
    assert!(
        d[0].message.contains("expected <Int, Int>, found Int"),
        "{}",
        d[0].message
    );
}

#[test]
fn indexing_a_function_is_an_index_target_error() {
    // A dependent selector over the heterogeneous tuple `(1, Int)` is not an
    // index of a function value.

    // The guard is attributed to that value, not to the `a[0]` marker.
    let d = diags("a = x => (1, Int)(x); a[0]");
    assert_eq!(d.len(), 1);
    // The caret is on the function value, not on the `a[0]` marker.
    assert_eq!(
        d[0].span,
        Some((1, 5)),
        "the caret is on the function value"
    );
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Guard);
    assert!(
        d[0].message.contains("found"),
        "the message renders both sides: {}",
        d[0].message
    );
    // the corrected program applies the selector instead of indexing it
    let (module, root) = run("a = x => (1, Int)(x); a 0 : Int");
    let mut module = module;
    assert_eq!(
        usize_of(&module.evaluate_node_deep(root, None).unwrap()),
        1,
        "the dependent selector applied to 0 reads the value"
    );
    let (module, root) = run("a = x => (1, Int)(x); a 1 : Type");
    let mut module = module;
    assert_eq!(
        module.evaluate_node_deep(root, None).unwrap(),
        LangValue::TypeValue(TypeValue::TypeInt),
        "applied to 1 it reads the type constant"
    );
}

#[test]
fn a_bare_lambda_checks() {
    // The root type is the arrow `?a → ?a` — undecided components, but a
    // determined shape.
    let report = compile("x => x");
    assert!(report.ok(), "bare lambdas check: {:?}", report.diagnostics);
}

#[test]
fn an_unannotated_call_runs_and_its_type_is_derived() {
    // The call's result type cell is a lazy record, so the checker
    // pre-evaluates the root type from the evaluated value (5).
    assert_eq!(usize_of(&evaluate("((id => id 5) (x => x))")), 5);
}

#[test]
fn a_heterogeneous_array_is_rejected() {
    let d = diags("[1, x => x]");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::ArrayElement);
    // The conflict is top-level: a function's type (`f : f`) is not `Int`.
}

#[test]
fn an_array_of_the_wrong_length_is_rejected() {
    let d = diags("([1, 2] : array<Int, 3>)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
    assert_eq!(check.value_a, Some(LangValue::LowValue(LowValue::USize(2))));
    assert_eq!(check.value_b, Some(LangValue::LowValue(LowValue::USize(3))));
}

#[test]
fn an_out_of_bounds_index_is_rejected() {
    // The type side indexes the element-type list structurally, so the bounds
    // check fires at check time.
    let d = diags("([1, 2, 3])[5]");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::IndexOutOfBounds);
    assert_eq!(
        check.value_a,
        Some(LangValue::LowValue(LowValue::USize(5))),
        "the index"
    );
    assert_eq!(
        check.value_b,
        Some(LangValue::LowValue(LowValue::USize(3))),
        "the length"
    );
    assert_eq!(d[0].span, Some((1, 13)), "the index's span");
}

// --- the frontend -----------------------------------------------------------

#[test]
fn the_frontend_builds_a_rooted_table() {
    let ir = frontend("(x => x) 5").ir.unwrap();
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
        // The frontend *recovers*: an error node marks the gap and the
        // checker runs on the partial program.
    }
}

// --- rendering --------------------------------------------------------------

#[test]
fn diagnostics_render_with_carets() {
    let d = diags("x => y");
    let out = lichen_language::render::render("x => y", &d[0]);
    assert_eq!(
        out,
        "error: unresolved name 'y'\n  --> 1:6\n   |\n 1 | x => y\n   |      ^\n"
    );
}

// --- the `_` placeholder ----------------------------------------------------

#[test]
fn an_underscore_annotation_infers_the_type() {
    assert_eq!(usize_of(&evaluate("5 : _")), 5);
}

#[test]
fn an_underscore_annotation_on_an_apply() {
    // (x => x) 5 : _ — the annotation binds loosest, so the apply is the
    // annotated value.
    assert_eq!(usize_of(&evaluate("(x => x) 5 : _")), 5);
}

#[test]
fn partial_inference_in_an_arrow_type() {
    // The input is fixed to `Int` by the annotation, the return inferred.
    assert_eq!(usize_of(&evaluate("(((x => x) : (Int -> _)) 5) : Int")), 5);
}

#[test]
fn an_underscore_in_the_array_length_position() {
    // [1, 2, 3] : array<Int, _> — the length is inferred from the literal.
    let (module, value, _) = common::evaluate("[1, 2, 3] : array<Int, _>");
    assert_eq!(common::usize_array(&module, &value), vec![1, 2, 3]);
}

#[test]
fn a_mismatch_against_a_partial_type_is_reported() {
    let d = diags("5 : (Int -> _)");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Check);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
}

#[test]
fn an_underscore_checks_as_a_value_hole() {
    // `_` is a placeholder in *value* position too: a typed hole whose
    // value slot stays underdetermined.
    let report = compile("_ : Int");
    assert!(
        report.ok(),
        "_ : Int should check as a typed value hole, got: {:?}",
        report.diagnostics
    );
    let (mut module, root) = run("_ : Int");
    let value = module.evaluate_node_deep(root, None);
    assert!(
        value.is_none(),
        "the typed hole's value is underdetermined, got {value:?}"
    );
}

#[test]
fn an_underscore_cannot_be_bound() {
    // `_` is never a name, so it cannot be a binding target — `_ = 5` is a
    // parse error, not a discard binding.
    let d = diags("_ = 5; _");
    assert_eq!(d[0].stage, Stage::Parse);
}

#[test]
fn an_underscore_cannot_be_a_lambda_parameter() {
    // `_ => _` fails: a placeholder is not a name, so the lambda has no
    // binder for its parameter.
    let d = diags("(_ => _) 5");
    assert_eq!(d[0].stage, Stage::Parse);
}

#[test]
fn a_shallow_marked_recursive_tail_stays_lazy() {
    // `f = x => [x, ~ f (x + 1)]` — the bare `~` cuts the deep pass at the tail,
    // so the definition pass terminates.

    // Each index read forces the next apply, so the *values* resolve
    // level by level; the *types* stay underdetermined.
    let (module, value, _) = common::evaluate(
        "f = x => [x, ~ f (x + 1)]; inf = f 0; (inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))",
    );
    let elements = common::array_values(&module, &value);
    assert_eq!(common::usize_of(&elements[0]), 1);
    assert_eq!(common::usize_of(&elements[1]), 2);
    assert_eq!(common::usize_of(&elements[2]), 3);
}

#[test]
fn a_tilde_n_wrap_marks_value_slots_shallow() {
    // `~2` on a plain array: the deep pass terminates, and the read gives the
    // element's value with an underdetermined type.

    // The wrapped term is a lazy region, so its reads never claim a concrete
    // type that would silently mismatch it.
    let (module, value, root_ty) = common::evaluate("([1, ~2 [2, 3]])(1)(0)");
    assert_eq!(
        common::usize_array(&module, &value),
        vec![2, 3],
        "value concrete"
    );
    assert!(
        common::type_is_undecided(&module, root_ty),
        "type underdetermined"
    );
}

#[test]
fn a_tilde_one_on_a_recursive_tail_terminates() {
    // `~1` on the recursive tail: the old depth-budget descent used to loop on
    // this.

    // The compile-time wrap cannot descend the undecided spine, so the
    // definition pass terminates.
    let (module, value, _) = common::evaluate(
        "f = x => [x, ~1 f (x + 1)]; inf = f 0; (inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))",
    );
    // The reads stay lazy under the `~1` mark, so the tuple's elements are
    // underdetermined cells rather than pinned scalars.
    assert_eq!(
        common::array_values(&module, &value).len(),
        3,
        "a tuple value"
    );
}
