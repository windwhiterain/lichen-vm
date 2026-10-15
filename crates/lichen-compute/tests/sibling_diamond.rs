//! A sibling diamond: two independent producers one consumer reads both.
//! See docs/notes/llm-operators.md.

mod common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::{clear_graph_pass, consult_graph_pass, graph, take_graph_pass_lines};
use lichen_compute::fusion_claim::{
    Instance, RegionSchema, claim_of, derive_regions, fragment_of, kernel_id, region_positions,
};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::region_merge::install_region_merge;
use lichen_graph_ir::{Graph, Node};
use lichen_kernel_ir::{Producer, compose_fan_in};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The elements the diamond runs over, as [`consult_graph_pass`] is handed.
const COUNT: usize = 8;

/// The normalise's input positions the two producers reach, in input-ordinal order.
const SUM: usize = 1;
const SUMSQ: usize = 2;

/// The root tuple's items, read from the value the program answered.
///
/// # Invariant
/// The module's root node is not read back, so a program whose root is a value answers its items
/// here: the three kernels and the graph they recorded, in that order.
fn root_items(value: &LangValue) -> &[ArrayItem] {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("the program's root is a tuple");
    };
    // SAFETY: `array` is the payload of the root value just produced, whose block is alive.
    unsafe { array.items() }
}

/// The three claimed kernels the recording's root tuple holds, in region order.
fn instances(module: &Module<LangProgram>, value: &LangValue) -> Vec<Instance> {
    root_items(value)
        .iter()
        .take(3)
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

/// The graph the recording's fourth root element names.
fn recorded(module: &Module<LangProgram>, value: &LangValue) -> Graph {
    let item = root_items(value).get(3).expect("the root holds a graph");
    let AnyNodeId::Dynamic(node) = item.node else {
        panic!("the graph comes from a frozen artifact");
    };
    let value = module
        .node_value(AnyNodeId::Dynamic(node))
        .or_else(|| module.node_value(AnyNodeId::Dynamic(node)))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value));
    let Some(ComputeValue::Graph(id, _)) = value else {
        panic!("the program's last root element is not a graph");
    };
    graph::lookup(id).expect("the recorded graph is registered")
}

/// The module, the recording and the one schema its three kernels derive, or the refusal.
///
/// # Invariant
/// **One region and one schema**: the diamond is the whole recording, so a grouping that reported
/// anything else would be a grouping that saw a region the program does not have.
fn derived(
    source: &str,
) -> (
    Module<LangProgram>,
    Vec<Instance>,
    Result<RegionSchema, String>,
    Graph,
) {
    let (module, value, _root) = common::run(source);
    let instances = instances(&module, &value);
    let graph = recorded(&module, &value);
    assert_eq!(region_positions(&graph, &instances), vec![vec![0, 1, 2]]);
    let mut regions = derive_regions(&graph, &instances);
    assert_eq!(regions.len(), 1, "the diamond is one region");
    let schema = regions.pop().expect("one region");
    let schema = schema.map_err(|refusal| format!("{}: {}", refusal.name(), refusal.message()));
    (module, instances, schema, graph)
}

