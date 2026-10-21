//! What the tiling pass reports about a node no row of its join holds.
//! See docs/notes/graph-fusion.md §5.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::automatic_tiling::install_tiling;
use lichen_compute::compute::{clear_graph_pass, consult_graph_pass, graph, take_graph_pass_lines};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::region_merge::install_region_merge;
use super::common::LANES;
use lichen_graph_ir::{Graph, Node};
use lichen_kernel_ir::fragment_digest;
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// One workgroup covers the launch, so the tile divides the extent exactly.
const COUNT: usize = LANES as usize;

/// A `Σ` producer feeding a `max(d − r)` consumer, recorded and never dispatched.
///
/// # Invariant
/// The consumer reads the producer's output buffer at input position 1, which is what the region
/// derivation reads as an edge, and the recording is the root's third element: a test reads the
/// graph and the two claimed kernels without a device.
fn chain() -> String {
    r#"
---
  compute = import "compute.lichen"
---
In   = struct{.d (compute.Buf Float)}
OutR = struct{.r (compute.Buf Float)}
Par  = compute.P (compute.KT Float)(.I In, .O OutR)
producer_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (k.n, 0.0 ~ "identity")))
}
producer = compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "gpu" OutR
In2  = struct{.d (compute.Buf Float), .p (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutZ)
consumer_raw = (k : Par2) => {
  i = compute.range k.n
  r = (compute.read ((compute.Read _)(.from k.in.p, .at 0)) : Float)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((((compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)) - r) ~ "element")) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, (0.0 - 1000000.0) ~ "identity")))
}
consumer = compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "gpu" OutZ
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  first = (compute.plrun producer ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutR)
  compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.r)))
}
built = compute.graph step
(producer, consumer, built)
"#
    .to_string()
}

/// The graph the recording's third root element names.
///
/// # Invariant
/// A read of what the program recorded and not of a dispatched graph: the pass is consulted on
/// the recording, which is the graph a run hands it.
fn recorded(module: &Module<LangProgram>, value: &LangValue) -> Graph {
    let item = root_items(value)
        .get(2)
        .expect("the program's root holds a graph");
    let AnyNodeId::Dynamic(node) = item.node else {
        panic!("the graph comes from a frozen artifact");
    };
    let value = module
        .node_value(AnyNodeId::Dynamic(node))
        .or_else(|| module.node_value(AnyNodeId::Dynamic(node)))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value));
    let Some(ComputeValue::Graph(id, _)) = value else {
        panic!("the program's third root element is not a graph");
    };
    graph::lookup(id).expect("the recorded graph is registered")
}

/// The root tuple's items, read from the value the program answered.
///
/// # Invariant
/// The module's root node is not read back: a program whose root is a value has one node for its
/// value and its type, and `common::run` evaluates the type after the value.
fn root_items(value: &LangValue) -> &[ArrayItem] {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("the program's root is a tuple");
    };
    // SAFETY: `array` is the payload of the root value just produced, whose block is alive.
    unsafe { array.items() }
}

/// The fragment digest of a graph node.
fn digest_of(graph: &Graph, node: usize) -> u64 {
    let Node::Kernel(kernel) = &graph.nodes()[node];
    fragment_digest(&kernel.fragment)
}

/// A merged node is reported by name, and the merge left it exactly as it was.
///
/// # Invariant
/// The graph under test is the merge pass's own answer, so the node really holds the composed
/// fragment and not one of the two the registry holds: the merge's report says the fragment is
/// unregistered, and the tiling pass's line is the only other word about that node.
#[test]
fn a_composed_node_is_reported_by_name_and_nothing_lands_in_it() {
    clear_graph_pass();
    println!("device: none, the recording and the two consultations are host work");
    let build = common::check(&chain())
    .build
    .expect("the program checks");
let module = build.module;
let value = module
    .node_value(AnyNodeId::Dynamic(build.root_val))
    .expect("the root is decided");
    let recording = recorded(&module, &value);
    assert_eq!(
        recording.len(),
        2,
        "the recording is two nodes: a producer and the consumer that reads it"
    );

    // The merge, whose answer is the only composed fragment a program of this shape reaches.
    install_region_merge(&module);
    let composed = consult_graph_pass(&recording, &|_node| Some(COUNT))
        .expect("the region's chain merges under the floor");
    let merged_report = take_graph_pass_lines();
    clear_graph_pass();
    assert_eq!(composed.len(), 1, "the region became one node");
    assert!(
        merged_report
            .iter()
            .any(|line| line.contains("the merged fragment is unregistered")),
        "the merge's own report says the fragment it left behind is unregistered, which is what \
         makes it a miss for a later join: {merged_report:?}"
    );

    install_tiling(&module);
    let rewritten = consult_graph_pass(&composed, &|_node| Some(COUNT))
        .expect("the installed pass answers a graph even when it tiles nothing");
    let lines = take_graph_pass_lines();
    clear_graph_pass();

    assert_eq!(
        lines,
        vec![
            "node 0: refused: no claim is registered for the fragment at 0, so it is in no row of \
             the digest join and no algebra can be derived for it; a composed fragment is exactly \
             this, because composition registers neither body it joins"
                .to_string()
        ],
        "the composed node is named, and the reason is the join's own miss rather than a \
         derivation that never ran"
    );
    assert_eq!(
        (
            rewritten.len(),
            rewritten.value_count(),
            rewritten.returns()
        ),
        (composed.len(), composed.value_count(), composed.returns()),
        "the node count, the value count and the return are the merge's own, so nothing about the \
         graph moved"
    );
    assert_eq!(
        digest_of(&rewritten, 0),
        digest_of(&composed, 0),
        "the node still holds the composed fragment, so the pass derived nothing for it"
    );
}

/// An ordinary graph is reported once per node, so the contract costs one line and no more.
///
/// # Invariant
/// Both nodes are rows of the join — they are the two kernels the module's own values name — so
/// every line here is one the pass owed before this change, and the count is what says the new
/// lines are not noise on a graph that never misses.
#[test]
fn an_ordinary_graph_reports_one_line_per_node() {
    clear_graph_pass();
    println!("device: none, the recording and the two consultations are host work");
    let build = common::check(&chain())
    .build
    .expect("the program checks");
let module = build.module;
let value = module
    .node_value(AnyNodeId::Dynamic(build.root_val))
    .expect("the root is decided");
    let recording = recorded(&module, &value);
    let nodes = recording.nodes().len();
    assert_eq!(
        nodes, 2,
        "the recording is two nodes and both are the program's own claimed kernels"
    );

    install_tiling(&module);
    let rewritten = consult_graph_pass(&recording, &|_node| Some(COUNT))
        .expect("the installed pass answers a graph");
    let lines = take_graph_pass_lines();
    clear_graph_pass();

    assert_eq!(
        lines.len(),
        nodes,
        "one line per node the pass looked at, and it looked at every node: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("node 0:") && line.contains("tiled")),
        "the producer's own marks license a tiled fold, so its line says so: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("node 1: refused:")),
        "and the consumer's `element` role is refused by the claim shape, so its line says that: \
         {lines:?}"
    );
    assert_ne!(
        digest_of(&rewritten, 0),
        digest_of(&recording, 0),
        "the producer was tiled, so the count above is one line per node and not one per rewrite"
    );
    assert_eq!(
        digest_of(&rewritten, 1),
        digest_of(&recording, 1),
        "the consumer was refused, so its node holds what the program wrote"
    );
}
