//! Probe: the `if` in a fold's step, and the body the loop conversion lowers.
//!
//! # Invariant
//!
//! Print-only. The program checks and lowers; the refusal comes from the wasm
//! assembler's class walk, so the body is dumped to name the instruction the two
//! classes meet in.

use lichen_compute::language::new_store;

use super::common;

/// The fold whose arm is a `Float`-armed `if`.
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
tiny = compute.parallel (tiny_raw ~ "claim:Reduction(combine,identity,element)") "cpu" TOut
x = (compute.buffer ([1.0, 1.0] : array{Float, 2}) : (compute.Buf Float))
out = (compute.plrun tiny ((TArg)(.n 2, .taps 3, .I TIn(.x x))) : TOut)
compute.collect out.y : array{Float, 2}
"#;

/// What a run answered, or how it refused.
fn ran(source: &str) -> String {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(source, None, &mut store) {
        Ok(text) => format!("answered {text}"),
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
fn probe_the_body_an_if_in_a_fold_lowers_to() {
    let report = common::check(IF_IN_FOLD);
    println!("[check] ok={} {:?}", report.ok(), report.diagnostics);
    let build = report.build.expect("the program builds");
    let fragment = common::sole_fragment(&build.module);
    println!("[body] {:#?}", fragment.body);
    println!("[run] {}", ran(IF_IN_FOLD));
}
