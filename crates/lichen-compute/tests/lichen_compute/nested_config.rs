//! A config of any nesting, carrying any number of scalars and buffers.
//! See `docs/notes/two-dimensional-launch.md` §8.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::graph;
use lichen_compute::language::new_store;
use lichen_compute::language::{LangProgram, LangValue};
use lichen_graph_ir::{Graph, Node};
use lichen_language::Report;
use lichen_utils::extend::AsEnum;

/// The lanes both launches run over, and the one factor the nested one multiplies by.
///
/// # Invariant
/// `COLS` is neither `0` nor `1`, so a launch value that crossed under the wrong ordinal
/// changes every element of the answer rather than leaving it the same.
const ROWS: usize = 6;
const COLS: usize = 2;

/// The inner dimension of the two-scalar launch, and its column count.
///
/// # Invariant
/// `K != SHAPE_COLS`, so the two runtime scalars are not interchangeable: a launch that
/// read them under each other's names would answer a different list.
const K: usize = 2;
const SHAPE_COLS: usize = 3;

/// The input buffers' length.
///
/// # Invariant
/// One workgroup's worth of lanes, so a lane past the extent still reads inside the
/// buffer: a dispatch over `ROWS` lanes runs the surplus lanes of its last workgroup,
/// and a shorter buffer would make this measure an out-of-bounds read instead.
const ELEMENTS: usize = 64;

/// The nested config: scalar leaves and buffer leaves at different depths.
///
/// # Invariant
/// `.meta` is a group outside `.in`/`.out`, so both its leaves are scalars and the extent
/// is the **first** of them, whatever depth the walk reached it at; `.in` and `.out` hold
/// the buffers, one level down. `kind` is the class of every leaf past the extent, `ga`
/// the step's own parameter, `cols` what it passes for `.meta.cols`.
fn nested(kind: &str, ga: &str, cols: &str) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
Meta  = struct{{.rows Int, .cols {kind}}}
In2   = struct{{.a (compute.Buf {kind}), .b (compute.Buf {kind})}}
Out   = struct{{.z (compute.Buf {kind})}}
Par   = struct{{.meta Meta, .in In2, .out Out}}
Arg   = struct{{.meta Meta, .I In2}}
GA    = {ga}
nested_raw = (k : Par) => {{
  i = compute.range k.meta.rows
  v = (compute.read ((compute.Read _)(.from k.in.a, .at i)) : {kind})
  w = (compute.read ((compute.Read _)(.from k.in.b, .at i)) : {kind})
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value ((v * k.meta.cols + w) ~ "element")))
}}
nested = compute.parallel (nested_raw ~ "claim:Elementwise(element)") "gpu" Out
step = (s : GA) => {{
  compute.plrun nested ((Arg)(.meta Meta(.rows s.rows, .cols {cols}), .I In2(.a s.in.a, .b s.in.b))) : Out
}}
built = compute.graph step
built
"#
    )
}

/// The flat config with **two** runtime scalars — the operator-shape case.
///
/// # Invariant
/// `.n` is the extent by position and the other two are launch values, so the fragment
/// declares two runtime scalars and the recording has to carry both.
fn flat() -> String {
    r#"
---
  compute = import "compute.lichen"
---
In1   = struct{.a (compute.Buf Int)}
Out1  = struct{.z (compute.Buf Int)}
Par3  = struct{.n Int, .K Int, .cols Int, .in In1, .out Out1}
Arg3  = struct{.n Int, .K Int, .cols Int, .I In1}
GA3   = struct{.n Int, .K Int, .cols Int, .in In1}
shape_raw = (k : Par3) => {
  i = compute.range k.n
  r = i / k.cols
  c = i % k.cols
  v = (compute.read ((compute.Read _)(.from k.in.a, .at (r * k.K + c))) : Int)
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value (v ~ "element")))
}
shape = compute.parallel (shape_raw ~ "claim:Elementwise(element)") "gpu" Out1
step3 = (s : GA3) => {
  compute.plrun shape ((Arg3)(.n s.n, .K s.K, .cols s.cols, .I In1(.a s.in.a))) : Out1
}
built3 = compute.graph step3
built3
"#
    .to_string()
}

/// An input buffer of `n` elements, `offset + i` in each, in the class `kind` names.
fn buffer(name: &str, kind: &str, offset: usize) -> String {
    let elements = (0..ELEMENTS)
        .map(|at| offset + at)
        .map(|at| format!("{at}"))
        .collect::<Vec<String>>()
        .join(", ");
    format!(
        "{name} = (compute.buffer ([{elements}] : array{{{kind}, {n}}}) : (compute.Buf {kind}))",
        n = ELEMENTS
    )
}

