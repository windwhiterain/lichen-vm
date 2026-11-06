//! Probe: an inner `@loop` nest called inside the enclosing step's state.
//!
//! # Invariant
//!
//! Print-only, and the counts are the parameter's (`k.n`), so the two folds stay loops:
//! a literal count is expanded by the definition pass and never reaches the lowering.
//! The inner adds one per trip and the outer calls it once per trip, so `n = 3` is `9`
//! per lane if the nest lowers, and the dominance refusal if it does not.

use super::common;

use lichen_compute::language::new_store;

const PROGRAM: &str = r#"
--- compute = import "compute.lichen" ---
Src := {.a @any : Int}
SrcOut := {.z (compute.Buf (compute.BufParam{.type Float, .length 3}))}
SrcPar := {.n @any : Int, .in Src}
fill := ('k = SrcPar) => {
  i := compute.range k.n
  z := compute.buf (compute.BufParam{.type Float, .length k.n})
  ((SrcOut){.z (compute.write ((compute.Write _){.to z, .at i, .value (int2float (i + 1))}))})
}
source_kernel := compute.parallel fill "cpu" SrcOut
inbuf := (compute.plrun source_kernel ((compute.A _){.n 3, .I Src{.a 0}}) : Struct)
In  := {.a (compute.Buf (compute.BufParam{.type Float, .length 3}))}
Out := {.z (compute.Buf (compute.BufParam{.type Float, .length 3}))}
Par := {.n @any : Int, .in In}
body := ('k = Par) => {
  i := compute.range k.n
  @loop inner := ('s => if s(0) == 0 then s(1) else inner (s(0) - 1, (s(1) + 1.0)))
  @loop outer := ('t => if t(0) == 0 then t(1) else outer (t(0) - 1, (t(1) + (inner (k.n, 0.0)))))
  z := outer (k.n, 0.0)
  o := compute.buf (compute.BufParam{.type Float, .length k.n})
  ((Out){.z (compute.write ((compute.Write _){.to o, .at i, .value z}))})
}
k := compute.parallel body "cpu" Out
out := (compute.plrun k ((compute.A _){.n 3, .I In{.a inbuf.z}}) : Struct)
compute.collect out.z : Array
"#;

#[test]
fn probe_a_nested_loop_result() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(PROGRAM, None, &mut store) {
        Ok(text) => println!("[nested] {text}"),
        Err(diagnostics) => println!(
            "[nested] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        ),
    }
}
