//! A fused kernel lands in a recorded graph node, and a refused one leaves it alone
//! (`docs/notes/graph-fusion.md` §5).

use lichen_compute::{FusedRefusal, LandingRefusal, land_fused_kernel, read_fused_kernel};
use lichen_graph_ir::{Count, FragmentShape, Graph, GraphRefusal, KernelNode, Node};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    LaneWidth, PlanRefusal, ScalarClass, TargetFacts, fragment_digest,
};

/// The artifact a caller lands: `out[i] = in[i] + 2`, hand-written.
///
/// # Invariant
/// Its fragment's signature — one input buffer, one output buffer, two parameter
/// leaves — is the one the node below was built with, so the two differ only in the
/// body, which is what a landed replacement is supposed to change.
const ARTIFACT: &str = r#"
{
  "version": 1,
  "fragment": {
    "param_shape": { "tuple": [{ "scalar": "int" }, { "scalar": "int" }] },
    "roles": { "scalars": [], "inputs": [], "outputs": [], "input_classes": [], "output_classes": [] },
    "body": {
      "values": [
        { "block_param": { "block": 0 } },
        { "block_param": { "block": 0 } },
        { "instr": { "op": { "const": { "class": "int", "bits": 0 } }, "args": [], "classes": ["int"] } },
        { "instr": { "op": { "const": { "class": "int", "bits": 0 } }, "args": [], "classes": ["int"] } },
        { "instr": { "op": { "buffer_read_call": { "class": "int" } }, "args": [3, 1], "classes": ["int"] } },
        { "instr": { "op": { "const": { "class": "int", "bits": 2 } }, "args": [], "classes": ["int"] } },
        { "instr": { "op": { "bin": { "class": "int", "op": "add" } }, "args": [4, 5], "classes": ["int"] } },
        { "instr": { "op": { "buffer_write_call": { "class": "int" } }, "args": [2, 1, 6], "classes": [] } },
        { "instr": { "op": { "const": { "class": "int", "bits": 0 } }, "args": [], "classes": ["int"] } }
      ],
      "blocks": [
        { "params": [0, 1], "instrs": [2, 3, 4, 5, 6, 7, 8], "terminator": { "return": { "values": [8] } } }
      ],
      "entry": 0,
      "barriers": []
    },
    "inputs": 1,
    "outputs": 1,
    "input_classes": ["int"],
    "output_classes": ["int"],
    "result_classes": ["int"],
    "target": { "int_width": "i64", "lane_width": 64 },
    "slices": [],
    "marks": []
  },
  "claim": null,
  "plan": { "staging": [] }
}
"#;

/// `out[i] = in[i] + k`, built with the IR's own constructors.
fn fragment_adding(k: i64) -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out position
                FlatOp::Read(1),                                        // the index
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // the input buffer
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, k)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // a returned value
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// A one-node graph over `fragment`, whose return is that node's output.
///
/// # Invariant
/// One kernel node, one value table and one recorded return, so a landing has edges,
/// a value count and a return it must leave alone.
fn graph_over(fragment: KernelFragment) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment,
                inputs: vec![0],
                count: Count::Constant(4),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");
    graph
}

/// The fragment the graph's one node holds.
fn node_fragment(graph: &Graph) -> &KernelFragment {
    match &graph.nodes()[0] {
        Node::Kernel(kernel) => &kernel.fragment,
    }
}

/// [`ARTIFACT`] with a second input buffer declared and its classes, nothing else moved.
///
/// # Invariant
/// Only the fragment's `inputs` differs, so a refusal of it is about the shape and not
/// about anything the reader can see.
fn artifact_with_two_inputs() -> String {
    ARTIFACT
        .replace("\"inputs\": 1,", "\"inputs\": 2,")
        .replace(
            "\"input_classes\": [\"int\"],",
            "\"input_classes\": [\"int\", \"int\"],",
        )
}

/// [`ARTIFACT`] with a plan naming a mark the body never published.
fn artifact_naming_an_absent_mark() -> String {
    ARTIFACT.replace(
        "\"plan\": { \"staging\": [] }",
        "\"plan\": { \"staging\": [{ \"name\": \"absent\", \"space\": \"shared\", \
         \"group_width\": 64 }] }",
    )
}

