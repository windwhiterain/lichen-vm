//! Probe: argmax as three single-class folds, with no pair accumulator.
//!
//! # Invariant
//!
//! Print-only. `x = [1.0, 3.0, 2.0, 7.0, 0.5, 9.0, 4.0, 2.5]` has its unique maximum `9.0` at
//! index `5`, so the hand answers are `m = 9.0`, `hit = [BIG, BIG, BIG, BIG, BIG, 5, BIG, BIG]`
//! and `token = 5`. Each stage prints its own answer, so a failure names the stage.

use super::common;

use lichen_compute::language::new_store;

/// The three kernels: a `Float` max fold, a top-level conditional in an `Int` body, an `Int`
/// min fold. `BIG` is `1000`, above every index.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
MIn  = struct{.x (compute.Buf Float)}
MOut = struct{.m (compute.Buf Float)}
MPar = struct{.n Int, .w Int, .least Float, .in MIn, .out MOut}
MArg = struct{.n Int, .w Int, .least Float, .I MIn}
peak_raw = (c : MPar) => {
  lane = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) (compute.read ((compute.Read _)(.from c.in.x, .at (lane * c.w + (s(0) - 1))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.m, .at lane, .value (fold (c.w, c.least ~ "identity"))))
}
peak = compute.parallel (peak_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
HIn  = struct{.x (compute.Buf Float), .m (compute.Buf Float)}
HOut = struct{.h (compute.Buf Int)}
HPar = struct{.n Int, .big Int, .in HIn, .out HOut}
HArg = struct{.n Int, .big Int, .I HIn}
mark_raw = (c : HPar) => {
  i = compute.range c.n
  v = compute.read ((compute.Read _)(.from c.in.x, .at i))
  m = compute.read ((compute.Read _)(.from c.in.m, .at 0))
  hit = if v == m then i else c.big
  compute.write ((compute.Write _)(.to c.out.h, .at i, .value hit))
}
mark = compute.parallel mark_raw "gpu"
TIn  = struct{.h (compute.Buf Int)}
TOut = struct{.t (compute.Buf Int)}
TPar = struct{.n Int, .w Int, .big Int, .in TIn, .out TOut}
TArg = struct{.n Int, .w Int, .big Int, .I TIn}
pick_raw = (c : TPar) => {
  lane = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (min s(1) (compute.read ((compute.Read _)(.from c.in.h, .at (lane * c.w + (s(0) - 1))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.t, .at lane, .value (fold (c.w, c.big ~ "identity"))))
}
pick = compute.parallel (pick_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
x = (compute.buffer ([1.0, 3.0, 2.0, 7.0, 0.5, 9.0, 4.0, 2.5] : array{Float, 8}) : (compute.Buf Float))
"#;

/// Stage 1 alone: the `Float` max fold over one lane of eight. Hand answer `[9.0]`.
const STAGE_ONE: &str = r#"
peak_out = (compute.plrun peak ((MArg)(.n 1, .w 8, .least (0.0 - 100.0), .I MIn(.x x))) : MOut)
compute.collect peak_out.m : array{Float, 1}
"#;

/// Stage 2: the conditional marking the maximum's index. Hand answer eight `BIG` but one `5`.
const STAGE_TWO: &str = r#"
peak_out = (compute.plrun peak ((MArg)(.n 1, .w 8, .least (0.0 - 100.0), .I MIn(.x x))) : MOut)
marked = (compute.plrun mark ((HArg)(.n 8, .big 1000, .I HIn(.x x, .m peak_out.m))) : HOut)
compute.collect marked.h : array{Int, 8}
"#;

/// Stage 3: the `Int` min fold over the marks. Hand answer `[5]`.
const STAGE_THREE: &str = r#"
peak_out = (compute.plrun peak ((MArg)(.n 1, .w 8, .least (0.0 - 100.0), .I MIn(.x x))) : MOut)
marked = (compute.plrun mark ((HArg)(.n 8, .big 1000, .I HIn(.x x, .m peak_out.m))) : HOut)
token = (compute.plrun pick ((TArg)(.n 1, .w 8, .big 1000, .I TIn(.h marked.h))) : TOut)
compute.collect token.t : array{Int, 1}
"#;

/// What the raw runner renders for `body`, or how it refused.
fn rendered(body: &str) -> String {
    let source = format!("{KERNEL}{body}");
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
fn probe_the_argmax_folds() {
    let _guard = common::device("zz_argmax_probe");
    println!("[stage1_max] {}", rendered(STAGE_ONE));
    println!("[stage2_mark] {}", rendered(STAGE_TWO));
    println!("[stage3_token] {}", rendered(STAGE_THREE));
}
