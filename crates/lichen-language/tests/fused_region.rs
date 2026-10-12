//! The region merge pass: a producer→consumer chain becomes one node, and runs.
//! See docs/notes/graph-fusion.md §5 and §8.

mod common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::{
    GraphPass, LaunchExtent, clear_graph_pass, consult_graph_pass, graph, set_graph_pass,
    take_graph_pass_lines,
};
use lichen_compute_gpu::{GpuContext, install_default};
use lichen_graph_ir::{Count, Graph, KernelNode, Node};
use lichen_kernel_ir::{
    KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles, ScalarClass, ValueDef,
    compose, fragment_digest,
};
use lichen_language::floor::{floor_pass_with_packs, install_floor};
use lichen_language::fusion_claim::{Instance, claim_of, fragment_of, kernel_id};
use lichen_language::program::{LangProgram, LangValue};
use lichen_language::region_merge::{install_region_merge, region_merge_pass};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The elements the chain runs over: a small count, because every lane folds the whole extent.
const COUNT: usize = 8;

/// The consumer's input position the producer's output feeds.
const ABSORBED: u32 = 1;

/// The answer the chain has at every position: `max d - Σ d` over `d = 1 … 8`.
fn closed_form() -> f32 {
    COUNT as f32 - (COUNT * (COUNT + 1) / 2) as f32
}

/// The answer the chain has if the element becomes `r - d`: `Σ d - min d`.
fn swapped_form() -> f32 {
    (COUNT * (COUNT + 1) / 2) as f32 - 1.0
}

/// A context on a device, or `None` when there is none here.
///
/// # Invariant
/// The skip is reported and never turned into a failure, and the context is installed as the
/// process's backend because a graph run reaches a device only through `ParallelBackend`.
fn device(test: &str) -> Option<GpuContext> {
    match install_default() {
        Ok(()) => Some(GpuContext::new().expect("a context was just installed")),
        Err(reason) => {
            eprintln!("SKIPPED {test}: no device to run on: {reason}");
            None
        }
    }
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

/// The program's answer: the `Float` elements the running program's root holds.
fn answer(module: &Module<LangProgram>, value: &LangValue) -> Vec<f32> {
    common::float_array(module, value)
}

/// The chain program, with `step` as the graph function's body.
///
/// # Invariant
/// The consumer reads the producer's output at input position [`ABSORBED`], so the recorded edge
/// and the `element` projection's `Producer` read name the same input.
fn program(step: &str) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   = struct{{.d (compute.Buf Float)}}
OutR = struct{{.r (compute.Buf Float)}}
Par  = compute.P (compute.KT Float)(.I In, .O OutR)
producer_raw = (k : Par) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (k.n, 0.0 ~ "identity")))
}}
producer = compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "gpu"
In2  = struct{{.d (compute.Buf Float), .p (compute.Buf Float)}}
OutZ = struct{{.z (compute.Buf Float)}}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutZ)
consumer_raw = (k : Par2) => {{
  i = compute.range k.n
  r = compute.read ((compute.Read _)(.from k.in.p, .at 0))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) (((compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1))) - r) ~ "element")) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, (0.0 - 1000000.0) ~ "identity")))
}}
consumer = compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
GArg = struct{{.n Int, .in In}}
{step}
built = compute.graph step
"#
    )
}

/// The one producer and one consumer whose output the function returns.
fn two_stage() -> &'static str {
    r#"step = (s : GArg) => {
  first = (compute.plrun producer ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutR)
  compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.r)))
}"#
}

/// The chain as a recording: the two kernels and the graph they recorded, nothing run.
fn recording() -> String {
    format!("{}(producer, consumer, built)\n", program(two_stage()))
}

/// The chain as a program that runs its recording and answers it.
fn running() -> String {
    format!(
        "{}\ncount = ({count})\ndata = (compute.buffer ([{elements}] : array{{Float, {count}}}) : \
         (compute.Buf Float))\nout = compute.collect (compute.graphrun built (count, data))\nout\n",
        program(two_stage()),
        count = COUNT,
        elements = elements(),
    )
}

/// A program whose producer feeds the same consumer twice, so the region fans out.
fn fan_out() -> String {
    format!(
        "{}(producer, consumer, built)\n",
        program(
            r#"step = (s : GArg) => {
  first = (compute.plrun producer ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutR)
  left = (compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.r))) : OutZ)
  right = (compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.r))) : OutZ)
  (left, right)
}"#
        )
    )
}

