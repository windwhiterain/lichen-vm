//! Probe: the attribute certificate the remaining perspective tests read.
//!
//! # Invariant
//!
//! Print-only. Each program prints every diagnostic with its span, the check kind
//! and the two conflicting node values, so a failure names the site that refused.

use super::common;

use lichen_compute::language::LangProgram;
use lichen_language::Report;

/// The head `perspective_requirements` builds: a three-element buffer from a kernel.
fn head() -> String {
    r#"
---
  order = "30"
  compute = import "compute.lichen"
---
Out := {.b (compute.Buf (compute.BufParam{.type Int, .length 3}))}
Fill := {.n @any : Int}
fill := (c = Fill) => {
  i := compute.range c.n
  d := compute.buf (compute.BufParam{.type Int, .length c.n})
  ((Out){.b (compute.write ((compute.Write _){.to d, .at i, .value (i + 1)}))})
}
filler := compute.parallel fill "cpu" Out
b := (compute.plrun filler ((Fill){.n 3}) : Struct)
"#
    .to_string()
}

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
fn probe_the_write_perspective() {
    let _ = std::panic::catch_unwind(|| {
        let head = head();
        show(
            "write positive",
            &format!(
                "{head}\nw := compute.write ((compute.Write _){{.to (b.b # 0), .at (0 # 1), .value (0 # 1)}})\n(w # 0)\n"
            ),
        );
        show(
            "write negative",
            &format!(
                "{head}\nw := compute.write ((compute.Write _){{.to (b.b # compute.warp), .at (0 # 1), .value (0 # 1)}})\n(w # 0)\n"
            ),
        );
    });
}
