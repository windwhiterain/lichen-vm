//! End-to-end tests: source text → `lichen_language::compile` → checked build →
//! evaluation, and the diagnostics (frontend + checker) with their spans.

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::program::TypeValue;
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};

use lichen_language::diag::Stage;
use lichen_language::program::{LangProgram, LangValue};
use lichen_language::{compile, frontend};

mod common;

/// Compile and run a program, asserting it checks; returns the module and the
/// root value node.
/// The dynamic node behind an item ref — the checker builds only dynamic graphs.
fn dyn_node(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => unreachable!("language graphs are dynamic"),
    }
}

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
    module.evaluate_node_deep(root, None)
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
    // The three-line regression of the unify/evaluate rework:
    // `f = p : <Int, Int> => p(0)`; `f (1, 2)` — resolves on the baseline,
    // fails here with `expected raw[?a, Int], found raw[?a, Int]`.
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
    let value = module.evaluate_node_deep(root, None);
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
    // f1 = x => { b = 2; f2 = y => [a, b, x, y]; f2 }; f1 3 4 — the returned
    // closure captures x's binding: the parameter must not leak through as
    // the unbound marker.
    let (mut module, root) = run("a = 1; f1 = x => { b = 2; f2 = y => [a, b, x, y]; f2 }; f1 3 4");
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
fn expression_statements_check_and_evaluate() {
    // A bare expression is a statement anywhere; the program's value is the
    // last expression.  The statements are wired into the root, so their
    // type errors fire — an annotation mismatch...
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
    // `omega omega` beta-reduces to itself (infinite); `w` is only referenced
    // in the unselected `if` branch.  The checker evaluates every top-level
    // statement, hits the VM depth guard, and reports `w` as a
    // `NonTerminating` error — it does not certify it and does not panic.
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
    // x : Int => x — the parameter is pinned to Int; applying at Int
    // checks and runs, the body's use of the parameter is the identity.
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
    // x : Int => x renders `Int -> Int`, and a `_` annotation renders the
    // class it bound to (`Int`, not the raw `[Int, Type]` pair) — the type
    // printer recognizes a cell unified into the universe class.
    assert_eq!(
        lichen_language::run::evaluate("x : Int => x").unwrap(),
        "Function: Int -> Int"
    );
    assert_eq!(lichen_language::run::evaluate("5 : _").unwrap(), "5: Int");
}

/// The root being an annotation over an already-annotated value: the rendered
/// output must list the **merged** attribute set — the slot the annotation
/// spelled next to the one it preserved.  Only the runtime pair carries that
/// width, so a renderer reading the frontend's own IR stamp silently drops the
/// preserved slot (rendering `5 # 4` instead of `5 # 4 ? tag = 7`).
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
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // The shared function node keeps per-apply fresh clones, so one binding
    // used at Int and at Type still checks — graph sharing does not
    // monomorphize functions.
    let (module, root) = run("a = x => x; ((a 5 : Int), (a Type : Type))");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // f = x => {y = x; y} — the block is the lambda's body; its bindings
    // resolve through the graph and its final expression is the body.
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
    // g bound inside the block is one shared function node; each apply gets
    // fresh clones, so it still checks at Int and at Type.
    let (module, root) =
        run("(((x => {g = y => y; ((g x : Int), (g Type : Type))}) 5) : <Int, Type>)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // `!=` is the generalized equality's other face, so it works on the values
    // `==` does — two type values, not just two `Int`s.
    assert_eq!(usize_of(&evaluate("Int != string")), 1);
    assert_eq!(usize_of(&evaluate("Int != Int")), 0);
}

/// An `Int` is a machine-sized **unsigned** integer, so the operators that can
/// tell the two readings apart are the unsigned ones.
///
/// This is the pin for a decision that is otherwise invisible: `0 - 1` wraps, and
/// every value from `2^63` up is reachable that way. A signed `/` `%` `<` `<=`
/// `>` `>=` — in the interpreter or in either JIT backend — would agree with
/// this on every small program and disagree here.
#[test]
fn an_int_is_unsigned_where_the_two_readings_differ() {
    assert_eq!(usize_of(&evaluate("0 - 1 > 1")), 1);
    assert_eq!(usize_of(&evaluate("(0 - 1) / 2")), usize::MAX / 2);
    assert_eq!(usize_of(&evaluate("(0 - 1) % 2")), 1);
    assert_eq!(usize_of(&evaluate("(0 - 1) >= 0")), 1);
}

/// A division or remainder by zero has no value, and the operator says so —
/// with the lazy marker, like every other refused computation, so the program
/// reports an unbound result and the recorded reason explains it.
///
/// **Only the interpreter refuses.** A jitted kernel has left this crate: wasm's
/// integer division traps and SPIR-V's is undefined, and a guard would cost a
/// branch on the GPU path — see `docs/notes/operators.md`.
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