/// The elements the launch reads: `1.0 … 8.0`.
fn elements() -> String {
    (1..=COUNT)
        .map(|position| format!("{position}.0"))
        .collect::<Vec<String>>()
        .join(", ")
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

/// The kernel one item names, as the derivation reads it.
fn instance(module: &Module<LangProgram>, item: &ArrayItem) -> Instance {
    let AnyNodeId::Dynamic(node) = item.node else {
        panic!("a kernel comes from a frozen artifact");
    };
    let kernel = kernel_id(module, node).expect("the item names a kernel");
    let claim = claim_of(kernel).expect("the kernel claims an interface");
    let fragment = fragment_of(kernel).expect("the registry holds the dispatched fragment");
    Instance {
        kernel,
        claim,
        fragment,
    }
}

/// The two claimed kernels the recording's root tuple holds.
fn instances(module: &Module<LangProgram>, value: &LangValue) -> Vec<Instance> {
    let items = root_items(value);
    assert!(items.len() >= 2, "the root names a producer and a consumer");
    items
        .iter()
        .take(2)
        .map(|item| instance(module, item))
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
        .class_value(node)
        .or_else(|| module.node_value(AnyNodeId::Dynamic(node)))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value));
    let Some(ComputeValue::Graph(id, _)) = value else {
        panic!("the program's third root element is not a graph");
    };
    graph::lookup(id).expect("the recorded graph is registered")
}

/// The two recorded fragments as two nodes, with the extents and the returns a test states.
fn two_node_graph(
    producer: &KernelFragment,
    consumer: &KernelFragment,
    producer_extent: usize,
    consumer_extent: usize,
    returned: &[usize],
) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let first = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: producer.clone(),
                inputs: vec![0],
                count: Count::Constant(producer_extent),
            }),
            1,
        )
        .expect("one output, one declared");
    let second = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: consumer.clone(),
                inputs: vec![0, first],
                count: Count::Constant(consumer_extent),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(
            returned
                .iter()
                .map(|which| [first, second][*which])
                .collect(),
        )
        .expect("a value the graph defines");
    graph
}

/// The fragment digest of a graph node.
fn digest_of(graph: &Graph, node: usize) -> u64 {
    let Node::Kernel(kernel) = &graph.nodes()[node];
    fragment_digest(&kernel.fragment)
}

/// The one-instance refusals name both nodes and the check, and change nothing.
fn refuses_by_name(graph: &Graph, expected: &str) -> Vec<String> {
    let lines = consulted(graph);
    assert!(
        lines.len() == 1 && lines[0].contains(expected),
        "the pass refuses the region by name `{expected}`: {lines:?}"
    );
    lines
}

/// Hand `graph` to the installed pass and answer the lines it reported.
fn consulted(graph: &Graph) -> Vec<String> {
    let rewritten = consult_graph_pass(graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    assert!(
        rewritten.is_none(),
        "a refused region changes nothing, so the pass answers no graph: {lines:?}"
    );
    lines
}

/// The merge with the consumer's element subtraction's operands swapped.
///
/// # Invariant
/// The mutation is inside the composed body alone, so an answer that changes says the composed node
/// executed and an answer that does not says the recording did.
struct SwappedElement(Box<dyn GraphPass>);

impl GraphPass for SwappedElement {
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
        swap_element_subtraction(&mut fragment.body);
        mutated
            .replace_kernel(0, fragment)
            .expect("a swapped operand is the same shape");
        Some(mutated)
    }
}

/// Swap the operands of the one float subtraction that reads a computed value.
///
/// # Invariant
/// The consumer's element `d - r` is the float subtraction with a computed operand; its identity
/// `0.0 - 1000000.0` is the other one, and it is left alone.
fn swap_element_subtraction(body: &mut KernelBody) {
    let chosen = body
        .values
        .iter()
        .enumerate()
        .find_map(|(index, definition)| {
            let ValueDef::Instr {
                op: KernelInstr::Bin(ScalarClass::Float, KernelBin::Sub),
                args,
                ..
            } = definition
            else {
                return None;
            };
            let computed = args.iter().any(|value| {
                !matches!(
                    body.values.get(value.0 as usize),
                    Some(ValueDef::Instr {
                        op: KernelInstr::Const(..),
                        ..
                    })
                )
            });
            computed.then_some(index)
        });
    let Some(index) = chosen else {
        panic!("the composed body holds the consumer's element subtraction");
    };
    let ValueDef::Instr { args, .. } = &mut body.values[index] else {
        unreachable!("the index came from an instruction");
    };
    args.swap(0, 1);
}

