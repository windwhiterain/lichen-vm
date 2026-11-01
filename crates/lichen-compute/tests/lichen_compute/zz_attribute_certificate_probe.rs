//! Probe: the attribute certificate the remaining perspective tests read.
//!
//! # Invariant
//!
//! Print-only. Each program prints every diagnostic with its span, the check kind
//! and the two conflicting node values, so a failure names the site that refused.

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
fn probe_the_field_reference_certificate() {
    let _ = std::panic::catch_unwind(|| {
        // The write's shape, inline: the statement names the field's own slot.
        show(
            "inline field reference",
            "---\n  compute = import \"compute.lichen\"\n---\nT := {.to @any : Int}\nw := (x = T) => {r := x.to; r = (r # (compute.perspective_of x.to))}\n(w (T{.to (0 # 32)})) # 0\n",
        );
        // The same call, demanded at the field's own perspective.
        show(
            "inline field reference admitted",
            "---\n  compute = import \"compute.lichen\"\n---\nT := {.to @any : Int}\nw := (x = T) => {r := x.to; r = (r # (compute.perspective_of x.to))}\n(w (T{.to (0 # 32)})) # 32\n",
        );
    });
}