/// `Σx` and `Σx²` as two independent producers and one normalise reading both.
///
/// # Invariant
/// **The one-pass LayerNorm shape of `examples/layernorm.lichen`**, which records rather than
/// runs: the second producer reads no value the first produced, so the two are siblings and only
/// the normalise joins them.
fn layernorm() -> String {
    r#"
---
  compute = import "compute.lichen"
---
Row   = struct{.x (compute.Buf Float)}
Sums  = struct{.s (compute.Buf Float)}
ParS  = compute.P (compute.KT Float)(.I Row, .O Sums)
row_sum_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.s, .at i, .value (fold (k.n, 0.0 ~ "identity"))))
}
row_sum = compute.parallel (row_sum_raw ~ "claim:Reduction(combine,identity)") "cpu"
Sumsq = struct{.q (compute.Buf Float)}
ParQ  = compute.P (compute.KT Float)(.I Row, .O Sumsq)
row_sumsq_raw = (k : ParQ) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) * compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.q, .at i, .value (fold (k.n, 0.0 ~ "identity"))))
}
row_sumsq = compute.parallel (row_sumsq_raw ~ "claim:Reduction(combine,identity,element)") "cpu"
Moments = struct{.x (compute.Buf Float), .w (compute.Buf Float), .s (compute.Buf Float), .q (compute.Buf Float)}
OutY    = struct{.y (compute.Buf Float)}
ParY    = compute.P (compute.KT Float)(.I Moments, .O OutY)
normalise_raw = (k : ParY) => {
  i = compute.range k.n
  count = int2float k.n
  m = compute.read ((compute.Read _)(.from k.in.s, .at 0)) / count
  v = compute.read ((compute.Read _)(.from k.in.q, .at 0)) / count - m * m
  d = sqrt (v + 1.0)
  x = compute.read ((compute.Read _)(.from k.in.x, .at i))
  w = compute.read ((compute.Read _)(.from k.in.w, .at i))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (((x - m) / d * w) ~ "element")))
}
normalise = compute.parallel (normalise_raw ~ "claim:Reduction(element)") "cpu"
All   = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
GArg  = struct{.n Int, .in All}
step = (s : GArg) => {
  a = (compute.plrun row_sum ((compute.A Row)(.n s.n, .I Row(.x s.in.x))) : Sums)
  b = (compute.plrun row_sumsq ((compute.A Row)(.n s.n, .I Row(.x s.in.x))) : Sumsq)
  compute.plrun normalise ((compute.A Moments)(.n s.n, .I Moments(.x s.in.x, .w s.in.w, .s a.s, .q b.q))) : OutY
}
built = compute.graph step
(row_sum, row_sumsq, normalise, built)
"#
    .to_string()
}

/// The same diamond with a normalise that divides by the sum of squares alone.
///
/// # Invariant
/// **Only the normalise's element changes**: the two producers, their claims and the two arriving
/// edges are LayerNorm's own, and the element reaches neither `k.n` nor a division by it — which
/// is the whole difference between this normalise and the operator's.
fn minimal() -> String {
    r#"
---
  compute = import "compute.lichen"
---
Row   = struct{.x (compute.Buf Float)}
Sums  = struct{.s (compute.Buf Float)}
ParS  = compute.P (compute.KT Float)(.I Row, .O Sums)
row_sum_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.s, .at i, .value (fold (k.n, 0.0 ~ "identity"))))
}
row_sum = compute.parallel (row_sum_raw ~ "claim:Reduction(combine,identity)") "cpu"
Sumsq = struct{.q (compute.Buf Float)}
ParQ  = compute.P (compute.KT Float)(.I Row, .O Sumsq)
row_sumsq_raw = (k : ParQ) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) * compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.q, .at i, .value (fold (k.n, 0.0 ~ "identity"))))
}
row_sumsq = compute.parallel (row_sumsq_raw ~ "claim:Reduction(combine,identity,element)") "cpu"
Moments = struct{.x (compute.Buf Float), .s (compute.Buf Float), .q (compute.Buf Float)}
OutY    = struct{.y (compute.Buf Float)}
ParY    = compute.P (compute.KT Float)(.I Moments, .O OutY)
normalise_raw = (k : ParY) => {
  i = compute.range k.n
  s = compute.read ((compute.Read _)(.from k.in.s, .at 0))
  q = compute.read ((compute.Read _)(.from k.in.q, .at 0))
  x = compute.read ((compute.Read _)(.from k.in.x, .at i))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (((x - s) / q) ~ "element")))
}
normalise = compute.parallel (normalise_raw ~ "claim:Reduction(element)") "cpu"
GArg = struct{.n Int, .in Row}
step = (s : GArg) => {
  a = (compute.plrun row_sum ((compute.A Row)(.n s.n, .I Row(.x s.in.x))) : Sums)
  b = (compute.plrun row_sumsq ((compute.A Row)(.n s.n, .I Row(.x s.in.x))) : Sumsq)
  compute.plrun normalise ((compute.A Moments)(.n s.n, .I Moments(.x s.in.x, .s a.s, .q b.q))) : OutY
}
built = compute.graph step
(row_sum, row_sumsq, normalise, built)
"#
    .to_string()
}

