//! Probe: which graph-step parameter shapes record, and which the recorder refuses.
//!
//! # Invariant
//! Print-only. The kernel is the same body in every case — read one `Float` buffer, double it —
//! and the four cases cross two axes: how many buffers the **graph parameter's** `.in` holds, and
//! whether the **dispatch's** argument struct is that same struct or a different one. A run that
//! records answers `[2.0, 4.0, 6.0]`; every other outcome is the recorder's own sentence.

use super::common;

/// What the compile recorded: its diagnostics, and nothing a run answered.
fn recorded(body: &str) -> String {
    let report = common::check(&format!("{KERNEL}{body}"));
    format!(
        "ok={} {}",
        report.ok(),
        report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<String>>()
            .join(" | ")
    )
}

/// Three kernels over the three input structs the cases below need, and the data they read.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
Inc    := _x => {.d (compute.Buf (compute.BufParam{.type Float, .length _}))}
Two    := _x => {.d (compute.Buf (compute.BufParam{.type Float, .length _})), .z (compute.Buf (compute.BufParam{.type Float, .length _}))}
Single := _x => {.q (compute.Buf (compute.BufParam{.type Float, .length _}))}
Out    := _x => {.e (compute.Buf (compute.BufParam{.type Float, .length _}))}
ParD := _x => compute.P compute.KT{.I Inc _, .O Out _}
ParT := _x => compute.P compute.KT{.I Two _, .O Out _}
ParQ := _x => compute.P compute.KT{.I Single _, .O Out _}
dblD_raw := (k <: ParD) => {
  i := compute.range k.n
  compute.write (compute.Write{.to k.out.e, .at i, .value ((compute.read (compute.Read{.from k.in.d, .at i}) : Float) * 2.0)})
}
dblD := compute.parallel dblD_raw "gpu" (Out _)
dblT_raw := (k <: ParT) => {
  i := compute.range k.n
  compute.write (compute.Write{.to k.out.e, .at i, .value ((compute.read (compute.Read{.from k.in.d, .at i}) : Float) * 2.0)})
}
dblT := compute.parallel dblT_raw "gpu" (Out _)
dblQ_raw := (k <: ParQ) => {
  i := compute.range k.n
  compute.write (compute.Write{.to k.out.e, .at i, .value ((compute.read (compute.Read{.from k.in.q, .at i}) : Float) * 2.0)})
}
dblQ := compute.parallel dblQ_raw "gpu" (Out _)
data := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
"#;

/// One buffer under `.in`, and the body **reads its own extent argument**.
const SAME_ONE: &str = r#"
GArg := _x => {.n @any : Int, .in Inc _, .out Out}
step := (s <: GArg) => {
  compute.plrun dblD (compute.A{.n s.n, .I Inc{.d s.in.d}})
}
built := compute.graph step
compute.collect (compute.graphrun built (3, data)) : Array
"#;

/// Two buffers under `.in`, and the dispatch's argument is that same struct.
const SAME_TWO: &str = r#"
GArg := _x => {.n @any : Int, .in Two _}
step := (s <: GArg) => {
  compute.plrun dblT (compute.A{.n s.n, .I Two{.d s.in.d, .z s.in.z}})
}
built := compute.graph step
compute.collect (compute.graphrun built (3, data, data)) : Array
"#;

/// Two buffers under `.in`, and the dispatch's argument is a **different** struct.
const OTHER_TWO: &str = r#"
GArg := _x => {.n @any : Int, .in Two _}
step := (s <: GArg) => {
  compute.plrun dblQ (compute.A{.n 3, .I Single{.q s.in.d}})
}
built := compute.graph step
compute.collect (compute.graphrun built (3, data)) : Array
"#;

/// One buffer under `.in`, and the dispatch's argument is a **different** struct.
const OTHER_ONE: &str = r#"
GArg := _x => {.n @any : Int, .in Inc _}
step := (s <: GArg) => {
  compute.plrun dblQ (compute.A{.n 3, .I Single{.q s.in.d}})
}
built := compute.graph step
compute.collect (compute.graphrun built (3, data)) : Array
"#;

/// Two dispatches, the second reading the first's answer — the one axis still uncrossed.
const TWO_NODES: &str = r#"
GArg := _x => {.n @any : Int, .in Inc _}
step := (s <: GArg) => {
  a := (compute.plrun dblD (compute.A{.n s.n, .I Inc{.d s.in.d}}) : Struct)
  compute.plrun dblD (compute.A{.n 3, .I Inc{.d a.e}})
}
built := compute.graph step
compute.collect (compute.graphrun built (3, data)) : Array
"#;

#[test]
fn probe_the_graph_step_shape() {
    for (name, body) in [
        ("same_one", SAME_ONE),
        ("same_two", SAME_TWO),
        ("other_two", OTHER_TWO),
        ("other_one", OTHER_ONE),
        ("two_nodes", TWO_NODES),
    ] {
        println!("[{name}] {}", recorded(body));
    }
}
