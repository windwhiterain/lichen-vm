//! Probe: a `Float`-armed `if` inside a fold, which is what a masked window needs.
//!
//! # Invariant
//!
//! Print-only. The 2D convolution's fold masks the **horizontal** axis only, because the vertical one
//! already reads the padding; the tap value is `if col + k - 1 < 0 then 0.0 else read`, so a cell at
//! `col = 0` drops its left tap instead of wrapping to the previous row's last cell. The data puts a
//! value at `(1, 4)`, so the wrap is the difference: masked, cell `(2, 0)` answers `0.0`.

use super::common;

use lichen_compute::language::new_store;

/// The 2D convolution with the horizontal axis masked.
const CONVOLVE_MASKED: &str = r#"
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
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from c.in.w, .at (s(0) - 1))) * (if col + (s(0) - 1) % 3 - 1 < 0 then 0.0 else (compute.read ((compute.Read _)(.from c.in.x, .at (base + (s(0) - 1) / 3 * c.cols + (s(0) - 1) % 3))))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
convolve = compute.parallel (convolve_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
x = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 25}) : (compute.Buf Float))
w = (compute.buffer ([0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125] : array{Float, 9}) : (compute.Buf Float))
out = (compute.plrun convolve ((CArg)(.n 25, .cols 5, .taps 9, .I CIn(.x x, .w w))) : COut)
compute.collect out.y : array{Float, 25}
"#;

/// The same convolution with no mask, so the wrap reading is the control.
const CONVOLVE_UNMASKED: &str = r#"
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
x = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 25}) : (compute.Buf Float))
w = (compute.buffer ([0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125] : array{Float, 9}) : (compute.Buf Float))
out = (compute.plrun convolve ((CArg)(.n 25, .cols 5, .taps 9, .I CIn(.x x, .w w))) : COut)
compute.collect out.y : array{Float, 25}
"#;

/// The smallest question: a fold whose arm is only a `Float`-armed `if`, with no read inside it.
const IF_IN_FOLD: &str = r#"
--- compute = import "compute.lichen" ---
TIn  = struct{.x (compute.Buf Float)}
TOut = struct{.y (compute.Buf Float)}
TPar = struct{.n Int, .taps Int, .in TIn, .out TOut}
TArg = struct{.n Int, .taps Int, .I TIn}
tiny_raw = (c : TPar) => {
  i = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (if s(0) > 1 then 1.0 else 0.0)) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
tiny = compute.parallel (tiny_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
x = (compute.buffer ([1.0, 1.0] : array{Float, 2}) : (compute.Buf Float))
out = (compute.plrun tiny ((TArg)(.n 2, .taps 3, .I TIn(.x x))) : TOut)
compute.collect out.y : array{Float, 2}
"#;

/// What the raw runner renders for `source`, or how it refused.
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
fn probe_the_masked_window() {
    let _guard = common::device("zz_masked_conv_probe");
    println!("[unmasked] {}", rendered(CONVOLVE_UNMASKED));
    println!("[masked] {}", rendered(CONVOLVE_MASKED));
    println!("[if_in_fold] {}", rendered(IF_IN_FOLD));
}
