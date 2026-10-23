//! Probe: a kernel that declares its own buffer, and a kernel-built input.
//!
//! # Invariant
//!
//! Print-only. No `.out` group exists: the body declares the output buffer with
//! `compute.buffer`, writes it, and returns the codomain. The input of the second kernel
//! is the *first kernel's output* — a host builds data by running a kernel
//! (`docs/notes/kernel-buffer-declarations.md`). The reading is the refusal, or the value
//! a run gives: the fill kernel writes 1.0, the compute kernel writes 2.0.

use lichen_compute::language::new_store;

use super::common;

/// The program: one kernel builds its input, another declares its output.
const PROGRAM: &str = r#"
--- compute = import "compute.lichen" ---
In  = struct{.unused (compute.Buf Float)}
Out = struct{.y (compute.Buf Float)}
Fill = struct{.n Int}
FillArg = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  y = compute.buffer ((compute.Buffer _)(.count c.n, .element Float))
  ((Out)(.y y))
}
fk = compute.parallel fill "cpu" Out
filled = (compute.plrun fk ((FillArg)(.n 2)) : Out)
Par = struct{.n Int, .in In}
Arg = struct{.n Int, .I In}
raw = (c : Par) => {
  i = compute.range c.n
  y = compute.buffer ((compute.Buffer _)(.count c.n, .element Float))
  ((Out)(.y (compute.write ((compute.Write _)(.to y, .at i, .value 2.0)))))
}
k = compute.parallel raw "cpu" Out
out = (compute.plrun k ((Arg)(.n 2, .I In(.unused filled.y))) : Out)
compute.collect out.y : array{Float, 2}
"#;

#[test]
fn probe_a_kernel_that_declares_its_output() {
    let _slot = common::slot();
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(PROGRAM, None, &mut store) {
        Ok(text) => println!("[declared] answered {text}"),
        Err(diagnostics) => println!(
            "[declared] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}
