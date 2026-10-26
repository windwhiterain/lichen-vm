//! Probe: what a recorded graph says to a kernel declaring a runtime scalar.
//!
//! # Invariant
//!
//! Print-only. The kernel is `zz_subrange_write_probe`'s offset-carrying one, driven through
//! `compute.graph`/`compute.graphrun` instead of `compute.plrun`, so the only difference from
//! that probe's accepted case is the recording.

use super::common;

use lichen_compute::language::new_store;

/// The kernel: `y[i + off] = x[i]`, with the offset a runtime scalar leaf beside the extent.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
PIn  := _x => {.x (compute.Buf (compute.BufParam{.type Int, .length _}))}
POut := _x => {.y (compute.Buf (compute.BufParam{.type Int, .length _}))}
PPar := _x => {.n @any : Int, .off @any : Int, .in PIn _, .out POut}
PArg := _x => {.n @any : Int, .off @any : Int, .I PIn _}
append_raw := (c <: PPar) => {
  i := compute.range c.n
  v := (compute.read (compute.Read{.from c.in.x, .at i}) : Int)
  compute.write (compute.Write{.to c.out.y, .at (i + c.off), .value v})
}
append := compute.parallel append_raw "gpu" (POut _)
x := (compute.buffer ([@any : Int, 4][1, 2, 3, 4] : Array) : (compute.Buf (compute.BufParam{.type Int, .length _})))
"#;

/// The step, recorded: the extent, the offset and the buffer are the graph's three arguments.
const RECORDED: &str = r#"
GArg := _x => {.n @any : Int, .off @any : Int, .in PIn _}
step := (s <: GArg) => {
  compute.plrun append ((PArg){.n s.n, .off s.off, .I PIn{.x s.in.x}}) : Struct
}
built := compute.graph step
compute.collect (compute.graphrun built (4, 4, x))
"#;

/// The same recording, collected at the eight the offset reaches for.
const RECORDED_WIDE: &str = r#"
GArg := _x => {.n @any : Int, .off @any : Int, .in PIn _}
step := (s <: GArg) => {
  compute.plrun append ((PArg){.n s.n, .off s.off, .I PIn{.x s.in.x}}) : Struct
}
built := compute.graph step
compute.collect (compute.graphrun built (4, 4, x)) : Array
"#;

/// The control for `RECORDED`: offset 0 through the same graph, hand answer `[1, 2, 3, 4]`.
const RECORDED_OFFSET_ZERO: &str = r#"
GArg := _x => {.n @any : Int, .off @any : Int, .in PIn _}
step := (s <: GArg) => {
  compute.plrun append ((PArg){.n s.n, .off s.off, .I PIn{.x s.in.x}}) : Struct
}
built := compute.graph step
compute.collect (compute.graphrun built (4, 0, x)) : Array
"#;

/// The control that names the offset: 2 lands half inside the extent, answer `[0, 0, 1, 2]`.
const RECORDED_OFFSET_TWO: &str = r#"
GArg := _x => {.n @any : Int, .off @any : Int, .in PIn _}
step := (s <: GArg) => {
  compute.plrun append ((PArg){.n s.n, .off s.off, .I PIn{.x s.in.x}}) : Struct
}
built := compute.graph step
compute.collect (compute.graphrun built (4, 2, x)) : Array
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
fn probe_the_graph_scalar() {
    let (_stub, _guard) = common::stub();
    println!("[recorded] {}", rendered(RECORDED));
    println!("[recorded_wide] {}", rendered(RECORDED_WIDE));
    println!("[recorded_off0] {}", rendered(RECORDED_OFFSET_ZERO));
    println!("[recorded_off2] {}", rendered(RECORDED_OFFSET_TWO));
}
