//! Probe: what the replicate kernel's window addresses actually sum to per lane.
//!
//! # Invariant
//!
//! Print-only. The kernel is `replicate_raw`'s window with its `compute.read` replaced by the address
//! itself, accumulated as a `Float` (the accumulator is one `OpPhi`, so the address needs `int2float`).
//! Lane `(0, 0)`'s nine addresses are `0 0 1 0 0 1 5 5 6`, so the model says `18.0`. The reading is
//! the **recorded** address expression: no case is launched, so nothing dispatches.

use super::common;

use lichen_compute::fusion_claim::{fragment_of, kernel_id};

/// The address sum of the replicate window over a 5x5 grid.
///
/// # Invariant
/// The fold's bound is the kernel's own `.taps`, so the recorded address expression is one body
/// and the trip count the run was handed is not a reading this probe can make.
fn program() -> String {
    r#"
--- compute = import "compute.lichen" ---
AIn  = struct{.x (compute.Buf Float)}
AOut = struct{.y (compute.Buf Float)}
APar = struct{.n Int, .cols Int, .taps Int, .in AIn, .out AOut}
sum_raw = (c : APar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  rows = (c.n / c.cols)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + int2float (min (max (row + (s(0) - 1) / 3 - 1) 0) (rows - 1) * c.cols + (min (max (col + (s(0) - 1) % 3 - 1) 0) (c.cols - 1)))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
sums = compute.parallel (sum_raw ~ "claim:Reduction(combine,identity,element)") "gpu" AOut
sums
"#
    .to_string()
}

/// One lane's sub-terms, so a clamp that returns its bound is visible.
///
/// # Invariant
/// Lane `(0, 0)` with the tap `(0, 0)`: `min (max (row - 1) 0) (rows - 1)` reads the row `0` and
/// `min (max (col - 1) 0) (cols - 1)` the column `0`; a `4` in either is a bound leaking through.
fn terms(expression: &str) -> String {
    let template = r#"
--- compute = import "compute.lichen" ---
TIn  = struct{.x (compute.Buf Float)}
TOut = struct{.y (compute.Buf Float)}
TPar = struct{.n Int, .cols Int, .in TIn, .out TOut}
term_raw = (c : TPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  rows = (c.n / c.cols)
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (int2float @EXPR@)))
}
kterm = compute.parallel term_raw "gpu" TOut
kterm
"#;
    template.replace("@EXPR@", expression)
}

/// The address expression the compile recorded for `source`'s one kernel body.
///
/// # Invariant
/// The body is the registry's own fragment and never a run's answer, so the addresses a reader
/// sees are the ones the compiler recorded, without a dispatch.
fn recorded(source: &str) -> String {
    let report = common::check(source);
    if !report.ok() {
        return format!(
            "refused: {}",
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<String>>()
                .join(" | ")
        );
    }
    let module = report
        .build
        .expect("an accepted program has a build")
        .module;
    let Some(kernel) = module
        .nodes
        .iter()
        .find_map(|(id, _node)| kernel_id(&module, id).ok())
    else {
        return "no kernel recorded".to_string();
    };
    let Ok(fragment) = fragment_of(kernel) else {
        return "no fragment recorded".to_string();
    };
    format!("{:#?}", fragment.body)
}

#[test]
fn probe_the_replicate_addresses() {
    println!("[sums] {}", recorded(&program()));
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
        println!("[{label}] {}", recorded(&terms(expression)));
    }
}
