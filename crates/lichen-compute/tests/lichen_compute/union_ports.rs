//! The merged fragment's ports are the union of the two bodies' inputs.
//! See `docs/notes/graph-fusion.md` §5.

use super::common;

use lichen_compute::compute::{clear_graph_pass, consult_graph_pass, take_graph_pass_lines};
use lichen_compute::fusion_claim::{Instance, claim_of, fragment_of, kernel_id};
use lichen_compute::language::LangProgram;
use lichen_compute::region_merge::install_region_merge;
use lichen_graph_ir::{Count, Graph, KernelNode, Node};
use lichen_kernel_ir::{
    KernelBody, KernelFragment, KernelInstr, ScalarClass, ValueDef, ValueId, compose,
};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The elements the chain runs over.
const COUNT: usize = 8;

/// The natural chain: `r[i] = Σ d[k]`, then `z[i] = Σ r[k] + 1`.
///
/// # Invariant
/// The consumer's parameter struct names **only** the buffer it absorbs, so the merged node has
/// one port and it is the producer's. This is the program the old numbering refused, and the one
/// `fuse_licence.rs` worked around by declaring an input its consumer never reads. Each body
/// declares its output with `compute.slice` (`docs/notes/kernel-buffer-declarations.md`).
fn natural() -> String {
    r#"
---
  compute = import "compute.lichen"
---
In   = struct{.d (compute.Buf Float)}
OutR = struct{.r (compute.Buf Float)}
Par  = struct{.n Int, .in In}
producer_raw = (k : Par) => {
  i = compute.range k.n
  r = compute.slice ((compute.Slice _)(.count k.n, .element Float))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)) ~ "combine"))
  ((OutR)(.r (compute.write ((compute.Write _)(.to r, .at i, .value fold (k.n, 0.0 ~ "identity"))))))
}
producer = compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "gpu" OutR
In2  = struct{.e (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = struct{.n Int, .in In2}
consumer_raw = (k : Par2) => {
  i = compute.range k.n
  z = compute.slice ((compute.Slice _)(.count k.n, .element Float))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.e, .at s(0) - 1)) : Float) ~ "element")) ~ "combine"))
  ((OutZ)(.z (compute.write ((compute.Write _)(.to z, .at i, .value ((fold (k.n, 0.0 ~ "identity")) + 1.0))))))
}
consumer = compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "gpu" OutZ
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  first = (compute.plrun producer ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutR)
  compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.e first.r)))
}
built = compute.graph step
(producer, consumer, built)
"#
    .to_string()
}

/// The root tuple's items, read from the value the program answered.
///
/// # Invariant
/// The module's root node is not read back: a program whose root is a value has one node for its
/// value and its type, and `common::run` evaluates the type after the value.
fn root_items(value: &lichen_compute::language::LangValue) -> &[ArrayItem] {
    let lichen_compute::language::LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("the program's root is a tuple");
    };
    // SAFETY: `array` is the payload of the root value just produced, whose block is alive.
    unsafe { array.items() }
}

