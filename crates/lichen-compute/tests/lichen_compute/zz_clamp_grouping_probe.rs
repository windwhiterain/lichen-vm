//! Probe: how a `min`/`max` application groups against `+` and `*` in a kernel body.
//!
//! # Invariant
//!
//! Print-only. Three spellings of the same clamp over `i` in `0..6`, only the first parenthesised: the
//! readings say whether an application binds tighter than an arithmetic operator, which decides how a
//! clamped address has to be written. `(min (max i 2) 4) + 100` must answer `102 102 102 103 104 104`.

use super::common;

use lichen_compute::language::new_store;

/// A kernel that writes `int2float` of the expression substituted for `@EXPR@` at every lane.
fn program(expression: &str) -> String {
    let template = r#"
--- compute = import "compute.lichen" ---
IIn  = struct{.x (compute.Buf Float)}
IOut = struct{.y (compute.Buf Float)}
IPar = struct{.n Int, .in IIn, .out IOut}
IArg = struct{.n Int, .I IIn}
probe_raw = (c : IPar) => {
  i = compute.range c.n
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (int2float @EXPR@)))
}
probe = compute.parallel probe_raw "gpu"
x = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 6}) : (compute.Buf Float))
out = (compute.plrun probe ((IArg)(.n 6, .I IIn(.x x))) : IOut)
compute.collect out.y : array{Float, 6}
"#;
    template.replace("@EXPR@", expression)
}

/// The same clamp **inside** a fold: `sum over s = 1..4 of clamp(s - 1, 0, 4)` must answer `6`.
const CLAMP_IN_FOLD: &str = r#"
--- compute = import "compute.lichen" ---
FIn  = struct{.x (compute.Buf Float)}
FOut = struct{.y (compute.Buf Float)}
FPar = struct{.n Int, .taps Int, .in FIn, .out FOut}
FArg = struct{.n Int, .taps Int, .I FIn}
clamped_raw = (c : FPar) => {
  i = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (min (max (s(0) - 1) 0) 4)) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
clamped = compute.parallel (clamped_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
x = (compute.buffer ([0.0, 0.0] : array{Float, 2}) : (compute.Buf Float))
out = (compute.plrun clamped ((FArg)(.n 2, .taps 4, .I FIn(.x x))) : FOut)
compute.collect out.y : array{Float, 2}
"#;

/// A kernel that writes the clamped address of the tap `(2, 2)` at every lane of a 5x5 grid.
///
/// # Invariant
/// `clamp(row + 1, 0, 4) * 5 + clamp(col + 1, 0, 4)`, so lane `(0, 0)` answers `1` and lane `(0, 4)`
/// answers `9` — the address the convolution reads for its bottom-right tap.
const ADDRESS: &str = r#"
--- compute = import "compute.lichen" ---
AIn  = struct{.x (compute.Buf Float)}
AOut = struct{.y (compute.Buf Float)}
APar = struct{.n Int, .cols Int, .in AIn, .out AOut}
AArg = struct{.n Int, .cols Int, .I AIn}
address_raw = (c : APar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  rows = (c.n / c.cols)
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (int2float (min (max (row + 2 - 1) 0) (rows - 1) * c.cols + (min (max (col + 2 - 1) 0) (c.cols - 1))))))
}
address = compute.parallel address_raw "gpu"
x = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 25}) : (compute.Buf Float))
out = (compute.plrun address ((AArg)(.n 25, .cols 5, .I AIn(.x x))) : AOut)
compute.collect out.y : array{Float, 25}
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
fn probe_the_clamp_grouping() {
    let _guard = common::device("zz_clamp_grouping_probe");
    println!("[clamp_in_fold] {}", rendered(CLAMP_IN_FOLD));
    println!("[address] {}", rendered(ADDRESS));
    for (label, expression) in [
        ("parenthesised", "((min (max i 2) 4) + 100)"),
        ("plus_tail", "(min (max i 2) 4 + 100)"),
        ("times_tail", "(min (max i 2) 4 * 10 + 1)"),
    ] {
        println!("[{label}] {}", rendered(&program(expression)));
    }
}
