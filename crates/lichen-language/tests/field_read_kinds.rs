//! `a(k)` is the tuple read and every struct field is named.
//! See eval-before-unify.md §2.2.

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

// --- `a(k)` is the tuple read --------------------------------------------

/// A parameter container: the apply that supplies it reconciles the pin —
/// the `Runtime` tier, not a `Guard`.
#[test]
fn a_paren_read_of_a_deferred_array_is_refused_at_the_application() {
    let (message, kind) = refused("f = x => x(0)\nf [10, 20]");
    assert_eq!(kind, DiagKind::Runtime);
    assert!(
        message.contains("expected <?a, …>") && message.contains("array<Int, 2>"),
        "the requirement is the open tuple type, the found side the array: {message}"
    );
}

/// A decided container is refused where it stands.
#[test]
fn a_paren_read_of_a_decided_array_is_refused() {
    let (message, kind) = refused("l = [10, 20]\nl(0)");
    assert_eq!(kind, DiagKind::Guard);
    assert!(
        message.contains("expected <?a, …>") && message.contains("array<Int, 2>"),
        "the same stated requirement: {message}"
    );
}

/// A struct instance is not a tuple, and its fields are read by name.
#[test]
fn a_paren_read_of_a_struct_instance_is_refused() {
    let (message, kind) = refused("S = struct<.x Int, .y Type>\ns = S(.x 1, .y Int)\ns(0)");
    assert_eq!(kind, DiagKind::Guard);
    assert!(
        message.contains("expected <?a, …>") && message.contains("struct<.x Int, .y Type>"),
        "the requirement is the tuple type, the found side the struct: {message}"
    );
}

/// The read itself is unaffected: a tuple's slot reads, at any arity.
#[test]
fn a_paren_read_of_a_tuple_resolves() {
    assert_eq!(output("l = (10, 20)\nl(0)"), "10: Int");
    assert_eq!(output("l = (10, 20, 30)\nl(2)"), "30: Int");
}

/// The array read is the mirror, refused on a tuple.
#[test]
fn a_bracket_read_of_a_tuple_is_refused() {
    let (message, kind) = refused("f = x => x[0]\nf (10, 20)");
    assert_eq!(
        kind,
        DiagKind::Runtime,
        "the same apply tier the tuple read uses"
    );
    assert!(
        message.contains("expected array") && message.contains("found <Int, Int>"),
        "the array pin refuses a tuple: {message}"
    );
}

// --- every struct field is named -----------------------------------------

/// An unnamed field has no read, so the definition is refused.
#[test]
fn an_unnamed_struct_field_is_refused() {
    let report = compile("A = struct<Int, Type>\nA");
    assert!(!report.ok(), "the definition is refused");
    let first = &report.diagnostics[0];
    assert_eq!(
        first.check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::StructFieldName
    );
    assert_eq!(first.span, Some((1, 12)), "the caret is the unnamed field");
}

/// A block's fields are its bindings; a bare expression is an ordinary statement.
#[test]
fn a_bare_expression_in_a_block_is_not_a_field() {
    assert_eq!(output("a = { 1; x = 2 }\na"), "(2,): struct<.x Int>");
    assert_eq!(output("a = { 1; x = 2 }\na.x"), "2: Int");
}

/// The named forms are untouched, and a struct instance reads by name.
#[test]
fn a_named_struct_and_a_named_block_read_by_name() {
    assert_eq!(
        output("A = struct<.f Int, .g Type>\na = A(.f 1, .g Type)\n(a.f, a.g)"),
        "(1, Type): <Int, Type>"
    );
    assert_eq!(
        output("a = { x = 1; y = Int }\n(a.x, a.y)"),
        "(1, Int): <Int, Type>"
    );
}

// --- the raw reads state their kind too -----------------------------------

/// `X<e>` reads a tuple type value, so a `TypeStruct` container is refused.
#[test]
fn a_raw_index_of_a_struct_type_value_is_refused() {
    let (message, kind) = refused("struct<.a Int, .b string><0>");
    assert_eq!(kind, DiagKind::Guard);
    assert_eq!(message, "expected TypeTuple, found TypeStruct");
}

/// A tuple *value*'s type is the tuple shape, not the kind `TypeTuple`.
#[test]
fn a_raw_index_of_a_tuple_value_is_refused() {
    let (message, kind) = refused("(1, 2)<0>");
    assert_eq!(kind, DiagKind::Guard);
    assert_eq!(message, "expected TypeTuple, found <Int, Int>");
}

/// `.a` states the container's kind, `::a` its whole type; an array is
/// neither.
#[test]
fn a_named_read_of_an_array_is_refused() {
    let (message, kind) = refused("l = [10, 20]\nl.a");
    assert_eq!(kind, DiagKind::Guard);
    assert_eq!(message, "expected TypeStruct, found TypeArray");
    let (message, kind) = refused("l = [10, 20]\nl::a");
    assert_eq!(kind, DiagKind::Guard);
    assert_eq!(message, "expected TypeStruct, found array<Int, 2>");
}

/// A deferred container is refused at the argument; this used to panic inside
/// the apply.
#[test]
fn a_raw_named_read_of_a_deferred_array_is_refused_at_the_application() {
    let (message, kind) = refused("f = x => x::a\nf [10, 20]");
    assert_eq!(kind, DiagKind::Runtime);
    assert_eq!(message, "expected TypeStruct, found array<Int, 2>");
}

// --- the deferred `.a`: a registered condition, not a pin ------------------

/// The requirement is an assert-channel condition, re-checked per apply.
/// See eval-before-unify.md §6.2.
#[test]
fn a_named_read_of_a_deferred_array_is_refused_by_its_condition() {
    let report = compile("f = x => x.a\nf [10, 20]");
    assert!(!report.ok(), "the deferred read is refused");
    let diag = report
        .diagnostics
        .iter()
        .find(|d| matches!(d.check.as_ref().map(|c| c.kind), Some(DiagKind::Assert)))
        .expect("the requirement is a check diagnostic");
    assert_eq!(diag.stage, Stage::Check);
    assert_eq!(diag.message, "expected a struct type, found array<Int, 2>");
    assert_eq!(diag.span, Some((1, 5)), "the caret is the container");
}

/// A decided container registers no condition at all, so nothing the decided
/// tier accepts is refused here.
#[test]
fn a_named_read_of_a_deferred_struct_is_accepted() {
    assert_eq!(
        output("S = struct<.a Int, .b string>\nf = x => x.a\nf (S(.a 1, .b \"h\"))"),
        "1: Int"
    );
    assert_eq!(
        output("f = k => (k.a, k.b)\nf { a = 1; b = 2 }"),
        "(1, 2): <Int, Int>"
    );
}

/// A deferred tuple and a deferred atomic type are the same requirement
/// failing.
#[test]
fn a_named_read_of_a_deferred_non_struct_is_refused() {
    for (source, found) in [
        ("f = x => x.a\nf (1, 2)", "<Int, Int>"),
        ("f = x => x.a\nf 5", "Int"),
    ] {
        let report = compile(source);
        assert!(!report.ok(), "{source:?} is refused");
        let diag = report
            .diagnostics
            .iter()
            .find(|d| matches!(d.check.as_ref().map(|c| c.kind), Some(DiagKind::Assert)))
            .unwrap_or_else(|| panic!("{source:?} carries the requirement's diagnostic"));
        assert_eq!(
            diag.message,
            format!("expected a struct type, found {found}")
        );
    }
}
