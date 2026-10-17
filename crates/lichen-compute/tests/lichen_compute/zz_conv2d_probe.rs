//! Probe: a two-dimensional convolution with a 3x3 kernel buffer, folded over nine taps.
//!
//! # Invariant
//!
//! Print-only. One lane per cell of a row-major 5x5 grid; the nine taps are one fold splitting its
//! count-down index by `/ 3` and `% 3`, and the window address is a precomputed base plus
//! `(s(0) - 1) / 3 * cols + (s(0) - 1) % 3`. Nine `0.125` taps under one `8.0` answer `1.0` wherever
//! the centre is inside the window.

use super::common;

use lichen_compute::language::new_store;

/// The kernel and the grid.
const PROGRAM: &str = r#"
--- compute = import "compute.lichen" ---
CIn  = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
COut = struct{.y (compute.Buf Float)}
CPar = struct{.n Int, .cols Int, .taps Int, .in CIn, .out COut}
CArg = struct{.n Int, .cols Int, .taps Int, .I CIn}
convolve_raw = (c : CPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  base = ((row - 1) * c.cols + (col - 1))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from c.in.w, .at (s(0) - 1))) * compute.read ((compute.Read _)(.from c.in.x, .at (base + (s(0) - 1) / 3 * c.cols + (s(0) - 1) % 3))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
convolve = compute.parallel (convolve_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
x = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 25}) : (compute.Buf Float))
w = (compute.buffer ([0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125] : array{Float, 9}) : (compute.Buf Float))
out = (compute.plrun convolve ((CArg)(.n 25, .cols 5, .taps 9, .I CIn(.x x, .w w))) : COut)
compute.collect out.y : array{Float, 25}
"#;

/// The same kernel with a second `8.0` at `(1, 4)`, so the flat wrap is visible.
///
/// # Invariant
/// Cell `(2, 0)`'s left tap is `(1, 4)`: its address is `base - 1`, and the row-major layout makes that
/// the previous row's last cell rather than the padding, so the answer carries an extra `1.0` there.
const WRAP_PROGRAM: &str = r#"
--- compute = import "compute.lichen" ---
CIn  = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
COut = struct{.y (compute.Buf Float)}
CPar = struct{.n Int, .cols Int, .taps Int, .in CIn, .out COut}
CArg = struct{.n Int, .cols Int, .taps Int, .I CIn}
convolve_raw = (c : CPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  base = ((row - 1) * c.cols + (col - 1))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from c.in.w, .at (s(0) - 1))) * compute.read ((compute.Read _)(.from c.in.x, .at (base + (s(0) - 1) / 3 * c.cols + (s(0) - 1) % 3))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
convolve = compute.parallel (convolve_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
x = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 25}) : (compute.Buf Float))
w = (compute.buffer ([0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125] : array{Float, 9}) : (compute.Buf Float))
out = (compute.plrun convolve ((CArg)(.n 25, .cols 5, .taps 9, .I CIn(.x x, .w w))) : COut)
compute.collect out.y : array{Float, 25}
"#;

/// What the raw runner renders, or how it refused.
fn rendered(source: &str) -> String {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(source, None, &mut store) {
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
fn probe_the_two_dimensional_convolution() {
    let _guard = common::device("zz_conv2d_probe");
    println!("[centre] {}", rendered(PROGRAM));
    println!("[wrap] {}", rendered(WRAP_PROGRAM));
}
