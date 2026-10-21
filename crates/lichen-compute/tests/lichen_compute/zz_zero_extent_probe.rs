//! Probe: a dispatch over extent 0, plain and inside a graph.
//!
//! # Invariant
//!
//! Print-only. The simplest kernel, `z[i] = i + 10`, launched with `.n 0`, in both forms; the
//! reading is whether an end-of-sequence flag can gate a step by driving its extent to zero.

use super::common;

use lichen_compute::language::new_store;

/// The kernel, and the launch value a zero extent leaves nothing of.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
In0  = struct{.a Int}
Out0 = struct{.z (compute.Buf _)}
Par0 = compute.P (compute.KT _)(.I In0, .O Out0)
g = (k : Par0) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value (i + 10)))
}
kg = compute.parallel g "gpu" Out0
"#;

/// The plain form: one `compute.plrun` at extent 0, then the collect.
const PLAIN: &str = r#"
out = (compute.plrun kg ((compute.A In0)(.n 0, .I In0(.a 0))) : Out0)
compute.collect out.z
"#;

/// The recorded form: the same dispatch as a graph run at extent 0.
const RECORDED: &str = r#"
GArg = struct{.n Int, .in In0}
step = (s : GArg) => {
  compute.plrun kg ((compute.A In0)(.n s.n, .I In0(.a 0))) : Out0
}
built = compute.graph step
compute.collect (compute.graphrun built 0)
"#;

/// What the raw runner renders for `body`, or how it refused.
fn rendered(body: &str) -> String {
    let source = format!("{KERNEL}{body}");
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(&source, None, &mut store) {
        Ok(text) => text,
        Err(diagnostics) => format!(
            "refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_the_zero_extent() {
    let (_stub, _guard) = common::stub();
    println!("[plain] {}", rendered(PLAIN));
    println!("[recorded] {}", rendered(RECORDED));
}
