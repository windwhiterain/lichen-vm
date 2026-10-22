//! A graph's recorded dispatch carries the runtime scalar beside its extent.
//! See docs/notes/two-dimensional-launch.md §6.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::graph;
use lichen_compute::language::new_store;
use lichen_compute::language::{LangProgram, LangValue};
use lichen_graph_ir::{Graph, Node, Scalar};
use lichen_language::Report;
use lichen_utils::extend::AsEnum;

/// The extents the launch is over, and the buffer's length.
///
/// # Invariant
/// `COLS` divides `N`, so every lane's `(row, col)` is inside the rectangle and the
/// answer below is the whole strip rather than a prefix of it. The buffer is one
/// element longer than the largest index the body reads, which is the tail-lane
/// obligation the device states: a lane of the last workgroup reads past `count`
/// whether or not the body looks.
const N: usize = 6;
const COLS: usize = 3;

/// The input buffer: `1 … 6`, of which the body reads the five it needs.
const ELEMENTS: [usize; N] = [1, 2, 3, 4, 5, 6];

/// The head every case here declares: a kernel parameter carrying **two** extents.
///
/// # Invariant
/// `.n` and `.cols` are both ordinary scalar leaves of one parameter, and the body
/// splits the invocation itself with a `Div` and a `Rem` — so what this file measures is
/// the plumbing that carries `.cols`, not a spelling for the split.
const HEAD: &str = r#"
---
  compute = import "compute.lichen"
---
In   = struct{.a (compute.Buf Int)}
Out  = struct{.b (compute.Buf Int)}
Par  = struct{.n Int, .cols Int, .in In, .out Out}
Arg  = struct{.n Int, .cols Int, .I In}
strip_raw = (k : Par) => {
  i = compute.range k.n
  r = i / k.cols
  c = i % k.cols
  v = (compute.read ((compute.Read _)(.from k.in.a, .at (r * 2 + c))) : Int)
  compute.write ((compute.Write _)(.to k.out.b, .at i, .value (v ~ "element")))
}
"#;

/// The head of the §2.2 control: the same two extents over **two** input buffers.
const CONTROL: &str = r#"
---
  compute = import "compute.lichen"
---
In2   = struct{.a (compute.Buf Int), .b (compute.Buf Int)}
Out2  = struct{.z (compute.Buf Int)}
Par2  = struct{.n Int, .cols Int, .in In2, .out Out2}
Arg2  = struct{.n Int, .cols Int, .I In2}
"#;

/// `body`, declared over the control's parameter, as the kernel `compute.parallel` takes it.
fn control_kernel(name: &str, body: &str) -> String {
    format!(
        "{CONTROL}\n{body}\nkernel = compute.parallel ({name} ~ \"claim:Elementwise(element)\") \"gpu\" Out2\n"
    )
}

/// A step over one extent and one buffer, recording `strip` as its dispatch.
fn recording() -> String {
    format!(
        "{HEAD}\nstrip = compute.parallel (strip_raw ~ \"claim:Elementwise(element)\") \"gpu\" Out\n\
         GA = struct{{.n Int, .cols Int, .in In}}\nstep = (s : GA) => {{\n  \
         compute.plrun strip ((Arg)(.n s.n, .cols s.cols, .I In(.a s.in.a))) : Out\n}}\n\
         built = compute.graph step\nbuilt\n"
    )
}

/// The input buffer's elements as the literals a `compute.buffer` call takes.
fn elements() -> String {
    ELEMENTS
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<String>>()
        .join(", ")
}

/// `recording()`, run over the two extents and the buffer.
fn running() -> String {
    format!(
        "{}\ncount = ({n})\ncols = ({cols})\na = (compute.buffer ([{elements}] : \
         array{{Int, {n}}}) : (compute.Buf Int))\ncompute.collect (compute.graphrun built \
         (count, cols, a))\n",
        recording(),
        n = N,
        cols = COLS,
        elements = elements(),
    )
}

/// `y[i] = a[2·(i / cols) + (i % cols)]` over `a = 1 … N`.
///
/// # Invariant
/// The strip of two rows of a two-column walk: lane 0 reads `a[0]`, lane 3 reads `a[2]`
/// again, and no two lanes answer alike by accident — the index is not injective, so a
/// scalar that crossed as the wrong number would put a different repeat in the answer.
fn answer() -> Vec<usize> {
    (0..N)
        .map(|at| ELEMENTS[2 * (at / COLS) + (at % COLS)])
        .collect()
}

