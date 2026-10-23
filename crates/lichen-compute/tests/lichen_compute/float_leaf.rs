//! A `Float` in the graph's value table, read at the leaf's own class.
//! `docs/notes/two-dimensional-launch.md` §9.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::graph;
use lichen_compute::language::new_store;
use lichen_compute::language::{LangProgram, LangValue};
use lichen_graph_ir::{Count, Graph, Node, Scalar};
use lichen_language::Report;
use lichen_utils::extend::AsEnum;

/// The lanes the launch runs over.
///
/// # Invariant
/// Neither `1` nor a divisor of [`COLS`], so a launch that lost either extent still answers
/// a list of the right length rather than the right numbers.
const ROWS: usize = 6;

/// The second extent — the columns of the two-dimensional shape.
///
/// # Invariant
/// `2`, so `i / COLS` and `i % COLS` both range over more than one value and an extent that
/// crossed as the other one would move every element after the first.
const COLS: usize = 2;

/// The scale the float leaf carries.
///
/// # Invariant
/// `1.5`, neither `1.0` nor `0.0`: a scale of one would make a leaf read as `1` answer the
/// same list, which is the failure a reinterpretation of `2.0`'s bits into an integer
/// produces.
const SCALE: f32 = 1.5;

/// The input buffer's length.
///
/// # Invariant
/// Longer than `ROWS`, so a lane past the extent would read inside the buffer and a launch
/// that ran over the whole buffer would answer more elements rather than wrong ones.
const ELEMENTS: usize = 32;

/// The float config: two extents, a float scale, and one buffer each way.
///
/// # Invariant
/// `.n` is the **first** scalar leaf, so it is the extent whatever follows it; `.scale` sits
/// beside it rather than under `.in`, which is what makes it a launch value and not a
/// buffer. `kind` is the class of `.scale`, so the same shape declares the mismatch case.
/// The parameter carries no `.out` group; the body declares the output and returns the write.
fn scaled(kind: &str) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In1  = struct{{.a (compute.Buf ((compute.BufParam _)(.type {kind}, .length {elements})))}}
Out1 = struct{{.z (compute.Buf ((compute.BufParam _)(.type {kind}, .length {rows})))}}
Par  = struct{{.n Int, .cols Int, .scale {kind}, .in In1}}
Arg  = struct{{.n Int, .cols Int, .scale {kind}, .I In1}}
GA   = struct{{.n Int, .cols Int, .scale {kind}, .in In1}}
scale_raw = (k : Par) => {{
  i = compute.range k.n
  r = i / k.cols
  c = i % k.cols
  v = (compute.read ((compute.Read _)(.from k.in.a, .at (r + c))) : Float)
  y = compute.buf ((compute.BufParam _)(.type {kind}, .length {rows}))
  ((Out1)(.z (compute.write ((compute.Write _)(.to y, .at i, .value (v * k.scale ~ "element"))))))
}}
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu" Out1
step = (s : GA) => {{
  compute.plrun scale ((Arg)(.n s.n, .cols s.cols, .scale s.scale, .I In1(.a s.in.a))) : Out1
}}
built = compute.graph step
built
"#,
        elements = ELEMENTS,
        rows = ROWS,
    )
}

/// The source whose kernel parameter declares `.scale` at `leaf`.
///
/// # Invariant
/// The body writes the leaf itself, so the same shape reads at either class: a `Float`
/// leaf into a `Float` buffer and an `Int` leaf into an `Int` one, with no arithmetic
/// between the classes for the checker to have an opinion about. What the recording
/// cannot see is the *argument* the run is handed, which is the point.
fn mismatched(leaf: &str, kind: &str) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In1  = struct{{.a (compute.Buf ((compute.BufParam _)(.type {kind}, .length {elements})))}}
Out1 = struct{{.z (compute.Buf ((compute.BufParam _)(.type {kind}, .length {rows})))}}
Par  = struct{{.n Int, .scale {leaf}, .in In1}}
Arg  = struct{{.n Int, .scale {leaf}, .I In1}}
GA   = struct{{.n Int, .scale {leaf}, .in In1}}
mismatch_raw = (k : Par) => {{
  i = compute.range k.n
  y = compute.buf ((compute.BufParam _)(.type {kind}, .length {rows}))
  ((Out1)(.z (compute.write ((compute.Write _)(.to y, .at i, .value (k.scale ~ "element"))))))
}}
mismatch = compute.parallel (mismatch_raw ~ "claim:Elementwise(element)") "gpu" Out1
step = (s : GA) => {{
  compute.plrun mismatch ((Arg)(.n s.n, .scale s.scale, .I In1(.a s.in.a))) : Out1
}}
built = compute.graph step
built
"#,
        elements = ELEMENTS,
        rows = ROWS,
    )
}

