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

#[test]
fn probe_the_body_requirement() {
    let _ = std::panic::catch_unwind(|| {
        show(
            "body requirement",
            &format!(
                "{}
use := (x = (compute.Buf (compute.BufParam{{.type Int, .length 3}}))) => compute.read ((compute.Read _){{.from x, .at 1}})
use (b.b # 1)
",
                head()
            ),
        );
    });
}
