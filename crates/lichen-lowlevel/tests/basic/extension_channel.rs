//! The extension channel: one list, both kinds, the flag the verdict reads.

use lichen_lowlevel::ExtensionDiagnostic;

use crate::*;

/// # Invariant
/// A note and a refusal are two records in one list, told apart only by
/// `fails_build`, and the verdict predicate answers for the refusal alone.
#[test]
fn one_list_holds_both_kinds_and_the_verdict_reads_only_the_flag() {
    let mut m = Module::<TestProgram>::new();

    m.record_extension_diagnostic(ExtensionDiagnostic::note("compute.graph", None, "a note"));
    assert!(
        !m.has_extension_failures(),
        "a note leaves the build usable"
    );

    m.record_extension_diagnostic(ExtensionDiagnostic::failure(
        "compute.graph",
        None,
        "a refusal",
    ));

    assert!(
        m.has_extension_failures(),
        "the verdict follows the flag, not the list"
    );
    assert_eq!(
        m.extension_diagnostics
            .iter()
            .map(|entry| entry.fails_build)
            .collect::<Vec<_>>(),
        vec![false, true],
        "both kinds are in the one container, in insertion order"
    );
}