/// The two class conversions each cross **one** way, and neither one converts on
/// its own: `int2float` is exact for every `Int` an `f32` can hold and rounds
/// beyond it, while `float2int` truncates toward zero.  A prefix operator binds
/// tighter than `+`, so `int2float 1 + 2.0` converts the `1` and not the sum
/// (`docs/notes/floating-point.md` §4.2, §4.3).
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
        // Above 2^24 the destination cannot carry the source: the crossing is a
        // float's, and nearest rounding is the float's own answer.
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

/// **Which way the conversion goes is the operator's, and the operand has to
/// agree.**  The checker says so with the same expected/found shape as any other
/// operator, under its own kind — the two classes never convert implicitly, so
/// the only way an `Int` reaches a `Float` is the word that names it.
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

/// A `float2int` whose operand has no `Int` to truncate toward is a **run-time**
/// refusal, recorded the way a zero divisor is: in range is a fact about the
/// value rather than its type, so nothing in the checker can see it.  The three
/// cases are the three the language's unsigned `Int` cannot name — a `NaN`, an
/// infinity, and a negative (`docs/notes/floating-point.md` §4.3).
///
/// **Only the interpreter refuses.**  A jitted kernel has left this crate: wasm's
/// `i64.trunc_f32_u` traps and SPIR-V's `OpConvertFToU` is undefined, which is
/// the same promise the integer division by zero makes
/// (`docs/notes/operators.md`).
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

/// Two tokens are both a bracket and a comparison, and the grammar's rule for
/// telling them apart is a rule about the *shape* around them, not a mode.
#[test]
fn comparisons_share_their_tokens_with_the_angle_bracket_forms() {
    // An expression before the token, and an expression after it: a comparison.
    assert_eq!(usize_of(&evaluate("2 > 1")), 1);
    assert_eq!(usize_of(&evaluate("2 >= 2")), 1);
    assert_eq!(usize_of(&evaluate("1 < 2 > 0")), 1); // (1 < 2) > 0
    // ...and an angle bracket whose content is a *tuple type* wins, because the
    // application is the tighter reading: `f <Int, Type>` is `f` applied to the
    // tuple type, exactly as before the comparison existed.
    assert_eq!(
        usize_of(&evaluate("f = x => x<1>; f <Int, string> == string")),
        1
    );
    // A `>` with no expression after it closes the bracket it is in, so every
    // angle-bracket form still parses — including ones followed by another
    // form's glued delimiter.  The struct form states the *tuple* kind, so a
    // struct type value is read by name (`::a`) rather than positionally.
    assert_eq!(usize_of(&evaluate("<Int, string><1> == string")), 1);
    assert_eq!(
        usize_of(&evaluate("struct<.a Int, .b string>::a == Int")),
        1
    );
    assert_eq!(usize_of(&evaluate("a = <Int, string>; a<0> == Int")), 1);
    // …including one whose closing `>` is followed by the *glued* `(` of an
    // instantiation: the glued delimiter belongs to the angle form, so the `>`
    // is a closer.  (The field is read by name `s.a`; the positional `s(0)` is
    // the tuple read, and indexing an instance with `s[0]` is a separate,
    // pre-existing refusal.)
    assert_eq!(
        usize_of(&evaluate(
            "s = struct<.a Int, .b string>(1, \"a\"); s.a == 1"
        )),
        1
    );
    // An array type's `>` (the keyword-led form) closes as it always did, and
    // the annotation still pins the literal's length.
    let (module, value, _) = common::evaluate("[1, 2] : array<Int, 2>");
    assert_eq!(common::usize_array(&module, &value), vec![1, 2]);
    // A `<` glued to the previous token is still the raw component read, so a
    // comparison is written with a space before it — the Glue rule that was
    // already there.
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
    // The new levels, each checked against the reading that would come out
    // wrong if it were in the wrong place: `* / %` tighter than `+ -`, the
    // bitwise trio nested `&` in `^` in `|`, and all of it tighter than a
    // comparison.
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
    // An unbound operand is pinned to Int: applying the function at a
    // non-Int is a runtime failure, not a panic inside the operator.
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
    // `!([1, 2][5])` — the condition is a failed read: its residue is the
    // concrete computed-nothing value, so the assert FAILS (an unbound
    // condition would stay untriggered) and the value spells `none`.
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
    // The body's assert cannot resolve at normalize (x is unbound), so the
    // apply clones it and re-checks against the argument — the failure is
    // rendered, not silently dropped, and the caret points at the body's `!`.
    let d = diags("f = x => @assert (x == 1); f 2");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Check);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Assert);
    assert_eq!(d[0].message, "assertion failed: expected 1, found 0");
    // The failure is inside the apply's clone of the body's assert, so it is
    // attributed through the clone's template — the caret points at the
    // body's `!`, the expression the user actually wrote.
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
    // The annotation desugars like a plain lambda's — `n : Int => e` is
    // `(n => e) : (Int -> _)` — and the `_` codomain binds lazily, so a
    // runtime-resolved return type (an `if`'s) is not forced at check time.
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
    // A block-wide binding may be any value, not only a lambda: `a = a`
    // resolves `a` to itself (no "must be a lambda" resolve error) — a
    // self-referential, non-productive value.  It *checks*; evaluating it is
    // the programmer's responsibility, like any non-termination.
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
    // VM's application-depth guard refuses the walk — reported as a
    // `NonTerminating` diagnostic instead of panicking the build.
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
    // A recursion *chain* across two block bindings: f calls g, g calls f.
    // Block-wide visibility (the default) lets either reference the other,
    // in both directions, without `rec`, and the checker totalizes the cycle
    // (no stack overflow, no diagnostics).  Sibling template scopes are
    // disjoint, so the runtime descends in place — see the evaluate test
    // below; this one pins the check-time capability with a non-forcing
    // program.
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
    // The *runtime* of a mutual chain: f calls g, g calls f, down to the
    // base case.  Sibling functions' template scopes are disjoint, so the
    // apply clone references the peer in place instead of cloning it per
    // level — the recursion descends and terminates, and exactly two
    // function templates exist.
    let (mut module, root) = run("f = n => if n <= 0 then 0 else g (n - 1);
         g = n => if n <= 0 then 0 else f (n - 1);
         f 5");
    assert_eq!(usize_of(&module.evaluate_node_deep(root, None)), 0);
    assert_eq!(module.functions.len(), 2, "peers are referenced in place");
}

