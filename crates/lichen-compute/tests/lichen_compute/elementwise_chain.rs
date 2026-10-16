//! An elementwise chain derives a schema, and the floor declines it by name.
//! See docs/notes/graph-fusion.md §4 and §5.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::{clear_graph_pass, consult_graph_pass, graph, take_graph_pass_lines};
use lichen_compute::fusion_claim::{
    Instance, RegionSchema, SchemaHash, claim_of, derive_regions, fragment_of, kernel_id,
};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::region_merge::install_region_merge;
use lichen_graph_ir::Graph;
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;
use lichen_utils::hash::{hex, sha256};

/// The extent the test consults the pass with, standing in for the run's own count.
const COUNT: usize = 8;

/// The chain's first kernel: `e[i] = 2·d[i]`, marked as the function it computes.
///
/// # Invariant
/// The mark rides the **whole** expression — a top-level routed operator anchors to the
/// product and not to its first operand (`docs/notes/graph-fusion.md` §3) — so the region's
/// node 0 records the element function this kernel computes and nothing else. The extent is
/// `k.n`, a run-time value, so the recorded node's count is the graph's own.
const SCALE: &str = r#"
In   = struct{.d (compute.Buf Float)}
OutE = struct{.e (compute.Buf Float)}
ParE = compute.P (compute.KT Float)(.I In, .O OutE)
scale_raw = (k : ParE) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) * 2.0) ~ "element")))
}
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu"
"#;

/// The `[elementwise, reduction]` chain: the elementwise output feeding a `Σ(d − e)` consumer.
///
/// # Invariant
/// The consumer reads the elementwise buffer at input position 1, so the recorded edge and the
/// `element` projection's `r` name the same input, and its `combine` and `identity` are the two
/// roles the reduction's own reading demands.
fn chain() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
{SCALE}
In2  = struct{{.d (compute.Buf Float), .p (compute.Buf Float)}}
OutZ = struct{{.z (compute.Buf Float)}}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutZ)
consumer_raw = (k : Par2) => {{
  i = compute.range k.n
  r = compute.read ((compute.Read _)(.from k.in.p, .at 0))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (((compute.read ((compute.Read _)(.from k.in.d, .at i)) - r) ~ "element")) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, 0.0 ~ "identity")))
}}
consumer = compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.e)))
}}
built = compute.graph step
(scale, consumer, built)
"#
    )
}

/// The `[elementwise, elementwise]` chain: a second elementwise kernel over the first's output.
///
/// # Invariant
/// The second kernel reads only the buffer its producer filled, so its `element` role is a
/// function of the value that arrives — the interface's `r` — and the region records it as
/// the code between the two.
fn elementwise_chain() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
{SCALE}
In2  = struct{{.p (compute.Buf Float)}}
OutF = struct{{.f (compute.Buf Float)}}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutF)
rescale_raw = (k : Par2) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.f, .at i, .value ((compute.read ((compute.Read _)(.from k.in.p, .at i)) * 3.0) ~ "element")))
}}
rescale = compute.parallel (rescale_raw ~ "claim:Elementwise(element)") "gpu"
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun rescale ((compute.A In2)(.n s.n, .I In2(.p first.e)))
}}
built = compute.graph step
(scale, rescale, built)
"#
    )
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

/// The one schema `source`'s recording derives.
fn derived(source: &str) -> (Module<LangProgram>, RegionSchema, Graph) {
    let (module, value, _root) = common::run(source);
    let instances = instances(&module, &value);
    let graph = recorded(&module, &value);
    let regions = derive_regions(&graph, &instances);
    assert_eq!(regions.len(), 1, "the recording is one claimed region");
    let schema = regions
        .into_iter()
        .next()
        .expect("the one region")
        .expect("an elementwise chain derives a schema");
    (module, schema, graph)
}

/// §4's rule, tested rather than asserted: the hash is over the text a report prints.
fn hash_is_of_printed_form(schema: &RegionSchema) -> String {
    let printed = schema.canonical_form();
    println!("the canonical form the hash is taken over:\n{printed}");
    let recomputed = hex(&sha256(printed.as_bytes()));
    println!("the region's hash: {}", schema.hash);
    assert_eq!(
        recomputed,
        schema.hash.hex(),
        "a pack author recomputes the hash from the text the report shows"
    );
    assert_eq!(
        SchemaHash::parse(&schema.hash.to_string()),
        Some(schema.hash.clone()),
        "the hash the report prints is one a reader takes back"
    );
    recomputed
}

