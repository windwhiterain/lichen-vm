//! Probe: a fold's trip count read out of a buffer, not fixed at launch.
//!
//! # Invariant
//!
//! Print-only. The body is `y[i] = x[0] + … + x[lim - 1]` where `lim` is a read of
//! `c.in.lim` at `0`, so one compiled kernel folds a different number of terms per
//! launch: `3` answers `7.0` at every lane and `5` answers `31.0`, over
//! `x = [1, 2, 4, 8, 16, 32]`, every value and both sums exact in binary.

use super::common;

use lichen_compute::language::new_store;

/// The sum of the first `lim` elements, `lim` being a buffer read.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
PIn  := _x => {.x (compute.Buf (compute.BufParam{.type Float, .length _})), .lim (compute.Buf (compute.BufParam{.type Int, .length _}))}
POut := _x => {.y (compute.Buf (compute.BufParam{.type Float, .length _}))}
PPar := _x => {.n @any : Int, .least @any : Float, .in PIn _, .out POut}
PArg := _x => {.n @any : Int, .least @any : Float, .I PIn _}
sum_raw := (c <: PPar) => {
  i := compute.range c.n
  lim := (compute.read (compute.Read{.from c.in.lim, .at 0}) : Int)
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read (compute.Read{.from c.in.x, .at (lim - s(0))}) : Float))) ~ "combine" ~ "element"))
  compute.write (compute.Write{.to c.out.y, .at i, .value (fold (lim, c.least ~ "identity"))})
}
sumk := compute.parallel (sum_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (POut _)
x := (compute.buffer ([@any : Float, 6][1.0, 2.0, 4.0, 8.0, 16.0, 32.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
"#;

/// `lim = 3`, so `1 + 2 + 4 = 7.0` at all six lanes.
const THREE: &str = r#"
lim := (compute.buffer ([@any : Int, 1][3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
summed := (compute.plrun sumk ((PArg){.n 6, .least 0.0, .I PIn{.x x, .lim lim}}) : Struct)
compute.collect summed.y : Array
"#;

/// `lim = 5`, so `1 + 2 + 4 + 8 + 16 = 31.0` at all six lanes.
const FIVE: &str = r#"
lim := (compute.buffer ([@any : Int, 1][5] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
summed := (compute.plrun sumk ((PArg){.n 6, .least 0.0, .I PIn{.x x, .lim lim}}) : Struct)
compute.collect summed.y : Array
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
fn probe_the_buffer_read_trip_count() {
    let (_stub, _guard) = common::stub();
    println!("[lim_three] {}", rendered(THREE));
    println!("[lim_five] {}", rendered(FIVE));
}