#[test]
fn a_binding_can_forward_reference_a_later_block_wide_binding() {
    // `a = b` reads `b` before it is defined: block-wide names are entered
    // before any value compiles, so a forward (and self/mutual) reference
    // resolves.  `a` aliases `b`'s node.
    assert_eq!(usize_of(&evaluate("a = b; b = [1, 2]; a[0]")), 1);
}

#[test]
fn a_let_binding_is_visible_only_to_later_statements() {
    // `let a = a` is restrictive: the value compiles before the name enters
    // scope, so `a` resolves to the block-wide `a` (the outer `5`) — the
    // sequential rebinding semantics, not a self-reference.
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
    // `a = [a]` — a non-lambda self-reference.  It must check (the checker
    // cuts the cycle with a skeleton pair; it must not stack-overflow); a
    // self-referential value is a benign knot, and forcing it is the
    // programmer's responsibility.
    let report = compile("a = [a]; a");
    // It either checks cleanly or reports a type diagnostic — but must never
    // panic (the checker's cycle cut totalizes the IR term).
    if let Some(s) = report.diagnostics.first() {
        assert_eq!(s.stage, Stage::Resolve, "{s:?}");
    }
}

#[test]
fn a_self_nested_struct_checks_without_overflow() {
    // `s = struct<.f s>` — a struct type whose field is the struct type itself.
    // The checker cuts the type-level cycle (a struct is a nominal type, not
    // a value, so the nominal id is allocated once); it must not overflow.
    let report = compile("s = struct<.f s>; s");
    if let Some(s) = report.diagnostics.first() {
        assert_eq!(s.stage, Stage::Resolve, "{s:?}");
    }
}

#[test]
fn a_self_referential_field_read_checks_without_overflow() {
    // `a = a(0)`, `a = a.x`, `a = a::x` — a block-wide binding referencing
    // itself through a field read.  The frontend transplants the value's kind
    // into the binding's placeholder, so a block root may be *any* expression
    // kind and the checker's cycle cut gates on block-root membership alone;
    // these three kinds fell through the old hand-maintained kind list and
    // overflowed the stack.  Each now checks like the `a = a + 1` control:
    // no diagnostics, and the root deep-evaluates to the lazy parameterized
    // marker instead of hanging.
    for source in ["a = a + 1; a", "a = a(0); a", "a = a.x; a", "a = a::x; a"] {
        let (mut module, root) = run(source);
        assert!(
            matches!(
                module.evaluate_node_deep(root, None),
                LangValue::LowValue(LowValue::Parameterized)
            ),
            "{source:?} must yield the parameterized marker like the control"
        );
    }
}

#[test]
fn a_self_referential_record_checks_without_overflow() {
    // `a = {x = a}` — a self-reference through a record block (the fourth
    // kind the old skeleton gate missed).  The record value is concrete — a
    // one-field struct whose single element is the knot itself — so the deep
    // evaluation terminates on the runtime cycle guard rather than yielding
    // the bare parameterized marker.
    let (mut module, root) = run("a = {x = a}; a");
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // `1+1+…` is flat in the token stream but left-nested in the AST, so the
    // frontend's expression walks recurse once per term on the caller's thread
    // (`#[stacksafe]`: they grow the stack instead of overflowing it).  It is
    // the shape that reaches them — nested brackets recurse in the parser
    // first, and the parser's 16 MiB worker thread overflows at ~175 levels, so
    // a bracket test cannot pin these walks; see `docs/notes/code-audit.md`
    // (P1-22).  `TERMS` aborts this test process before the fix.
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
    // struct<.f Int, .g Int> — the pair [[Int, Int], [TypeId(n), Type]]; a bare
    // struct type is a well-typed program with a determined root.
    run("struct<.f Int, .g Int>");
}

