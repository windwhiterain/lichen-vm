//! Probe: what the installed floor makes of a real operator's recorded graph.

use super::common;

use lichen_compute::compute::{LANDING_DIAGNOSTIC, clear_graph_pass};
use lichen_compute::floor::install_floor_for_run;
use lichen_compute::language::new_store;
use lichen_language::run::{PassReport, RunOutput, evaluate_raw_reporting};

/// The GEMV of `examples/gemv.lichen`, through the graph leg, collected directly.
const GEMV: &str = r#"
--- compute = import "compute.lichen" ---
InG  = struct{.w (compute.Buf Float), .x (compute.Buf Float)}
OutG = struct{.y (compute.Buf Float)}
Par  = struct{.n Int, .kdim Int, .in InG, .out OutG}
ArgK = struct{.n Int, .kdim Int, .I InG}
gemv_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (i * k.kdim + (s(0) - 1)))) * compute.read ((compute.Read _)(.from k.in.x, .at (s(0) - 1))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (fold (k.kdim, 0.0 ~ "identity"))))
}
kg = compute.parallel (gemv_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
step = (g : Par) => (compute.plrun kg ((ArgK)(.n g.n, .kdim g.kdim, .I InG(.w g.in.w, .x g.in.x))) : OutG)
built = compute.graph step
w = (compute.buffer ([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0] : array{Float, 12}) : (compute.Buf Float))
x = (compute.buffer ([1.0, 2.0, 3.0] : array{Float, 3}) : (compute.Buf Float))
compute.collect (compute.graphrun built (4, 3, w, x))
"#;

/// The elements of a `Float` literal list, as lichen spells them.
fn floats(values: &[f32]) -> String {
    values
        .iter()
        .map(|value| format!("{value}.0"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The decode step: the KV-cache append, then the GEMV scorer, over the appended cache.
///
/// # Invariant
/// The append launches over `capacity · dim` lanes and the scorer over `capacity`, so the two
/// extents are equal exactly when `dim == 1`, which is the one case a merged node could have.
fn decode_pair(capacity: usize, dim: usize, direct: bool) -> String {
    let count = capacity * dim;
    let cache = floats(&(1..=count).map(|value| value as f32).collect::<Vec<f32>>());
    let values = floats(
        &(1..=count)
            .map(|value| (100 + value) as f32)
            .collect::<Vec<f32>>(),
    );
    let key = floats(&[100.0]);
    let value = floats(&[300.0]);
    let query = floats(&vec![1.0; dim]);
    let tail = if direct {
        format!(
            r#"fresh = (compute.plrun append ((KArg)(.n {count}, .cap {capacity}, .dim {dim}, .t 1, .I DIn(.k k0, .v v0, .nk nk, .nv nv, .q q))) : KVOut)
y = (compute.plrun gemv ((GArg)(.n {capacity}, .kdim {dim}, .I GIn(.w fresh.k, .x q))) : GOut)
(compute.collect fresh.k, compute.collect fresh.v, compute.collect y.y)"#
        )
    } else {
        format!(
            r#"SArg = struct{{.n Int, .cap Int, .dim Int, .t Int, .m Int, .in DIn}}
step = (s : SArg) => {{{{
  fresh = (compute.plrun append ((KArg)(.n s.n, .cap s.cap, .dim s.dim, .t s.t, .I DIn(.k s.in.k, .v s.in.v, .nk s.in.nk, .nv s.in.nv, .q s.in.q))) : KVOut)
  compute.plrun gemv ((GArg)(.n s.m, .kdim s.dim, .I GIn(.w fresh.k, .x s.in.q))) : GOut
}}}}
built = compute.graph step
compute.collect (compute.graphrun built ({count}, {capacity}, {dim}, 1, {capacity}, k0, v0, nk, nv, q))"#
        )
    };
    format!(
        r#"
--- compute = import "compute.lichen" ---
DIn   = struct{{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float), .q (compute.Buf Float)}}
KVOut = struct{{.k (compute.Buf Float), .v (compute.Buf Float)}}
KPar  = struct{{.n Int, .cap Int, .dim Int, .t Int, .in DIn, .out KVOut}}
KArg  = struct{{.n Int, .cap Int, .dim Int, .t Int, .I DIn}}
append_raw = (k : KPar) => {{
  column = compute.row k
  position = compute.col k
  slot = position * k.dim + column
  old_k = compute.read ((compute.Read _)(.from k.in.k, .at slot))
  old_v = compute.read ((compute.Read _)(.from k.in.v, .at slot))
  fresh = position == k.t
  new_k = if fresh then (compute.read ((compute.Read _)(.from k.in.nk, .at column))) else old_k
  new_v = if fresh then (compute.read ((compute.Read _)(.from k.in.nv, .at column))) else old_v
  (compute.write ((compute.Write _)(.to k.out.k, .at slot, .value (new_k ~ "element"))), compute.write ((compute.Write _)(.to k.out.v, .at slot, .value (new_v ~ "element"))))
}}
append = compute.parallel (append_raw ~ "claim:Reduction(element)") "gpu"
GIn  = struct{{.w (compute.Buf Float), .x (compute.Buf Float)}}
GOut = struct{{.y (compute.Buf Float)}}
GPar = struct{{.n Int, .kdim Int, .in GIn, .out GOut}}
GArg = struct{{.n Int, .kdim Int, .I GIn}}
gemv_raw = (k : GPar) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (i * k.kdim + (s(0) - 1)))) * compute.read ((compute.Read _)(.from k.in.x, .at (s(0) - 1))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (fold (k.kdim, 0.0 ~ "identity"))))
}}
gemv = compute.parallel (gemv_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
k0 = (compute.buffer ([{cache}] : array{{Float, {count}}}) : (compute.Buf Float))
v0 = (compute.buffer ([{values}] : array{{Float, {count}}}) : (compute.Buf Float))
nk = (compute.buffer ([{key}] : array{{Float, 1}}) : (compute.Buf Float))
nv = (compute.buffer ([{value}] : array{{Float, 1}}) : (compute.Buf Float))
q  = (compute.buffer ([{query}] : array{{Float, {dim}}}) : (compute.Buf Float))
{tail}
"#,
        count = count,
        dim = dim,
        cache = cache,
        values = values,
        key = key,
        value = value,
        query = query,
        tail = tail,
    )
}

