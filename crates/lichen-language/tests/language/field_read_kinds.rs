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
        undecided_read(
            "f := x => x(0)\nA := _x => [@any, 2]\nf A[10, 20]",
            "tuple read"
        ),
        Some((1, 6)),
        "the caret is the container, not the read"
    );
}

/// A decided container is refused where it stands, naming both **kinds**.
#[test]
fn a_paren_read_of_a_decided_array_is_refused() {
    let (message, kind) = refused("A := _x => [@any, 2]\nl := A[10, 20]\nl(0)");
    assert_eq!(kind, DiagKind::Guard);
    assert!(
        message.contains("Tuple") && message.contains("Array"),
        "the same stated requirement: {message}"
    );
}

/// A struct instance is not a tuple, and its fields are read by name.
#[test]
fn a_paren_read_of_a_struct_instance_is_refused() {
    let (message, kind) =
        refused("S := _x => {.x @any : Int, .y @any : Type}\ns := S{.x 1, .y Int}\ns(0)");
    assert_eq!(kind, DiagKind::Guard);
    assert!(
        message.contains("Tuple") && message.contains("Struct"),
        "the requirement is the tuple kind, the found side the struct's: {message}"
    );
}

/// The read itself is unaffected: a tuple's slot reads, at any arity.
#[test]
fn a_paren_read_of_a_tuple_resolves() {
    assert_eq!(output("l := (10, 20)\nl(0)"), "10: Int");
    assert_eq!(output("l := (10, 20, 30)\nl(2)"), "30: Int");
}

/// The array read is the mirror, refused on a tuple. The parameter's written
/// type is what decides the container.
#[test]
fn a_bracket_read_of_a_tuple_is_refused() {
    let (message, kind) =
        refused("T := _x => (@any : Int, @any : Int)\nf := (x <: T) => x[0]\nf (10, 20)");
    assert_eq!(
        kind,
        DiagKind::Guard,
        "the same decided tier the tuple read uses"
    );
    assert!(
        message.contains("expected Array") && message.contains("Tuple"),
        "the array requirement refuses a tuple: {message}"
    );
}

// --- every struct field is named -----------------------------------------

/// An unnamed brace group is a block, not a struct instance, so it checks.
#[test]
fn an_unnamed_struct_field_is_refused() {
    let report = compile("A := {Int, Type}\nA");
    assert!(
        report.ok(),
        "the brace group is a block: {:?}",
        report.diagnostics
    );
}

/// A block's fields are its bindings; a bare expression is an ordinary statement.
#[test]
fn a_bare_expression_in_a_block_is_not_a_field() {
    assert_eq!(output("a := { 1; x := 2 }\na"), "(2,): Struct");
    assert_eq!(output("a := { 1; x := 2 }\na.x"), "2: Int");
}

/// The named forms are untouched, and a struct instance reads by name.
#[test]
fn a_named_struct_and_a_named_block_read_by_name() {
    assert_eq!(
        output("A := _x => {.f @any : Int, .g @any : Type}\na := A{.f 1, .g Type}\n(a.f, a.g)"),
        "(1, Type): Tuple"
    );
    assert_eq!(
        output("a := { x := 1; y := Int }\n(a.x, a.y)"),
        "(1, Int): Tuple"
    );
}

/// `.a` states the container's kind; an array is not a struct.
#[test]
fn a_named_read_of_an_array_is_refused() {
    let (message, kind) = refused("A := _x => [@any, 2]\nl := A[10, 20]\nl.a");
    assert_eq!(kind, DiagKind::Guard);
    assert_eq!(message, "expected Struct, found Array");
}

// --- the deferred `.a`: no read until the container's type is decided ------

/// A deferred container is refused at the read itself, so nothing is evaluated
/// behind it.
#[test]
fn a_named_read_of_a_deferred_array_is_refused_at_the_read() {
    assert_eq!(
        undecided_read(
            "f := x => x.a\nA := _x => [@any, 2]\nf A[10, 20]",
            "field read",
        ),
        Some((1, 6)),
        "the caret is the container, not the read"
    );
}

/// A decided container registers no condition at all, so nothing the decided
/// tier accepts is refused here.
#[test]
fn a_named_read_of_a_deferred_struct_is_accepted() {
    assert_eq!(
        output(
            "S := _x => {.a @any : Int, .b @any : string}\nf := (x <: S) => x.a\nf (S{.a 1, .b \"h\"})"
        ),
        "1: Int"
    );
    // A block's type is decided where it stands; its struct type is nominal
    // per occurrence, so no annotation names it.
    assert_eq!(
        output("k := { a := 1; b := 2 }\n(k.a, k.b)"),
        "(1, 2): Tuple"
    );
}

/// A deferred tuple and a deferred atomic type are the same refusal: neither
/// has a read.
#[test]
fn a_named_read_of_a_deferred_non_struct_is_refused() {
    for source in ["f := x => x.a\nf (1, 2)", "f := x => x.a\nf 5"] {
        assert_eq!(
            undecided_read(source, "field read"),
            Some((1, 6)),
            "{source:?} names the container the read would have used"
        );
    }
}
