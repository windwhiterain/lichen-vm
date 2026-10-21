//! A region of more than two instances: a chain merges, one `compose` per link.
//! See docs/notes/graph-fusion.md §5.

use super::common;


use lichen_compute::ComputeValue;
use lichen_compute::compute::{
    GraphPass, LaunchExtent, clear_graph_pass, consult_graph_pass, graph, set_graph_pass,
    take_graph_pass_lines,
};
use lichen_compute::fusion_claim::{Instance, claim_of, fragment_of, kernel_id};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::region_merge::{install_region_merge, region_merge_pass};
use lichen_graph_ir::{Count, Graph, KernelNode, Node};
use lichen_kernel_ir::{KernelBody, KernelInstr, ScalarClass, ValueDef, compose};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The elements the chain runs over: a small count, because every lane folds the whole extent.
const COUNT: usize = 8;

/// Each link's absorbed input position: every kernel here reads its predecessor at `.p` / `.q`.
const ABSORBED: u32 = 1;

/// The literal only the third kernel's element carries, so the counterfactual has one to move.
const BIAS: f32 = 100.0;

/// The bias the counterfactual moves it to.
const MOVED_BIAS: f32 = 10000.0;

/// The three kernels as the recording hands them back, in region order.
const KERNELS: usize = 3;

/// The chain's answers, each derived from the one before: `Σ d`, `max (d − Σ d)`, `max (d − that + 100)`.
fn total() -> f32 {
    COUNT as f32 * (COUNT as f32 + 1.0) / 2.0
}

fn shifted() -> f32 {
    COUNT as f32 - total()
}

fn lifted() -> f32 {
    COUNT as f32 - shifted() + BIAS
}

/// The chain program: three claimed reductions, each reading the one before it and `d`.
///
/// # Invariant
/// **One incoming edge per node**: `shift` reads `total`'s output at `.p` and `lift` reads
/// `shift`'s at `.q`, and each kernel's other buffer is the recording's own `d`. So the region is a
/// chain and nothing else, and `BIAS` appears in `lift`'s element alone.
fn program(step: &str) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   = struct{{.d (compute.Buf Float)}}
OutR = struct{{.r (compute.Buf Float)}}
Par  = compute.P (compute.KT Float)(.I In, .O OutR)
total_raw = (k : Par) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (k.n, 0.0 ~ "identity")))
}}
total = compute.parallel (total_raw ~ "claim:Reduction(combine,identity)") "gpu" OutR
In2  = struct{{.d (compute.Buf Float), .p (compute.Buf Float)}}
OutZ = struct{{.z (compute.Buf Float)}}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutZ)
shift_raw = (k : Par2) => {{
  i = compute.range k.n
  r = (compute.read ((compute.Read _)(.from k.in.p, .at 0)) : Float)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((((compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)) - r) ~ "element")) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, (0.0 - 1000000.0) ~ "identity")))
}}
shift = compute.parallel (shift_raw ~ "claim:Reduction(combine,identity,element)") "gpu" OutZ
In3  = struct{{.d (compute.Buf Float), .q (compute.Buf Float)}}
OutY = struct{{.y (compute.Buf Float)}}
Par3 = compute.P (compute.KT Float)(.I In3, .O OutY)
lift_raw = (k : Par3) => {{
  i = compute.range k.n
  q = (compute.read ((compute.Read _)(.from k.in.q, .at 0)) : Float)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((((compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)) - q + {BIAS:.1}) ~ "element")) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value fold (k.n, (0.0 - 1000000.0) ~ "identity")))
}}
lift = compute.parallel (lift_raw ~ "claim:Reduction(combine,identity,element)") "gpu" OutY
GArg = struct{{.n Int, .in In}}
{step}
built = compute.graph step
"#
    )
}

/// The three-kernel chain as a recording: the kernels and the graph they recorded, nothing run.
fn recording() -> String {
    format!(
        "{}(total, shift, lift, built)\n",
        program(
            r#"step = (s : GArg) => {
  first = (compute.plrun total ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutR)
  second = (compute.plrun shift ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.r))) : OutZ)
  compute.plrun lift ((compute.A In3)(.n s.n, .I In3(.d s.in.d, .q second.z))) : OutY
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

/// The chain as a program that runs its recording and answers it.
fn running() -> String {
    format!(
        "{}\ncount = ({count})\ndata = (compute.buffer ([{elements}] : array{{Float, {count}}}) : \
         (compute.Buf Float))\nout = compute.collect (compute.graphrun built (count, data))\nout\n",
        recording(),
        count = COUNT,
        elements = elements(),
    )
}

