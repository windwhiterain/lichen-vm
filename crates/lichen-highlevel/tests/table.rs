//! The `table(k ==> v)` literal and the `t <== k` read, which compiles
//! straight to `TableGet`; see docs/notes/record-program.md.

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::ExprId;
use lichen_highlevel::program::ProgramImpl;
use lichen_testkit::{Probe, Writer};

/// The diagnostic kinds a program reports, written by `build` and checked
/// without evaluating it: a refusal is the fact under test.
fn kinds(build: impl FnOnce(&mut Writer) -> ExprId) -> Vec<DiagKind> {
    let mut w = Writer::new();
    let root = build(&mut w);
    Probe::<ProgramImpl>::build_refusing(w.finish(root)).diagnostic_kinds()
}

/// A `table` literal probe over the default program.
fn value_of(build: impl FnOnce(&mut Writer) -> ExprId, what: &str) -> usize {
    let mut w = Writer::new();
    let root = build(&mut w);
    let mut probe = Probe::<ProgramImpl>::build(w.finish(root));
    probe.usize_of(what)
}

#[test]
fn a_table_literal_checks_and_reads_by_deep_key() {
    // The query key is a separate node group; deep-content keys make them equal.
    for (first, second, expected) in [(1usize, 2usize, 3usize), (4, 5, 6)] {
        let answer = value_of(
            |w| {
                let one = w.int("k0", first);
                let two = w.int("k1", second);
                let three = w.int("v0", expected);
                let four = w.int("k2", 1);
                let five = w.int("k3", 2);
                let six = w.int("v1", 4);
                let a = w.array("key_a", &[one, two]);
                let b = w.array("key_b", &[four, five]);
                let table = w.table("table", &[(a, three), (b, six)]);
                let p0 = w.int("p0", first);
                let p1 = w.int("p1", second);
                w.find("read", table, w.array("probe_key", &[p0, p1]))
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
        let one = w.int("one", 1);
        let two = w.int("two", 2);
        let three = w.int("three", 3);
        let table = w.table("table", &[(w.array("key", &[one, two]), three)]);
        let miss = w.array("miss", &[w.int("seven", 7), w.int("eight", 8)]);
        w.find("read", table, miss)
    });
    assert_eq!(found, vec![DiagKind::TableMiss]);
}

#[test]
fn an_undecided_key_is_dropped_with_an_error() {
    // The table is built eagerly, so a key reading the parameter — undecided
    // at build time — cannot be hashed and its entry is dropped.
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
    let found = kinds(|w| {
        let one = w.int("one", 1);
        let two = w.int("two", 2);
        let three = w.int("three", 3);
        let short = w.array("short_key", &[one, two]);
        let long = w.array("long_key", &[one, two, three]);
        let four = w.int("four", 4);
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
        let one = w.int("one", 1);
        let two = w.int("two", 2);
        let key = w.array("key", &[one, two]);
        let other = w.array("other_key", &[w.int("four", 4), w.int("five", 5)]);
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
    // The lookup's pin fixes `t`'s undecided type; the argument unify binds it.
    let answer = value_of(
        |w| {
            let t = w.param("t");
            let body = w.find("body", t, w.int("key", 1));
            let get = w.lam("get", t, body);
            let table = w.table("table", &[(w.int("key", 1), w.int("seven", 7))]);
            w.app("call", get, table)
        },
        "call",
    );
    assert_eq!(answer, 7);
}

#[test]
fn a_failed_read_key_never_phantom_matches() {
    // Both keys are failed reads: the build drops its entry and the lookup
    // misses rather than matching an unhashable key against another.
    let found = kinds(|w| {
        let out_of_bounds = w.index(
            "source_read",
            w.array("source_key", &[w.int("one", 1), w.int("two", 2)]),
            w.int("five", 5),
        );
        let table = w.table("table", &[(out_of_bounds, w.int("three", 3))]);
        let bad = w.index(
            "probe_key",
            w.array("probe_source", &[w.int("nine", 9)]),
            w.int("seven", 7),
        );
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
