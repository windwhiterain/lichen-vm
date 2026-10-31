//! Probe: the attribute certificate the three remaining perspective tests read.
//!
//! # Invariant
//!
//! Print-only. Each program prints every diagnostic with its span, the check kind
//! and the two conflicting node values, so a failure names the site that refused.
//!
//! The measured finding (`docs/notes/handoff-remaining-six.md`, round 28): a
//! **statement's** stated attribute is not what a call site reads. A body whose
//! value is an annotated expression carries its attribute (`x => (x # 32)` is
//! refused `expected 64, found 32`), while the identical claim written as a
//! statement (`x => {r := x; r = (r # 32)}`) is dropped, because the statement's
//! attribute never reaches the returned value's expr.

use super::common;

use lichen_compute::language::LangProgram;
use lichen_language::Report;

/// Every diagnostic, with the facts the checker recorded.
fn print(label: &str, report: &Report<LangProgram>) {
    println!("=== {label}: ok={} ===", report.ok());
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        println!(
            "  [{index}] span={:?} stage={:?} msg={}",
            diagnostic.span, diagnostic.stage, diagnostic.message
        );
        if let Some(check) = diagnostic.check.as_deref() {
            println!(
                "      kind={:?} loc={:?} a={:?} b={:?} value_a={:?} value_b={:?}",
                check.kind, check.loc, check.a, check.b, check.value_a, check.value_b
            );
        }
    }
}

fn show(label: &str, source: &str) {
    let report = common::check(source);
    print(label, &report);
}

#[test]
fn probe_the_statement_return_certificate() {
    let _ = std::panic::catch_unwind(|| {
        show(
            "annotated return",
            "g := x => (x # 32)\n(g 0) # 64\n",
        );
        show(
            "statement return",
            "g := x => {r := x; r = (r # 32)}\n(g 0) # 64\n",
        );
        show(
            "statement return admitted",
            "g := x => {r := x; r = (r # 32)}\n(g 0) # 32\n",
        );
        show(
            "statement return narrow",
            "g := x => {r := x; r = (r # 32)}\n(g 0) # 1\n",
        );
    });
}
