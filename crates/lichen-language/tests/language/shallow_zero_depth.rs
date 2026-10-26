//! The depth `@shallow(n)` carries, and what a depth of zero earns.
//! See docs/language-spec.md §"Shallow markers".

use lichen_language::diag::Stage;
use lichen_language::program::LangProgram;
use lichen_language::{Diag, compile};

/// The diagnostics of a program the front end refused.
fn refusals(source: &str) -> Vec<Diag<LangProgram>> {
    let report = compile(source);
    assert!(!report.ok(), "{source:?} should fail");
    report.diagnostics
}

/// Assert `source` is refused at `span`, once, carrying the depth's sentence.
fn assert_refused_once(source: &str, span: (u32, u32)) {
    let d = refusals(source);
    // One, not a cascade: the refusal stands alone rather than riding on errors.
    assert_eq!(d.len(), 1, "{source:?} is refused alone: {d:?}");
    assert_eq!(d[0].stage, Stage::Resolve, "the resolver owns the spelling");
    assert_eq!(d[0].span, Some(span), "the caret is on the marker");
    let message = &d[0].message;
    assert!(
        message.contains("erased") && message.contains("at least 1"),
        "the sentence states the consequence and the way out: {message}"
    );
}

/// A depth of zero stands alone: it turns a passing program into a refusal.
#[test]
fn a_shallow_depth_of_zero_is_refused_where_it_stands_alone() {
    // The unmarked program is what a depth of zero degrades to, and it checks.
    let unmarked = compile("A := _x => [@any, 2]\nA[1, 2]");
    assert!(
        unmarked.ok(),
        "the control checks: {:?}",
        unmarked.diagnostics
    );
    assert_refused_once("A := _x => [@any, 2]\nA[@shallow(0) 1, 2]", (2, 3));
}

/// A depth of at least 1 is accepted — the bound's other side.
#[test]
fn a_shallow_depth_of_one_is_still_accepted() {
    let report = compile("A := _x => [@any, 2]\nA[@shallow(1) 1, 2]");
    assert!(report.ok(), "a depth of 1 checks: {:?}", report.diagnostics);
}

/// The bare mark carries no depth, so the bound does not reach it.
#[test]
fn a_bare_shallow_mark_is_still_accepted() {
    let report = compile("A := _x => [@any, 2]\nA[@shallow 1, 2]");
    assert!(
        report.ok(),
        "the bare mark checks: {:?}",
        report.diagnostics
    );
}

/// The resolver reports once per marker, and this stage never deduplicates.
#[test]
fn a_repeated_zero_depth_is_refused_once_at_each_marker() {
    let d = refusals("A := _x => [@any, 2]\nA[@shallow(0) 1, @shallow(0) 2]");
    assert_eq!(d.len(), 2, "one refusal per marker: {d:?}");
    assert_eq!(d[0].span, Some((2, 3)), "the first marker");
    assert_eq!(d[1].span, Some((2, 18)), "the second marker");
}