/// The nested kernel launched directly, over the two buffers and the two scalars.
fn nested_direct() -> String {
    format!(
        "{head}{a}\n{b}\nout = (compute.plrun nested ((Arg)(.meta Meta(.rows {rows}, .cols {cols}), \
         .I In2(.a a, .b b))) : Out)\ncompute.collect out.z\n",
        head = nested(INT, CARRYING, "s.cols"),
        a = buffer("a", INT, 1),
        b = buffer("b", INT, 101),
        rows = ROWS,
        cols = COLS,
    )
}

/// The nested kernel recorded, and run over the two buffers and the two scalars.
fn nested_graph() -> String {
    format!(
        "{head}{a}\n{b}\ncompute.collect (compute.graphrun built ({rows}, {cols}, a, b))\n",
        head = nested(INT, CARRYING, "s.cols"),
        a = buffer("a", INT, 1),
        b = buffer("b", INT, 101),
        rows = ROWS,
        cols = COLS,
    )
}

/// The nested head whose every leaf but the extent is an `Int`, and whose step carries
/// them as arguments.
const INT: &str = "Int";
const CARRYING: &str = "struct{.rows Int, .cols Int, .in In2}";

/// What the nested launch answers: `a[i] * COLS + b[i]`.
fn nested_answer() -> Vec<usize> {
    (0..ROWS).map(|at| (at + 1) * COLS + (at + 101)).collect()
}

/// The flat kernel launched directly, over the one buffer and the three scalars.
fn flat_direct() -> String {
    format!(
        "{head}{a}\nout = (compute.plrun shape ((Arg3)(.n {rows}, .K {k}, .cols {cols}, .I In1(.a a))) : \
         Out1)\ncompute.collect out.z\n",
        head = flat(),
        a = buffer("a", INT, 10),
        rows = ROWS,
        k = K,
        cols = SHAPE_COLS,
    )
}

/// The flat kernel recorded, and run over the buffer and the three scalars.
fn flat_graph() -> String {
    format!(
        "{head}{a}\ncompute.collect (compute.graphrun built3 ({rows}, {k}, {cols}, a))\n",
        head = flat(),
        a = buffer("a", INT, 10),
        rows = ROWS,
        k = K,
        cols = SHAPE_COLS,
    )
}

