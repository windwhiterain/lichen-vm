//! The two checks `docs/notes/eval-before-unify.md` §2.2/§2.4 closed, pinned:
//! the positional read `a(k)` is the **tuple** read — its requirement is a
//! *unify* against a tuple type, so a container that is not a tuple is refused
//! at check time or at the application that binds it, never skipped — and
//! **every struct field is named**, so a struct instance reads by name.
//!
//! Both are read *kinds*, which is why they live together: `a(k)` over a
//! struct, `a.name` over a tuple, and an unnamed field are one rule seen from
//! three sides (`docs/language-spec.md` §Indexing, §Structs).

use lichen_highlevel::diagnostic::DiagKind;

use lichen_language::compile;
use lichen_language::diag::Stage;
use lichen_language::run::evaluate;

/// The single checker diagnostic of a program that must be refused, with the
/// rendered output and the diagnostic's own kind.
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

/// The §2.2 repro's read-first order: the container is a parameter, so its type
/// is a cell at check time.  The pin is what refuses it — the application that
/// supplies the array meets the pinned tuple type — where the guard this
/// replaced was asked once and never again (the read used to be *accepted*).
/// The refusal is the apply's own parameter check (`Runtime`, the tier every
/// pin is enforced at), not a check-time `Guard`.
#[test]
fn a_paren_read_of_a_deferred_array_is_refused_at_the_application() {
    let (message, kind) = refused("f = x => x(0)\nf [10, 20]");
    assert_eq!(kind, DiagKind::Runtime);
    assert!(
        message.contains("expected <?a, …>") && message.contains("array<Int, 2>"),
        "the requirement is the open tuple type, the found side the array: {message}"
    );
}

/// A container the graph has already decided is refused where it stands, and
/// the expected side is the same open tuple spelling.
#[test]
fn a_paren_read_of_a_decided_array_is_refused() {
    let (message, kind) = refused("l = [10, 20]\nl(0)");
    assert_eq!(kind, DiagKind::Guard);
    assert!(
        message.contains("expected <?a, …>") && message.contains("array<Int, 2>"),
        "the same stated requirement: {message}"
    );
}

/// A struct instance is not a tuple: the read that used to reach its field list
/// is refused, and the instance's fields are read by name instead.
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

/// The array read is the mirror, and it is refused on a tuple — the pair §2.4
/// measured (`expected array<…>, found <Int, Int>`), still true after the fix.
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

/// `struct<Int, Type>` — an unnamed field has no read (a struct instance reads
/// by name), so the definition is refused, with the caret on that field.
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

/// A block's fields are its **bindings**: a bare expression is an ordinary
/// statement — checked, its value discarded — so it is not a field, and the
/// record holds the named one alone.
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
