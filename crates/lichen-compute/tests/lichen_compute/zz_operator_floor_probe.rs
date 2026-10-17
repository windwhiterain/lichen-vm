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

/// The stencil of `examples/stencil.lichen`, through the graph leg, collected directly.
///
/// # Invariant
/// Its write address is the lane's own index and its read addresses are computed (`i - 1`,
/// `i + 1`, and a `Select` at each end), which is the shape `gemv`'s refusal calls an element
/// that read another node's output.
const STENCIL_RING: &str = r#"
--- compute = import "compute.lichen" ---
XIn  = struct{.x (compute.Buf Float)}
XOut = struct{.y (compute.Buf Float)}
XPar = struct{.n Int, .alpha Float, .in XIn, .out XOut}
XArg = struct{.n Int, .alpha Float, .I XIn}
ring_raw = (c : XPar) => {
  i = compute.range c.n
  centre = compute.read ((compute.Read _)(.from c.in.x, .at i))
  left = if i == 0 then (compute.read ((compute.Read _)(.from c.in.x, .at (c.n - 1)))) else (compute.read ((compute.Read _)(.from c.in.x, .at (i - 1))))
  right = if i == (c.n - 1) then (compute.read ((compute.Read _)(.from c.in.x, .at 0))) else (compute.read ((compute.Read _)(.from c.in.x, .at (i + 1))))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre))))
}
ks = compute.parallel ring_raw "gpu"
step = (s : XPar) => (compute.plrun ks ((XArg)(.n s.n, .alpha s.alpha, .I XIn(.x s.in.x))) : XOut)
built = compute.graph step
x = (compute.buffer ([4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : array{Float, 6}) : (compute.Buf Float))
compute.collect (compute.graphrun built (6, 0.25, x))
"#;

/// The stencil with no `Select`: both boundaries are the padding.
///
/// # Invariant
/// Three computed reads and a lane-index write, so the body is the Dirichlet form with no case
/// analysis at all.
const STENCIL_DIRICHLET: &str = r#"
--- compute = import "compute.lichen" ---
XIn  = struct{.x (compute.Buf Float)}
XOut = struct{.y (compute.Buf Float)}
XPar = struct{.n Int, .alpha Float, .in XIn, .out XOut}
XArg = struct{.n Int, .alpha Float, .I XIn}
step_raw = (c : XPar) => {
  i = compute.range c.n
  centre = compute.read ((compute.Read _)(.from c.in.x, .at i))
  left = compute.read ((compute.Read _)(.from c.in.x, .at (i - 1)))
  right = compute.read ((compute.Read _)(.from c.in.x, .at (i + 1)))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre))))
}
ks = compute.parallel step_raw "gpu"
step = (s : XPar) => (compute.plrun ks ((XArg)(.n s.n, .alpha s.alpha, .I XIn(.x s.in.x))) : XOut)
built = compute.graph step
x = (compute.buffer ([4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : array{Float, 6}) : (compute.Buf Float))
compute.collect (compute.graphrun built (6, 0.25, x))
"#;

/// The two-dimensional step: `compute.row`/`compute.col`, four neighbours, two masks.
const STENCIL_GRID: &str = r#"
--- compute = import "compute.lichen" ---
GIn  = struct{.x (compute.Buf Float)}
GOut = struct{.y (compute.Buf Float)}
GPar = struct{.n Int, .cols Int, .alpha Float, .in GIn, .out GOut}
GArg = struct{.n Int, .cols Int, .alpha Float, .I GIn}
grid_raw = (c : GPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  centre = compute.read ((compute.Read _)(.from c.in.x, .at i))
  above = compute.read ((compute.Read _)(.from c.in.x, .at ((row - 1) * c.cols + col)))
  below = compute.read ((compute.Read _)(.from c.in.x, .at ((row + 1) * c.cols + col)))
  left = if col == 0 then 0.0 else (compute.read ((compute.Read _)(.from c.in.x, .at (i - 1))))
  right = if col == (c.cols - 1) then 0.0 else (compute.read ((compute.Read _)(.from c.in.x, .at (i + 1))))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (centre + c.alpha * ((above + below + left + right) - 4.0 * centre))))
}
kg = compute.parallel grid_raw "gpu"
step = (s : GPar) => (compute.plrun kg ((GArg)(.n s.n, .cols s.cols, .alpha s.alpha, .I GIn(.x s.in.x))) : GOut)
built = compute.graph step
plane = (compute.buffer ([0.0, 0.0, 4.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 9}) : (compute.Buf Float))
compute.collect (compute.graphrun built (9, 3, 0.25, plane))
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

