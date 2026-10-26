//! Probe: what the installed floor makes of a real operator's recorded graph.

use super::common;

use lichen_compute::compute::{
    clear_graph_pass, consult_graph_pass, set_graph_pass, take_graph_pass_lines,
};
use lichen_compute::floor::floor_pass_with_packs;
use lichen_compute::fusion_claim::{fragment_of, kernel_id};
use lichen_compute::language::LangProgram;
use lichen_graph_ir::{Count, Graph, KernelNode, Node};
use lichen_lowlevel::Module;

/// The extent a consultation hands the pass.
///
/// # Invariant
/// One count for every node of the graph, so the pass lines a case prints are the pass's answer
/// to that extent and not to one the reader has to re-derive from the program.
const EXTENT: usize = common::LANES as usize;

/// The GEMV of `examples/gemv.lichen`, through the graph leg, collected directly.
const GEMV: &str = r#"
--- compute = import "compute.lichen" ---
InG  := _x => {.w (compute.Buf (compute.BufParam{{.type Float, .length _}})), .x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
OutG := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Par  := _x => {.n @any : Int, .kdim @any : Int, .in InG _, .out OutG}
ArgK := _x => {.n @any : Int, .kdim @any : Int, .I InG _}
gemv_raw := (k <: Par) => {
  i := compute.range k.n
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read (compute.Read{.from k.in.w, .at (i * k.kdim + (s(0) - 1))}) : Float) * (compute.read (compute.Read{.from k.in.x, .at (s(0) - 1)}) : Float))) ~ "combine" ~ "element"))
  compute.write (compute.Write{.to k.out.y, .at i, .value (fold (k.kdim, 0.0 ~ "identity"))})
}
kg := compute.parallel (gemv_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (OutG _)
step := (g <: Par) => (compute.plrun kg ((ArgK){.n g.n, .kdim g.kdim, .I InG{.w g.in.w, .x g.in.x}}) : Struct)
built := compute.graph step
w := (compute.buffer ([@any : Float, 12][1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
x := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
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
XIn  := _x => {.x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
XOut := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
XPar := _x => {.n @any : Int, .alpha @any : Float, .in XIn _, .out XOut}
XArg := _x => {.n @any : Int, .alpha @any : Float, .I XIn _}
ring_raw := (c <: XPar) => {
  i := compute.range c.n
  centre := (compute.read (compute.Read{.from c.in.x, .at i}) : Float)
  left := if i == 0 then ((compute.read (compute.Read{.from c.in.x, .at (c.n - 1)}) : Float)) else ((compute.read (compute.Read{.from c.in.x, .at (i - 1)}) : Float))
  right := if i == (c.n - 1) then ((compute.read (compute.Read{.from c.in.x, .at 0}) : Float)) else ((compute.read (compute.Read{.from c.in.x, .at (i + 1)}) : Float))
  compute.write (compute.Write{.to c.out.y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre))})
}
ks := compute.parallel ring_raw "gpu" (XOut _)
step := (s <: XPar) => (compute.plrun ks ((XArg){.n s.n, .alpha s.alpha, .I XIn{.x s.in.x}}) : Struct)
built := compute.graph step
x := (compute.buffer ([@any : Float, 6][4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
compute.collect (compute.graphrun built (6, 0.25, x))
"#;

/// The stencil with no `Select`: both boundaries are the padding.
///
/// # Invariant
/// Three computed reads and a lane-index write, so the body is the Dirichlet form with no case
/// analysis at all.
const STENCIL_DIRICHLET: &str = r#"
--- compute = import "compute.lichen" ---
XIn  := _x => {.x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
XOut := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
XPar := _x => {.n @any : Int, .alpha @any : Float, .in XIn _, .out XOut}
XArg := _x => {.n @any : Int, .alpha @any : Float, .I XIn _}
step_raw := (c <: XPar) => {
  i := compute.range c.n
  centre := (compute.read (compute.Read{.from c.in.x, .at i}) : Float)
  left := (compute.read (compute.Read{.from c.in.x, .at (i - 1)}) : Float)
  right := (compute.read (compute.Read{.from c.in.x, .at (i + 1)}) : Float)
  compute.write (compute.Write{.to c.out.y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre))})
}
ks := compute.parallel step_raw "gpu" (XOut _)
step := (s <: XPar) => (compute.plrun ks ((XArg){.n s.n, .alpha s.alpha, .I XIn{.x s.in.x}}) : Struct)
built := compute.graph step
x := (compute.buffer ([@any : Float, 6][4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
compute.collect (compute.graphrun built (6, 0.25, x))
"#;

/// The two-dimensional step: `compute.row`/`compute.col`, four neighbours, two masks.
const STENCIL_GRID: &str = r#"
--- compute = import "compute.lichen" ---
GIn  := _x => {.x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
GOut := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
GPar := _x => {.n @any : Int, .cols @any : Int, .alpha @any : Float, .in GIn _, .out GOut}
GArg := _x => {.n @any : Int, .cols @any : Int, .alpha @any : Float, .I GIn _}
grid_raw := (c <: GPar) => {
  i := compute.range c.n
  row := compute.row c
  col := compute.col c
  centre := (compute.read (compute.Read{.from c.in.x, .at i}) : Float)
  above := (compute.read (compute.Read{.from c.in.x, .at ((row - 1) * c.cols + col)}) : Float)
  below := (compute.read (compute.Read{.from c.in.x, .at ((row + 1) * c.cols + col)}) : Float)
  left := if col == 0 then 0.0 else ((compute.read (compute.Read{.from c.in.x, .at (i - 1)}) : Float))
  right := if col == (c.cols - 1) then 0.0 else ((compute.read (compute.Read{.from c.in.x, .at (i + 1)}) : Float))
  compute.write (compute.Write{.to c.out.y, .at i, .value (centre + c.alpha * ((above + below + left + right) - 4.0 * centre))})
}
kg := compute.parallel grid_raw "gpu" (GOut _)
step := (s <: GPar) => (compute.plrun kg ((GArg){.n s.n, .cols s.cols, .alpha s.alpha, .I GIn{.x s.in.x}}) : Struct)
built := compute.graph step
plane := (compute.buffer ([@any : Float, 9][0.0, 0.0, 4.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
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
fn decode_expr(capacity: usize, dim: usize, direct: bool) -> String {
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
            r#"fresh := (compute.plrun append ((KArg){{.n {count}, .cap {capacity}, .dim {dim}, .t 1, .I DIn{{.k k0, .v v0, .nk nk, .nv nv, .q q}}}}) : Struct)
y := (compute.plrun gemv ((GArg){{.n {capacity}, .kdim {dim}, .I GIn{{.w fresh.k, .x q}}}}) : Struct)
(compute.collect fresh.k, compute.collect fresh.v, compute.collect y.y)"#
        )
    } else {
        format!(
            r#"SArg := _x => {{.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .m @any : Int, .in DIn _}}
step := (s <: SArg) => {{{{
  fresh := (compute.plrun append ((KArg){{.n s.n, .cap s.cap, .dim s.dim, .t s.t, .I DIn{{.k s.in.k, .v s.in.v, .nk s.in.nk, .nv s.in.nv, .q s.in.q}}}}) : Struct)
  compute.plrun gemv ((GArg){{.n s.m, .kdim s.dim, .I GIn{{.w fresh.k, .x s.in.q}}}}) : Struct
}}}}
built := compute.graph step
compute.collect (compute.graphrun built ({count}, {capacity}, {dim}, 1, {capacity}, k0, v0, nk, nv, q))"#
        )
    };
    format!(
        r#"
--- compute = import "compute.lichen" ---
DIn   := _x => {{.k (compute.Buf (compute.BufParam{{.type Float, .length _}})), .v (compute.Buf (compute.BufParam{{.type Float, .length _}})), .nk (compute.Buf (compute.BufParam{{.type Float, .length _}})), .nv (compute.Buf (compute.BufParam{{.type Float, .length _}})), .q (compute.Buf (compute.BufParam{{.type Float, .length _}}))}}
KVOut := _x => {{.k (compute.Buf (compute.BufParam{{.type Float, .length _}})), .v (compute.Buf (compute.BufParam{{.type Float, .length _}}))}}
KPar  := _x => {{.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .in DIn _, .out KVOut}}
KArg  := _x => {{.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .I DIn _}}
append_raw := (k <: KPar) => {{
  column := compute.row k
  position := compute.col k
  slot := position * k.dim + column
  old_k := (compute.read (compute.Read{{.from k.in.k, .at slot}}) : Float)
  old_v := (compute.read (compute.Read{{.from k.in.v, .at slot}}) : Float)
  fresh := position == k.t
  new_k := if fresh then ((compute.read (compute.Read{{.from k.in.nk, .at column}}) : Float)) else old_k
  new_v := if fresh then ((compute.read (compute.Read{{.from k.in.nv, .at column}}) : Float)) else old_v
  (compute.write (compute.Write{{.to k.out.k, .at slot, .value (new_k ~ "element")}}), compute.write (compute.Write{{.to k.out.v, .at slot, .value (new_v ~ "element")}}))
}}
append := compute.parallel (append_raw ~ "claim:Reduction(element)") "gpu" (KVOut _)
GIn  := _x => {{.w (compute.Buf (compute.BufParam{{.type Float, .length _}})), .x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}}
GOut := _x => {{.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}}
GPar := _x => {{.n @any : Int, .kdim @any : Int, .in GIn _, .out GOut}}
GArg := _x => {{.n @any : Int, .kdim @any : Int, .I GIn _}}
gemv_raw := (k <: GPar) => {{
  i := compute.range k.n
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read (compute.Read{{.from k.in.w, .at (i * k.kdim + (s(0) - 1))}}) : Float) * (compute.read (compute.Read{{.from k.in.x, .at (s(0) - 1)}}) : Float))) ~ "combine" ~ "element"))
  compute.write (compute.Write{{.to k.out.y, .at i, .value (fold (k.kdim, 0.0 ~ "identity"))}})
}}
gemv := compute.parallel (gemv_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (GOut _)
k0 := (compute.buffer ([@any : Float, {count}][{cache}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
v0 := (compute.buffer ([@any : Float, {count}][{values}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
nk := (compute.buffer ([@any : Int, 1][{key}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
nv := (compute.buffer ([@any : Int, 1][{value}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
q  := (compute.buffer ([@any : Float, {dim}][{query}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
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
FIn  := _x => {.x (compute.Buf (compute.BufParam{{.type Float, .length _}})), .w (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
FOut := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
FPar := _x => {.n @any : Int, .taps @any : Int, .in FIn _, .out FOut}
FArg := _x => {.n @any : Int, .taps @any : Int, .I FIn _}
filter_raw := (c <: FPar) => {
  i := compute.range c.n
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read (compute.Read{.from c.in.w, .at (s(0) - 1)}) : Float) * (compute.read (compute.Read{.from c.in.x, .at (i - (s(0) - 1))}) : Float))) ~ "combine" ~ "element"))
  compute.write (compute.Write{.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))})
}
kf := compute.parallel (filter_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (FOut _)
smooth := (s <: FPar) => (compute.plrun kf ((FArg){.n s.n, .taps s.taps, .I FIn{.x s.in.x, .w s.in.w}}) : Struct)
built := compute.graph smooth
x := (compute.buffer ([@any : Float, 6][1.0, 2.0, 3.0, 4.0, 5.0, 6.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
w := (compute.buffer ([@any : Float, 3][1.0, 2.0, 1.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
compute.collect (compute.graphrun built (6, 3, x, w))
"#;

/// The library's `rowsum`: a **claimed** reduction reading `r * kdim + k`, one lane per row.
const ROWSUM_LIBRARY: &str = r#"
--- compute = import "compute.lichen" ---
RIn  := _x => {.x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
ROut := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
RPar := _x => {.n @any : Int, .kdim @any : Int, .in RIn _, .out ROut}
RArg := _x => {.n @any : Int, .kdim @any : Int, .I RIn _}
rowsum_raw := (c <: RPar) => {
  r := compute.range c.n
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read (compute.Read{.from c.in.x, .at (r * c.kdim + (s(0) - 1))}) : Float)) ~ "combine" ~ "element"))
  compute.write (compute.Write{.to c.out.y, .at r, .value (fold (c.kdim, 0.0 ~ "identity"))})
}
kr := compute.parallel (rowsum_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (ROut _)
sums := (s <: RPar) => (compute.plrun kr ((RArg){.n s.n, .kdim s.kdim, .I RIn{.x s.in.x}}) : Struct)
built := compute.graph sums
x := (compute.buffer ([@any : Float, 8][1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
compute.collect (compute.graphrun built (2, 4, x))
"#;

/// The library's `scale`: elementwise, so there is **no claim** to register — the control.
const SCALE_ELEMENTWISE: &str = r#"
--- compute = import "compute.lichen" ---
SIn  := _x => {.x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
SOut := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
SPar := _x => {.n @any : Int, .k @any : Float, .in SIn _, .out SOut}
SArg := _x => {.n @any : Int, .k @any : Float, .I SIn _}
scale_raw := (c <: SPar) => {
  i := compute.range c.n
  x := (compute.read (compute.Read{.from c.in.x, .at i}) : Float)
  compute.write (compute.Write{.to c.out.y, .at i, .value (x * c.k)})
}
ks := compute.parallel scale_raw "gpu" (SOut _)
twice := (s <: SPar) => (compute.plrun ks ((SArg){.n s.n, .k s.k, .I SIn{.x s.in.x}}) : Struct)
built := compute.graph twice
x := (compute.buffer ([@any : Float, 4][1.0, 2.0, 3.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
compute.collect (compute.graphrun built (4, 2.0, x))
"#;

/// The one-node graph the recording's own fragment makes.
///
/// # Invariant
/// The node is built from the registry's own fragment and not from a run's, so the pass is
/// handed exactly the body the program compiled, and the consultation dispatches nothing.
fn recorded_graph(module: &Module<LangProgram>) -> Graph {
    let kernel = module
        .nodes
        .iter()
        .find_map(|(id, _node)| kernel_id(module, id).ok())
        .expect("the program compiled one parallel kernel");
    let fragment = fragment_of(kernel).expect("the registry holds the fragment it compiled");
    let mut graph = Graph::with_inputs(1);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment,
                inputs: vec![0],
                count: Count::Constant(EXTENT),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");
    graph
}

/// The lines the installed floor reports for `source`'s recorded graph.
///
/// # Invariant
/// The pass is consulted over the recording and never through a run, so its lines are read
/// without a dispatch: the merge stage's answer and the tiling stage's are both host work.
fn measure(label: &str, source: &str) {
    clear_graph_pass();
    let report = common::check(source);
    if !report.ok() {
        println!(
            "[{label}] refused: {}",
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<String>>()
                .join(" | ")
        );
        return;
    }
    let module = report
        .build
        .expect("an accepted program has a build")
        .module;
    set_graph_pass(floor_pass_with_packs(&module, &[]));
    let graph = recorded_graph(&module);
    consult_graph_pass(&graph, &|_node| Some(EXTENT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    println!("[{label}] pass: {lines:?}");
}

#[test]
fn probe_the_floor_on_a_real_operator() {
    measure("gemv", GEMV);
    measure("decode_direct", &decode_expr(8, 1, true));
    measure(
        "decode_direct_cpu",
        &decode_expr(8, 1, true).replace("\"gpu\"", "\"cpu\""),
    );
    measure("decode_direct_4x2", &decode_expr(4, 2, true));
    measure(
        "decode_direct_4x2_cpu",
        &decode_expr(4, 2, true).replace("\"gpu\"", "\"cpu\""),
    );
    measure("decode_expr", &decode_expr(4, 2, false));
    measure("decode_expr_equal", &decode_expr(8, 1, false));
    measure("stencil_dirichlet", STENCIL_DIRICHLET);
    measure("stencil_ring", STENCIL_RING);
    measure("stencil_grid", STENCIL_GRID);
    measure("fir_smooth", FIR_SMOOTH);
    measure("rowsum_library", ROWSUM_LIBRARY);
    measure("scale_elementwise", SCALE_ELEMENTWISE);
}