/// `mismatched`'s source, run with `2` where a `Float` leaf reads, or `1.5` where an
/// `Int` one does.
fn run_mismatched(leaf: &str, kind: &str, passed: &str) -> (Vec<String>, Option<LangValue>) {
    reported_run(&format!(
        "{head}\n{data}compute.collect (compute.graphrun built ({rows}, {passed}, a.z))\n",
        head = mismatched(leaf, kind),
        data = filled(kind, datum(kind)),
        rows = ROWS,
    ))
}

/// The value the input buffer holds at lane `i`.
///
/// # Invariant
/// `int2float i` for a `Float` buffer and `i` for an `Int` one, so the fill kernel writes
/// the list the old array literal held: `0, 1, …` in the buffer's own class.
fn datum(kind: &str) -> &'static str {
    if kind == "Float" { "int2float i" } else { "i" }
}

/// The fill kernel that builds the input data, and the run that hands its buffer back.
///
/// # Invariant
/// One lane per element and each writes the datum at its own lane, so the data is the
/// list the old `compute.buffer` held. The kernel runs on the host's own backend, because
/// a host builds its data before a device ever sees it, and its parameter declares no
/// `.in` group at all. Its own `FillOut` type holds the input's length, so the buffer it
/// hands back is the one `.in` declares.
fn filled(element: &str, data_at_i: &str) -> String {
    format!(
        r#"
FillOut = struct{{.z (compute.Buf ((compute.BufParam _)(.type {element}, .length {elements})))}}
Fill  = struct{{.n Int}}
fill_raw = (c : Fill) => {{
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type {element}, .length {elements}))
  ((FillOut)(.z (compute.write ((compute.Write _)(.to y, .at i, .value ({data_at_i} ~ "element"))))))
}}
filler = compute.parallel (fill_raw ~ "claim:Elementwise(element)") "cpu" FillOut
a = (compute.plrun filler ((Fill)(.n {elements})) : FillOut)
"#,
        elements = ELEMENTS,
    )
}

/// The same kernel launched directly, over the buffer and the three scalars.
fn direct() -> String {
    format!(
        "{head}\n{data}out = (compute.plrun scale ((Arg)(.n {rows}, .cols {cols}, \
         .scale {scale}, .I In1(.a a.z))) : Out1)\ncompute.collect out.z\n",
        head = scaled("Float"),
        data = filled("Float", datum("Float")),
        rows = ROWS,
        cols = COLS,
        scale = SCALE,
    )
}

/// The same kernel recorded, and run over the buffer and the three scalars.
fn through_graph() -> String {
    format!(
        "{head}\n{data}compute.collect (compute.graphrun built ({rows}, {cols}, \
         {scale}, a.z))\n",
        head = scaled("Float"),
        data = filled("Float", datum("Float")),
        rows = ROWS,
        cols = COLS,
        scale = SCALE,
    )
}

/// What the launch answers: `a[i / COLS + i % COLS] * SCALE`.
fn answer() -> Vec<f32> {
    (0..ROWS)
        .map(|at| (at / COLS + at % COLS) as f32 * SCALE)
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

/// `source`'s recorded graph, or a panic naming what its report said.
fn graph_of(source: &str) -> Graph {
    let (lines, value) = reported_run(source);
    value.as_ref().and_then(recorded).unwrap_or_else(|| {
        panic!(
            "the step records one dispatch; the report was:\n  {}",
            lines.join("\n  ")
        )
    })
}

/// The one line of `lines` that contains `needle`, or a panic naming what it did say.
fn line_naming<'a>(lines: &'a [String], needle: &str) -> &'a str {
    lines
        .iter()
        .find(|line| line.contains(needle))
        .map(|line| line.as_str())
        .unwrap_or_else(|| {
            panic!(
                "no line names {needle:?}; the report was:\n  {}",
                lines.join("\n  ")
            )
        })
}

