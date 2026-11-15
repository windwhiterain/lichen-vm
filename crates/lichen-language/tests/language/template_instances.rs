//! A template's open cells belong to the instance; its decided cells are shared.
//!
//! # Invariant
//! A template's decided half is a requirement every instance shares, while an
//! undecided cell — the value slot and an undecided type cell alike — is the
//! use's own fresh cell (SPEC §Struct's `v`), so one instance never decides
//! another.

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
fn a_struct_types_wildcard_fields_stay_open_across_instances() {
    assert_eq!(
        output("D := {.a @any, .b @any}\n(D{.a 1, .b 2}, D{.a 1.5, .b 2.5})"),
        "((1, 2), (1.5, 2.5)): Tuple"
    );
}

#[test]
fn an_inline_literal_domain_is_fresh_per_application() {
    assert_eq!(
        output(
            "div := {.'dividend, .'dividor} -> dividend / dividor\n\
             (div _{.dividend 1, .dividor 2}, div _{.dividend 1.0, .dividor 2.0})"
        ),
        "(0, 0.5): Tuple"
    );
}

#[test]
fn a_function_generics_literal_domain_takes_anonymous_instances() {
    assert_eq!(
        output(
            "div := 'T -> {.'dividend: T, .'dividor: T} -> dividend / dividor\n\
             (div _ _{.dividend 1, .dividor 2}, div _ _{.dividend 1.0, .dividor 2.0})"
        ),
        "(0, 0.5): Tuple"
    );
}

#[test]
fn a_named_generic_types_instance_is_fresh_per_use() {
    assert_eq!(
        output(
            "Div := 'T -> {.dividen @any: T, .dividor @any: T}\n\
             ((Div _){.dividen 1, .dividor 2}, (Div _){.dividen 1.0, .dividor 2.0})"
        ),
        "((1, 2), (1.0, 2.0)): Tuple"
    );
}

#[test]
fn an_annotated_field_still_constrains_every_instance() {
    assert_eq!(
        output("D := {.a @any : Int}\n(D{.a 1}, D{.a 2})"),
        "((1,), (2,)): Tuple"
    );
    let (message, kind) = refused("D := {.a @any : Int}\nD{.a 1.5}");
    assert_eq!(kind, DiagKind::Annotation);
    assert_eq!(message, "expected Int, found Float");
}

/// A struct type whose field's type is another applied struct type.
#[test]
fn a_nested_instance_reads_through_the_outer_field() {
    let inner = "A := _ -> {.'x: Int, .'y: Float}\n\
                 B := _ -> {.a A _, .'z: string}\n";
    assert_eq!(
        output(&format!(
            "{inner}b := B{{.a (A{{.x 1, .y 2.0}}), .z \"s\"}}\nb.a.x"
        )),
        "1: Int",
        "the nested instance's field reads through the outer field"
    );
    assert_eq!(
        output(&format!(
            "{inner}f := {{.a A _, .b B _, .'w: Type}} -> w\n\
             f _{{.a (A{{.x 1, .y 2.0}}), .b (B{{.a (A{{.x 1, .y 2.0}}), .z \"s\"}}), .w Int}}"
        )),
        "Int: Type",
        "the nested signature admits an inline nested instance"
    );
}
