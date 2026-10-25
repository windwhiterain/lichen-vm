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
/// The mark rides the **whole** expression — a routed operator anchors to the product, not
/// its first operand (`docs/notes/graph-fusion.md` §3) — so node 0 records the element
/// function and nothing else. The extent is `k.n`, a runtime value. `ParE` carries no
/// `.out` group (`docs/notes/kernel-buffer-declarations.md`).
fn scale() -> String {
    format!(
        r#"
In   = struct{{.d (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
OutE = struct{{.e (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
ParE = struct{{.n Int, .in In}}
scale_raw = (k : ParE) => {{
  i = compute.range k.n
  e = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutE)(.e (compute.write ((compute.Write _)(.to e, .at i, .value (((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) * 2.0) ~ "element"))))))
}}
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu" OutE
"#
    )
}

/// The `[elementwise, reduction]` chain: the elementwise output feeding a `Σ(d − e)` consumer.
///
/// # Invariant
/// The consumer reads the elementwise buffer at input position 1, so the recorded edge and the
/// `element` projection's `r` name the same input, and its `combine` and `identity` are the two
/// roles the reduction's own reading demands.
fn chain() -> String {
    let scale = scale();
    format!(
        r#"
---
  compute = import "compute.lichen"
---
{scale}
In2  = struct{{.d (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT}))), .p (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
OutZ = struct{{.z (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
Par2 = struct{{.n Int, .in In2}}
consumer_raw = (k : Par2) => {{
  i = compute.range k.n
  r = (compute.read ((compute.Read _)(.from k.in.p, .at 0)) : Float)
  z = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) - r) ~ "element")) ~ "combine"))
  ((OutZ)(.z (compute.write ((compute.Write _)(.to z, .at i, .value fold (k.n, 0.0 ~ "identity"))))))
}}
consumer = compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "gpu" OutZ
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
    let scale = scale();
    format!(
        r#"
---
  compute = import "compute.lichen"
---
{scale}
In2  = struct{{.p (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
OutF = struct{{.f (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
Par2 = struct{{.n Int, .in In2}}
rescale_raw = (k : Par2) => {{
  i = compute.range k.n
  f = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutF)(.f (compute.write ((compute.Write _)(.to f, .at i, .value (((compute.read ((compute.Read _)(.from k.in.p, .at i)) : Float) * 3.0) ~ "element"))))))
}}
rescale = compute.parallel (rescale_raw ~ "claim:Elementwise(element)") "gpu" OutF
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
    // A compound's element is a slot, so the value is the one its slot carries
    // (`docs/notes/compound-element-slots.md`).
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

/// The one schema `source`'s recording derives.
fn derived(source: &str) -> (Module<LangProgram>, RegionSchema, Graph) {
    let build = common::check(source).build.expect("the program checks");
    let module = build.module;
    let value = module
        .node_value(AnyNodeId::Dynamic(build.root_val))
        .expect("the root is decided");
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
        rewritten.is_some(),
        "the floor proves the region, so the pass answers a graph: {lines:?}"
    );
    lines
}

/// The line the pass reports for a region holding an elementwise instance and no installed pack.
///
/// # Invariant
/// The sentence names the **region's** own hash, because that is what a pack's `fuse` must
/// match: §5's rule is that a pack that steers a fusion names the region's schema, so a runner
/// reading this line can write the pack that would change it.
fn merged(absorbed: usize, interfaces: &str, digest: &str) -> String {
    format!(
        "region 0: nodes 0 and 1 merged into one node at 1 (the survivor), absorbing input \
         position {absorbed} of node 1, licensed by the floor: every node of the region claims \
         {interfaces} and nothing else; the merged fragment is registered with the conservative \
         claim `Opaque`: a pass reading the registry names it and refuses it by name, and no \
         reader reads algebra from it; the merged node's identity is digest:{digest}"
    )
}

/// An `[elementwise, reduction]` chain derives, prints and hashes, and the floor declines it.
#[test]
fn an_elementwise_chain_derives_a_schema_the_floor_proves() {
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
        vec![merged(1, "Elementwise or Reduction", "d96ece43a96974d1")],
        "the floor proves the region: the elementwise kernel and the reduction it feeds are one          kernel the pass's own proof holds"
    );
}

/// An `[elementwise, elementwise]` chain derives too, and the floor declines that one as well.
#[test]
fn two_elementwise_kernels_derive_a_region_the_floor_proves() {
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
        vec![merged(0, "Elementwise", "7634538532464ab7")],
        "the floor proves the chain: two elementwise kernels are one kernel the pass can build"
    );
}