/// What the flat launch answers: `a[2 * (i / 3) + (i % 3)]`, which repeats an element.
fn flat_answer() -> Vec<usize> {
    (0..ROWS)
        .map(|at| 10 + (at / SHAPE_COLS) * K + (at % SHAPE_COLS))
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

/// A nested config's leaves each become what they are, and the extent is the first scalar.
///
/// # Invariant
/// The four argument slots are the four leaves the walk found, **sorted by path**: the two
/// scalars before the two buffers, because `GA` names them in that order. Naming them by
/// role is what shows the extent read as a count, the second leaf read as a launch value,
/// and the two `Buf` fields read as buffers rather than as numbers.
#[test]
fn a_nested_config_records_each_leaf_for_what_it_is() {
    let graph = graph_of(&nested(INT, CARRYING, "s.cols"));
    assert_eq!(
        (graph.len(), graph.input_count()),
        (1, 4),
        "one dispatch over two scalars and two buffers, at four argument slots"
    );
    let Node::Kernel(node) = &graph.nodes()[0];
    assert_eq!(
        node.count,
        lichen_graph_ir::Count::Value(0),
        "the extent is the first scalar leaf, reached at the depth `.meta.rows` sits"
    );
    assert_eq!(
        node.scalars,
        vec![lichen_graph_ir::Scalar::Value(1)],
        "the second scalar leaf is a launch value beside it, not another count"
    );
    assert_eq!(
        node.inputs,
        vec![2, 3],
        "and both buffers under `.in` are edges, one level down and one field across"
    );

    // The same config with a `Float` `.meta.cols` passed as a literal.
    let graph = graph_of(&nested("Float", "struct{.rows Int, .in In2}", "2.5"));
    let Node::Kernel(node) = &graph.nodes()[0];
    assert_eq!(
        node.count,
        lichen_graph_ir::Count::Value(0),
        "the extent is still the first scalar leaf, and a `Float` beside it does not move it"
    );
    assert_eq!(
        node.scalars,
        vec![lichen_graph_ir::Scalar::Constant(
            lichen_kernel_ir::ScalarLeaf {
                class: lichen_kernel_ir::ScalarClass::Float,
                bits: f32::to_bits(2.5) as i64,
            }
        )],
        "a leaf's class is the one its own field declares, at its own ordinal"
    );
    assert_eq!(
        node.inputs,
        vec![1, 2],
        "and the buffers are numbered over the slots that were read, the literal's not"
    );
}

/// The nested launch records and answers the same thing on the device as it does directly.
///
/// # Invariant
/// Both legs are the same program over the same numbers, and the answer is computed here
/// rather than written out, so a launch value that crossed as the wrong number fails in
/// the comparison rather than passing on the device's word.
#[test]
fn a_nested_config_runs_on_the_device_through_the_graph() {
    let test = "a_nested_config_runs_on_the_device_through_the_graph";
    let (stub, _guard) = common::stub();
    let (_module, _value, _root) = common::run(&nested_direct());
    let direct = stub.calls();
    let (_module, _value, _root) = common::run(&nested_graph());
    let through_graph = stub.calls();
    assert_eq!(
        direct.len(),
        1,
        "the direct launch reaches the backend once: {test}"
    );
    assert_eq!(
        through_graph.len(),
        2,
        "and the recorded graph's element crosses once more: {test}"
    );
    assert_eq!(
        through_graph.last().map(|call| &call.fragment),
        direct.first().map(|call| &call.fragment),
        "the same kernel reached the backend on both legs, so both leaves crossed: {test}"
    );
    assert_eq!(
        through_graph.last().map(|call| call.count),
        direct.first().map(|call| call.count),
        "and over the same extent: {test}"
    );
}

/// Two runtime scalars: the operator's shape is a parameter, not a literal.
///
/// # Invariant
/// `K` and `SHAPE_COLS` sit at the same shape and are not equal, so the list order is
/// the only thing that puts the right number under each name.
#[test]
fn a_two_scalar_config_carries_both_and_runs_on_the_device() {
    let graph = graph_of(&flat());
    let Node::Kernel(node) = &graph.nodes()[0];
    assert_eq!(
        node.count,
        lichen_graph_ir::Count::Value(0),
        "`.n` is the extent, still the first scalar leaf"
    );
    assert_eq!(
        node.scalars,
        vec![
            lichen_graph_ir::Scalar::Value(1),
            lichen_graph_ir::Scalar::Value(2)
        ],
        "`.K` and `.cols` are two launch values, each an edge of its own"
    );

    let test = "a_two_scalar_config_carries_both_and_runs_on_the_device";
    let (_stub, _guard) = common::stub();
    let (module, value, _root) = common::run(&flat_direct());
    let direct = common::usize_array(&module, &value);
    let (module, value, _root) = common::run(&flat_graph());
    let through_graph = common::usize_array(&module, &value);
    assert_eq!(
        direct,
        flat_answer(),
        "the direct launch answers the strided strip"
    );
    assert_eq!(
        through_graph, direct,
        "and the recorded graph answers it element for element, so both scalars crossed"
    );
}

/// A shape the graph still cannot carry is refused at the leaf that misses it.
///
/// # Invariant
/// The case names the place it is missing — the field position of a buffer the parameter
/// has no group for — and arrives while the shape is read, not at the run. The late
/// sentence this replaces named a **number** read as a **buffer**. The `Float` case that
/// sat here is now carried; `float_leaf.rs` reads it at its own class.
#[test]
fn a_leaf_the_graph_cannot_carry_is_refused_at_its_own_path() {
    let (lines, _value) = reported_run(
        r#"
---
  compute = import "compute.lichen"
---
In2  = struct{.a (compute.Buf Int)}
Out  = struct{.z (compute.Buf Int)}
Par  = struct{.n Int, .in In2, .out Out, .spare (compute.Buf Int)}
Arg  = struct{.n Int, .in In2, .spare (compute.Buf Int)}
stray_raw = (k : Par) => {
  i = compute.range k.n
  v = (compute.read ((compute.Read _)(.from k.in.a, .at i)) : {kind})
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value (v ~ "element")))
}
stray = compute.parallel (stray_raw ~ "claim:Elementwise(element)") "gpu" Out
built = compute.graph ((s : Arg) => {
  compute.plrun stray ((Arg)(.n s.n, .in In2(.a s.in.a), .spare s.spare)) : Out
})
built
"#,
    );
    let refusal = lines
        .iter()
        .find(|line| line.contains("a buffer outside `.in`/`.out` at position 2"))
        .unwrap_or_else(|| {
            panic!(
                "the refusal names the stray buffer's own field position; the report was:\n  {}",
                lines.join("\n  ")
            )
        });
    assert!(
        !lines.iter().any(|line| line.contains("as a buffer")),
        "and no late refusal names a number read as a buffer:\n  {}",
        lines.join("\n  ")
    );
    println!("a buffer the parameter cannot name is refused by:\n  {refusal}");
}
