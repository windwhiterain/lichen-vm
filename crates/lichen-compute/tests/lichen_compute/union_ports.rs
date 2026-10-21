//! The merged fragment's ports are the union of the two bodies' inputs.
//! See `docs/notes/graph-fusion.md` §5.

use super::common;


use lichen_compute::compute::{
    GraphPass, LaunchExtent, clear_graph_pass, consult_graph_pass, set_graph_pass,
    take_graph_pass_lines,
};
use lichen_compute::fusion_claim::{Instance, claim_of, fragment_of, kernel_id};
use lichen_compute::language::LangProgram;
use lichen_compute::region_merge::{install_region_merge, region_merge_pass};
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
/// `fuse_licence.rs` worked around by declaring an input its consumer never reads.
fn natural() -> String {
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
In2  = struct{.e (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutZ)
consumer_raw = (k : Par2) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.e, .at s(0) - 1)) : Float) ~ "element")) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value ((fold (k.n, 0.0 ~ "identity")) + 1.0)))
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

fn running(recording: &str, second: bool) -> String {
    let launch = if second {
        "(count, data, other)"
    } else {
        "(count, data)"
    };
    format!(
        "{recording}\ncount = ({count})\ndata = (compute.buffer ([{data}] : array{{Float, {count}}}) : \
         (compute.Buf Float))\nother = (compute.buffer ([{other}] : array{{Float, {count}}}) : \
         (compute.Buf Float))\nout = compute.collect (compute.graphrun built {launch})\nout\n",
        count = COUNT,
        data = values(1, COUNT),
        other = values(10, COUNT),
    )
}

/// The `elements` float literals `first.0 … (first + elements - 1).0`.
fn values(first: usize, elements: usize) -> String {
    (first..first + elements)
        .map(|position| format!("{position}.0"))
        .collect::<Vec<String>>()
        .join(", ")
}

/// The answer `z[i] = Σ (a[k] + r[k]) + 1` has: `bias` is the consumer's own tail.
fn answer(sum: f32, bias: f32) -> Vec<f32> {
    vec![sum + bias; COUNT]
}

/// The sum `d = 1 … COUNT` folds to, and `10 … COUNT + 9` beside it.
const SUM_D: f32 = 36.0;

/// The sum `d = 1 … COUNT` folds to.

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
        .node_value(AnyNodeId::Dynamic(node))
        .or_else(|| module.node_value(AnyNodeId::Dynamic(node)))
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

/// The lines the installed pass reported for a module, through the run report's own channel.
fn pass_report(module: &Module<LangProgram>) -> Vec<String> {
    module
        .extension_diagnostics
        .iter()
        .filter(|entry| entry.category.contains("pass"))
        .map(|entry| entry.message.clone())
        .collect()
}

/// The merge with the consumer's `+ 1.0` tail turned into `+ 10.0`.
///
/// # Invariant
/// The mutation is inside the composed body alone, so an answer that changes says the composed
/// node executed and an answer that does not says the recording did.
struct MovedBias(Box<dyn GraphPass>);

impl GraphPass for MovedBias {
    fn apply(
        &self,
        graph: &Graph,
        extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        let rewritten = self.0.apply(graph, extent, lines)?;
        let mut mutated = rewritten.clone();
        let Node::Kernel(node) = &rewritten.nodes()[0];
        let mut fragment = node.fragment.clone();
        move_bias(&mut fragment.body);
        mutated
            .replace_kernel(0, fragment)
            .expect("a moved constant is the same shape");
        Some(mutated)
    }
}

/// Turn the one float constant `1.0` in the composed body into `10.0`.
fn move_bias(body: &mut KernelBody) {
    let one = 1.0f32.to_bits() as i64;
    let chosen = body
        .values
        .iter()
        .position(|definition| {
            matches!(
                definition,
                ValueDef::Instr {
                    op: KernelInstr::Const(ScalarClass::Float, bits),
                    ..
                } if *bits == one
            )
        })
        .expect("the composed body holds the consumer's own `+ 1.0`");
    let ValueDef::Instr { op, .. } = &mut body.values[chosen] else {
        unreachable!("the index came from a constant");
    };
    *op = KernelInstr::Const(ScalarClass::Float, 10.0f32.to_bits() as i64);
}

/// The fragment a merged graph's one node holds.
fn merged(rewritten: &Graph) -> &KernelFragment {
    let Node::Kernel(node) = &rewritten.nodes()[0];
    &node.fragment
}

/// Run `recording` three times on a device: as recorded, merged, and merged with the tail moved.
///
/// # Invariant
/// The three answers are the whole of the evidence, and the **counterfactual is the proof**: a
/// fused chain computes the same numbers unfused, so only an answer that moves with the composed
/// body says which node the device dispatched.
fn device_triple(name: &str, recording: &str, second: bool, sum: f32) {
    let (_stub, _guard) = common::stub();
    let program = running(recording, second);
    clear_graph_pass();
    let (base_module, base_value, _root) = common::run(&program);
    let recorded = common::float_array(&base_module, &base_value);
    assert_eq!(
        recorded,
        answer(sum, 1.0),
        "with no pass installed the program runs its own two kernels"
    );

    let (module, _instances, _graph) = derived(recording);
    install_region_merge(&base_module);
    let (merged_module, merged_value, _root) = common::run(&program);
    let merged = common::float_array(&merged_module, &merged_value);
    let report = pass_report(&merged_module);
    assert!(
        report
            .iter()
            .any(|line| line.contains("licensed by the floor:")),
        "the run's own report says the floor licensed the merge while the run was dispatched: \
         {report:?}"
    );
    assert_eq!(
        merged, recorded,
        "the merged graph answers the program's answer element for element"
    );

    // **The counterfactual**: only the composed node can answer a value the recording cannot.
    set_graph_pass(Box::new(MovedBias(region_merge_pass(&module))));
    let (moved_module, moved_value, _root) = common::run(&program);
    let moved = common::float_array(&moved_module, &moved_value);
    clear_graph_pass();
    println!(
        "the device ran `{name}`: recorded {recorded:?}, merged {merged:?}, `+ 1.0` moved to \
         `+ 10.0` {moved:?}"
    );
    assert_eq!(
        moved,
        answer(sum, 10.0),
        "the composed body with the tail moved answers what the recording cannot"
    );
    assert_ne!(
        moved, merged,
        "the answer moved with the composed body, so the composed node is what the run dispatched"
    );
}

/// A consumer whose parameter struct names only the buffer it absorbs merges, and runs.
///
/// # Invariant
/// Nothing is declared that the consumer never reads, so the merged port list is the producer's
/// alone: this is the natural program the old numbering refused.
#[test]
fn a_consumer_naming_only_the_absorbed_buffer_merges_and_the_device_agrees() {
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

    device_triple(
        "a_consumer_naming_only_the_absorbed_buffer_merges_and_the_device_agrees",
        &natural(),
        false,
        COUNT as f32 * SUM_D,
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
