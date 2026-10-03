//! End-to-end tests for the deferred unification a type read goes through
//! when a struct type is built from a partially applied type function
//! (docs/notes/defer-pending-type-forms.md): the read resolves against the
//! argument's field type, the merge pins the type value, and the field type
//! prints decided instead of leaking an undecided layout.

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;

/// Compile and run `source`, returning the rendered `value: type` output.
fn run(source: &str) -> String {
    let mut store = PackageStore::<LangProgram>::new();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .unwrap_or_else(|diags| panic!("expected {source:?} to check and run, got: {diags:?}"))
}

/// A single field read (the silent half of the defect): the field type used
/// to merge against the argument's type and stay undecided (`?a`); the pin
/// commits the type value now.
#[test]
fn a_deferred_field_read_binds_the_type_value() {
    let out = run(r#"
P = ins => struct<.I ins.x>
y = (P _)(.I Int)
"#);
    assert!(
        out.ends_with("struct<.I Type>>"),
        "the field type decides to Type: {out}"
    );
}

/// Two reads of one unbound placeholder (the error half of the defect): both
/// reads defer against the same argument field types and both decide.
#[test]
fn two_deferred_field_reads_both_bind() {
    let out = run(r#"
P = ins => struct<.I ins.x, .O ins.y>
y = (P _)(.I Int, .O Int)
"#);
    assert!(
        out.ends_with("struct<.I Type, .O Type>>"),
        "both field types decide to Type: {out}"
    );
}

/// A struct type value in the slot: its kind carries a names table, so the
/// class is never a skeleton — this row used to reach the spurious
/// `expected [?a], found TypeStruct` error at every arity.
#[test]
fn a_deferred_field_read_binds_a_struct_type_value() {
    let out = run(r#"
P = ins => struct<.I ins.x>
y = (P _)(.I struct<.a Int>)
"#);
    assert!(
        out.ends_with("struct<.I TypeStruct>>"),
        "the field type decides to TypeStruct: {out}"
    );
}

/// The `type_of` spelling: `type_of` is an ordinary generic function, so the
/// pending side of the stall is a lazy Apply, not an Index read — the
/// deferral gate must cover calls too.
#[test]
fn a_deferred_type_of_call_binds_the_type_value() {
    let out = run(r#"
P = ins => struct<.I type_of ins.x>
y = (P _)(.I struct<.a Int>)
"#);
    assert!(
        out.ends_with("struct<.I TypeStruct>>"),
        "the field type decides to TypeStruct through the lazy call: {out}"
    );
}
