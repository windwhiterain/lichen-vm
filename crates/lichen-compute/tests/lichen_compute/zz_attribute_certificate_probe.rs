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
fn probe_the_directed_attribute_unify() {
    let _ = std::panic::catch_unwind(|| {
        // Both stated: the actual is the provider, the parameter the requirement.
        show(
            "64 provided under 32 required",
            "f := (x : Int # 32) => x\nf (0 # 64)\n",
        );
        show(
            "32 provided under 64 required",
            "f := (x : Int # 64) => x\nf (0 # 32)\n",
        );
        // A statement decides a template's open position: `r` should carry 32.
        show(
            "statement states the return",
            "g := x => {r := x; r = (r # 32)}\n(g 0) # 64\n",
        );
        // A callee that states nothing hands no certificate: the demand is the slot.
        show(
            "a silent callee hands no certificate",
            "id := x => x + 1\n(id 0) # 32\n",
        );
    });
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