/// The same diamond whose normalise names only the **first** of the two producers.
fn unnamed() -> String {
    minimal().replace(
        "(((x - s) / q) ~ \"element\")",
        "(((x - s) / s) ~ \"element\")",
    )
}

/// A **position-dependent** element reads the invocation's index, and the region merges.
///
/// # Invariant
/// **What the licence widened bought.** `compute.range` is the domain's *last* leaf, so
/// `int2float i` inside the marked element names `i` at a slot of its own — a relative-position
/// term, a mask — while each `Between` still names its own producer slot and the fan-in merges
/// under the floor's licence. Nothing here reads a buffer, so nothing new is carried.
#[test]
fn an_element_reading_the_invocation_index_is_one_region_and_merges() {
    let positional = layernorm().replace(
        "(((x - m) / d * w) ~ \"element\")",
        "(((x - m) / d * w + int2float i) ~ \"element\")",
    );
    let (module, _instances, schema, graph) = derived(&positional);
    let schema =
        schema.expect("the element reads the invocation's index, which is `the invocation`");
    let variables: Vec<String> = schema.nodes[2]
        .projection("element")
        .expect("the normalise declares an element")
        .variables()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        variables.contains(&"i".to_string()) && variables.contains(&"r".to_string()),
        "the element names the invocation's index **and** the producer it arrived from: {variables:?}"
    );

    clear_graph_pass();
    install_region_merge(&module);
    let rewritten = consult_graph_pass(&graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    let rewritten = rewritten.expect("the floor's own proof licenses a proved fan-in");
    assert_eq!(rewritten.len(), 1, "the diamond becomes one node");
    assert_eq!(
        lines.len(),
        1,
        "the pass's own line is the merge, with no refusal beside it: {lines:?}"
    );
    assert!(
        lines[0].contains("nodes 0, 1, 2 merged into one node at 2"),
        "and it names the three nodes it merged: {lines:?}"
    );
}

/// LayerNorm's own diamond reads the launch's **extent**, and the region still merges.
///
/// # Invariant
/// **The extent is not the lane index.** The element reads `int2float k.n`, and `k.n` is the
/// ABI's *first* leaf while `compute.range` is the domain's *last* — so the leaf the refusal
/// named is the extent. The element names `n` beside both producers, each `Between` still names
/// its own slot, and the region merges under the floor's licence.
#[test]
fn a_sibling_diamond_reading_the_launch_extent_is_one_region_and_merges() {
    let (module, _instances, schema, graph) = derived(&layernorm());
    let schema = schema.expect("the element reads the launch's extent, which is `the invocation`");
    let variables: Vec<String> = schema.nodes[2]
        .projection("element")
        .expect("the normalise declares an element")
        .variables()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        variables.contains(&"n".to_string()),
        "the element names the launch's extent at a slot of its own: {variables:?}"
    );
    assert!(
        !variables.contains(&"i".to_string()),
        "and it does not name the invocation's index, because `k.n` is the extent and not the \
         index: {variables:?}"
    );

    clear_graph_pass();
    install_region_merge(&module);
    let rewritten = consult_graph_pass(&graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    let rewritten = rewritten.expect("the floor's own proof licenses a proved fan-in");
    assert_eq!(rewritten.len(), 1, "the diamond becomes one node");
    assert_eq!(
        lines,
        vec![format!(
            "region 0: nodes 0, 1, 2 merged into one node at 2 (the survivor), absorbing input \
             position 2 of node 2 and input position 3 of node 2, licensed by the floor: every \
             node of the region claims Reduction and nothing else; the merged fragment is \
             unregistered, so it carries no claim and a later digest-keyed pass cannot name it"
        )],
        "the pass's own line: one region, three nodes, both arrivals absorbed. Positions `2` and \
         `3` are `.s` and `.q` here, not the `minimal()` pair's `{SUM}` and `{SUMSQ}`, because \
         this normalise also declares `.w`"
    );
}

