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
PIn  = struct{.x (compute.Buf Float), .lim (compute.Buf Int)}
POut = struct{.y (compute.Buf Float)}
PPar = struct{.n Int, .least Float, .in PIn, .out POut}
PArg = struct{.n Int, .least Float, .I PIn}
sum_raw = (c : PPar) => {
  i = compute.range c.n
  lim = compute.read ((compute.Read _)(.from c.in.lim, .at 0))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from c.in.x, .at (lim - s(0)))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (lim, c.least ~ "identity"))))
}
sumk = compute.parallel (sum_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
x = (compute.buffer ([1.0, 2.0, 4.0, 8.0, 16.0, 32.0] : array{Float, 6}) : (compute.Buf Float))
"#;

/// `lim = 3`, so `1 + 2 + 4 = 7.0` at all six lanes.
const THREE: &str = r#"
lim = (compute.buffer ([3] : array{Int, 1}) : (compute.Buf Int))
summed = (compute.plrun sumk ((PArg)(.n 6, .least 0.0, .I PIn(.x x, .lim lim))) : POut)
compute.collect summed.y : array{Float, 6}
"#;

/// `lim = 5`, so `1 + 2 + 4 + 8 + 16 = 31.0` at all six lanes.
const FIVE: &str = r#"
lim = (compute.buffer ([5] : array{Int, 1}) : (compute.Buf Int))
summed = (compute.plrun sumk ((PArg)(.n 6, .least 0.0, .I PIn(.x x, .lim lim))) : POut)
compute.collect summed.y : array{Float, 6}
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
    let _guard = common::device("zz_dynamic_trip_probe");
    println!("[lim_three] {}", rendered(THREE));
    println!("[lim_five] {}", rendered(FIVE));
}
