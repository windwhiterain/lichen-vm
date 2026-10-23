//! A placeholder callee: the expected type decides it.
//! See docs/notes/function-normalization.md §3.7.

use lichen_highlevel::diagnostic::DiagKind;

use lichen_language::compile;
use lichen_language::diag::Stage;
use lichen_language::run::evaluate;

/// The rendered answer of a program that must check.
fn output(source: &str) -> String {
    evaluate(source).expect("the program runs clean")
}

/// The single checker diagnostic of a refused program, and its kind.
fn refused(source: &str) -> (String, DiagKind) {
    let report = compile(source);
    assert!(!report.ok(), "expected {source:?} to be refused");
    let first = report.diagnostics.first().expect("a diagnostic");
    assert_eq!(first.stage, Stage::Check, "a check-stage refusal");
    let kind = first.check.as_ref().expect("a checker diagnostic").kind;
    (first.message.clone(), kind)
}

/// A **named** type-function parameter: `_x` binds, and `x.type`/`x.length`
/// read the parameter's own fields.
#[test]
fn a_named_type_function_parameter_and_its_field_reads() {
    assert_eq!(
        output(
            "Param = _x => struct{.type _, .length _}\n\
             buf = (x : (Param _)) => (x.type, x.length)\n\
             buf ((Param _)(.type Float, .length 192))"
        ),
        "(Float, 192): tuple{Type, Int}",
        "the two reads on `(Param _)` answer the instance's own fields"
    );
}

/// The instantiation whose callee is the placeholder: the call's expected
/// type decides it.
#[test]
fn an_instantiation_whose_callee_is_inferred() {
    assert_eq!(
        output(
            "Param = _x => struct{.type _, .length _}\n\
             BufT  = x : (Param _) => struct{.type x.type, .length x.length}\n\
             buf   = (x : (Param _)) => x\n\
             e     = buf _(.type Float, .length 192)\n\
             e"
        ),
        "(Float, 192): struct{.type Type, .length Int}",
        "the callee is the parameter's own type, so the instance is that type"
    );
}

/// A context that decides nothing still refuses by name: the placeholder is
/// not guessed into a struct type.
#[test]
fn an_instantiation_whose_callee_is_not_decided_is_refused() {
    let (message, kind) =
        refused("Param = _x => struct{.type _, .length _}\ne = _(.type Float, .length 192)");
    assert_eq!(kind, DiagKind::InstantiateCalleeUndetermined);
    assert_eq!(
        message,
        "the callee of an instantiation is not determined here — name the struct \
         type where the instantiation is written"
    );
    // A decided expected type that is not a struct is a non-struct callee.
    let (message, kind) = refused("f = (x : Int) => x\ne = f _(.a 1)");
    assert_eq!(kind, DiagKind::InstantiateCallee);
    assert!(
        message.contains("must be a struct type"),
        "the decided callee names what it is: {message}"
    );
}
