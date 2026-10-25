//! The `table(k ==> v)` literal and the `t <== k` read; see
//! record-program.md.

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprId, ExprKind};
use lichen_highlevel::program::{AnyLit, HighProgramLiteral, ProgramImpl};
use lichen_testkit::{Probe, Writer};

/// The diagnostic kinds a program reports, written by `build`.
///
/// # Invariant
/// Never evaluated: a refusal is the fact under test, and evaluating a program
/// that was refused reads the graph it never finished building.
fn kinds(build: impl FnOnce(&Writer) -> ExprId) -> Vec<DiagKind> {
    let w = Writer::new();
    let root = build(&w);
    Probe::<ProgramImpl>::build_refusing(w.finish(root)).diagnostic_kinds()
}

/// The `usize` a named expression evaluated to.
fn value_of(build: impl FnOnce(&Writer) -> ExprId, what: &str) -> usize {
    let w = Writer::new();
    let root = build(&w);
    let mut probe = Probe::<ProgramImpl>::build(w.finish(root));
    probe.usize_of(what)
}

/// An array instance of `values` — the compound key a table hashes by content.
///
/// # Invariant
/// The element template is the wildcard bound to `Int`, so the array states
/// its element type in the template and not in its type: every array shares
/// the one `Array` layout.
///
/// The instance's own content holds that wildcard, so the instance is
/// **undecided** as a whole: a table drops such a key rather than hashing it,
/// and this helper is for the keys whose *type* is what the test reads.
fn int_array(w: &Writer, name: &str, values: &[usize]) -> ExprId {
    let literal = w.hole(&format!("{name}_element_value"));
    w.fill(literal, ExprKind::Literal(HighProgramLiteral::from(AnyLit)));
    let element = w.annotate(
        &format!("{name}_element"),
        literal,
        w.int_type(&format!("{name}_int_ty")),
    );
    let template = w.array_template(
        &format!("{name}_template"),
        element,
        w.int(&format!("{name}_len"), values.len()),
    );
    let elements: Vec<ExprId> = values
        .iter()
        .enumerate()
        .map(|(index, value)| w.int(&format!("{name}_{index}"), *value))
        .collect();
    w.array_inst(name, template, &elements)
}

/// A tuple of `values` — the compound key a table hashes by content.
///
/// # Invariant
/// The tuple's value is the elements' own terms, so its content is decided
/// and the key can be hashed.
fn int_tuple(w: &Writer, name: &str, values: &[usize]) -> ExprId {
    let elements: Vec<ExprId> = values
        .iter()
        .enumerate()
        .map(|(index, value)| w.int(&format!("{name}_{index}"), *value))
        .collect();
    w.tuple(name, &elements)
}

#[test]
fn a_table_literal_checks_and_reads_by_deep_key() {
    // The query key is a separate node group; deep-content keys make them equal.
    for (first, second, expected) in [(1usize, 2usize, 3usize), (4, 5, 6)] {
        let answer = value_of(
            |w| {
                let three = w.int("v0", expected);
                let six = w.int("v1", 4);
                let a = int_tuple(w, "key_a", &[first, second]);
                let b = int_tuple(w, "key_b", &[1, 2]);
                let table = w.table("table", &[(a, three), (b, six)]);
                let probe_key = int_tuple(w, "probe_key", &[first, second]);
                w.find("read", table, probe_key)
            },
            "read",
        );
        assert_eq!(
            answer, expected,
            "the query key must match by content, not by identity"
        );
    }
}

#[test]
fn an_empty_table_misses() {
    let found = kinds(|w| {
        let table = w.table("table", &[]);
        let key = w.int("key", 1);
        w.find("read", table, key)
    });
    assert_eq!(found, vec![DiagKind::TableMiss]);
}

#[test]
fn a_miss_is_a_recorded_error() {
    let found = kinds(|w| {
        let three = w.int("three", 3);
        let key = int_tuple(w, "key", &[1, 2]);
        let table = w.table("table", &[(key, three)]);
        let miss = int_tuple(w, "miss", &[7, 8]);
        w.find("read", table, miss)
    });
    assert_eq!(found, vec![DiagKind::TableMiss]);
}