/// The merge happens, the report names it, and the surviving node holds the composed fragment.
///
/// # Invariant
/// The pass is consulted on the **recorded** graph, which is the graph a run dispatches: the seam
/// stores the pass's answer before dispatching it, so the node count the pass answers is the node
/// count the run has.
#[test]
fn the_region_merges_into_one_node_and_the_surviving_node_holds_the_composed_fragment() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&recording());
    let instances = instances(&module, &value);
    let graph = recorded(&module, &value);
    let before = (graph.len(), graph.value_count());
    assert_eq!(
        before,
        (2, 4),
        "the recording is two nodes and four values: a count input, a buffer input, and one \
         output each"
    );

    install_region_merge(&module);
    let merged = consult_graph_pass(&graph, &|_node| Some(COUNT))
        .expect("the pass answers a graph for a region it may merge");
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    let after = (merged.len(), merged.value_count());
    println!(
        "the recording {} node(s) and {} value(s); the merge {} node(s) and {} value(s); the pass \
         reported {lines:?}",
        before.0, before.1, after.0, after.1,
    );
    assert_eq!(
        after,
        (1, 3),
        "the region becomes one node, and the renumbering drops exactly the producer's output"
    );
    assert_eq!(
        lines,
        vec![format!(
            "region 0: nodes 0 and 1 merged into one node at 1 (the survivor), absorbing input \
             position {ABSORBED} of node 1, licensed by the floor: every node of the region claims \
             Reduction and nothing else; the merged fragment is unregistered, so it carries no \
             claim and a later digest-keyed pass cannot name it"
        )],
        "the report names both nodes, the survivor, the absorbed input and the licence that \
         answered: with no pack installed it is the floor's own proof"
    );
    let Node::Kernel(node) = &merged.nodes()[0];
    assert_eq!(
        node.fragment,
        compose(&instances[0].fragment, &instances[1].fragment, ABSORBED)
            .expect("the recorded chain composes"),
        "the surviving node holds the composed fragment, element for element"
    );
    assert_eq!(
        node.inputs,
        vec![1, 1],
        "the merged node reads the union: the producer's own input, value 1, and the consumer's \
         surviving one, which names the same buffer, and neither is the deleted output 2"
    );
    assert_eq!(
        node.count,
        Count::Value(0),
        "the merged node keeps the recording's runtime extent"
    );
    assert_eq!(
        merged.returns(),
        Some(&[2][..]),
        "the return is repointed at the surviving node's own output"
    );
    assert_eq!(
        node.fragment
            .marks
            .iter()
            .map(|mark| mark.text().unwrap_or("<named>"))
            .collect::<Vec<_>>(),
        instances[1]
            .fragment
            .marks
            .iter()
            .map(|mark| mark.text().unwrap_or("<named>"))
            .collect::<Vec<_>>(),
        "the merged fragment keeps the consumer's marks and not the producer's"
    );
    assert_eq!(
        node.fragment.roles,
        KernelRoles::default(),
        "the merged fragment names no parameter path, so no claim can be joined to it"
    );

    let Some(_context) = device(
        "the_region_merges_into_one_node_and_the_surviving_node_holds_the_composed_fragment",
    ) else {
        return;
    };
    clear_graph_pass();
    let (base_module, base_value, _root) = common::run(&running());
    let baseline = answer(&base_module, &base_value);
    assert_eq!(
        baseline,
        vec![closed_form(); COUNT],
        "the two-node program answers the hand-derived closed form"
    );

    install_region_merge(&base_module);
    let (merged_module, merged_value, _root) = common::run(&running());
    let passed = answer(&merged_module, &merged_value);
    let report = pass_report(&merged_module);
    assert!(
        report
            .iter()
            .any(|line| line.contains("merged into one node at 1")),
        "the run's own report says the pass merged while the run was dispatched: {report:?}"
    );
    assert_eq!(
        passed, baseline,
        "the merged graph answers the program's answer element for element"
    );

    // **The counterfactual**: only the composed node can answer a value the recording cannot.
    set_graph_pass(Box::new(SwappedElement(region_merge_pass(&base_module))));
    let (swapped_module, swapped_value, _root) = common::run(&running());
    let swapped = answer(&swapped_module, &swapped_value);
    println!(
        "the device ran: recorded {baseline:?}, merged {passed:?}, merged with `d - r` swapped \
         {swapped:?}"
    );
    assert_eq!(
        swapped,
        vec![swapped_form(); COUNT],
        "the composed body with `d - r` swapped answers `Σ d - min d`, which the recording cannot \
         answer"
    );
    assert_ne!(
        swapped, passed,
        "the answer moved with the composed body, so the composed node is what the run dispatched"
    );
    clear_graph_pass();
}

