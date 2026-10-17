//! Probe: a convolution whose coefficients are a buffer rather than leaves.
//!
//! # Invariant
//!
//! Print-only. One kernel, three taps: `y[i] = sum_k w[k] * x[i - k]`, with the tap count as a leaf
//! and `w` as an input buffer, so the same kernel is a smoothing filter or a difference. The left
//! edge reads `i - k` below zero, which is the padding's `0` — the causal filter's zero prehistory.

use super::common;

use lichen_compute::language::new_store;

/// The kernel and the input, shared by both weight sets.
const KERNEL: &str = r#"
FIn  = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
FOut = struct{.y (compute.Buf Float)}
FPar = struct{.n Int, .taps Int, .in FIn, .out FOut}
FArg = struct{.n Int, .taps Int, .I FIn}
filter_raw = (c : FPar) => {
  i = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from c.in.w, .at (s(0) - 1))) * compute.read ((compute.Read _)(.from c.in.x, .at (i - (s(0) - 1)))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
filter = compute.parallel (filter_raw ~ "claim:Reduction(combine,identity,element)") "cpu"
x = (compute.buffer ([1.0, 2.0, 3.0, 4.0, 5.0, 6.0] : array{Float, 6}) : (compute.Buf Float))
"#;

/// A three-tap smoothing filter: `w = 1 2 1`.
const SMOOTH: &str = r#"
w = (compute.buffer ([1.0, 2.0, 1.0] : array{Float, 3}) : (compute.Buf Float))
out = (compute.plrun filter ((FArg)(.n 6, .taps 3, .I FIn(.x x, .w w))) : FOut)
compute.collect out.y : array{Float, 6}
"#;

/// The same kernel as a first difference: `w = 1 -1 0`.
const DIFFERENCE: &str = r#"
w = (compute.buffer ([1.0, (0.0 - 1.0), 0.0] : array{Float, 3}) : (compute.Buf Float))
out = (compute.plrun filter ((FArg)(.n 6, .taps 3, .I FIn(.x x, .w w))) : FOut)
compute.collect out.y : array{Float, 6}
"#;

/// What the raw runner renders for `body`, or how it refused.
fn rendered(body: &str) -> String {
    let source = format!("--- compute = import \"compute.lichen\" ---\n{KERNEL}{body}");
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
fn probe_the_runtime_coefficients() {
    let _guard = common::device("zz_filter_probe");
    for (label, body) in [("smooth_1_2_1", SMOOTH), ("difference_1_m1_0", DIFFERENCE)] {
        println!("[{label}] {}", rendered(body));
    }
}
