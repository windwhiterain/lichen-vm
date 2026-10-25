//! A fan-in — a consumer reading two producers — merges under the floor's own licence.
//! See docs/notes/graph-fusion.md §5.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::{clear_graph_pass, consult_graph_pass, graph, take_graph_pass_lines};
use lichen_compute::fusion_claim::{
    Instance, RegionSchema, claim_of, derive_regions, fragment_of, kernel_id,
};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::region_merge::install_region_merge;
use lichen_graph_ir::Graph;
use lichen_kernel_ir::{Producer, compose_fan_in};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The elements the diamond runs over: a small count, because every lane folds the whole extent.
const COUNT: usize = 8;

/// The three kernels as the recording hands them back, in region order.
const KERNELS: usize = 3;

/// The two producers of the normalise and the input each reaches.
const MAXIMUM: usize = 1;
const SUM: usize = 2;

/// The textbook three-kernel softmax: the row maximum, the shifted sum, and the normalise.
///
/// # Invariant
/// **A diamond, and irreducibly one**: the normalise reads the maximum at `.m` **and** the sum at
/// `.s`, and the maximum is also read by the sum, so a node has two edges leave and two arrive.
/// The normalise claims `Reduction(element)` alone, and its `element` names **both** producers.
/// Each body declares its output with `compute.buf`
/// (`docs/notes/kernel-buffer-declarations.md`).
fn program() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   = struct{{.d (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
OutM = struct{{.m (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
ParM = struct{{.n Int, .in In}}
rowmax_raw = (k : ParM) => {{
  i = compute.range k.n
  m = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)) ~ "combine"))
  ((OutM)(.m (compute.write ((compute.Write _)(.to m, .at i, .value fold (k.n, ((0.0 - 1.0) / 0.0) ~ "identity"))))))
}}
rowmax = compute.parallel (rowmax_raw ~ "claim:Reduction(combine,identity)") "gpu" OutM
Soft = struct{{.d (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT}))), .m (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
OutS = struct{{.s (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
ParS = struct{{.n Int, .in Soft}}
shifted_raw = (k : ParS) => {{
  i = compute.range k.n
  m = (compute.read ((compute.Read _)(.from k.in.m, .at 0)) : Float)
  y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (exp ((compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float) - m) ~ "element")) ~ "combine"))
  ((OutS)(.s (compute.write ((compute.Write _)(.to y, .at i, .value fold (k.n, 0.0 ~ "identity"))))))
}}
shifted = compute.parallel (shifted_raw ~ "claim:Reduction(combine,identity,element)") "gpu" OutS
Norm = struct{{.d (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT}))), .m (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT}))), .s (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
OutY = struct{{.y (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
ParY = struct{{.n Int, .in Norm}}
norm_raw = (k : ParY) => {{
  i = compute.range k.n
  m = (compute.read ((compute.Read _)(.from k.in.m, .at 0)) : Float)
  s = (compute.read ((compute.Read _)(.from k.in.s, .at 0)) : Float)
  y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutY)(.y (compute.write ((compute.Write _)(.to y, .at i, .value (((exp ((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) - m)) / s) ~ "element"))))))
}}
norm = compute.parallel (norm_raw ~ "claim:Reduction(element)") "gpu" OutY
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
  mx = (compute.plrun rowmax ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutM)
  tot = (compute.plrun shifted ((compute.A Soft)(.n s.n, .I Soft(.d s.in.d, .m mx.m))) : OutS)
  compute.plrun norm ((compute.A Norm)(.n s.n, .I Norm(.d s.in.d, .m mx.m, .s tot.s))) : OutY
}}
built = compute.graph step
(rowmax, shifted, norm, built)
"#
    )
}