/// The two claimed kernels the recording's root tuple holds.
fn instances(
    module: &Module<LangProgram>,
    value: &lichen_compute::language::LangValue,
) -> Vec<Instance> {
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
fn recorded(module: &Module<LangProgram>, value: &lichen_compute::language::LangValue) -> Graph {
    let item = root_items(value)
        .get(2)
        .expect("the program's root holds a graph");
    let AnyNodeId::Dynamic(node) = item.node else {
        panic!("the graph comes from a frozen artifact");
    };
    let value = module
        .element_value_node(AnyNodeId::Dynamic(node))
        .or(Some(AnyNodeId::Dynamic(node)))
        .and_then(|node| module.node_value(node))
        .and_then(|value| AsEnum::<lichen_compute::ComputeValue>::as_enum(&value));
    let Some(lichen_compute::ComputeValue::Graph(id, _)) = value else {
        panic!("the program's third root element is not a graph");
    };
    lichen_compute::compute::graph::lookup(id).expect("the recorded graph is registered")
}

/// The two recorded fragments and the one recorded graph a recording names.
fn derived(recording: &str) -> (Module<LangProgram>, Vec<Instance>, Graph) {
    let (module, value, _root) = common::run(recording);
    let instances = instances(&module, &value);
    let graph = recorded(&module, &value);
    (module, instances, graph)
}

/// The value ids of the reads of `body` addressing `position`, in table order.
fn buffer_reads_at(body: &KernelBody, position: i64) -> Vec<ValueId> {
    body.blocks
        .iter()
        .flat_map(|block| block.instrs.iter().copied())
        .filter(|value| {
            let ValueDef::Instr {
                op: KernelInstr::BufferReadCall(_),
                args,
                ..
            } = &body.values[value.0 as usize]
            else {
                return false;
            };
            matches!(
                &body.values[args[0].0 as usize],
                ValueDef::Instr {
                    op: KernelInstr::Const(ScalarClass::Int, bits),
                    ..
                } if *bits == position
            )
        })
        .collect()
}

/// The fragment a merged graph's one node holds.
fn merged(rewritten: &Graph) -> &KernelFragment {
    let Node::Kernel(node) = &rewritten.nodes()[0];
    &node.fragment
}

/// A consumer whose parameter struct names only the buffer it absorbs merges.
///
/// # Invariant
/// Nothing is declared that the consumer never reads, so the merged port list is the producer's
/// alone: this is the natural program the old numbering refused.
#[test]
fn a_consumer_naming_only_the_absorbed_buffer_merges() {
    let (module, instances, graph) = derived(&natural());
    install_region_merge(&module);
    let rewritten = consult_graph_pass(&graph, &|_node| Some(COUNT))
        .expect("a consumer that declares no unread input still merges");
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    assert_eq!(
        (rewritten.len(), rewritten.value_count()),
        (1, 3),
        "the region becomes one node and the renumbering drops the producer's output"
    );
    assert!(
        lines[0].contains("merged into one node at 1"),
        "the report names the merge: {lines:?}"
    );
    let fragment = merged(&rewritten);
    assert_eq!(
        fragment,
        &compose(&instances[0].fragment, &instances[1].fragment, 0)
            .expect("the recorded chain composes"),
        "the surviving node holds the composed fragment, element for element"
    );
    assert_eq!(
        (fragment.inputs, fragment.input_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "the union is the producer's one input: the consumer named nothing beside it"
    );
    let Node::Kernel(node) = &rewritten.nodes()[0];
    assert_eq!(
        node.inputs,
        vec![1],
        "and the merged node is wired to that one buffer, the graph's own input"
    );
    assert!(
        !buffer_reads_at(&fragment.body, 0).is_empty(),
        "the producer's own fold reads survive at port 0"
    );
}

/// A node naming a buffer the merged fragment has no port for is refused by name.
///
/// # Invariant
/// The graph is the recorded pair wired by hand with the producer naming one buffer more than
/// its body reads a port for, so the union is longer than the merged ports — the mismatch the
/// union cannot paper over, and the one `PortsDoNotCorrespond` is written for.
#[test]
fn a_node_whose_ordinals_disagree_with_its_body_is_refused_by_name() {
    clear_graph_pass();
    let (module, instances, _graph) = derived(&natural());
    let mut graph = Graph::with_inputs(3);
    let produced = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: instances[0].fragment.clone(),
                inputs: vec![1, 2],
                count: Count::Constant(COUNT),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    let second = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: instances[1].fragment.clone(),
                inputs: vec![produced],
                count: Count::Constant(COUNT),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![second])
        .expect("a value the graph defines");
    install_region_merge(&module);
    let rewritten = consult_graph_pass(&graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    assert!(
        rewritten.is_none(),
        "a refused region changes nothing, so the pass answers no graph: {lines:?}"
    );
    assert_eq!(
        lines,
        vec![
            "region 0 (nodes 0, 1): refused: PortsDoNotCorrespond: the merged node reads graph \
             value 2 at input position 1, and the merged fragment puts None there"
                .to_string()
        ],
        "the refusal names the buffer and the position the merged fragment has no port for"
    );
}