/// The program's answer: the `Float` elements the running program's root holds.
fn answer(module: &Module<LangProgram>, value: &LangValue) -> Vec<f32> {
    common::float_array(module, value)
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

/// The three recorded fragments as a hand-wired chain, returned at `returned`.
///
/// # Invariant
/// Each node's inputs are named by hand, so a shape the recording cannot express — a middle output
/// the graph returns, a node reading **two** producers — is a graph the pass can be asked about
/// without a program that would refuse to record it.
fn chain_graph(instances: &[Instance], wires: &[&[usize]], returned: &[usize]) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let mut starts = vec![1];
    for (index, fragment) in instances
        .iter()
        .map(|instance| &instance.fragment)
        .enumerate()
    {
        let first = graph
            .push(
                Node::Kernel(KernelNode {
                    fragment: fragment.clone(),
                    inputs: wires[index].to_vec(),
                    count: Count::Constant(COUNT),
                    scalars: Vec::new(),
                }),
                fragment.outputs,
            )
            .expect("one declared output per fragment");
        assert_eq!(
            first, starts[index],
            "a node's outputs follow the ones before it"
        );
        starts.push(first + fragment.outputs);
    }
    graph
        .returning(returned.iter().map(|&ordinal| starts[ordinal]).collect())
        .expect("a value the graph defines");
    graph
}

/// The merge with the third kernel's element bias moved, as one pass that wraps the merge.
///
/// # Invariant
/// The mutation is inside the composed body alone, so an answer that moves says the composed node
/// executed and an answer that does not says the recording did.
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

/// Move the composed body's **one** `BIAS` constant into `MOVED_BIAS`.
///
/// # Invariant
/// `BIAS` is in the third kernel's element alone, so exactly one definition carries it and no other
/// kernel's arithmetic can answer the moved value.
fn move_bias(body: &mut KernelBody) {
    let held = BIAS.to_bits() as i64;
    let moved = MOVED_BIAS.to_bits() as i64;
    let biases: Vec<usize> = body
        .values
        .iter()
        .enumerate()
        .filter_map(|(at, definition)| match definition {
            ValueDef::Instr {
                op: KernelInstr::Const(ScalarClass::Float, bits),
                ..
            } if *bits == held => Some(at),
            _ => None,
        })
        .collect();
    assert_eq!(
        biases.len(),
        1,
        "the composed body holds the third kernel's bias alone"
    );
    let ValueDef::Instr {
        op: KernelInstr::Const(ScalarClass::Float, bits),
        ..
    } = &mut body.values[biases[0]]
    else {
        unreachable!("the index came from a constant");
    };
    *bits = moved;
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

/// A chain of three merges into one node, under the floor's own licence, and holds the fold.
///
/// # Invariant
/// The surviving node holds **exactly** `compose` applied twice, so a chain is the pair's own
/// operation twice over and not a second mechanism's answer. With no pack installed the licence is
/// the floor's, which is what the three `Reduction` claims buy and a fan-out never gets.
#[test]
fn a_chain_of_three_merges_into_one_node_under_the_floor() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&recording());
    let instances = instances(&module, &value);
    let graph = recorded(&module, &value);
    assert_eq!(
        (graph.len(), graph.value_count()),
        (3, 5),
        "the recording is three nodes and five values: a count input, a buffer input, and one \
         output each"
    );

    install_region_merge(&module);
    let (rewritten, lines) = consulted(&graph);
    let rewritten = rewritten.expect("the floor's own proof licenses a chain of three");
    assert_eq!(
        (rewritten.len(), rewritten.value_count()),
        (1, 3),
        "the chain becomes one node, and the renumbering drops both links' outputs"
    );
    assert_eq!(
        lines,
        vec![format!(
            "region 0: nodes 0, 1, 2 merged into one node at 2 (the survivor), absorbing input \
             position {ABSORBED} of node 1 and input position {ABSORBED} of node 2, licensed by the \
             floor: every node of the region claims Reduction and nothing else; the merged \
             fragment is unregistered, so it carries no claim and a later digest-keyed pass cannot \
             name it"
        )],
        "the report names every node, every absorbed position, and the licence that answered"
    );
    let Node::Kernel(node) = &rewritten.nodes()[0];
    let composed = compose(
        &compose(&instances[0].fragment, &instances[1].fragment, ABSORBED)
            .expect("the recorded first pair composes"),
        &instances[2].fragment,
        ABSORBED,
    )
    .expect("the composed fragment takes its third kernel as a consumer");
    assert_eq!(
        node.fragment, composed,
        "the surviving node holds `compose` applied twice, element for element"
    );
    assert_eq!(
        (node.fragment.inputs, node.fragment.outputs),
        (3, 1),
        "the union is `d` three times over and the one output is the survivor's"
    );
    assert_eq!(
        node.inputs,
        vec![1, 1, 1],
        "the merged node reads the union: `d` three times, and neither deleted output"
    );
}