/// The same diamond with the normalise's `element` marked **inside** the division.
///
/// # Invariant
/// **Only the mark moves**: the body reads both producers at the same two inputs and writes the
/// same value, so the refusal is a statement about the derivation and not about the shape.
fn unnamed() -> String {
    program().replace(
        "((exp ((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) - m)) / s) ~ \"element\"",
        "((exp ((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) - m) ~ \"element\") / s)",
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

/// The claimed instances the recording's root tuple names, in region order.
fn instances(module: &Module<LangProgram>, value: &LangValue) -> Vec<Instance> {
    root_items(value)
        .iter()
        .take(KERNELS)
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

/// The graph the recording's last root element names.
fn recorded(module: &Module<LangProgram>, value: &LangValue) -> Graph {
    let item = root_items(value)
        .get(KERNELS)
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
        panic!("the program's last root element is not a graph");
    };
    graph::lookup(id).expect("the recorded graph is registered")
}

/// The only schema the recording derives, or the refusal that stopped it.
fn derived(source: &str) -> Result<RegionSchema, String> {
    clear_graph_pass();
    let build = common::check(source).build.expect("the program checks");
    let module = build.module;
    let value = module
        .node_value(AnyNodeId::Dynamic(build.root_val))
        .expect("the root is decided");
    let regions = derive_regions(&recorded(&module, &value), &instances(&module, &value));
    assert_eq!(regions.len(), 1, "the diamond is one region");
    regions
        .into_iter()
        .next()
        .expect("one region")
        .map_err(|refusal| format!("{}: {}", refusal.name(), refusal.message()))
}

/// Hand `graph` to the installed pass and answer the lines it reported.
///
/// # Invariant
/// The pass is already installed: the helper clears the slot only after the consultation, so a
/// caller that installed nothing here has answered whatever the slot already held.
fn consulted(graph: &Graph) -> (Option<Graph>, Vec<String>) {
    let rewritten = consult_graph_pass(graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    (rewritten, lines)
}

/// The diamond derives one `Between` per arriving edge and merges under the floor's own licence.
///
/// # Invariant
/// The surviving node holds **exactly** `compose_fan_in` over the region's own three bodies, so a
/// fan-in is the expr's own operation with a second producer rather than a second mechanism's
/// answer, and the union is `d` three times over — each producer's absorbed input dropped.
#[test]
fn a_consumer_of_two_producers_merges_into_one_node_under_the_floor() {
    clear_graph_pass();
    let schema = derived(&program()).expect("the diamond derives");
    assert_eq!(
        schema
            .between
            .iter()
            .map(|between| (between.producer, between.consumer, between.input))
            .collect::<Vec<_>>(),
        vec![(0, 1, MAXIMUM), (0, 2, MAXIMUM), (1, 2, SUM)],
        "the maximum feeds the sum at `.m` and the normalise at `.m`, and the sum at `.s`: one \
         `Between` per arriving edge, in input-ordinal order"
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
        "a chain whose nodes read earlier producers is the shape the floor's own proof holds"
    );

    let build = common::check(&program()).build.expect("the program checks");
    let module = build.module;
    let value = module
        .node_value(AnyNodeId::Dynamic(build.root_val))
        .expect("the root is decided");
    let graph = recorded(&module, &value);
    assert_eq!(
        (graph.len(), graph.value_count()),
        (KERNELS, KERNELS + 2),
        "the recording is three nodes over an extent, a buffer and one output each"
    );
    install_region_merge(&module);
    let (rewritten, lines) = consulted(&graph);
    let rewritten = rewritten.expect("the floor's own proof licenses a proved fan-in");
    assert_eq!(rewritten.len(), 1, "the diamond becomes one node");
    assert_eq!(
        lines,
        vec![format!(
            "region 0: nodes 0, 1, 2 merged into one node at 2 (the survivor), absorbing input \
             position {MAXIMUM} of node 1 and input position {MAXIMUM} of node 2 and input position \
             {SUM} of node 2, licensed by the floor: every node of the region claims Reduction and \
             nothing else; the merged fragment is registered with the conservative claim `Opaque`: a pass reading the registry names it and refuses it by name, and no reader reads algebra from it; the merged node's identity is digest:ad704741ddf24282"
        )],
        "the line names every node and every absorbed position, the consumer's two included"
    );
    let lichen_graph_ir::Node::Kernel(node) = &rewritten.nodes()[0];
    assert_eq!(
        (node.fragment.inputs, node.fragment.outputs),
        (3, 1),
        "the union is `d` three times over — both absorbed inputs dropped — and the one output is \
         the survivor's"
    );
    assert_eq!(
        node.inputs,
        vec![1, 1, 1],
        "the merged node reads the union and neither deleted output"
    );
    let found = instances(&module, &value);
    let composed = compose_fan_in(&[
        Producer {
            fragment: &found[0].fragment,
            absorbs: Vec::new(),
        },
        Producer {
            fragment: &found[1].fragment,
            absorbs: vec![(0, MAXIMUM)],
        },
        Producer {
            fragment: &found[2].fragment,
            absorbs: vec![(0, MAXIMUM), (1, SUM)],
        },
    ])
    .expect("the recorded diamond composes");
    assert_eq!(
        node.fragment, composed,
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
fn a_fan_in_whose_element_does_not_name_its_second_producer_is_refused_by_name() {
    let refused = derived(&unnamed()).expect_err("the element names one producer of the two");
    assert!(
        refused.starts_with("ElementWithoutProducer"),
        "the second arrival is refused because the consumer's element never names it: {refused}"
    );
}
