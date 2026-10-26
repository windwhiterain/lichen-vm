//! Probe: argmax as three single-class folds, with no expr accumulator.
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
MIn  := _x => {.x (compute.Buf (compute.BufParam{.type Float, .length _}))}
MOut := _x => {.m (compute.Buf (compute.BufParam{.type Float, .length _}))}
MPar := _x => {.n @any : Int, .w @any : Int, .least @any : Float, .in MIn _, .out MOut}
MArg := _x => {.n @any : Int, .w @any : Int, .least @any : Float, .I MIn _}
peak_raw := (c <: MPar) => {
  lane := compute.range c.n
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((compute.read (compute.Read{.from c.in.x, .at (lane * c.w + (s(0) - 1))}) : Float)) ~ "combine" ~ "element"))
  compute.write (compute.Write{.to c.out.m, .at lane, .value (fold (c.w, c.least ~ "identity"))})
}
peak := compute.parallel (peak_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (MOut _)
HIn  := _x => {.x (compute.Buf (compute.BufParam{.type Float, .length _})), .m (compute.Buf (compute.BufParam{.type Float, .length _}))}
HOut := _x => {.h (compute.Buf (compute.BufParam{.type Int, .length _}))}
HPar := _x => {.n @any : Int, .big @any : Int, .in HIn _, .out HOut}
HArg := _x => {.n @any : Int, .big @any : Int, .I HIn _}
mark_raw := (c <: HPar) => {
  i := compute.range c.n
  v := (compute.read (compute.Read{.from c.in.x, .at i}) : Float)
  m := (compute.read (compute.Read{.from c.in.m, .at 0}) : Float)
  hit := if v == m then i else c.big
  compute.write (compute.Write{.to c.out.h, .at i, .value hit})
}
mark := compute.parallel mark_raw "gpu" (HOut _)
FIn  := _x => {.x (compute.Buf (compute.BufParam{.type Float, .length _})), .m (compute.Buf (compute.BufParam{.type Float, .length _}))}
FOut := _x => {.h (compute.Buf (compute.BufParam{.type Float, .length _}))}
FPar := _x => {.n @any : Int, .big @any : Float, .in FIn _, .out FOut}
FArg := _x => {.n @any : Int, .big @any : Float, .I FIn _}
markf_raw := (c <: FPar) => {
  i := compute.range c.n
  v := (compute.read (compute.Read{.from c.in.x, .at i}) : Float)
  m := (compute.read (compute.Read{.from c.in.m, .at 0}) : Float)
  hit := if v == m then int2float i else c.big
  compute.write (compute.Write{.to c.out.h, .at i, .value hit})
}
markf := compute.parallel markf_raw "gpu" (FOut _)
TIn  := _x => {.h (compute.Buf (compute.BufParam{.type Int, .length _}))}
TOut := _x => {.t (compute.Buf (compute.BufParam{.type Int, .length _}))}
TPar := _x => {.n @any : Int, .w @any : Int, .big @any : Int, .in TIn _, .out TOut}
TArg := _x => {.n @any : Int, .w @any : Int, .big @any : Int, .I TIn _}
pick_raw := (c <: TPar) => {
  lane := compute.range c.n
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (min s(1) ((compute.read (compute.Read{.from c.in.h, .at (lane * c.w + (s(0) - 1))}) : Int)) ~ "combine" ~ "element"))
  compute.write (compute.Write{.to c.out.t, .at lane, .value (fold (c.w, c.big ~ "identity"))})
}
pick := compute.parallel (pick_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (TOut _)
UIn  := _x => {.h (compute.Buf (compute.BufParam{.type Float, .length _}))}
UOut := _x => {.t (compute.Buf (compute.BufParam{.type Float, .length _}))}
UPar := _x => {.n @any : Int, .w @any : Int, .big @any : Float, .in UIn _, .out UOut}
UArg := _x => {.n @any : Int, .w @any : Int, .big @any : Float, .I UIn _}
pickf_raw := (c <: UPar) => {
  lane := compute.range c.n
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (min s(1) ((compute.read (compute.Read{.from c.in.h, .at (lane * c.w + (s(0) - 1))}) : Int)) ~ "combine" ~ "element"))
  compute.write (compute.Write{.to c.out.t, .at lane, .value (fold (c.w, c.big ~ "identity"))})
}
pickf := compute.parallel (pickf_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (UOut _)
ZIn  := _x => {.a @any : Int}
ZOut := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length _}))}
ZPar := _x => {.n @any : Int, .in ZIn _, .out ZOut}
ZArg := _x => {.n @any : Int, .I ZIn _}
idx_raw := (c <: ZPar) => {
  i := compute.range c.n
  compute.write (compute.Write{.to c.out.z, .at i, .value (i + 10)})
}
idx := compute.parallel idx_raw "gpu" (ZOut _)
x := (compute.buffer ([@any : Float, 8][1.0, 3.0, 2.0, 7.0, 0.5, 9.0, 4.0, 2.5] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
"#;

/// Stage 1: the `Float` max fold over one lane of eight. Hand answer `[9.0]`.
const STAGE_ONE: &str = r#"
peak_out := (compute.plrun peak ((MArg){.n 1, .w 8, .least (0.0 - 100.0), .I MIn{.x x}}) : Struct)
compute.collect peak_out.m : Array
"#;

/// Stage 2 as asked: `Float` buffers read, an `Int` buffer written.
const STAGE_TWO_INT: &str = r#"
peak_out := (compute.plrun peak ((MArg){.n 1, .w 8, .least (0.0 - 100.0), .I MIn{.x x}}) : Struct)
marked := (compute.plrun mark ((HArg){.n 8, .big 1000, .I HIn{.x x, .m peak_out.m}}) : Struct)
compute.collect marked.h : Array
"#;

/// Stage 2 in one class: the same conditional, everything `Float`.
const STAGE_TWO_FLOAT: &str = r#"
peak_out := (compute.plrun peak ((MArg){.n 1, .w 8, .least (0.0 - 100.0), .I MIn{.x x}}) : Struct)
marked := (compute.plrun markf ((FArg){.n 8, .big 1000.0, .I FIn{.x x, .m peak_out.m}}) : Struct)
compute.collect marked.h : Array
"#;

/// Stage 3 over the `Float` marks: the min fold, seeded at `1000.0`. Hand answer `[5.0]`.
const STAGE_THREE_FLOAT: &str = r#"
peak_out := (compute.plrun peak ((MArg){.n 1, .w 8, .least (0.0 - 100.0), .I MIn{.x x}}) : Struct)
marked := (compute.plrun markf ((FArg){.n 8, .big 1000.0, .I FIn{.x x, .m peak_out.m}}) : Struct)
token := (compute.plrun pickf ((UArg){.n 1, .w 8, .big 1000.0, .I UIn{.h marked.h}}) : Struct)
compute.collect token.t : Array
"#;

/// The `Int` min fold alone, over `z = [10 … 17]`. Hand answer `[10]`.
const INT_FOLD_CONTROL: &str = r#"
idxed := (compute.plrun idx ((ZArg){.n 8, .I ZIn{.a 0}}) : Struct)
token := (compute.plrun pick ((TArg){.n 1, .w 8, .big 1000, .I TIn{.h idxed.z}}) : Struct)
compute.collect token.t : Array
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
    let (_stub, _guard) = common::stub();
    println!("[stage1_max] {}", rendered(STAGE_ONE));
    println!("[stage2_int] {}", rendered(STAGE_TWO_INT));
    println!("[stage2_float] {}", rendered(STAGE_TWO_FLOAT));
    println!("[stage3_float_token] {}", rendered(STAGE_THREE_FLOAT));
    println!("[int_fold_control] {}", rendered(INT_FOLD_CONTROL));
}
