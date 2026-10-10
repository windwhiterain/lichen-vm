//! The `table { k ==> v }` literal and the `t{k}` lookup, which compiles
//! straight to `TableGet`. See record-program.md.

use lichen_highlevel::diagnostic::DiagKind;
use lichen_lowlevel::LowValue;

use lichen_language::compile;
use lichen_language::program::LangValue;

fn evaluate(source: &str) -> LangValue {
    let report = compile(source);
    assert!(
        report.ok(),
        "expected {source:?} to check, got: {:?}",
        report.diagnostics
    );
    let build = report.build.unwrap();
    let (mut module, root) = (build.module, build.root_val);
    module
        .evaluate_node_deep(root, None)
        .expect("the program's root value is undecided")
}

fn usize_of(value: &LangValue) -> usize {
    let LangValue::LowValue(LowValue::USize(n)) = value else {
        panic!("expected a usize value, got {value:?}");
    };
    *n
}

/// The rendered diagnostics of a failing program.
fn diags(source: &str) -> Vec<lichen_language::Diag<lichen_language::program::LangProgram>> {
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
    // The query key is a separate node group; deep-content keys make them equal.
    assert_eq!(
        usize_of(&evaluate(
            "t = table { [1, 2] ==> 3, [4, 5] ==> 6 }; t{[1, 2]}"
        )),
        3
    );
    assert_eq!(
        usize_of(&evaluate(
            "t = table { [1, 2] ==> 3, [4, 5] ==> 6 }; t{[4, 5]}"
        )),
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
    let d = diags("t = table { [1, 2] ==> 3 }; t{[7, 8]}");
    let check = d[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::TableMiss);
}

#[test]
fn an_undecided_key_is_dropped_with_an_error() {
    // A key reading the parameter cannot be forced concrete at build time.
    assert!(has_check_kind(
        "f = x => table{ x ==> 1 }; f 5",
        DiagKind::TableKeyUndecided
    ));
}

#[test]
fn table_keys_share_one_type() {
    assert!(has_check_kind(
        "t = table { [1, 2] ==> 3, [1, 2, 3] ==> 4 }",
        DiagKind::TableKey
    ));
}

#[test]
fn table_values_share_one_type() {
    assert!(has_check_kind(
        "t = table { [1, 2] ==> 3, [4, 5] ==> Int }",
        DiagKind::TableValue
    ));
}

#[test]
fn a_find_on_a_concretely_non_table_container_is_a_guard_error() {
    // The lookup pins the container's type to a table, so `1{2}` fails the check.
    assert!(has_check_kind("1{2}", DiagKind::Guard));
}

#[test]
fn a_table_flows_through_a_function() {
    // The apply clones the table, re-pointing its entries at the call's clones.
    assert_eq!(
        usize_of(&evaluate("f = x => table{ 1 ==> x }; (f 5){1}")),
        5
    );
}

#[test]
fn a_table_behind_a_parameter_reads_through_tableget() {
    // The lookup's pin fixes `t`'s undecided type; the argument unify binds its cells.
    assert_eq!(
        usize_of(&evaluate("get = t => t{1}; t = table { 1 ==> 7 }; get t")),
        7
    );
}

#[test]
fn a_failed_read_key_never_phantom_matches() {
    // Both keys are failed reads: the build drops its entry and the lookup misses.
    let source = "t = table{[1,2][5] ==> 3}\nt{[9][7]}";
    assert!(has_check_kind(source, DiagKind::TableKeyUndecided));
    assert!(has_check_kind(source, DiagKind::TableMiss));
}

#[test]
fn a_non_pair_entry_is_a_parse_error() {
    let d = diags("t = table { 5 }; 0");
    assert!(
        d.iter().any(|diag| diag.check.is_none()),
        "the malformed entry is a parse diagnostic: {d:?}"
    );
}
