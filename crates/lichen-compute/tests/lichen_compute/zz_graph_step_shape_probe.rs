//! Probe: which graph-step parameter shapes record, and which the recorder refuses.
//!
//! # Invariant
//! Print-only. The kernel is the same body in every case — read one `Float` buffer, double it —
//! and the four cases cross two axes: how many buffers the **graph parameter's** `.in` holds, and
//! whether the **dispatch's** argument struct is that same struct or a different one. A run that
//! records answers `[2.0, 4.0, 6.0]`; every other outcome is the recorder's own sentence.

use super::common;

use lichen_compute::language::new_store;

/// Three kernels over the three input structs the cases below need, and the data they read.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
Inc    = struct{.d (compute.Buf Float)}
Two    = struct{.d (compute.Buf Float), .z (compute.Buf Float)}
Single = struct{.q (compute.Buf Float)}
Out    = struct{.e (compute.Buf Float)}
ParD = compute.P (compute.KT Float)(.I Inc, .O Out)
ParT = compute.P (compute.KT Float)(.I Two, .O Out)
ParQ = compute.P (compute.KT Float)(.I Single, .O Out)
dblD_raw = (k : ParD) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) * 2.0)))
}
dblD = compute.parallel dblD_raw "gpu" Out
dblT_raw = (k : ParT) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) * 2.0)))
}
dblT = compute.parallel dblT_raw "gpu" Out
dblQ_raw = (k : ParQ) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.q, .at i)) : Float) * 2.0)))
}
dblQ = compute.parallel dblQ_raw "gpu" Out
data = (compute.buffer ([1.0, 2.0, 3.0] : array{Float, 3}) : (compute.Buf Float))
"#;

/// One buffer under `.in`, and the body **reads its own extent argument**.
const SAME_ONE: &str = r#"
GArg = struct{.n Int, .in Inc, .out Out}
step = (s : GArg) => {
  compute.plrun dblD ((compute.A Inc)(.n s.n, .I Inc(.d s.in.d)))
}
built = compute.graph step
compute.collect (compute.graphrun built (3, data)) : array{Float, 3}
"#;

/// Two buffers under `.in`, and the dispatch's argument is that same struct.
const SAME_TWO: &str = r#"
GArg = struct{.n Int, .in Two}
step = (s : GArg) => {
  compute.plrun dblT ((compute.A Two)(.n s.n, .I Two(.d s.in.d, .z s.in.z)))
}
built = compute.graph step
compute.collect (compute.graphrun built (3, data, data)) : array{Float, 3}
"#;

/// Two buffers under `.in`, and the dispatch's argument is a **different** struct.
const OTHER_TWO: &str = r#"
GArg = struct{.n Int, .in Two}
step = (s : GArg) => {
  compute.plrun dblQ ((compute.A Single)(.n 3, .I Single(.q s.in.d)))
}
built = compute.graph step
compute.collect (compute.graphrun built (3, data)) : array{Float, 3}
"#;

/// One buffer under `.in`, and the dispatch's argument is a **different** struct.
const OTHER_ONE: &str = r#"
GArg = struct{.n Int, .in Inc}
step = (s : GArg) => {
  compute.plrun dblQ ((compute.A Single)(.n 3, .I Single(.q s.in.d)))
}
built = compute.graph step
compute.collect (compute.graphrun built (3, data)) : array{Float, 3}
"#;

/// Two dispatches, the second reading the first's answer — the one axis still uncrossed.
const TWO_NODES: &str = r#"
GArg = struct{.n Int, .in Inc}
step = (s : GArg) => {
  a = (compute.plrun dblD ((compute.A Inc)(.n s.n, .I Inc(.d s.in.d))) : Out)
  compute.plrun dblD ((compute.A Inc)(.n 3, .I Inc(.d a.e)))
}
built = compute.graph step
compute.collect (compute.graphrun built (3, data)) : array{Float, 3}
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
fn probe_the_graph_step_shape() {
    let Some((_context, _guard)) = common::device("zz_graph_step_shape_probe") else {
        return;
    };
    for (name, body) in [
        ("same_one", SAME_ONE),
        ("same_two", SAME_TWO),
        ("other_two", OTHER_TWO),
        ("other_one", OTHER_ONE),
        ("two_nodes", TWO_NODES),
    ] {
        println!("[{name}] {}", rendered(body));
    }
}
