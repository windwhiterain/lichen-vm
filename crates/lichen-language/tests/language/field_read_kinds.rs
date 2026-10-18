//! `a(k)` is the tuple read and every struct field is named.
//! See docs/notes/undecided-reads.md.

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

/// A read on an undecided container: the form the refusal names, and the
/// caret, which is the container.
fn undecided_read(source: &str, form: &str) -> Option<(u32, u32)> {
    let report = compile(source);
    assert!(!report.ok(), "expected {source:?} to be refused");
    let first = report.diagnostics.first().expect("a diagnostic");
    assert_eq!(first.stage, Stage::Check, "a check-stage refusal");
    let check = first.check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::UndecidedContainer);
    assert_eq!(check.field.as_deref(), Some(form), "the read form");
    assert_eq!(
        first.message,
        format!(
            "this {form} cannot be checked: the container's type is not decided \
             here — annotate the container's type"
        )
    );
    first.span
}

// --- `a(k)` is the tuple read --------------------------------------------

/// A parameter container: the read is refused where it is checked, because
/// its type is not decided there.
#[test]
fn a_paren_read_of_a_deferred_array_is_refused_at_the_read() {
    assert_eq!(
        undecided_read("f = x => x(0)\nf [10, 20]", "tuple read"),
        Some((1, 5)),
        "the caret is the container, not the read"
    );
}

/// A decided container is refused where it stands, naming both **kinds**.
#[test]
fn a_paren_read_of_a_decided_array_is_refused() {
    let (message, kind) = refused("l = [10, 20]\nl(0)");
    assert_eq!(kind, DiagKind::Guard);
    assert!(
        message.contains("TypeTuple") && message.contains("TypeArray"),
        "the same stated requirement: {message}"
    );
}

/// A struct instance is not a tuple, and its fields are read by name.
#[test]
fn a_paren_read_of_a_struct_instance_is_refused() {
    let (message, kind) = refused("S = struct{.x Int, .y Type}\ns = S(.x 1, .y Int)\ns(0)");
    assert_eq!(kind, DiagKind::Guard);
    assert!(
        message.contains("TypeTuple") && message.contains("TypeStruct"),
        "the requirement is the tuple kind, the found side the struct's: {message}"
    );
}

/// The read itself is unaffected: a tuple's slot reads, at any arity.
#[test]
fn a_paren_read_of_a_tuple_resolves() {
    assert_eq!(output("l = (10, 20)\nl(0)"), "10: Int");
    assert_eq!(output("l = (10, 20, 30)\nl(2)"), "30: Int");
}

/// The array read is the mirror, refused on a tuple. The parameter's written
/// type is what decides the container.
#[test]
fn a_bracket_read_of_a_tuple_is_refused() {
    let (message, kind) = refused("f = (x : tuple{Int, Int}) => x[0]\nf (10, 20)");
    assert_eq!(
        kind,
        DiagKind::Guard,
        "the same decided tier the tuple read uses"
    );
    assert!(
        message.contains("expected TypeArray") && message.contains("TypeTuple"),
        "the array requirement refuses a tuple: {message}"
    );
}

// --- every struct field is named -----------------------------------------

/// An unnamed field has no read, so the definition is refused.
#[test]
fn an_unnamed_struct_field_is_refused() {
    let report = compile("A = struct{Int, Type}\nA");
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
    assert_eq!(output("a = { 1; x = 2 }\na"), "(2,): struct{.x Int}");
    assert_eq!(output("a = { 1; x = 2 }\na.x"), "2: Int");
}

/// The named forms are untouched, and a struct instance reads by name.
#[test]
fn a_named_struct_and_a_named_block_read_by_name() {
    assert_eq!(
        output("A = struct{.f Int, .g Type}\na = A(.f 1, .g Type)\n(a.f, a.g)"),
        "(1, Type): tuple{Int, Type}"
    );
    assert_eq!(
        output("a = { x = 1; y = Int }\n(a.x, a.y)"),
        "(1, Int): tuple{Int, Type}"
    );
}

// --- the type accesses state their kind too -------------------------------

/// `X{n}` reads a tuple type value, so a `TypeStruct` container is refused.
#[test]
fn a_type_access_of_a_struct_type_value_is_refused() {
    let (message, kind) = refused("struct{.a Int, .b string}{0}");
    assert_eq!(kind, DiagKind::Guard);
    assert_eq!(message, "expected TypeTuple, found TypeStruct");
}

/// A tuple *value*'s type is the tuple shape, not the kind `TypeTuple`.
#[test]
fn a_type_access_of_a_tuple_value_is_refused() {
    let (message, kind) = refused("(1, 2){0}");
    assert_eq!(kind, DiagKind::Guard);
    assert_eq!(message, "expected TypeTuple, found tuple{Int, Int}");
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
    assert_eq!(message, "expected TypeStruct, found array{Int, 2}");
}

/// A deferred container has no read at all: the type access is refused where
/// it is checked, before the apply.
#[test]
fn a_named_type_access_of_a_deferred_array_is_refused_at_the_read() {
    assert_eq!(
        undecided_read("f = x => x::a\nf [10, 20]", "named type access"),
        Some((1, 5)),
        "the caret is the container, not the access"
    );
}

// --- the deferred `.a`: no read until the container's type is decided ------

/// A deferred container is refused at the read itself, so nothing is evaluated
/// behind it.
#[test]
fn a_named_read_of_a_deferred_array_is_refused_at_the_read() {
    assert_eq!(
        undecided_read("f = x => x.a\nf [10, 20]", "field read"),
        Some((1, 5)),
        "the caret is the container, not the read"
    );
}

/// A decided container registers no condition at all, so nothing the decided
/// tier accepts is refused here.
#[test]
fn a_named_read_of_a_deferred_struct_is_accepted() {
    assert_eq!(
        output("S = struct{.a Int, .b string}\nf = (x : S) => x.a\nf (S(.a 1, .b \"h\"))"),
        "1: Int"
    );
    // A block's type is decided where it stands; its struct type is nominal
    // per occurrence, so no annotation names it.
    assert_eq!(
        output("k = { a = 1; b = 2 }\n(k.a, k.b)"),
        "(1, 2): tuple{Int, Int}"
    );
}

/// A deferred tuple and a deferred atomic type are the same refusal: neither
/// has a read.
#[test]
fn a_named_read_of_a_deferred_non_struct_is_refused() {
    for source in ["f = x => x.a\nf (1, 2)", "f = x => x.a\nf 5"] {
        assert_eq!(
            undecided_read(source, "field read"),
            Some((1, 5)),
            "{source:?} names the container the read would have used"
        );
    }
}
