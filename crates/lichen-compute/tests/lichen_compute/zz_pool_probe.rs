//! Probe: max-pooling, where the boundary and the identity have to be chosen together.
//!
//! # Invariant
//!
//! Print-only. One 3x3 window over a 5x5 grid whose cells are all `-1.0` except `4.0` at the centre.
//! The two cases differ only in the seed leaf: with `0.0` every window that holds no `4.0` answers
//! `0.0`, because the padding's zero beats a true maximum of `-1.0`; with `(0.0 - 100.0)` those cells
//! answer `-1.0`, their real maximum.

use super::common;

use lichen_compute::language::new_store;

/// The window max, with the seed as the caller's leaf.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
PIn  = struct{.x (compute.Buf Float)}
POut = struct{.y (compute.Buf Float)}
PPar = struct{.n Int, .cols Int, .taps Int, .least Float, .in PIn, .out POut}
PArg = struct{.n Int, .cols Int, .taps Int, .least Float, .I PIn}
pool_raw = (c : PPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  base = ((row - 1) * c.cols + (col - 1))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((max s(1) (compute.read ((compute.Read _)(.from c.in.x, .at (base + (s(0) - 1) / 3 * c.cols + (s(0) - 1) % 3))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, c.least ~ "identity"))))
}
pool = compute.parallel (pool_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
x = (compute.buffer ([0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 4.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0, 0.0 - 1.0] : array{Float, 25}) : (compute.Buf Float))
"#;

/// The seed-padding case: `0.0` as the identity.
const SEED_ZERO: &str = r#"
pooled = (compute.plrun pool ((PArg)(.n 25, .cols 5, .taps 9, .least 0.0, .I PIn(.x x))) : POut)
compute.collect pooled.y : array{Float, 25}
"#;

/// The honest case: a seed below every datum.
const SEED_LOW: &str = r#"
pooled = (compute.plrun pool ((PArg)(.n 25, .cols 5, .taps 9, .least (0.0 - 100.0), .I PIn(.x x))) : POut)
compute.collect pooled.y : array{Float, 25}
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
fn probe_the_pool_identity() {
    let _guard = common::device("zz_pool_probe");
    println!("[seed_zero] {}", rendered(SEED_ZERO));
    println!("[seed_low] {}", rendered(SEED_LOW));
}