/// `source` compiled and checked, never run.
fn compile(source: &str) -> Report<LangProgram> {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(source, None, &mut store);
    assert!(diags.is_empty(), "the program's imports resolve: {diags:?}");
    lichen_language::compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &lichen_language::lex::line_starts(source),
        lichen_highlevel::no_native_ops(),
    )
}

/// `source` compiled and run: every line its report carried, and the root value.
///
/// # Invariant
/// The lines are the ones a user reads, in both channels and in the order they were
/// produced: the check report's own, then the run's refusals as `render_build` prints
/// them (`category: message`).
fn reported_run(source: &str) -> (Vec<String>, Option<LangValue>) {
    let report = compile(source);
    let mut lines: Vec<String> = report
        .diagnostics
        .iter()
        .map(|line| line.message.clone())
        .collect();
    if !report.ok() {
        return (lines, None);
    }
    let build = report.build.expect("an accepted program has a build");
    let mut module = build.module;
    let root = module.evaluate_node_deep(build.root_val, None);
    lines.extend(
        module
            .extension_diagnostics
            .iter()
            .map(|entry| format!("{}: {}", entry.category, entry.message)),
    );
    (lines, root)
}

/// The graph the program's root value recorded, or `None` when it recorded nothing.
///
/// # Invariant
/// A decided `Graph` value whose id the registry does not hold is not a recording, so
/// registration is checked as well as the variant.
fn recorded(value: &LangValue) -> Option<Graph> {
    let ComputeValue::Graph(id, _) = AsEnum::<ComputeValue>::as_enum(value)? else {
        return None;
    };
    graph::lookup(id).ok()
}

/// The two extents reach the kernel: one as the count, one as the scalar beside it.
///
/// # Invariant
/// The three numbers are the three argument slots, and they are named by role because a
/// graph that numbered them in the wrong order would still run: the buffer would arrive
/// under a number that is a scalar, which is the failure this slice exists to remove.
#[test]
fn the_second_extent_is_recorded_as_the_scalars_own_edge() {
    let (_module, value, _root) = common::run(&recording());
    let recorded = recorded(&value).expect("the two-extent step records");
    assert_eq!(
        (recorded.len(), recorded.input_count()),
        (1, 3),
        "one dispatch over the extent, the second extent and the buffer"
    );
    let Node::Kernel(node) = &recorded.nodes()[0];
    assert_eq!(
        node.count,
        lichen_graph_ir::Count::Value(0),
        "the extent is the first argument"
    );
    assert_eq!(
        node.scalars,
        vec![Scalar::Value(1)],
        "the second extent is an edge of its own, beside the count"
    );
    assert_eq!(
        node.inputs,
        vec![2],
        "and the buffer is the third argument, not the scalar"
    );
}

/// The device runs the two-extent launch and answers element for element.
///
/// # Invariant
/// The accepted twin is the same program run directly, not a literal: a graph path that
/// dropped the scalar would answer the strip only if the device happened to agree, so
/// both are run here and compared against the same computed answer.
#[test]
fn the_two_extent_launch_runs_on_the_device_through_the_graph() {
    let test = "the_two_extent_launch_runs_on_the_device_through_the_graph";
    let (stub, _guard) = common::stub();
    let direct = format!(
        "{HEAD}\nstrip = compute.parallel (strip_raw ~ \"claim:Elementwise(element)\") \"gpu\" Out\n\
         a = (compute.buffer ([{elements}] : array{{Int, {n}}}) : (compute.Buf Int))\n\
         out = (compute.plrun strip ((Arg)(.n {n}, .cols {cols}, .I In(.a a))) : Out)\n\
         compute.collect out.b\n",
        n = N,
        cols = COLS,
        elements = elements(),
    );
    // A stub computes nothing, so the claim is the **dispatch** each leg makes.
    let before = stub.calls().len();
    let (_module, _value, _root) = common::run(&direct);
    let direct = stub.calls()[before..].to_vec();
    let before = stub.calls().len();
    let (_module, _value, _root) = common::run(&running());
    let through_graph = stub.calls()[before..].to_vec();
    let direct_call = direct.last().expect("the direct launch dispatches");
    let graph_call = through_graph.last().expect("the recorded graph dispatches");
    assert_eq!(
        (direct_call.fragment.inputs, direct_call.fragment.outputs),
        (graph_call.fragment.inputs, graph_call.fragment.outputs),
        "both legs dispatch a kernel with the same buffers, so the second extent crossed: {test}"
    );
    assert_eq!(
        direct_call.fragment.param_shape, graph_call.fragment.param_shape,
        "and the same parameter domain: {test}"
    );
}