/// A sibling diamond merges into one node under the floor's own licence.
///
/// # Invariant
/// **One `Between` per arriving edge, each over a slot of its own**, and the surviving node holds
/// **exactly** `compose_fan_in` over the region's own three bodies — so the diamond is the pair's
/// own operation with two independent producers and not a second mechanism's answer.
#[test]
fn a_sibling_diamond_merges_into_one_node_under_the_floor() {
    let (module, instances, schema, graph) = derived(&minimal());
    let schema = schema.expect("the element names neither producer's extent");
    assert_eq!(
        schema
            .between
            .iter()
            .map(|between| (between.producer, between.consumer, between.input))
            .collect::<Vec<_>>(),
        vec![(0, 2, SUM), (1, 2, SUMSQ)],
        "the sum feeds the normalise at `.s` and the sum of squares at `.q`: one `Between` per \
         arriving edge, in input-ordinal order"
    );
    let variables: Vec<String> = schema.nodes[2]
        .projection("element")
        .expect("the normalise declares an element")
        .variables()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        variables.contains(&"r".to_string()) && variables.contains(&"r1".to_string()),
        "the normalise's own element names **both** producers, each at its own slot: {variables:?}"
    );
    assert_eq!(
        schema.off_chain_node(),
        None,
        "two siblings are not a fan-out: no producer is left twice, which is the shape the floor \
         admits"
    );

    clear_graph_pass();
    install_region_merge(&module);
    let rewritten = consult_graph_pass(&graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    let rewritten = rewritten.expect("the floor's own proof licenses a proved fan-in");
    assert_eq!(rewritten.len(), 1, "the diamond becomes one node");
    assert_eq!(
        lines,
        vec![format!(
            "region 0: nodes 0, 1, 2 merged into one node at 2 (the survivor), absorbing input \
             position {SUM} of node 2 and input position {SUMSQ} of node 2, licensed by the floor: \
             every node of the region claims Reduction and nothing else; the merged fragment is \
             unregistered, so it carries no claim and a later digest-keyed pass cannot name it"
        )],
        "the line names every node and both absorbed positions of the one consumer"
    );
    let Node::Kernel(node) = &rewritten.nodes()[0];
    assert_eq!(
        (node.fragment.inputs, node.fragment.outputs),
        (3, 1),
        "the union is the row buffer three times over — both absorbed inputs dropped — and the one \
         output is the survivor's"
    );
    assert_eq!(
        node.fragment,
        compose_fan_in(&[
            Producer {
                fragment: &instances[0].fragment,
                absorbs: Vec::new(),
            },
            Producer {
                fragment: &instances[1].fragment,
                absorbs: Vec::new(),
            },
            Producer {
                fragment: &instances[2].fragment,
                absorbs: vec![(0, SUM), (1, SUMSQ)],
            },
        ])
        .expect("the recorded sibling diamond composes"),
        "the surviving node holds `compose_fan_in` of the region's own bodies, element for element"
    );
}

/// The same diamond whose normalise names one producer of the two refuses by name.
///
/// # Invariant
/// **Nothing about the shape changes** — the same three bodies, the same two absorbed inputs — so
/// the refusal is the derivation's answer to an element that does not describe the second input,
/// and not the shape's answer to a fan-in.
#[test]
fn a_sibling_diamond_whose_element_names_one_producer_is_refused_by_name() {
    let (module, _instances, schema, graph) = derived(&unnamed());
    let refused = schema.expect_err("the element names one producer of the two");
    assert!(
        refused.starts_with("ElementWithoutProducer"),
        "the second arrival is refused because the consumer's element never names it: {refused}"
    );
    clear_graph_pass();
    install_region_merge(&module);
    let rewritten = consult_graph_pass(&graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    assert!(
        rewritten.is_none(),
        "a region whose element names one producer is not merged weakly: {lines:?}"
    );
}