#[test]
fn a_bound_struct_type_is_reusable() {
    // One occurrence bound, then used twice in an array — the array's
    // element check unifies the two uses, and they are the *same* compiled
    // node (the checker compiles each expression once, so the single
    // nominal id survives).
    let (module, root) = run("s = struct<.f Int>; [s, s]");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // Nominal identity is a *type*-level property now: a struct type's kind is
    // `[[TypeId(n), names, names_in_order], TypeStruct]`, so two distinct `struct<…>`
    // occurrences have different ids and therefore different *types*.  An
    // array of two distinct occurrences is heterogeneous and is rejected —
    // the nominality surfaces at the type slot, not the value slot.  (The
    // same occurrence shared across applications stays homogeneous, see
    // a_struct_type_in_a_function_body_is_shared_across_applications.)
    let report = compile("[struct<.f Int>, struct<.f Int>]");
    assert!(
        !report.ok(),
        "two distinct struct occurrences are different nominal types: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_struct_type_in_a_function_body_is_shared_across_applications() {
    // `f = t => struct<.f t>` — the struct occurrence lives in the function
    // body.  Its `Fresh` node does not read the parameter, so the deep pass
    // evaluates it to a concrete `TypeId` and the apply clone references the
    // node in place: every application of `f` shares the one nominal id.
    // Because the id lives in the type slot now, the shared kind makes
    // `[f (Int), f (Int)]` homogeneous — the array checks, which is exactly
    // the sharing proof.
    let report = compile("f = t => struct<.f t>; [f (Int), f (Int)]");
    assert!(
        report.ok(),
        "a body-local struct must be one nominal type across applications: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_polymorphic_struct_constructor_shares_one_nominal_kind() {
    // `Box = t => struct<.f t>` — a generic struct constructor.  The `Fresh` id
    // is per *occurrence* and is shared (referenced in place by every apply
    // clone), so all applications of `Box` resolve to one nominal kind: the
    // id lives in the kind slot while the field-type list rides in the value
    // shape.  Same constructor + same fields is homogeneous and checks;
    // same constructor with different field types is also one nominal kind —
    // the fields differ only in the shape (the value), not the type.
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
    // `A = I => struct<.n Int, .I I>` — one written struct type with *named*
    // fields, inside a function body.  Its identity is decided when the
    // occurrence is checked, so both applications of `A` are one nominal
    // type and the instance built through `S1` annotates against `S2`.  Both
    // halves of the identity matter: the nullary `Fresh` node (a copy
    // re-runs it) and the name table (an arena payload, so a copy is a
    // different table that does not unify).  Before the fix this program
    // failed the annotation with `…>#2` against `…>#1`
    // (`docs/notes/applied-struct-nominal-id.md`).
    let (module, root) = run("A = I => struct<.n Int, .I I>\n\
         In = struct<.x _, .y _>\n\
         S1 = A In\n\
         S2 = A In\n\
         x = S1(.n 3, .I In(.x 10, .y 20))\n\
         y = (x : S2)\n\
         y");
    let mut module = module;
    let instance = array_ids(module.evaluate_node_deep(root, None));
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
    let inner = array_ids(module.evaluate_node_deep(instance[1], None));
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
    // The control the fix must keep: the id is the *occurrence*, not the
    // instantiation, so one constructor applied to different field types is
    // still two types — the field types ride in the shape.
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
    // The runtime half of the same identity: here the annotation sits in the
    // *callee's* body, so the argument's type meets the declared one in the
    // apply-time parameter check rather than in a checker-issued unify.  A
    // pinned id alone does not fix this row — the copied name table is what
    // conflicts.
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
/// A field read's *type* used to be an `Index` node even when the container's
/// type was concrete and its field index already resolved, and a class question
/// is asked of a *cell* (`shape::low_type_of_slot`), which cannot see through an
/// unevaluated `Index`.  So `x.a + x.a` found neither operand concretely
/// `Float`, pinned the operation to the `+` default (`Int`), and then refused
/// both operands against it.  The named form (a struct) and the positional form
/// (a tuple) each resolve the field's type out of the container type's own field
/// list, so both are pinned here.
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
    // `a.sux` on a struct whose closest field is `sub`: the field-access
    // error message appends the struct's actually-close field name so the
    // editor can suggest a fix and power field completion.
    let d = diags("A = struct<.x Int, .sub Int>; a = A(1, 2); a.sux");
    let msg = &d[0].message;
    assert!(
        msg.contains("no field") && msg.contains("did you mean 'sub'?"),
        "a named-field miss should suggest the close field, got {msg}"
    );
}

#[test]
fn a_named_field_read_on_a_non_struct_is_rejected() {
    // reading `a.b` on a non-struct (an int) is a kind refusal: the read states
    // that the container's *kind* must be a struct kind, and an atomic type's
    // kind is `Type`.  It used to be the generic index-target guard
    // ("expected a tuple, array, or struct").
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
    // `X::a` requires the container's *type* to be a TypeStruct kind, so a
    // concretely non-struct container is a check-time error — the counterpart
    // of the tuple-kind requirement the positional `X<e>` now states (which
    // used to validate nothing).
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
    // The documented use: `X<e>` reads a component of a *type-as-value*, whose
    // elements are themselves `[value, type]` pairs — so the read yields the
    // element's value and its type.  Pinned on both halves: the value of
    // `<Int, string><0>` is the `Int` type, and comparing it against `Int` is
    // true (a `USize` answer here would print the same and compare false).
    assert_eq!(
        evaluate("<Int, string><0>"),
        LangValue::TypeValue(TypeValue::TypeInt)
    );
    assert_eq!(usize_of(&evaluate("<Int, string><0> == Int")), 1);
    // A struct type value states the *tuple* kind for `X<e>`, so its components
    // are read by name (`X::a`), which is the same pair read at the name
    // table's index.
    assert_eq!(
        usize_of(&evaluate("struct<.a Int, .b string>::b == string")),
        1
    );
    // The element's own *value* slot, which is what a `Type`-valued element
    // carries: `<Int, string><0> == Int` is the comparison of markers.
    assert_eq!(usize_of(&evaluate("<Int, string><1> == string")), 1);
    // An unbound container stays lazy and resolves at the apply.
    assert_eq!(
        usize_of(&evaluate("f = k => k<0>; f <Int, string> == Int")),
        1
    );
}

#[test]
fn a_raw_read_of_a_non_tuple_container_is_refused_by_kind() {
    // `X<e>` reads a component of a *type value*, so the container's type must
    // be the tuple kind: a plain array is refused where it stands, by the kind
    // wording, rather than reaching the element read (which used to report
    // "not a value/type pair" — or print `none` — after the build had decided
    // `ok`).
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
    // …and through a deferred parameter, where the apply that binds it states
    // the same requirement: the pin's own parameter check (`Runtime`, the tier
    // every pin is enforced at), refused where the argument is.
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
    // `S::a` reads field `a`'s *type* (as a value) from the struct type value
    // `S`; `s.a` reads field `a`'s *value* from the struct instance `s`.
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
    // constant by value, so `S::a == Int` is 1 and `S::a == string` is 0.
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
    // `s<0>` over an int: the container is a parameter, so the tuple-kind
    // requirement is deferred and the apply that binds it states it — the
    // pin's own parameter check (`Runtime`, the tier every pin is enforced
    // at), refused where the argument is.  It used to reach the lowlevel as a
    // `RuntimeIndexTarget`, blaming the target's value node with no caret.
    let d = diags("f = s => s<0>; f (1)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Runtime);
    assert_eq!(d[0].message, "expected TypeTuple, found Int");
    assert_eq!(d[0].span, Some((1, 19)), "the caret is on the argument");
}

#[test]
fn a_raw_read_whose_subscript_is_not_an_index_reports_a_runtime_subscript_error() {
    // `a<i>` reads element `i` structurally, so a string subscript is not a
    // check-time diagnostic either: the lowlevel records it and it arrives as
    // `RuntimeIndexSubscript`.  The caret is on the subscript, the one node
    // the raw read does give a source edge.  The container has to be a *type
    // value* of tuple kind to get here at all: an array container is refused
    // by the kind check before the subscript is read.
    let d = diags("a = <Int, string>\ni = \"x\"\na<i>");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::RuntimeIndexSubscript);
    assert_eq!(d[0].span, Some((2, 5)), "the caret is on the subscript `i`");
}

#[test]
fn an_apply_of_a_deferred_non_function_reports_a_runtime_apply_target_error() {
    // `f = g => g 1` applied to `5`: the callee is a parameter, so its type
    // cell stays unbound and the checker's function-ness guard is skipped.
    // The lowlevel records the runtime failure, and it reaches the
    // diagnostics as `RuntimeApplyTarget` — the value itself is the fact,
    // with no type to print.
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
    // `f = g => g 1` applied to a struct instance: the callee is a parameter,
    // so its type cell stays unbound and the checker's function-ness guard is
    // skipped, and the instance's value is structurally a `LowValue::Array` —
    // the same shape a compute kernel's `[native, sig]` pair takes, which the
    // lowlevel cannot tell apart.  Only the program knows which of its values
    // are callable (`Program::is_callable`), so its answer has to refuse this
    // one for the fact to be recorded like the scalar sibling's.
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
    // Two functions each contain their own struct occurrence — each body's
    // `Fresh` node is its own, so the nominal ids stay distinct across the
    // functions.  Distinct ids mean distinct kinds (distinct types), so
    // `[f (Int), g (Int)]` is heterogeneous and is rejected.
    let report = compile("f = t => struct<.f t>; g = t => struct<.f t>; [f (Int), g (Int)]");
    assert!(
        !report.ok(),
        "distinct body-local structs must keep distinct nominal ids: {:?}",
        report.diagnostics
    );
}

#[test]
fn an_annotation_against_a_struct_type_conflicts() {
    // 5 : struct<.f Int> — an annotation compares the full type expressions
    // and the literal's int type is not the struct type; instantiation is
    // the dedicated `s(1, 2)` form, not an annotation.
    let d = diags("5 : struct<.f Int>");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
}

#[test]
fn a_struct_type_application_is_an_instance() {
    // struct<.f Int, .g Int>(1, 2) — the struct type applied to a positional
    // tuple compiles to the Instantiate expression: the element types are
    // checked against the named fields' types, and the result has the struct
    // type.
    let (module, root) = run("struct<.f Int, .g Int>(1, 2)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
    assert_eq!(ids.len(), 2);
    // a bound struct type instantiates the same way
    let (module, root) = run("s = struct<.f Int, .g Int>; s(1, 2)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // s = struct<.f Int, .t Type>; a = s(1, Int); (a.f, a.t) — a named read
    // over an instance reads the wrapped tuple's elements, and each
    // element's type is the corresponding field type (Int and Type).
    let (module, root) = run("s = struct<.f Int, .t Type>; a = s(1, Int); (a.f, a.t)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // a named read through a parameter works too — the read's type is
    // `Index(shape, k)` over the container type's shape, and the field's
    // index comes from the struct's name table, so it resolves when the call
    // binds the parameter (the argument is parenthesized: `f s(1, Int)`
    // would parse as `(f s)(1, Int)`)
    let (module, root) = run("f = a => a.f; s = struct<.f Int, .t Type>; f (s(1, Int)) : Int");
    let mut module = module;
    assert_eq!(usize_of(&module.evaluate_node_deep(root, None)), 1);
}

#[test]
fn a_positional_read_of_a_struct_instance_is_refused() {
    // a(0) — the paren read is the *tuple* read; a struct instance reads by
    // name (`a.f`), so the positional form is refused (Guard) rather than
    // reading the field list.  The requirement is stated as the open tuple
    // type the container would have to be, and the caret is the container.
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
    // S(.y Int, .x 1) — the named arguments are reordered to the definition's
    // positional order, so a.x reads the .x field (1) and a.y the .y field
    // (Int).
    let (module, root) = run("S = struct<.x Int, .y Type>; a = S(.y Int, .x 1); (a.x, a.y)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // S(.y Int, 1) — the bare positional argument fills the lowest-numbered
    // unclaimed definition position (.x), so the instance is (1, Int).
    let (module, root) = run("S = struct<.x Int, .y Type>; a = S(.y Int, 1); (a.x, a.y)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // A struct type with no named fields is refused at the definition: every
    // struct field must carry a name, because a struct instance reads by name
    // (`s.x`) and the positional `a(k)` form is the tuple read.
    let d = diags("S = struct<Int, Type>; S(.x 1, .y Int)");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::StructFieldName);
}

#[test]
fn a_named_struct_instantiation_with_a_wrong_field_type_is_rejected() {
    // .x : Int but the argument is a Type value — after reordering the value's
    // element types are checked against the field list, so this fails as an
    // annotation mismatch.
    let d = diags("S = struct<.x Int, .y Type>; S(.x Type, .y Int)");
    assert_eq!(
        d[0].check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::Annotation
    );
}

#[test]
fn a_named_struct_instantiation_reads_through_a_parameter() {
    // Lazy resolution through a parameter: get = s => s.y, applied to a named
    // instantiation, resolves the field through the struct's name table at
    // the call.
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
    // `S = struct<Int, Type>` — the positional struct is refused at its
    // definition now, before the lazy read ever resolves: its fields carry
    // no names, and every struct field must be named.  The refusal is a
    // check diagnostic, never a panic and never a false non-termination
    // report from the read that follows.
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
    // `(mk (Int))(1, 2)` — the callee is an unevaluated call result; the
    // checker forces it and sees the concrete struct type (a panic was the
    // pre-fix behaviour).  Both spellings — the direct call result and a
    // bound alias of it — are the same graph.
    let (module, value, _) = common::evaluate("mk = u => struct<.f Int, .g Int>\n(mk (Int))(1, 2)");
    assert_eq!(common::usize_array(&module, &value), vec![1, 2]);
    let (module, value, _) =
        common::evaluate("mk = u => struct<.f Int, .g Int>\nt = mk (Int)\nt(1, 2)");
    assert_eq!(common::usize_array(&module, &value), vec![1, 2]);
}

#[test]
fn a_call_result_callee_of_a_non_struct_type_is_a_nominal_error() {
    // `(mk (Int))(1, 2)` with `mk = u => Int`: the forced callee is
    // concretely not a struct type — a reported diagnostic, never a panic.
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
    // `S = struct<.x Int, .y Type>; f = s => s(.y Int, .x 1); f (S)` — the
    // callee's name table is not statically known through the parameter, so
    // the instantiation is unresolved too: the reorder is a lazy read that
    // resolves at the apply binding `s` to `S`.
    let (module, root) =
        run("S = struct<.x Int, .y Type>\nf = s => s(.y Int, .x 1)\na = f (S)\n(a.x, a.y)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // The same deferred reorder: the supplying lookup carries each argument's
    // type beside its name, so a field whose declared type no supplying
    // argument matches is a miss there — refused when the callee's type
    // resolves, never a silently wrong field.
    let d = diags("S = struct<.x Int, .y Type>\nf = s => s(.x Int, .y 1)\nf (S)");
    assert_eq!(
        d[0].check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::TableMiss,
        "the supplying lookup misses on the mismatched field type: {d:?}"
    );
}

#[test]
fn a_deferred_instantiation_refusal_points_at_the_offending_argument() {
    // The refusal is recorded on a **per-apply clone** of the supplying key —
    // the checker never saw that node — so the caret comes from the clone's
    // template origin (`Module::node_origin`), which is the argument node the
    // checker did attribute.  Both the unknown-field and the mismatched-type
    // refusal point at the argument the user wrote, inside the lambda body.
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
    // Known limit, pinned deliberately.  The supplying key carries a type only
    // when every argument's type is decided, so one undecided argument (here
    // `.y _`) drops the type from **every** key and `.x`'s `string`-against-
    // `Int` mismatch is not checked: the program is accepted.  A per-argument
    // key form would check `.x` and refuse this; the mechanism for that is not
    // in place, so this records the behaviour rather than asserting the
    // intended one.
    let report = compile("S = struct<.x Int, .y Type>\nf = s => s(.x string, .y _)\nf (S)");
    assert!(
        report.ok(),
        "the undecided sibling suppresses the whole type check: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_deferred_instantiation_of_a_concrete_argument_type_is_accepted() {
    // A parameter annotated with the struct type resolves the callee at the
    // call, so the named instantiation checks against the real fields.
    let (module, root) = run("A = struct<.x Int, .y Int>\nf = x: A => x\nf (_(.x 1, .y 2))");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    // `f = s => s(1,2); f (Int)` — the body's callee is pinned to a struct
    // kind, so the non-struct argument fails the apply's parameter check:
    // the expected side names the struct requirement (not `?a`), at the
    // call's argument.
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
    // `a = c(1, 2); b = struct<.f Int, .g Int>; c = b` — the use of `c` captured
    // the reserved placeholder before `c = b` compiled; the alias re-points
    // the earlier uses to `b`'s node, so the instantiation sees the struct
    // type (it previously kept the stale placeholder's `?a`).
    let (module, value, _) = common::evaluate("a = c(1, 2)\nb = struct<.f Int, .g Int>\nc = b\na");
    assert_eq!(common::usize_array(&module, &value), vec![1, 2]);
}

#[test]
fn a_block_without_a_tail_returns_an_anonymous_struct_instance() {
    // { x = 1; y = Int } — a block whose last statement is a binding has no
    // tail expression, so it returns a struct instance (its type is a fresh,
    // unnamed one) whose fields are the named bindings x and y, read by name.
    let (module, root) = run("a = { x = 1; y = Int }; (a.x, a.y)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
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
    let value = module.evaluate_node_deep(root, None);
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
    let value = module.evaluate_node_deep(root, None);
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
    // { 1; x = 2 } — a bare expression is an ordinary statement: checked, its
    // value discarded, and never a field.  The block's record is its binding
    // `x` alone (one field), which is why a block's fields are always named and
    // the positional read `a(0)` has nothing to read there.
    let (module, root) = run("a = { 1; x = 2 }; (a, a.x)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
    assert_eq!(ids.len(), 2, "the record and the read of its field");
    let fields = array_ids(module.evaluate_node_deep(ids[0], None));
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
    // { x = 1; return 2; y = 3 } — `return` designates the tail expression no
    // matter where it appears; the other statements are block-locals.
    let value = evaluate("a = { x = 1; return 2; y = 3 }; a");
    assert_eq!(
        usize_of(&value),
        2,
        "the block's value is the return expression"
    );
}

#[test]
fn mutually_recursive_structs_check_and_evaluate() {
    // A = struct<.f Int, .g B>; B = struct<.f Type, .g A>; a = A(1, b);
    // b = B(Int, a) — two struct types that reference each other *as types*,
    // plus a pair of mutually-recursive instances.  The types close into
    // A = struct<.f Int, .g B>, B = struct<.f Type, .g A>; the checker's
    // skeleton cuts the IR cycle and the deep pass the value cycle.  The final
    // tuple prints the two struct types and both cyclic instances.
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
    let value = module.evaluate_node_deep(build.root_val, None);
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
    // `array<Int, n>` with a bound `n`: the check resolves the length read to a pure
    // reference of `n`'s cell and pins it to the literal's length — the
    // parameter is monomorphized, and applying the pinned length checks and
    // runs.  (The root apply is annotated to anchor its lazy result cell.)
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
    // `n` is pinned to 3 by the annotation; applying 5 clashes at the apply
    // (a runtime failure — the parameter's expected value against the
    // argument).
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
    // The resolve error is *absorbed* at its layer — it lowers to the same
    // inert ErrorBlock a parse error uses, so the pipeline stays total and the
    // checker still runs on the effective content.
    assert!(
        compile("x => y").build.is_some(),
        "the resolve error is absorbed; the frontend no longer fails first"
    );
}

#[test]
fn calling_an_unregistered_native_operator_is_a_diagnostic() {
    // `$name` resolves against the compiling module's own private registry,
    // which is empty for an ordinary file — so every `$name` is unresolved.
    // It is reported at the `$` rather than panicking: the frontend compiles
    // the call blind, so the checker is the first layer that can see the
    // registry, and the guard makes `Build::ok` false.
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
    // the expected side is the arrow type — an array of two elements
    assert_eq!(
        array_ids(check.value_b.expect("the expected arrow type")).len(),
        2
    );
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
    // g (5) where g declares a tuple parameter: the lowlevel apply rejects
    // the argument at the parameter check (no longer panicking on the body's
    // `Index` over a non-array), and the diagnostic points at the argument,
    // the declared parameter type as the expected side.  The highlevel
    // `check.message` is raw ([[TypeInt, TypeInt], TypeTuple]); the language
    // layer renders the pretty `<Int, Int>` spellings.
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
    // `a[0]` where `a` is a bound function — a dependent selector over the
    // heterogeneous tuple `(1, Int)` — is not an index of the function
    // itself: the checker reports it statically instead of the runtime
    // panicking on a non-array target.  The call is written `a 0`.
    let d = diags("a = x => (1, Int)(x); a[0]");
    assert_eq!(d.len(), 1);
    // The checker attributes the guard to the indexed function value (its
    // type is a function, not an indexable shape), not to the `a[0]` marker.
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
        usize_of(&module.evaluate_node_deep(root, None)),
        1,
        "the dependent selector applied to 0 reads the value"
    );
    let (module, root) = run("a = x => (1, Int)(x); a 1 : Type");
    let mut module = module;
    assert_eq!(
        module.evaluate_node_deep(root, None),
        LangValue::TypeValue(TypeValue::TypeInt),
        "applied to 1 it reads the type constant"
    );
}

#[test]
fn a_bare_lambda_checks() {
    // The root type is the arrow `?a → ?a` — unbound components, but the
    // arrow shape is determined, so there is no ambiguity diagnostic.
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
    // The conflict is now top-level — a function's type (`f : f`, a
    // function-type node) is not `Int` — so the found/expected leaves are the
    // function value and the `Int` marker, not the old arrow shape's cells.
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
    // The type side indexes the element-type list structurally, so the
    // bounds check fires at check time; the diagnostic carries the index and
    // the length, at the index's span.
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
        // The frontend *recovers*: the checker runs on the partial program
        // (an error node marks the gap), so `build` is usually `Some` — the
        // assertion is that garbage never panics, not that it fails fast.
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
    // ((x => x) : (Int -> _)) 5 : Int — the input is fixed to Int by the
    // annotation, the return inferred; the root annotation anchors the call's
    // lazy result cell.
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
    // `_` is a placeholder in *value* position too: `_ : Int` is a typed hole
    // — it checks (the value's type slot unifies with Int) and its value is
    // underdetermined (a `Parameterized` hole), never a resolve error.
    let report = compile("_ : Int");
    assert!(
        report.ok(),
        "_ : Int should check as a typed value hole, got: {:?}",
        report.diagnostics
    );
    let (mut module, root) = run("_ : Int");
    let value = module.evaluate_node_deep(root, None);
    assert!(
        matches!(value, LangValue::LowValue(LowValue::Parameterized)),
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
    // f = x => [x, ~ f (x + 1)] — the bare `~` cuts the deep pass at the
    // tail, so the definition pass terminates; each index read forces the
    // next apply on demand, so the *values* resolve level by level (1, 2, 3).
    // The reads' element *types* stay underdetermined (`?a`, `?b`, `?c`): a
    // paren read pins an undecided container to a fresh open tuple, so the type
    // is never claimed structurally across the lazy tail — a display change
    // only, and the values asserted here are unaffected.
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
    // ~2 on a plain array: the deep pass terminates (the marked value slots
    // are skipped), and the read gives the element's value with an
    // underdetermined type — the wrapped term is a lazy region, so its
    // reads never claim a concrete type that would silently mismatch it.  With
    // no type to read the value against, the value is a raw dump (`raw[…]`).
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
    // ~1 on the recursive tail: the old depth-budget descent used to loop
    // on this; the compile-time wrap cannot descend the unbound spine, so
    // the definition pass terminates and the reads stay underdetermined
    // (sound), never a guard panic.
    let (module, value, _) = common::evaluate(
        "f = x => [x, ~1 f (x + 1)]; inf = f 0; (inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))",
    );
    // The reads stay lazy under the `~1` mark, so the tuple's elements are
    // underdetermined cells rather than a pinned scalar; the point is that the
    // definition pass terminates and yields the three reads.
    assert_eq!(
        common::array_values(&module, &value).len(),
        3,
        "a tuple value"
    );
}