/// A returned producer output is refused by name, and the recording is left as it was.
///
/// # Invariant
/// The refusal is the merge's own, not `compose`'s: the graph's return names the value the merge
/// would delete, and a merged graph that dropped it would answer a different value table.
#[test]
fn a_returned_producer_output_is_refused_by_name_and_nothing_merges() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&recording());
    let instances = instances(&module, &value);
    let graph = two_node_graph(
        &instances[0].fragment,
        &instances[1].fragment,
        COUNT,
        COUNT,
        &[0, 1],
    );
    assert_eq!(
        (graph.len(), graph.value_count()),
        (2, 3),
        "the graph under test is the two recorded fragments, wired and returned by hand"
    );
    install_region_merge(&module);
    let lines = refuses_by_name(&graph, "ProducedValueIsReturned");
    assert_eq!(
        lines,
        vec![
            "region 0 (nodes 0, 1): refused: ProducedValueIsReturned: graph value 1 is one the \
             graph returns, and the merge deletes the node that produces it"
                .to_string()
        ],
        "the refusal names the returned value"
    );
    assert_eq!(
        (digest_of(&graph, 0), digest_of(&graph, 1)),
        (
            fragment_digest(&instances[0].fragment),
            fragment_digest(&instances[1].fragment)
        ),
        "the graph still holds the recorded fragments"
    );
    clear_graph_pass();
}

/// Two nodes whose extents do not correspond are refused by name, and nothing merges.
#[test]
fn extents_that_do_not_correspond_are_refused_by_name_and_nothing_merges() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&recording());
    let instances = instances(&module, &value);
    let graph = two_node_graph(
        &instances[0].fragment,
        &instances[1].fragment,
        COUNT,
        COUNT * 2,
        &[1],
    );
    install_region_merge(&module);
    let lines = refuses_by_name(&graph, "ExtentsDiffer");
    assert_eq!(
        lines,
        vec![format!(
            "region 0 (nodes 0, 1): refused: ExtentsDiffer: the producer dispatches over \
             Constant({COUNT}) and the consumer over Constant({}), and a merged node has one extent",
            COUNT * 2
        )],
        "the refusal names both extents"
    );
    clear_graph_pass();
}

/// A region whose consumer reads the absorbed output at a computed constant still merges.
///
/// # Invariant
/// The producer writes the fold at **every** address of the extent, so address 3 holds the same
/// value address 0 does and the merged body's own computation answers it: the refusal this shape
/// used to earn, `ReadAddressIsNotComputable`, is gone because a copy of the producer's cone
/// answers at any address the consumer computes (`docs/notes/graph-fusion.md` §5).
#[test]
fn an_address_the_consumer_computes_is_answered_by_the_producer_s_own_cone() {
    clear_graph_pass();
    let shifted = program(two_stage()).replace("(.from k.in.p, .at 0)", "(.from k.in.p, .at 3)");
    let (module, value, _root) = common::run(&format!("{shifted}(producer, consumer, built)\n"));
    let graph = recorded(&module, &value);
    install_region_merge(&module);
    let merged = consult_graph_pass(&graph, &|_node| Some(COUNT))
        .expect("the region's consumer reads an address the producer wrote");
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    assert_eq!(
        (merged.len(), merged.value_count()),
        (1, 3),
        "the region becomes one node: the floor's own proof licenses it"
    );
    assert!(
        lines[0].contains("licensed by the floor:"),
        "and nothing about the read address refused it: {lines:?}"
    );
}

