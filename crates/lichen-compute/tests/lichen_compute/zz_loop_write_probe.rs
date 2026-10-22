//! Probe: a `@loop` whose step writes, and what the codomain makes of it.
//!
//! # Invariant
//!
//! Print-only. The state carries the output buffer, so a step's write returns the
//! buffer it filled; the codomain's position is therefore the loop's result, not a
//! write node. The reading is what the check refuses with, or the answer a run gives
//! (the last iteration's store wins: `taps = 3` writes 3.0, 2.0, 1.0).
//!
//! `@B@` is the backend and `@BODY@` the kernel body, so a control and the loop are
//! one template.

use lichen_compute::language::new_store;

use super::common;

/// The program, with the backend and the body substituted.
const TEMPLATE: &str = r#"
--- compute = import "compute.lichen" ---
In  = struct{.unused (compute.Buf Float)}
Out = struct{.y (compute.Buf Float)}
Par = struct{.n Int, .taps Int, .in In, .out Out}
Arg = struct{.n Int, .taps Int, .I In}
raw = (c : Par) => {
  i = compute.range c.n
@BODY@
}
k = compute.parallel raw "@B@" Out
x = (compute.buffer ([1.0, 1.0] : array{Float, 2}) : (compute.Buf Float))
out = (compute.plrun k ((Arg)(.n 2, .taps 3, .I In(.unused x))) : Out)
compute.collect out.y : array{Float, 2}
"#;

/// One plain write: the control the loop is compared against.
const CONTROL: &str = r#"  compute.write ((compute.Write _)(.to c.out.y, .at i, .value 2.0))"#;

/// A fold whose step writes the countdown into the buffer it carries.
const LOOP: &str = r#"  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (compute.write ((compute.Write _)(.to s(1), .at i, .value (int2float s(0))))))
  fold (c.taps, c.out.y)"#;

/// What a run answered, or how it refused.
fn run(body: &str, backend: &str) -> String {
    let source = TEMPLATE.replace("@BODY@", body).replace("@B@", backend);
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(&source, None, &mut store) {
        Ok(text) => format!("answered {text}"),
        Err(diagnostics) => format!(
            "refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_loop_that_writes() {
    let _slot = common::slot();
    println!("[control] {}", run(CONTROL, "cpu"));
    println!("[loop_cpu] {}", run(LOOP, "cpu"));
    println!("[loop_gpu] {}", run(LOOP, "gpu"));
}
