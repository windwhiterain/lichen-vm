//! Probe: argmax as three single-class folds, with no pair accumulator.
//!
//! # Invariant
//!
//! Print-only. `x` has its unique maximum `9.0` at index `5`, so the hand answers are
//! `m = 9.0`, eight marks with one `5`, and `token = 5`. Every stage prints its own answer,
//! so a failure names the stage that broke.

use super::common;

use lichen_compute::language::new_store;

/// The three kernels: a `Float` max fold, a conditional mark, an `Int` min fold.
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
FIn  = struct{.x (compute.Buf Float), .m (compute.Buf Float)}
FOut = struct{.h (compute.Buf Float)}
FPar = struct{.n Int, .big Float, .in FIn, .out FOut}
FArg = struct{.n Int, .big Float, .I FIn}
markf_raw = (c : FPar) => {
  i = compute.range c.n
  v = compute.read ((compute.Read _)(.from c.in.x, .at i))
  m = compute.read ((compute.Read _)(.from c.in.m, .at 0))
  hit = if v == m then int2float i else c.big
  compute.write ((compute.Write _)(.to c.out.h, .at i, .value hit))
}
markf = compute.parallel markf_raw "gpu"
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
UIn  = struct{.h (compute.Buf Float)}
UOut = struct{.t (compute.Buf Float)}
UPar = struct{.n Int, .w Int, .big Float, .in UIn, .out UOut}
UArg = struct{.n Int, .w Int, .big Float, .I UIn}
pickf_raw = (c : UPar) => {
  lane = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (min s(1) (compute.read ((compute.Read _)(.from c.in.h, .at (lane * c.w + (s(0) - 1))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.t, .at lane, .value (fold (c.w, c.big ~ "identity"))))
}
pickf = compute.parallel (pickf_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
ZIn  = struct{.a Int}
ZOut = struct{.z (compute.Buf Int)}
ZPar = struct{.n Int, .in ZIn, .out ZOut}
ZArg = struct{.n Int, .I ZIn}
idx_raw = (c : ZPar) => {
  i = compute.range c.n
  compute.write ((compute.Write _)(.to c.out.z, .at i, .value (i + 10)))
}
idx = compute.parallel idx_raw "gpu"
x = (compute.buffer ([1.0, 3.0, 2.0, 7.0, 0.5, 9.0, 4.0, 2.5] : array{Float, 8}) : (compute.Buf Float))
"#;

/// Stage 1: the `Float` max fold over one lane of eight. Hand answer `[9.0]`.
const STAGE_ONE: &str = r#"
peak_out = (compute.plrun peak ((MArg)(.n 1, .w 8, .least (0.0 - 100.0), .I MIn(.x x))) : MOut)
compute.collect peak_out.m : array{Float, 1}
"#;

/// Stage 2 as asked: `Float` buffers read, an `Int` buffer written.
const STAGE_TWO_INT: &str = r#"
peak_out = (compute.plrun peak ((MArg)(.n 1, .w 8, .least (0.0 - 100.0), .I MIn(.x x))) : MOut)
marked = (compute.plrun mark ((HArg)(.n 8, .big 1000, .I HIn(.x x, .m peak_out.m))) : HOut)
compute.collect marked.h : array{Int, 8}
"#;

/// Stage 2 in one class: the same conditional, everything `Float`.
const STAGE_TWO_FLOAT: &str = r#"
peak_out = (compute.plrun peak ((MArg)(.n 1, .w 8, .least (0.0 - 100.0), .I MIn(.x x))) : MOut)
marked = (compute.plrun markf ((FArg)(.n 8, .big 1000.0, .I FIn(.x x, .m peak_out.m))) : FOut)
compute.collect marked.h : array{Float, 8}
"#;

/// Stage 3 over the `Float` marks: the min fold, seeded at `1000.0`. Hand answer `[5.0]`.
const STAGE_THREE_FLOAT: &str = r#"
peak_out = (compute.plrun peak ((MArg)(.n 1, .w 8, .least (0.0 - 100.0), .I MIn(.x x))) : MOut)
marked = (compute.plrun markf ((FArg)(.n 8, .big 1000.0, .I FIn(.x x, .m peak_out.m))) : FOut)
token = (compute.plrun pickf ((UArg)(.n 1, .w 8, .big 1000.0, .I UIn(.h marked.h))) : UOut)
compute.collect token.t : array{Float, 1}
"#;

/// The `Int` min fold alone, over `z = [10 … 17]`. Hand answer `[10]`.
const INT_FOLD_CONTROL: &str = r#"
idxed = (compute.plrun idx ((ZArg)(.n 8, .I ZIn(.a 0))) : ZOut)
token = (compute.plrun pick ((TArg)(.n 1, .w 8, .big 1000, .I TIn(.h idxed.z))) : TOut)
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
    println!("[stage2_int] {}", rendered(STAGE_TWO_INT));
    println!("[stage2_float] {}", rendered(STAGE_TWO_FLOAT));
    println!("[stage3_float_token] {}", rendered(STAGE_THREE_FLOAT));
    println!("[int_fold_control] {}", rendered(INT_FOLD_CONTROL));
}