/// A region whose producer feeds two consumers is refused by the derivation's own name.
///
/// # Invariant
/// The fan-out is expressible: both consumers are the same claimed kernel, so the region holds
/// three instances and `region_edges` refuses the producer's second edge.
#[test]
fn a_fan_out_region_is_refused_by_the_derivations_own_name() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&fan_out());
    let graph = recorded(&module, &value);
    assert_eq!(graph.len(), 3, "the producer feeds two consumers");
    install_region_merge(&module);
    let lines = refuses_by_name(&graph, "NotAReductionChain");
    assert_eq!(
        lines,
        vec![
            "region 0 (nodes 0, 1, 2): refused: NotAReductionChain: graph node 0's edges inside \
             the region do not make one producer-to-consumer chain"
                .to_string()
        ],
        "the region is refused by the derivation's own name, and nothing merged"
    );
    clear_graph_pass();
}

/// The floor's one report carries both stages: the merge, and the tiling pass after it.
///
/// # Invariant
/// The two lines are the whole claim. The seam holds **one** pass, so a chain is the only way
/// both transforms can run, and a tiling pass that stayed silent would leave a merged node
/// nobody looked at — which is the half-apply the floor exists to close.
#[test]
fn the_floor_reports_the_merge_and_then_the_tiling_pass_on_the_merged_node() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&recording());
    let graph = recorded(&module, &value);
    install_floor(&module);
    let rewritten = consult_graph_pass(&graph, &|_node| Some(COUNT))
        .expect("the merge stage answers a graph for a region it may merge");
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    assert_eq!(
        rewritten.len(),
        1,
        "the merge stage ran first and left the tiling stage one node"
    );
    assert_eq!(
        lines,
        vec![
            format!(
                "region 0: nodes 0 and 1 merged into one node at 1 (the survivor), absorbing input \
                 position {ABSORBED} of node 1, licensed by the floor: every node of the region \
                 claims Reduction and nothing else; the merged fragment is unregistered, so it \
                 carries no claim and a later digest-keyed pass cannot name it"
            ),
            "node 0: refused: no claim is registered for the fragment at 0, so it is in no row \
             of the digest join and no algebra can be derived for it; a composed fragment is \
             exactly this, because composition registers neither body it joins"
                .to_string()
        ],
        "one line per stage in stage order: the region line is the merge's, and the node line is \
         the tiling pass naming the merged node it was handed"
    );
}

/// The floor runs both stages on a real dispatch, and a counterfactual moves the answer.
///
/// # Invariant
/// The report is read from the run's own channel, not from a hand-installed consult: the claim
/// is that a dispatch under the floor reports both stages, which only a run can show.
#[test]
fn the_floor_run_reports_both_stages_and_answers_the_un_passed_program() {
    let Some(_context) =
        device("the_floor_run_reports_both_stages_and_answers_the_un_passed_program")
    else {
        return;
    };
    clear_graph_pass();
    let (base_module, base_value, _root) = common::run(&running());
    let baseline = answer(&base_module, &base_value);
    assert_eq!(
        baseline,
        vec![closed_form(); COUNT],
        "the un-passed two-node program answers the hand-derived closed form"
    );

    install_floor(&base_module);
    let (floored_module, floored_value, _root) = common::run(&running());
    let passed = answer(&floored_module, &floored_value);
    let report = pass_report(&floored_module);
    println!("the floor's run reported:\n  {}", report.join("\n  "));
    assert!(
        report
            .iter()
            .any(|line| line.contains("merged into one node at 1")
                && line.contains("licensed by the floor")),
        "the run's own report says the merge stage merged while the run was dispatched: {report:?}"
    );
    assert!(
        report
            .iter()
            .any(|line| line
                .contains("node 0: refused: no claim is registered for the fragment at 0")),
        "and the tiling stage looked at the node the merge left, by name: {report:?}"
    );
    assert_eq!(
        passed, baseline,
        "the floored graph answers the un-passed program's answer element for element"
    );

    // **The counterfactual**: only the composed node can answer a value the recording cannot.
    set_graph_pass(Box::new(SwappedElement(floor_pass_with_packs(
        &base_module,
        &[],
    ))));
    let (swapped_module, swapped_value, _root) = common::run(&running());
    let swapped = answer(&swapped_module, &swapped_value);
    println!(
        "the device ran: recorded {baseline:?}, floored {passed:?}, floored with `d - r` swapped {swapped:?}"
    );
    assert_eq!(
        swapped,
        vec![swapped_form(); COUNT],
        "the composed body with `d - r` swapped answers `Σ d - min d`, which the recording \
         cannot answer"
    );
    assert_ne!(
        swapped, passed,
        "the answer moved with the composed body, so the merged node is what the run dispatched"
    );
    clear_graph_pass();
}