#[test]
fn land_replaces_the_nodes_fragment_and_leaves_the_graph_around_it() {
    let mut graph = graph_over(fragment_adding(1));
    let before = fragment_digest(node_fragment(&graph));
    let nodes = graph.len();
    let values = graph.value_count();
    let returns = graph.returns().map(<[usize]>::to_vec);

    let kernel = land_fused_kernel(&mut graph, 0, ARTIFACT)
        .unwrap_or_else(|refusal| panic!("the artifact lands: {refusal}"));

    assert_eq!(
        node_fragment(&graph),
        &kernel.fragment,
        "the node holds the fragment the text named"
    );
    assert_ne!(
        fragment_digest(node_fragment(&graph)),
        before,
        "the node holds a different body than the one it was built with"
    );
    assert_eq!(graph.len(), nodes, "landing adds no node");
    assert_eq!(graph.value_count(), values, "landing renumbers no value");
    assert_eq!(
        graph.returns().map(<[usize]>::to_vec),
        returns,
        "the recorded return is untouched"
    );
    assert_eq!(
        kernel.plan,
        lichen_kernel_ir::Plan::default(),
        "the reader hands the plan back with the fragment"
    );
}

#[test]
fn a_fragment_reading_a_different_number_of_buffers_is_refused_by_shape() {
    let mut graph = graph_over(fragment_adding(1));
    let before = fragment_digest(node_fragment(&graph));

    let refusal = land_fused_kernel(&mut graph, 0, &artifact_with_two_inputs())
        .expect_err("a fragment reading two buffers does not fit a node built for one");

    assert_eq!(
        refusal,
        LandingRefusal::Graph(GraphRefusal::ShapeDisagrees {
            node: 0,
            had: FragmentShape {
                inputs: 1,
                outputs: 1,
                flat_arity: 2,
            },
            got: FragmentShape {
                inputs: 2,
                outputs: 1,
                flat_arity: 2,
            },
        }),
        "the refusal names both shapes and the node"
    );
    assert!(
        refusal.message().contains("the graph refused"),
        "the message says which side refused: {refusal}"
    );
    assert_eq!(
        fragment_digest(node_fragment(&graph)),
        before,
        "a refused replacement leaves the node as it was"
    );
}

#[test]
fn a_node_index_past_the_end_is_refused_by_name() {
    let mut graph = graph_over(fragment_adding(1));
    let before = fragment_digest(node_fragment(&graph));

    let refusal =
        land_fused_kernel(&mut graph, 1, ARTIFACT).expect_err("a one-node graph has no node 1");

    assert_eq!(
        refusal,
        LandingRefusal::Graph(GraphRefusal::NoSuchNode { node: 1, nodes: 1 }),
        "the refusal names the index and the node count it met"
    );
    assert_eq!(
        fragment_digest(node_fragment(&graph)),
        before,
        "nothing moved"
    );
}

#[test]
fn a_text_the_reader_refuses_leaves_the_graph_untouched() {
    let mut graph = graph_over(fragment_adding(1));
    let before = fragment_digest(node_fragment(&graph));
    let nodes = graph.len();
    let values = graph.value_count();

    let refusal = land_fused_kernel(&mut graph, 0, &artifact_naming_an_absent_mark())
        .expect_err("the plan names a mark the body never published");

    assert!(
        matches!(
            refusal,
            LandingRefusal::Read(FusedRefusal::PlanRefused {
                refusal: PlanRefusal::UnknownStagedValue { ref name }
            }) if name == "absent"
        ),
        "the reader's own refusal travels and names the mark: {refusal}"
    );
    assert!(
        refusal
            .message()
            .contains("the fused kernel's text was refused"),
        "the message says which side refused: {refusal}"
    );
    assert_eq!(
        fragment_digest(node_fragment(&graph)),
        before,
        "every check runs before anything is replaced"
    );
    assert_eq!(graph.len(), nodes, "a refused text adds no node");
    assert_eq!(
        graph.value_count(),
        values,
        "a refused text renumbers no value"
    );
}

#[test]
fn a_hand_written_literal_is_a_fused_kernel_the_reader_accepts() {
    let kernel = read_fused_kernel(ARTIFACT).expect("the hand-written artifact reads back");
    assert_eq!(kernel.fragment.inputs, 1);
    assert_eq!(kernel.fragment.outputs, 1);
    assert_eq!(kernel.fragment.param_shape.flat_arity(), 2);
    assert!(
        fragment_digest(&kernel.fragment) != fragment_digest(&fragment_adding(1)),
        "the artifact's body differs from the node's own, so a landing is observable"
    );
}