/// The convolution whose coefficients are a buffer: a **claimed** reduction with a window read.
const FIR_SMOOTH: &str = r#"
--- compute = import "compute.lichen" ---
FIn  = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
FOut = struct{.y (compute.Buf Float)}
FPar = struct{.n Int, .taps Int, .in FIn, .out FOut}
FArg = struct{.n Int, .taps Int, .I FIn}
filter_raw = (c : FPar) => {
  i = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from c.in.w, .at (s(0) - 1))) * compute.read ((compute.Read _)(.from c.in.x, .at (i - (s(0) - 1)))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
kf = compute.parallel (filter_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
smooth = (s : FPar) => (compute.plrun kf ((FArg)(.n s.n, .taps s.taps, .I FIn(.x s.in.x, .w s.in.w))) : FOut)
built = compute.graph smooth
x = (compute.buffer ([1.0, 2.0, 3.0, 4.0, 5.0, 6.0] : array{Float, 6}) : (compute.Buf Float))
w = (compute.buffer ([1.0, 2.0, 1.0] : array{Float, 3}) : (compute.Buf Float))
compute.collect (compute.graphrun built (6, 3, x, w))
"#;

/// The library's `rowsum`: a **claimed** reduction reading `r * kdim + k`, one lane per row.
const ROWSUM_LIBRARY: &str = r#"
--- compute = import "compute.lichen" ---
RIn  = struct{.x (compute.Buf Float)}
ROut = struct{.y (compute.Buf Float)}
RPar = struct{.n Int, .kdim Int, .in RIn, .out ROut}
RArg = struct{.n Int, .kdim Int, .I RIn}
rowsum_raw = (c : RPar) => {
  r = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + compute.read ((compute.Read _)(.from c.in.x, .at (r * c.kdim + (s(0) - 1))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at r, .value (fold (c.kdim, 0.0 ~ "identity"))))
}
kr = compute.parallel (rowsum_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
sums = (s : RPar) => (compute.plrun kr ((RArg)(.n s.n, .kdim s.kdim, .I RIn(.x s.in.x))) : ROut)
built = compute.graph sums
x = (compute.buffer ([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0] : array{Float, 8}) : (compute.Buf Float))
compute.collect (compute.graphrun built (2, 4, x))
"#;

/// The library's `scale`: elementwise, so there is **no claim** to register — the control.
const SCALE_ELEMENTWISE: &str = r#"
--- compute = import "compute.lichen" ---
SIn  = struct{.x (compute.Buf Float)}
SOut = struct{.y (compute.Buf Float)}
SPar = struct{.n Int, .k Float, .in SIn, .out SOut}
SArg = struct{.n Int, .k Float, .I SIn}
scale_raw = (c : SPar) => {
  i = compute.range c.n
  x = compute.read ((compute.Read _)(.from c.in.x, .at i))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (x * c.k)))
}
ks = compute.parallel scale_raw "gpu"
twice = (s : SPar) => (compute.plrun ks ((SArg)(.n s.n, .k s.k, .I SIn(.x s.in.x))) : SOut)
built = compute.graph twice
x = (compute.buffer ([1.0, 2.0, 3.0, 4.0] : array{Float, 4}) : (compute.Buf Float))
compute.collect (compute.graphrun built (4, 2.0, x))
"#;

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
    measure("stencil_dirichlet", STENCIL_DIRICHLET);
    measure("stencil_ring", STENCIL_RING);
    measure("stencil_grid", STENCIL_GRID);
    measure("fir_smooth", FIR_SMOOTH);
    measure("rowsum_library", ROWSUM_LIBRARY);
    measure("scale_elementwise", SCALE_ELEMENTWISE);
}