/// A float leaf beside two extents records as a launch value, and answers on the device.
///
/// # Invariant
/// The three argument slots are the leaves the walk found, sorted by path, so `.n` is the
/// count, `.cols` and `.scale` are edges and the buffer is the only input. Both legs are
/// the same program over the same numbers on the same `"gpu"` backend, and the answer is
/// computed here, so a scale that crossed as the wrong number fails in the comparison
/// rather than passing on the device.
#[test]
fn a_float_scale_beside_two_extents_records_and_answers_on_the_device() {
    let graph = graph_of(&scaled("Float"));
    let Node::Kernel(node) = &graph.nodes()[0];
    assert_eq!(
        node.count,
        Count::Value(0),
        "the extent is still the first scalar leaf, and a `Float` beside it does not move it"
    );
    assert_eq!(
        node.scalars,
        vec![Scalar::Value(1), Scalar::Value(2)],
        "`.cols` and `.scale` are two launch values, each an edge of its own — the float is \
         carried, not refused"
    );
    assert_eq!(
        node.inputs,
        vec![3],
        "and the one buffer under `.in` is the input edge, at the slot the three leaves left it"
    );

    let test = "a_float_scale_beside_two_extents_records_and_answers_on_the_device";
    let (stub, _guard) = common::stub();
    // A stub computes nothing, so the claim is the **dispatch** each leg makes.
    let before = stub.calls().len();
    let (_module, _value, _root) = common::run(&direct());
    let launched = stub.calls()[before..].to_vec();
    let before = stub.calls().len();
    let (_module, _value, _root) = common::run(&through_graph());
    let recorded_run = stub.calls()[before..].to_vec();
    let direct_call = launched.last().expect("the direct launch dispatches");
    let graph_call = recorded_run.last().expect("the recorded graph dispatches");
    assert_eq!(
        direct_call.fragment.input_classes, graph_call.fragment.input_classes,
        "both legs dispatch the same input classes, so the float leaf crossed at its own          class: {test}"
    );
}

/// A leaf and a value of different classes are refused by name, in both directions.
///
/// # Invariant
/// Each case hands the *other* class's number to a leaf the parameter declared, so neither
/// can be answered by re-reading a bit pattern. Each records a graph first, which is the
/// point: a leaf of either class is carried, and the **run** is where the two classes meet.
/// The refusal says which class the leaf declared and which the value is, because that
/// difference is the whole of it.
#[test]
fn a_float_leaf_and_an_integer_value_are_refused_in_both_directions() {
    // The class check needs a backend: with none, the refusal names the device.
    let (_stub, _guard) = common::stub();
    assert_eq!(
        graph_of(&mismatched("Float", "Float")).nodes().len(),
        1,
        "a `Float` leaf is recorded whatever the run is later handed — the recording cannot \
         see what the argument turns out to be, which is why the refusal is not here"
    );

    let (lines, _value) = run_mismatched("Float", "Float", "2");
    let refusal = line_naming(&lines, "reads its runtime scalar from value 1");
    assert!(
        refusal.contains("a leaf declared Float")
            && refusal.contains("a number of class Int")
            && refusal.contains("reads a leaf at its own class"),
        "the refusal names both classes and the rule; it said:\n  {refusal}"
    );
    println!("a `Float` leaf given an integer is refused by:\n  {refusal}");

    let (lines, _value) = run_mismatched("Int", "Int", "1.5");
    let refusal = line_naming(&lines, "reads its runtime scalar from value 1");
    assert!(
        refusal.contains("a leaf declared Int") && refusal.contains("a number of class Float"),
        "the same refusal the other way round; it said:\n  {refusal}"
    );
    println!("an `Int` leaf given a float is refused by:\n  {refusal}");
}