/// Run `source` once with no pass installed and once with the floor installed for the run.
fn measure(label: &str, source: &str) {
    clear_graph_pass();
    let baseline = one(source);
    install_floor_for_run(&[]);
    let floored = one(source);
    clear_graph_pass();
    for (name, report) in [("recorded", baseline), ("floored", floored)] {
        match report {
            Ok(output) => {
                println!("[{label}/{name}] runs: {}", output.output);
                if !output.pass_lines.is_empty() {
                    for line in &output.pass_lines {
                        println!("[{label}/{name}] pass: {line}");
                    }
                }
            }
            Err(diagnostics) => {
                println!("[{label}/{name}] refused:");
                for diagnostic in diagnostics {
                    println!("[{label}/{name}]   {diagnostic}");
                }
            }
        }
    }
}

/// One run of `source` with its pass report carried beside its value.
///
/// # Invariant
/// The refusal is rendered here, so the probe's own signature does not name the diagnostic type
/// the run's store fixes.
fn one(source: &str) -> Result<RunOutput, Vec<String>> {
    let mut store = new_store(None).expect("the embedded compute package registers");
    evaluate_raw_reporting(
        source,
        None,
        &mut store,
        PassReport::Reported(&[LANDING_DIAGNOSTIC]),
    )
    .map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|diagnostic| format!("{diagnostic:?}"))
            .collect()
    })
}

#[test]
fn probe_the_floor_on_a_real_operator() {
    let Some((_context, _guard)) = common::device("zz_operator_floor_probe") else {
        return;
    };
    measure("gemv", GEMV);
    measure("decode_direct", &decode_pair(8, 1, true));
    measure(
        "decode_direct_cpu",
        &decode_pair(8, 1, true).replace("\"gpu\"", "\"cpu\""),
    );
    measure("decode_direct_4x2", &decode_pair(4, 2, true));
    measure(
        "decode_direct_4x2_cpu",
        &decode_pair(4, 2, true).replace("\"gpu\"", "\"cpu\""),
    );
    measure("decode_pair", &decode_pair(4, 2, false));
    measure("decode_pair_equal", &decode_pair(8, 1, false));
}
