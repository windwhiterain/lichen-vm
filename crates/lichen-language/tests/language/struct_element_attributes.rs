//! A struct field's `# n` requirement and its `v : T` pin reach the arguments
//! filling it. See compound-element-slots.md.

use lichen_highlevel::diagnostic::DiagKind;

use lichen_language::compile;
use lichen_language::diag::Stage;
use lichen_language::run::evaluate;

/// The single checker diagnostic of a refused program, and its kind.
fn refused(source: &str) -> (String, DiagKind) {
    let report = compile(source);
    assert!(!report.ok(), "expected {source:?} to be refused");
    let first = report.diagnostics.first().expect("a diagnostic");
    assert_eq!(first.stage, Stage::Check, "a check-stage refusal");
    let kind = first.check.as_ref().expect("a checker diagnostic").kind;
    (first.message.clone(), kind)
}

/// The rendered `value: type` output of a program that must check.
fn output(source: &str) -> String {
    evaluate(source).expect("the program runs clean")
}

#[test]
fn a_field_requirement_refuses_an_argument_that_misses_it() {
    let (message, kind) = refused("S = struct{.a Int # 4}\nS(.a 1 # 2)");
    assert_eq!(kind, DiagKind::Attribute);
    assert_eq!(message, "expected 4, found 2");
}

#[test]
fn a_field_requirement_holds_for_an_argument_that_meets_it() {
    assert_eq!(
        output("S = struct{.a Int # 4}\nS(.a 1 # 4)"),
        "(1,): struct{.a Int # 4}"
    );
}

#[test]
fn a_field_pin_refuses_the_value_it_names() {
    let (message, kind) = refused("S = struct{.a 5 : Int}\nS(.a 6)");
    assert_eq!(kind, DiagKind::Annotation);
    assert_eq!(message, "expected 5, found 6");
}

/// The pairing is the **definition's** order, not the call's: the argument
/// written second supplies field `.a`.
#[test]
fn a_named_argument_reconciles_against_its_own_field() {
    let (message, _) = refused("S = struct{.a 5 : Int, .b 7 : Int}\nS(.b 8, .a 6)");
    assert_eq!(message, "expected 5, found 6");
    assert_eq!(
        output("S = struct{.a 5 : Int, .b 7 : Int}\nS(.b 7, .a 5)"),
        "(5, 7): struct{.a 5 : Int, .b 7 : Int}"
    );
}

/// A struct type may name itself or a cycle of siblings, so the walk must
/// visit each pair once.
#[test]
fn a_recursive_struct_type_reconciles_and_terminates() {
    assert!(
        output(
            "A = struct{.f Int, .g B}
             B = struct{.f Type, .g A}
             a = A(1, b)
             b = B(Int, a)
             (a, b)"
        )
        .starts_with("(("),
        "the mutually recursive instances still evaluate"
    );
}
