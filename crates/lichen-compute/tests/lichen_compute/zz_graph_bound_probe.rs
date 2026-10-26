//! Probe: a recorded graph whose second node folds a bound the first node wrote.
//!
//! # Invariant
//!
//! Print-only. `limk` writes `d[0] + 1` into a one-element buffer and `sumk` folds that
//! many leading elements of `x`, so over `d = [2]` and `x = [1, 2, 4, 8, 16, 32]` the
//! chained step answers `7.0` at all six lanes. What says the bound came through the
//! producer is the case that moves one numeral in it and answers `63.0` instead.

use super::common;

use lichen_compute::language::new_store;

/// The two kernels: one writes a bound, the other folds that many terms.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
LIn  := _x => {.d (compute.Buf (compute.BufParam{.type Int, .length _}))}
LOut := _x => {.lim (compute.Buf (compute.BufParam{.type Int, .length _}))}
LPar := _x => {.n @any : Int, .in LIn _, .out LOut}
lim_raw := (k <: LPar) => {
  i := compute.range k.n
  compute.write (compute.Write{.to k.out.lim, .at i, .value (((compute.read (compute.Read{.from k.in.d, .at i}) : Int)) + 1)})
}
limk := compute.parallel lim_raw "gpu" (LOut _)
lim10_raw := (k <: LPar) => {
  i := compute.range k.n
  compute.write (compute.Write{.to k.out.lim, .at i, .value (((compute.read (compute.Read{.from k.in.d, .at i}) : Int)) + 10)})
}
lim10k := compute.parallel lim10_raw "gpu" (LOut _)
SIn  := _x => {.x (compute.Buf (compute.BufParam{.type Float, .length _})), .lim (compute.Buf (compute.BufParam{.type Int, .length _}))}
SOut := _x => {.y (compute.Buf (compute.BufParam{.type Float, .length _}))}
SPar := _x => {.n @any : Int, .in SIn _, .out SOut}
sum_raw := (c <: SPar) => {
  i := compute.range c.n
  bound := (compute.read (compute.Read{.from c.in.lim, .at 0}) : Int)
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read (compute.Read{.from c.in.x, .at (bound - s(0))}) : Float))) ~ "combine" ~ "element"))
  compute.write (compute.Write{.to c.out.y, .at i, .value (fold (bound, 0.0 ~ "identity"))})
}
sumk := compute.parallel (sum_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (SOut _)
GArg := _x => {.n @any : Int, .in SIn _}
x := (compute.buffer ([@any : Float, 6][1.0, 2.0, 4.0, 8.0, 16.0, 32.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
d := (compute.buffer ([@any : Int, 1][2] : Array) : (compute.Buf (compute.BufParam{.type Int, .length _})))
"#;

/// The chained step: the fold's bound is the first dispatch's own output.
const CHAINED: &str = r#"
step := (s <: GArg) => {
  first := (compute.plrun limk (compute.A{.n 1, .I LIn{.d s.in.lim}}) : Struct)
  compute.plrun sumk (compute.A{.n s.n, .I SIn{.x s.in.x, .lim first.lim}})
}
built := compute.graph step
compute.collect (compute.graphrun built (6, x, d))
"#;

/// The control: a step reading the graph's own input, so it folds two terms.
///
/// # Invariant
/// A one-node step's return is refused rather than answering — two graph values meet on
/// one node — so this case reads as a refusal and says nothing about the bound. The
/// differential that does move is [`PLUS_TEN`].
const DIRECT: &str = r#"
step := (s <: GArg) => {
  compute.plrun sumk (compute.A{.n s.n, .I SIn{.x s.in.x, .lim s.in.lim}})
}
built := compute.graph step
compute.collect (compute.graphrun built (6, x, d))
"#;

/// The control that moves: only the producer's constant differs, by nine.
///
/// # Invariant
/// The bound becomes `2 + 10 = 12` while `x` holds six elements, so six of the folded
/// terms read past the buffer's end and answer `0`. The sum is therefore the whole of
/// `x`, `63.0`, against [`CHAINED`]'s `7.0` — and the two programs differ in one numeral
/// in the producer, which is what says the consumer's bound came through it.
const PLUS_TEN: &str = r#"
step := (s <: GArg) => {
  first := (compute.plrun lim10k (compute.A{.n 1, .I LIn{.d s.in.lim}}) : Struct)
  compute.plrun sumk (compute.A{.n s.n, .I SIn{.x s.in.x, .lim first.lim}})
}
built := compute.graph step
compute.collect (compute.graphrun built (6, x, d))
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
fn probe_the_bound_a_producer_wrote() {
    let (_stub, _guard) = common::stub();
    println!("[chained] {}", rendered(CHAINED));
    println!("[direct] {}", rendered(DIRECT));
    println!("[plus_ten] {}", rendered(PLUS_TEN));
}