/// The pass's own line for `graph`, with the pass installed for `module`.
fn merge_lines(module: &Module<LangProgram>, graph: &Graph) -> Vec<String> {
    clear_graph_pass();
    install_region_merge(module);
    let rewritten = consult_graph_pass(graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    assert!(
        rewritten.is_none(),
        "a region the pass declines changes nothing, so it answers no graph: {lines:?}"
    );
    lines
}

/// The line the pass reports for a region holding an elementwise instance and no installed pack.
///
/// # Invariant
/// The sentence names the **region's** own hash, because that is what a pack's `fuse` must
/// match: §5's rule is that a pack that steers a fusion names the region's schema, so a runner
/// reading this line can write the pack that would change it.
fn declined(schema: &RegionSchema) -> String {
    format!(
        "region 0 (nodes 0, 1): refused: InterfacesAreNotProved: the region's node(s) claim \
         Elementwise, and a merge is licensed by the reduction chain this pass can prove or by a \
         pack whose `fuse` names this region's schema {}: no installed pack asks for this region",
        schema.hash
    )
}

/// An `[elementwise, reduction]` chain derives, prints and hashes, and the floor declines it.
#[test]
fn an_elementwise_chain_derives_a_schema_the_floor_declines_by_name() {
    let (module, schema, graph) = derived(&chain());
    assert_eq!(
        schema
            .nodes
            .iter()
            .map(|node| node.interface.as_str())
            .collect::<Vec<_>>(),
        vec!["Elementwise", "Reduction"],
        "the region holds the elementwise kernel and the reduction it feeds, in evaluation order"
    );
    let text = schema.canonical_form();
    let printed: Vec<&str> = text.lines().collect();
    assert_eq!(
        printed[..7],
        [
            "region 2",
            "node 0 interface Elementwise",
            "role element (* 2.0 d)",
            "node 1 interface Reduction",
            "role combine (+ d (+ t (- r)))",
            "role element (+ d (- r))",
            "role identity 0.0",
        ],
        "the canonical form is the interfaces and their canonical roles, in name order"
    );
    assert!(
        printed.contains(&"between 0 1 input 1 (+ d (- r))"),
        "and the code between the two instances is the consumer's own element: {printed:?}"
    );
    assert!(
        printed
            .iter()
            .skip(7)
            .all(|line| line.starts_with("port ") || line.starts_with("between ")),
        "every remaining line is the between code or a port: {printed:?}"
    );
    hash_is_of_printed_form(&schema);

    assert_eq!(
        schema.nodes[0]
            .elementwise()
            .expect("node 0 claims Elementwise")
            .to_string(),
        "2 * d",
        "the elementwise reading is the node's own element function, in terms of its datum"
    );
    assert_eq!(
        schema.nodes[1]
            .elementwise()
            .expect_err("a reduction is not an elementwise node")
            .name(),
        "UnknownInterface",
        "and the wrong interface is refused by name"
    );
    assert_eq!(
        schema.between.len(),
        1,
        "the one edge carries the consumer's element, which reads the producer's value"
    );

    assert_eq!(
        merge_lines(&module, &graph),
        vec![declined(&schema)],
        "the floor declines the region by name: whether these operations become one kernel is a \
         pack's decision"
    );
}

/// An `[elementwise, elementwise]` chain derives too, and the floor declines that one as well.
#[test]
fn two_elementwise_kernels_derive_a_region_and_the_floor_declines_it_by_name() {
    let (module, schema, graph) = derived(&elementwise_chain());
    assert_eq!(
        schema.nodes[1]
            .projection("element")
            .map(ToString::to_string)
            .as_deref(),
        Some("r * 3"),
        "the second kernel's element is read as a function of the value that arrives"
    );
    assert!(
        schema
            .canonical_form()
            .contains("between 0 1 input 0 (* 3.0 r)"),
        "and the region records it as the code between the two instances:\n{}",
        schema.canonical_form()
    );
    hash_is_of_printed_form(&schema);
    assert_eq!(
        merge_lines(&module, &graph),
        vec![declined(&schema)],
        "the floor declines an elementwise region whatever the second node claims"
    );
}
