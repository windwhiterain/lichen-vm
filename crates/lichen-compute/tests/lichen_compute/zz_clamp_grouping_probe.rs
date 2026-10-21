//! Probe: how a `min`/`max` application groups against `+` and `*` in a kernel body.
//!
//! # Invariant
//!
//! Print-only. Three spellings of the same clamp over `i` in `0..6`, only the first parenthesised: the
//! readings say whether an application binds tighter than an arithmetic operator. The reading is the
//! **recorded** body, so no case is launched: a top-level `compute.plrun` is dispatched while the
//! program is compiled, which is a run this crate may not start.

use super::common;

use lichen_compute::fusion_claim::{fragment_of, kernel_id};

/// A kernel that writes `int2float` of the expression substituted for `@EXPR@` at every lane.
fn program(expression: &str) -> String {
    let template = r#"
--- compute = import "compute.lichen" ---
IIn  = struct{.x (compute.Buf Float)}
IOut = struct{.y (compute.Buf Float)}
IPar = struct{.n Int, .in IIn, .out IOut}
probe_raw = (c : IPar) => {
  i = compute.range c.n
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (int2float @EXPR@)))
}
probe = compute.parallel probe_raw "gpu" IOut
probe
"#;
    template.replace("@EXPR@", expression)
}

/// The same clamp **inside** a fold: `sum over s = 1..4 of clamp(s - 1, 0, 4)`.
const CLAMP_IN_FOLD: &str = r#"
--- compute = import "compute.lichen" ---
FIn  = struct{.x (compute.Buf Float)}
FOut = struct{.y (compute.Buf Float)}
FPar = struct{.n Int, .taps Int, .in FIn, .out FOut}
clamped_raw = (c : FPar) => {
  i = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (min (max (s(0) - 1) 0) 4)) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
clamped = compute.parallel (clamped_raw ~ "claim:Reduction(combine,identity,element)") "gpu" FOut
clamped
"#;

/// A kernel that writes the clamped address of the tap `(2, 2)` at every lane of a 5x5 grid.
///
/// # Invariant
/// `clamp(row + 1, 0, 4) * 5 + clamp(col + 1, 0, 4)`, so lane `(0, 0)` reads the address `1` and
/// lane `(0, 4)` the address `9` — the address the convolution reads for its bottom-right tap.
const ADDRESS: &str = r#"
--- compute = import "compute.lichen" ---
AIn  = struct{.x (compute.Buf Float)}
AOut = struct{.y (compute.Buf Float)}
APar = struct{.n Int, .cols Int, .in AIn, .out AOut}
address_raw = (c : APar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  rows = (c.n / c.cols)
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (int2float (min (max (row + 2 - 1) 0) (rows - 1) * c.cols + (min (max (col + 2 - 1) 0) (c.cols - 1))))))
}
address = compute.parallel address_raw "gpu" AOut
address
"#;

/// The expression the compile recorded for `source`'s one kernel body.
///
/// # Invariant
/// The body is the registry's own fragment and never a run's answer, so the grouping a reader
/// sees is the one the compiler recorded: whether an application binds tighter than an
/// arithmetic operator is visible in the tree, without a dispatch.
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
fn probe_the_clamp_grouping() {
    println!("[clamp_in_fold] {}", recorded(CLAMP_IN_FOLD));
    println!("[address] {}", recorded(ADDRESS));
    for (label, expression) in [
        ("parenthesised", "((min (max i 2) 4) + 100)"),
        ("plus_tail", "(min (max i 2) 4 + 100)"),
        ("times_tail", "(min (max i 2) 4 * 10 + 1)"),
    ] {
        println!("[{label}] {}", recorded(&program(expression)));
    }
}