/// On a device the merged chain answers the un-passed program, and a moved bias moves it.
///
/// # Invariant
/// **The counterfactual is the proof**: a fused chain computes the same numbers unfused, so only an
/// answer that moves with the composed body says which node the device dispatched. The bias is in
/// the third kernel's element alone, so no recording can answer the moved value.
#[test]
fn the_merged_chain_answers_the_un_passed_program_and_a_moved_bias_moves_it() {
    let (_stub, _guard) = common::stub();
    clear_graph_pass();
    let (base_module, base_value, _root) = common::run(&running());
    let recorded_answer = answer(&base_module, &base_value);
    assert_eq!(
        recorded_answer,
        vec![lifted(); COUNT],
        "with no pass installed the program runs its own three kernels and answers `max (d − \
         max (d − Σ d) + 100)`"
    );

    install_region_merge(&base_module);
    let (merged_module, merged_value, _root) = common::run(&running());
    let merged = answer(&merged_module, &merged_value);
    let report = pass_report(&merged_module);
    println!("the run's own report:\n  {}", report.join("\n  "));
    assert!(
        report.iter().any(
            |line| line.contains("nodes 0, 1, 2 merged into one node at 2")
                && line.contains("licensed by the floor")
        ),
        "the run's own report says the pass merged while the run was dispatched: {report:?}"
    );
    assert_eq!(
        merged, recorded_answer,
        "the merged chain answers the un-passed program's answer element for element"
    );

    set_graph_pass(Box::new(MovedBias(region_merge_pass(&base_module))));
    let (moved_module, moved_value, _root) = common::run(&running());
    let moved = answer(&moved_module, &moved_value);
    clear_graph_pass();
    println!(
        "the device ran: recorded {recorded_answer:?}, merged {merged:?}, the third kernel's \
         `{BIAS:.1}` moved to `{MOVED_BIAS:.1}` {moved:?}"
    );
    assert_eq!(
        moved,
        vec![COUNT as f32 - shifted() + MOVED_BIAS; COUNT],
        "the composed body answers `max (d − max (d − Σ d) + 10000)`, which the recording cannot"
    );
    assert_ne!(
        moved, merged,
        "the answer moved with the composed body, so the composed node is what the run dispatched"
    );
}

/// A returned link output is refused by name, and the recording is left as it was.
///
/// # Invariant
/// Every deleted value is checked, not only the first node's: a chain deletes the **middle** node's
/// output too, and a graph that returns it would answer a different value table.
#[test]
fn a_returned_middle_output_is_refused_by_name_and_nothing_merges() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&recording());
    let instances = instances(&module, &value);
    // `d` for the first kernel, `d` and the first output for the second, `d` and the second's.
    let graph = chain_graph(&instances, &[&[0], &[0, 1], &[0, 2]], &[1]);
    assert_eq!(
        graph.len(),
        3,
        "the graph under test is the three recorded fragments, wired and returned by hand"
    );

    install_region_merge(&module);
    let (rewritten, lines) = consulted(&graph);
    assert!(
        rewritten.is_none(),
        "a declined region changes nothing, so the pass answers no graph: {lines:?}"
    );
    assert_eq!(
        lines,
        vec![
            "region 0 (nodes 0, 1, 2): refused: ProducedValueIsReturned: graph value 2 is one the \
             graph returns, and the merge deletes the node that produces it"
                .to_string()
        ],
        "the refusal names the **middle** node's output, which the pair's own check never saw"
    );
}

/// The hand-wired consumer that reads **both** earlier outputs is a fan-in now, and merges.
///
/// # Invariant
/// **This test moved.** It pinned `NotAReductionChain` on this graph — the fan-in wall — and that
/// wall is what came down: a region two edges arrive at is served once each input is named. What is
/// kept is the evidence, because it is the chain's own kernels rewired rather than a program that
/// spells softmax: the shape is a property of the **edges**, not of an operator.
#[test]
fn a_consumer_that_reads_two_producers_is_a_fan_in_and_merges() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&recording());
    let instances = instances(&module, &value);
    let graph = chain_graph(&instances, &[&[0], &[0, 1], &[1, 2]], &[2]);
    install_region_merge(&module);
    let (rewritten, lines) = consulted(&graph);
    let rewritten = rewritten.expect("a proved fan-in is licensed by the floor's own proof");
    assert_eq!(
        (rewritten.len(), rewritten.value_count()),
        (1, 2),
        "the fan-in becomes one node, and the renumbering drops both producers' outputs"
    );
    assert_eq!(
        lines,
        vec![format!(
            "region 0: nodes 0, 1, 2 merged into one node at 2 (the survivor), absorbing input \
             position {ABSORBED} of node 1 and input position 0 of node 2 and input position \
             {ABSORBED} of node 2, licensed by the floor: every node of the region claims Reduction \
             and nothing else; the merged fragment is unregistered, so it carries no claim and a \
             later digest-keyed pass cannot name it"
        )],
        "the consumer's own two absorbed positions are both named, each with the node it is an \
         input of"
    );
}
