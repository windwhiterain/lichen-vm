//! The RMSNorm pair under the floor, per denominator. See docs/notes/llm-operators.md.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::{clear_graph_pass, consult_graph_pass, graph, take_graph_pass_lines};
use lichen_compute::fusion_claim::{Instance, claim_of, derive_regions, fragment_of, kernel_id};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::region_merge::install_region_merge;
use lichen_graph_ir::Graph;
use lichen_kernel_ir::{KernelFragment, KernelInstr, ValueDef};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The extent the recording is run over, as [`consult_graph_pass`] is handed.
const COUNT: usize = 8;

/// The producer's value, read as RMSNorm's consumer reads it.
const SUM: &str = "(compute.read ((compute.Read _)(.from k.in.s, .at 0)) : Float)";

/// The RMSNorm pair with `denominator` written into the consumer's `element`.
///
/// # Invariant
/// The two kernels, their claims and the edge between them are the operator's own
/// (`examples/rmsnorm.lichen`); only the denominator varies, so every refusal this measures is
/// about the denominator and about nothing else.
fn program(denominator: &str) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
Row  = struct{{.x (compute.Buf Float)}}
Sums = struct{{.s (compute.Buf Float)}}
ParS = compute.P (compute.KT Float)(.I Row, .O Sums)
sumsq_raw = (k : ParS) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.s, .at i, .value fold (k.n, 0.0 ~ "identity")))
}}
sumsq = compute.parallel (sumsq_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Sums
Scale = struct{{.x (compute.Buf Float), .w (compute.Buf Float), .s (compute.Buf Float)}}
OutY  = struct{{.y (compute.Buf Float)}}
ParY  = compute.P (compute.KT Float)(.I Scale, .O OutY)
rescale_raw = (k : ParY) => {{
  i = compute.range k.n
  x = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  w = (compute.read ((compute.Read _)(.from k.in.w, .at i)) : Float)
  d = {denominator}
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((x / d) * w) ~ "element"))
}}
rescale = compute.parallel (rescale_raw ~ "claim:Reduction(element)") "gpu" OutY
GArg = struct{{.n Int, .in All}}
All  = struct{{.x (compute.Buf Float), .w (compute.Buf Float)}}
step = (s : GArg) => {{
  first = (compute.plrun sumsq ((compute.A Row)(.n s.n, .I Row(.x s.in.x))) : Sums)
  compute.plrun rescale ((compute.A Scale)(.n s.n, .I Scale(.x s.in.x, .w s.in.w, .s first.s)))
}}
built = compute.graph step
(sumsq, rescale, built)
"#
    )
}

/// The root tuple's items.
///
/// # Invariant
/// The module's root node is not read back, so a program whose root is a value answers its
/// items here: the two kernels and the graph they recorded, in that order.
fn root_items(value: &LangValue) -> &[ArrayItem] {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("the program's root is a tuple");
    };
    // SAFETY: `array` is the payload of the root value just produced, whose block is alive.
    unsafe { array.items() }
}

/// The two claimed kernels the recording's root tuple holds.
fn instances(module: &Module<LangProgram>, value: &LangValue) -> Vec<Instance> {
    root_items(value)
        .iter()
        .take(2)
        .map(|item| {
            let AnyNodeId::Dynamic(node) = item.node else {
                panic!("a kernel comes from a frozen artifact");
            };
            let kernel = kernel_id(module, node).expect("the item names a kernel");
            Instance {
                kernel,
                claim: claim_of(kernel).expect("the kernel claims an interface"),
                fragment: fragment_of(kernel).expect("the registry holds the dispatched fragment"),
            }
        })
        .collect()
}

/// The graph the recording's third root element names.
fn recorded(module: &Module<LangProgram>, value: &LangValue) -> Graph {
    let item = root_items(value).get(2).expect("the root holds a graph");
    let AnyNodeId::Dynamic(node) = item.node else {
        panic!("the graph comes from a frozen artifact");
    };
    let value = module
        .element_value_node(AnyNodeId::Dynamic(node))
        .or(Some(AnyNodeId::Dynamic(node)))
        .and_then(|node| module.node_value(node))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value));
    let Some(ComputeValue::Graph(id, _)) = value else {
        panic!("the program's third root element is not a graph");
    };
    graph::lookup(id).expect("the recorded graph is registered")
}

/// The class conversions `fragment` holds, as the count and the source of each.
fn conversions(fragment: &KernelFragment) -> Vec<String> {
    fragment
        .body
        .values
        .iter()
        .filter(|definition| {
            matches!(
                definition,
                ValueDef::Instr {
                    op: KernelInstr::Conv { .. },
                    ..
                }
            )
        })
        .map(|definition| format!("{definition:?}"))
        .collect()
}

/// The pass's own line for one denominator, and whether the region merged.
fn verdict(denominator: &str) -> (String, bool) {
    let build = common::check(&program(denominator))
        .build
        .expect("the program checks");
    let module = build.module;
    let value = module
        .node_value(AnyNodeId::Dynamic(build.root_val))
        .expect("the root is decided");
    let found = instances(&module, &value);
    for (name, instance) in ["producer", "consumer"].into_iter().zip(&found) {
        println!(
            "the {name} holds {} conversion(s): {:?}",
            conversions(&instance.fragment).len(),
            conversions(&instance.fragment)
        );
    }
    let graph = recorded(&module, &value);
    for region in derive_regions(&graph, &found) {
        match region {
            Ok(schema) => println!("the schema's canonical form:\n{}", schema.canonical_form()),
            Err(refusal) => println!("the schema refused: {}: {refusal}", refusal.name()),
        }
    }
    clear_graph_pass();
    install_region_merge(&module);
    let merged = consult_graph_pass(&graph, &|_node| Some(COUNT)).is_some();
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    let line = lines.first().cloned().unwrap_or_default();
    println!("merged: {merged}\n{line}");
    (line, merged)
}

/// A fixed size and a **runtime extent** both fuse, because both name `the invocation`.
///
/// # Invariant
/// **Two facts.** A `sqrt` the lowering wrote out as its Newton iteration is read as the
/// operator, so a denominator the walk can name bridges and the floor's licence merges the
/// pair. The `int2float` reads `k.n`, which is the ABI's **first** leaf — the extent, not the
/// invocation's index, which is the **last** — so it is a value the whole launch fixes.
#[test]
fn a_fixed_size_and_a_runtime_extent_both_fuse() {
    let (line, merged) = verdict(&format!("sqrt ({SUM} / int2float 4 + 0.25)"));
    assert!(
        merged,
        "the pair fuses once the denominator bridges: {line}"
    );
    assert!(
        line.contains("licensed by the floor:"),
        "with no pack installed, the floor's own proof is the licence: {line}"
    );

    for denominator in [
        format!("{SUM} / int2float k.n"),
        format!("sqrt ({SUM} / int2float k.n + 0.25)"),
    ] {
        let (line, merged) = verdict(&denominator);
        assert!(
            merged,
            "the launch's extent is `the invocation`, so a denominator reading it fuses: {line}"
        );
        assert!(
            !line.contains("refused:"),
            "and nothing refuses the region: {line}"
        );
    }
}
