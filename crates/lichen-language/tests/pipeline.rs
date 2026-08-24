//! End-to-end tests: source text → `lichen_language::compile` → checked build →
//! evaluation, and the diagnostics (frontend + checker) with their spans.

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::program::{HighProgram, HighProgramValue};
use lichen_lowlevel::{Module, NodeId};

use lichen_language::diag::Stage;
use lichen_language::{compile, frontend};

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

fn evaluate(source: &str) -> HighProgramValue {
    let (mut module, root) = run(source);
    module.evaluate_node_deep(root, None)
}

/// The node ids of an array value.
fn array_ids(value: HighProgramValue) -> Vec<NodeId> {
    let HighProgramValue::Array(array) = value else {
        panic!("expected an array value, got {value:?}");
    };
    array.ids().to_vec()
}

fn usize_of(value: &HighProgramValue) -> usize {
    let HighProgramValue::USize(n) = value else {
        panic!("expected a usize value, got {value:?}");
    };
    *n
}

/// The rendered diagnostics of a failing program.
fn diags(source: &str) -> Vec<lichen_language::Diag> {
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
        matches!(module.nodes[ids[1]].value, Some(HighProgramValue::TypeType)),
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
            module.nodes[id].value,
            Some(HighProgramValue::USize(n)),
            "element {n} must be a bound value, not the leaked parameter"
        );
    }
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
    assert_eq!(check.value_a, Some(HighProgramValue::TypeInt));
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
        matches!(module.nodes[ids[1]].value, Some(HighProgramValue::TypeType)),
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
    assert_eq!(check.value_a, Some(HighProgramValue::USize(5)), "the index");
    assert_eq!(
        check.value_b,
        Some(HighProgramValue::USize(2)),
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
    assert_eq!(usize_of(module.nodes[ids[0]].value.as_ref().unwrap()), 5);
    assert!(
        matches!(module.nodes[ids[1]].value, Some(HighProgramValue::TypeType)),
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
    assert_eq!(
        lichen_language::run::evaluate(
            "fib = n => if n <= 1 then n else fib (n - 1) + fib (n - 2); fib 10"
        )
        .unwrap(),
        "55: Int"
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
#[should_panic(expected = "recursion depth exceeded")]
fn a_non_terminating_recursive_function_panics_at_the_guard() {
    // No base case: the definition pass runs the recursion forever, and the
    // VM's application-depth guard panics instead of exhausting memory —
    // the designed behavior of the core, not a diagnostic.
    let _ = compile("f = n => f n; f 3");
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
    // `s = struct<s>` — a struct type whose field is the struct type itself.
    // The checker cuts the type-level cycle (a struct is a nominal type, not
    // a value, so the nominal id is allocated once); it must not overflow.
    let report = compile("s = struct<s>; s");
    if let Some(s) = report.diagnostics.first() {
        assert_eq!(s.stage, Stage::Resolve, "{s:?}");
    }
}

// --- struct types ------------------------------------------------------------

#[test]
fn a_struct_type_kinds_and_evaluates() {
    // struct<Int, Int> — the pair [[Int, Int], [TypeId(n), Type]]; a bare
    // struct type is a well-typed program with a determined root.
    run("struct<Int, Int>");
}

#[test]
fn a_bound_struct_type_is_reusable() {
    // One occurrence bound, then used twice in an array — the array's
    // element check unifies the two uses, and they are the *same* compiled
    // node (the checker compiles each expression once, so the single
    // nominal id survives).
    let (module, root) = run("s = struct<Int>; [s, s]");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
    assert_eq!(ids.len(), 2);
    for id in ids {
        assert!(matches!(
            module.nodes[id].value,
            Some(HighProgramValue::Array(_))
        ));
    }
}

#[test]
fn two_struct_type_occurrences_do_not_unify() {
    // Nominal identity is a *value*-level property now: a struct type's
    // shape is [TypeId(n), fields], so two occurrences differ in their
    // values (TypeId(0) vs TypeId(1)) but share one type ([TypeStruct,
    // Type]) — a bare array of two struct type values is type-homogeneous
    // and checks.  The nominality surfaces when the occurrences are
    // *instantiated*: [s1(1, 2), s2(1, 2)] conflicts (covered by
    // a_struct_instance_with_mismatched_fields_is_rejected).
    let report = compile("[struct<Int>, struct<Int>]");
    assert!(
        report.ok(),
        "two struct types share the type [TypeStruct, Type]: {:?}",
        report.diagnostics
    );
}

#[test]
fn an_annotation_against_a_struct_type_conflicts() {
    // 5 : struct<Int> — an annotation compares the full type expressions
    // and the literal's int type is not the struct type; instantiation is
    // the dedicated `s(1, 2)` form, not an annotation.
    let d = diags("5 : struct<Int>");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
}

#[test]
fn a_struct_type_application_is_an_instance() {
    // struct<Int, Int>(1, 2) — the struct type applied to a positional
    // tuple compiles to the Instantiate expression: the element types are
    // checked against the fields, and the result has the struct type.
    let (module, root) = run("struct<Int, Int>(1, 2)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
    assert_eq!(ids.len(), 2);
    // a bound struct type instantiates the same way
    let (module, root) = run("s = struct<Int, Int>; s(1, 2)");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
    assert_eq!(ids.len(), 2);
}

#[test]
fn a_struct_instance_with_mismatched_fields_is_rejected() {
    // arity: two fields, one value
    let d = diags("struct<Int>(1, 2)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
    // field types: the tuple's Ints are not Type
    let d = diags("struct<Type, Type>(1, 2)");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::Annotation);
    // a different source occurrence is a different nominal type
    let d = diags("s1 = struct<Int, Int>; s2 = struct<Int, Int>; [s1(1, 2), s2(1, 2)]");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::ArrayElement);
    assert!(check.message.contains("TypeId("), "{}", check.message);
}

#[test]
fn a_struct_instance_indexes_its_fields() {
    // s = struct<Int, Type>; a = s(1, Int); (a[0], a[1]) — indexing an
    // instance reads the wrapped tuple's elements, and each element's type
    // is the corresponding field type (Int and Type).
    let (module, root) = run("s = struct<Int, Type>; a = s(1, Int); (a[0], a[1])");
    let mut module = module;
    let ids = array_ids(module.evaluate_node_deep(root, None));
    assert_eq!(ids.len(), 2);
    assert_eq!(usize_of(module.nodes[ids[0]].value.as_ref().unwrap()), 1);
    assert_eq!(
        module.nodes[ids[1]].value,
        Some(HighProgramValue::TypeInt),
        "the second field is the `Int` type constant"
    );
    // indexing the instance through a parameter works too — the runtime
    // dispatch sees the struct kind and selects its field list (the
    // argument is parenthesized: `f s(1, Int)` would parse as `(f s)(1, Int)`)
    let (module, root) = run("f = a => a[0]; s = struct<Int, Type>; f (s(1, Int)) : Int");
    let mut module = module;
    assert_eq!(usize_of(&module.evaluate_node_deep(root, None)), 1);
}

#[test]
fn a_struct_instance_index_out_of_bounds_is_rejected() {
    // a[5] — the field list is structural like a tuple's, so the bounds
    // check fires at check time.
    let d = diags("s = struct<Int, Type>; a = s(1, Int); a[5]");
    assert_eq!(d.len(), 1);
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::IndexOutOfBounds);
    assert_eq!(check.value_a, Some(HighProgramValue::USize(5)), "the index");
    assert_eq!(
        check.value_b,
        Some(HighProgramValue::USize(2)),
        "the field count"
    );
}

#[test]
fn mutually_recursive_structs_check_and_evaluate() {
    // A = struct<Int, B>; B = struct<Type, A>; a = A(1, b); b = B(Int, a) —
    // two struct types that reference each other *as types*, plus a pair of
    // mutually-recursive instances.  The types close into A = struct<Int, B>,
    // B = struct<Type, A>; the checker's skeleton cuts the IR cycle and the
    // deep pass the value cycle.  The final tuple prints the two struct types
    // and both cyclic instances.
    let report = compile(
        "A = struct<Int, B>
         B = struct<Type, A>
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
    assert_eq!(
        check.value_a,
        Some(HighProgramValue::USize(3)),
        "the pinned length"
    );
    assert_eq!(
        check.value_b,
        Some(HighProgramValue::USize(5)),
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
    assert_eq!(check.value_a, Some(HighProgramValue::TypeInt));
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
    assert_eq!(check.value_a, Some(HighProgramValue::TypeInt));
}

#[test]
fn indexing_a_function_is_an_index_target_error() {
    // `a[0]` where `a` is a bound function — a dependent selector over the
    // heterogeneous tuple `(1, Int)` — is not an index of the function
    // itself: the checker reports it statically instead of the runtime
    // panicking on a non-array target.  The call is written `a 0`.
    let d = diags("a = x => (1, Int)[x]; a[0]");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].span, Some((1, 23)), "the index starts at `a`");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::IndexTarget);
    assert!(
        check
            .message
            .contains("expected a tuple, array, or struct type"),
        "{}",
        check.message
    );
    // the corrected program applies the selector instead of indexing it
    let (module, root) = run("a = x => (1, Int)[x]; a 0 : Int");
    let mut module = module;
    assert_eq!(
        usize_of(&module.evaluate_node_deep(root, None)),
        1,
        "the dependent selector applied to 0 reads the value"
    );
    let (module, root) = run("a = x => (1, Int)[x]; a 1 : Type");
    let mut module = module;
    assert_eq!(
        module.evaluate_node_deep(root, None),
        HighProgramValue::TypeInt,
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
    assert_eq!(check.value_b, Some(HighProgramValue::TypeInt));
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
    assert_eq!(check.value_a, Some(HighProgramValue::USize(2)));
    assert_eq!(check.value_b, Some(HighProgramValue::USize(3)));
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
    assert_eq!(check.value_a, Some(HighProgramValue::USize(5)), "the index");
    assert_eq!(
        check.value_b,
        Some(HighProgramValue::USize(3)),
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
    // [1, 2, 3] : Int<_> — the length is inferred from the literal.
    let ids = array_ids(evaluate("[1, 2, 3] : Int<_>"));
    assert_eq!(ids.len(), 3);
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
fn an_underscore_in_term_position_is_still_a_name() {
    // `_` outside type positions is an ordinary name: a discard binding and
    // a lambda parameter named `_`.
    assert_eq!(usize_of(&evaluate("_ = 5; _")), 5);
    assert_eq!(usize_of(&evaluate("(_ => _) 5 : Int")), 5);
}

#[test]
fn an_underscore_as_a_value_is_unresolved() {
    // `_ : Int` — the placeholder is type-position only; as a value `_` is an
    // ordinary name and unresolved here.
    let d = diags("_ : Int");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].stage, Stage::Resolve);
    assert_eq!(d[0].message, "unresolved name '_'");
}

#[test]
fn a_shallow_marked_recursive_tail_stays_lazy() {
    // f = x => [x, ~ f (x + 1)] — the bare `~` cuts the deep pass at the
    // tail, so the definition pass terminates; each index read forces the
    // next apply on demand, and the stream's type resolves level by level.
    let out = lichen_language::run::evaluate(
        "f = x => [x, ~ f (x + 1)]; inf = f 0; (inf[1][0], inf[1][1][0], inf[1][1][1][0])",
    )
    .expect("the stream should check and terminate");
    assert_eq!(out, "[1, 2, 3]: <Int, Int, Int>");
}

#[test]
fn a_tilde_n_wrap_marks_value_slots_shallow() {
    // ~2 on a plain array: the deep pass terminates (the marked value slots
    // are skipped), and the read gives the element's value with an
    // underdetermined type — the wrapped term is a lazy region, so its
    // reads never claim a concrete type that would silently mismatch it.
    let out = lichen_language::run::evaluate("([1, ~2 [2, 3]])[1][0]")
        .expect("the marked array should check");
    assert!(
        out.starts_with("[2, 3]: ?"),
        "value concrete, type underdetermined, got {out:?}"
    );
}

#[test]
fn a_tilde_one_on_a_recursive_tail_terminates() {
    // ~1 on the recursive tail: the old depth-budget descent used to loop
    // on this; the compile-time wrap cannot descend the unbound spine, so
    // the definition pass terminates and the reads stay underdetermined
    // (sound), never a guard panic.
    let out = lichen_language::run::evaluate(
        "f = x => [x, ~1 f (x + 1)]; inf = f 0; (inf[1][0], inf[1][1][0], inf[1][1][1][0])",
    )
    .expect("the marked stream should terminate");
    assert!(
        out.ends_with(": <?a, ?b, ?c>"),
        "the reads are underdetermined, got {out:?}"
    );
    assert!(out.starts_with("["), "a tuple value, got {out:?}");
}