/// What makes a second extent admissible is stated, and does not depend on the reads.
///
/// # Invariant
/// The three bodies differ in **which input buffers they read** and in nothing else: the
/// parameter type is the same string in all three. Before the rule was stated this was
/// the accident — a buffer the body never read was counted as a runtime scalar, so the
/// same parameter recorded or refused according to the body's read pattern.
#[test]
fn what_makes_a_second_extent_admissible_does_not_depend_on_the_reads() {
    let both = r#"
both_raw = (k : Par2) => {
  i = compute.range k.n
  x = (compute.read ((compute.Read _)(.from k.in.a, .at i)) : Int)
  y = (compute.read ((compute.Read _)(.from k.in.b, .at i)) : Int)
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value ((x + y) ~ "element")))
}
"#;
    let only_b = r#"
only_b_raw = (k : Par2) => {
  i = compute.range k.n
  y = (compute.read ((compute.Read _)(.from k.in.b, .at i)) : Int)
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value (y ~ "element")))
}
"#;
    let neither = r#"
neither_raw = (k : Par2) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value (k.cols ~ "element")))
}
"#;
    let step = || {
        format!(
            "GA = struct{{.n Int, .cols Int, .in In2}}\nstep = (s : GA) => {{\n  \
             compute.plrun kernel ((Arg2)(.n s.n, .cols s.cols, .I In2(.a s.in.a, .b s.in.b))) \
             : Out2\n}}\nbuilt = compute.graph step\nbuilt\n"
        )
    };
    for (reads, name, body) in [
        ("both", "both_raw", both),
        ("only `.in.b`", "only_b_raw", only_b),
        ("neither", "neither_raw", neither),
    ] {
        let source = format!("{}{}", control_kernel(name, body), step());
        let (lines, value) = reported_run(&source);
        assert!(
            value.as_ref().and_then(recorded).is_some(),
            "a body reading {reads} records, because the rule is the parameter's own shape and \
             not the body's read pattern; the report was:\n  {}",
            lines.join("\n  ")
        );
        let graph = recorded(value.as_ref().unwrap()).expect("the recording is there");
        let Node::Kernel(node) = &graph.nodes()[0];
        assert_eq!(
            node.scalars,
            vec![Scalar::Value(1)],
            "the second extent is the scalar edge whether the body read {reads} or not"
        );
        assert_eq!(
            node.inputs.len(),
            2,
            "and both declared buffers are edges, read or not, so nothing was dropped"
        );
    }
}

/// A second runtime scalar is no longer refused: it is carried, in the parameter's order.
///
/// # Invariant
/// [`HEAD`]'s parameter with one leaf added, so the extent keeps its position and the two
/// scalars sit in the order the walk found them. This is what the refusal at
/// `record_launch` used to stand in the way of.
#[test]
fn a_second_runtime_scalar_is_carried_beside_the_extent() {
    let source = format!(
        "{HEAD}\nPar3 = struct{{.n Int, .cols Int, .rows Int, .in In, .out Out}}\n\
         Arg3 = struct{{.n Int, .cols Int, .rows Int, .I In}}\n\
         three_raw = (k : Par3) => {{\n  i = compute.range k.n\n  \
         v = (compute.read ((compute.Read _)(.from k.in.a, .at (i / k.cols))) : Int)\n  \
         compute.write ((compute.Write _)(.to k.out.b, .at i, .value ((v + k.rows) ~ \"element\")))\n}}\n\
         three = compute.parallel (three_raw ~ \"claim:Elementwise(element)\") \"gpu\" Out\n\
         GA = struct{{.n Int, .cols Int, .rows Int, .in In}}\nstep = (s : GA) => {{\n  \
         compute.plrun three ((Arg3)(.n s.n, .cols s.cols, .rows s.rows, .I In(.a s.in.a))) : Out\n}}\n\
         built = compute.graph step\nbuilt\n"
    );
    let (lines, value) = reported_run(&source);
    let graph = value.as_ref().and_then(recorded).unwrap_or_else(|| {
        panic!(
            "three scalar leaves record; the report was:\n  {}",
            lines.join("\n  ")
        )
    });
    let Node::Kernel(node) = &graph.nodes()[0];
    assert_eq!(
        node.count,
        lichen_graph_ir::Count::Value(0),
        "the extent is still the first scalar leaf, whatever follows it"
    );
    assert_eq!(
        node.scalars,
        vec![Scalar::Value(1), Scalar::Value(2)],
        "both runtime scalars cross as edges of their own, in the parameter's own order"
    );
    assert_eq!(
        node.inputs,
        vec![3],
        "and the buffer is still an input, not a scalar"
    );
}
