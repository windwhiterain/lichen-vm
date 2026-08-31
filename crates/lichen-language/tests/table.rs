//! End-to-end table tests: the `table { [k, v], … }` literal, deep-content
//! keys, the `t{k}` lookup (compiled straight to the lowlevel `TableGet` —
//! the container's type is pinned to a table, so the operator comes from
//! the syntax, never a runtime kind dispatch), and the recorded failures
//! (a miss, an unbound key dropped at build, key/value type mismatches).

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::program::HighProgramValue;
use lichen_lowlevel::LowValue;
use lichen_language::compile;

fn evaluate(source: &str) -> HighProgramValue {
    let report = compile(source);
    assert!(
        report.ok(),
        "expected {source:?} to check, got: {:?}",
        report.diagnostics
    );
    let build = report.build.unwrap();
    let (mut module, root) = (build.module, build.root_val);
    module.evaluate_node_deep(root, None)
}

fn usize_of(value: &HighProgramValue) -> usize {
    let HighProgramValue::LowValue(LowValue::USize(n)) = value else {
        panic!("expected a usize value, got {value:?}");
    };
    *n
}

/// The rendered diagnostics of a failing program.
fn diags(source: &str) -> Vec<lichen_language::Diag> {
    let report = compile(source);
    assert!(!report.diagnostics.is_empty(), "{source:?} should fail");
    report.diagnostics
}

fn has_check_kind(source: &str, kind: DiagKind) -> bool {
    diags(source)
        .iter()
        .any(|d| d.check.as_ref().is_some_and(|c| c.kind == kind))
}

#[test]
fn a_table_literal_checks_and_reads_by_deep_key() {
    // The query key `[1, 2]` is a *separate* node group from the stored
    // key — the deep-content key semantics make them the same key.  The
    // lookup syntax `t{...}` compiles straight to `TableGet`.
    assert_eq!(
        usize_of(&evaluate("t = table { [[1, 2], 3], [[4, 5], 6] }; t{[1, 2]}")),
        3
    );
    assert_eq!(
        usize_of(&evaluate("t = table { [[1, 2], 3], [[4, 5], 6] }; t{[4, 5]}")),
        6
    );
}

#[test]
fn an_empty_table_misses() {
    let d = diags("t = table {}; t{1}");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::TableMiss);
}

#[test]
fn a_miss_is_a_recorded_error() {
    let d = diags("t = table { [[1, 2], 3] }; t{[7, 8]}");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::TableMiss);
}

#[test]
fn an_unbound_key_is_dropped_with_an_error() {
    // A table literal inside a function body whose key reads the parameter
    // cannot be forced concrete at build time — the entry is dropped and
    // the failure recorded.
    assert!(has_check_kind(
        "f = x => table { [x, 1] }; f 5",
        DiagKind::TableKeyUnbound
    ));
}

#[test]
fn table_keys_share_one_type() {
    assert!(has_check_kind(
        "t = table { [[1, 2], 3], [[1, 2, 3], 4] }",
        DiagKind::TableKey
    ));
}

#[test]
fn table_values_share_one_type() {
    assert!(has_check_kind(
        "t = table { [[1, 2], 3], [[4, 5], Int] }",
        DiagKind::TableValue
    ));
}

#[test]
fn a_table_flows_through_a_function() {
    // A table literal inside a function body: the key is concrete at build,
    // the value stays a lazy reference to the parameter, and the apply
    // clones the table (entries re-pointed at the call's clones).
    assert_eq!(
        usize_of(&evaluate("f = x => table { [1, x] }; (f 5){1}")),
        5
    );
}

#[test]
fn a_table_behind_a_parameter_reads_through_tableget() {
    // `t`'s type is unbound at the read site — the lookup's pin fixes it
    // to a table type, and the argument unify binds the pinned key/value
    // cells when the call resolves.
    assert_eq!(
        usize_of(&evaluate("get = t => t{1}; t = table { [1, 7] }; get t")),
        7
    );
}

#[test]
fn a_non_pair_entry_is_a_parse_error() {
    let d = diags("t = table { 5 }; 0");
    assert!(
        d.iter().any(|diag| diag.check.is_none()),
        "the malformed entry is a parse diagnostic: {d:?}"
    );
}