#[test]
fn an_undecided_key_is_dropped_with_an_error() {
    // The table is built eagerly, so an undecided key cannot be hashed.
    let found = kinds(|w| {
        let x = w.param("x");
        let table = w.table("table", &[(x, w.int("one", 1))]);
        let f = w.lam("f", x, table);
        w.app("call", f, w.int("five", 5))
    });
    assert!(
        found.contains(&DiagKind::TableKeyUndecided),
        "the entry is dropped rather than hashed from an undecided key: {found:?}"
    );
}

#[test]
fn table_keys_share_one_type() {
    // An array's length is no longer part of its type, so the two keys
    // differ by their **layout**: an array and a tuple.
    let found = kinds(|w| {
        let three = w.int("three", 3);
        let four = w.int("four", 4);
        let short = int_array(w, "short_key", &[1, 2]);
        let long = w.tuple("long_key", &[w.int("one", 1), w.int("two", 2), three]);
        w.table("table", &[(short, w.int("v0", 3)), (long, four)])
    });
    assert!(
        found.contains(&DiagKind::TableKey),
        "the keys must share one type: {found:?}"
    );
}

#[test]
fn table_values_share_one_type() {
    let found = kinds(|w| {
        let key = int_array(w, "key", &[1, 2]);
        let other = int_array(w, "other_key", &[4, 5]);
        let type_value = w.int_type("type_value");
        w.table("table", &[(key, w.int("v0", 3)), (other, type_value)])
    });
    assert!(
        found.contains(&DiagKind::TableValue),
        "the values must share one type: {found:?}"
    );
}

#[test]
fn a_find_on_a_concretely_non_table_container_is_a_guard_error() {
    // The lookup pins the container's type to a table, so `1 <== 2` fails.
    let found = kinds(|w| {
        let one = w.int("one", 1);
        let two = w.int("two", 2);
        w.find("read", one, two)
    });
    assert!(
        found.contains(&DiagKind::Guard),
        "an int is not a table: {found:?}"
    );
}

#[test]
fn a_table_flows_through_a_function() {
    // The apply clones the table, re-pointing its entries at the call's clones.
    let answer = value_of(
        |w| {
            let x = w.param("x");
            let table = w.table("table", &[(w.int("key", 1), x)]);
            let f = w.lam("f", x, table);
            let applied = w.app("applied", f, w.int("five", 5));
            w.find("read", applied, w.int("probe_key", 1))
        },
        "read",
    );
    assert_eq!(answer, 5);
}

#[test]
fn a_table_behind_a_parameter_reads_through_tableget() {
    // The parameter's type is decided at the read; the pin checks it is a table.
    let answer = value_of(
        |w| {
            let t = w.param("t");
            let body = w.find("body", t, w.int("key", 1));
            let key_ty = w.int_type("key_ty");
            let value_ty = w.int_type("value_ty");
            let table_ty = w.type_table("table_ty", key_ty, value_ty);
            let get = w.lam_typed("get", t, Some(table_ty), body, false);
            let table = w.table("table", &[(w.int("key", 1), w.int("seven", 7))]);
            w.app("call", get, table)
        },
        "call",
    );
    assert_eq!(answer, 7);
}

#[test]
fn a_failed_read_key_never_phantom_matches() {
    // Both keys are failed reads: the entry is dropped and the lookup misses.
    let found = kinds(|w| {
        let source_key = int_array(w, "source_key", &[1, 2]);
        let out_of_bounds = w.index("source_read", source_key, w.int("five", 5));
        let table = w.table("table", &[(out_of_bounds, w.int("three", 3))]);
        let probe_source = int_array(w, "probe_source", &[9]);
        let bad = w.index("probe_key", probe_source, w.int("seven", 7));
        w.find("read", table, bad)
    });
    assert!(
        found.contains(&DiagKind::TableKeyUndecided),
        "an unhashable key is dropped: {found:?}"
    );
    assert!(
        found.contains(&DiagKind::TableMiss),
        "and the read therefore misses: {found:?}"
    );
}
