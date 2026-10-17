//! Probe: what the replicate kernel's window addresses actually sum to per lane.
//!
//! # Invariant
//!
//! Print-only. The kernel is `replicate_raw`'s window with its `compute.read` replaced by the address
//! itself, accumulated as a `Float` (the accumulator is one `OpPhi`, so the address needs `int2float`).
//! Lane `(0, 0)`'s nine addresses are `0 0 1 0 0 1 5 5 6`, so the model says `18.0`.

use super::common;

use lichen_compute::language::new_store;

/// The address sum of the replicate window over a 5x5 grid.
fn program(taps: usize) -> String {
    let template = r#"
--- compute = import "compute.lichen" ---
AIn  = struct{.x (compute.Buf Float)}
AOut = struct{.y (compute.Buf Float)}
APar = struct{.n Int, .cols Int, .taps Int, .in AIn, .out AOut}
AArg = struct{.n Int, .cols Int, .taps Int, .I AIn}
sum_raw = (c : APar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  rows = (c.n / c.cols)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + int2float (min (max (row + (s(0) - 1) / 3 - 1) 0) (rows - 1) * c.cols + (min (max (col + (s(0) - 1) % 3 - 1) 0) (c.cols - 1)))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
sums = compute.parallel (sum_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
x = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 25}) : (compute.Buf Float))
out = (compute.plrun sums ((AArg)(.n 25, .cols 5, .taps @TAPS@, .I AIn(.x x))) : AOut)
compute.collect out.y : array{Float, 25}
"#;
    template.replace("@TAPS@", &taps.to_string())
}

/// One lane's sub-terms, so a clamp that returns its bound is visible.
///
/// # Invariant
/// Lane `(0, 0)` with the tap `(0, 0)`: `min (max (row - 1) 0) (rows - 1)` must answer `0` and
/// `min (max (col - 1) 0) (cols - 1)` must answer `0`; a `4` in either is a bound leaking through.
fn terms(expression: &str) -> String {
    let template = r#"
--- compute = import "compute.lichen" ---
TIn  = struct{.x (compute.Buf Float)}
TOut = struct{.y (compute.Buf Float)}
TPar = struct{.n Int, .cols Int, .in TIn, .out TOut}
TArg = struct{.n Int, .cols Int, .I TIn}
term_raw = (c : TPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  rows = (c.n / c.cols)
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (int2float @EXPR@)))
}
kterm = compute.parallel term_raw "gpu"
x = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 25}) : (compute.Buf Float))
out = (compute.plrun kterm ((TArg)(.n 25, .cols 5, .I TIn(.x x))) : TOut)
compute.collect out.y : array{Float, 25}
"#;
    template.replace("@EXPR@", expression)
}

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
fn probe_the_replicate_addresses() {
    let _guard = common::device("zz_replicate_address_probe");
    for taps in 1..=2 {
        println!("[taps={taps}] {}", rendered(&program(taps)));
    }
    for (label, expression) in [
        ("row_clamp", "(min (max (row + 0 - 1) 0) (rows - 1))"),
        ("col_clamp", "(min (max (col + 0 - 1) 0) (c.cols - 1))"),
        (
            "row_scaled",
            "(min (max (row + 0 - 1) 0) (rows - 1) * c.cols)",
        ),
        (
            "sum",
            "(min (max (row + 0 - 1) 0) (rows - 1) * c.cols + (min (max (col + 0 - 1) 0) (c.cols - 1)))",
        ),
    ] {
        println!("[{label}] {}", rendered(&terms(expression)));
    }
}
