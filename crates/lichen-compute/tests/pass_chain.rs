//! The chain: several passes as the one installed pass, each seeing the one before it.

use lichen_compute::compute::{GraphPass, LaunchExtent, pass_chain};
use lichen_graph_ir::{Count, Graph, KernelNode, Node};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBody, KernelFragment, KernelRoles, KernelShape, LaneWidth, ScalarClass,
    TargetFacts,
};

/// The count every stub node dispatches over, and the extent every stub is handed.
const EXTENT: usize = 8;

/// A stub pass: it appends `added` nodes and reports the node count it saw.
struct Append {
    /// What the stub calls itself, which is how its line says which stage spoke.
    name: &'static str,
    /// How many nodes this stage appends; zero is a stage that rewrites nothing.
    added: usize,
}

impl GraphPass for Append {
    /// Report what the stage saw, then answer the graph with `added` nodes more.
    ///
    /// # Invariant
    /// The line is the only way the tests tell the stages apart, so it is recorded from the
    /// graph the chain **handed over** rather than from the clone this stage builds.
    fn apply(
        &self,
        graph: &Graph,
        _extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        lines.push(format!("{}: saw {} node(s)", self.name, graph.len()));
        let mut rewritten = graph.clone();
        for _ in 0..self.added {
            rewritten
                .push(
                    Node::Kernel(KernelNode {
                        fragment: one_output(),
                        inputs: vec![0],
                        count: Count::Constant(EXTENT),
                    }),
                    1,
                )
                .expect("the stub node declares one output and produces one");
        }
        (self.added > 0).then_some(rewritten)
    }
}

/// One kernel node's fragment, hand-written and otherwise inert.
///
/// # Invariant
/// It is pushed to change the graph's **shape** — a node and a value — and nothing in the
/// chain's contract reads its body, so a straight-line `out = in` is enough to carry it.
fn one_output() -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Scalar(ScalarClass::Int),
        roles: KernelRoles::default(),
        body: KernelBody::from_flat(1, &[FlatOp::Read(0)]),
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

/// The graph under test: one node, so "the original" and "one stage's rewrite" differ.
fn one_node() -> Graph {
    let mut graph = Graph::with_inputs(1);
    graph
        .push(
            Node::Kernel(KernelNode {
                fragment: one_output(),
                inputs: vec![0],
                count: Count::Constant(EXTENT),
            }),
            1,
        )
        .expect("the first node declares one output and produces one");
    graph
}

/// The extent closure every chain here is handed.
fn extent(_node: usize) -> Option<usize> {
    Some(EXTENT)
}

/// The second stage sees the first stage's rewrite, and both lines reach the one report.
///
/// # Invariant
/// The node counts are the whole claim: `saw 1` is the graph the chain was handed and
/// `saw 2` is the one stage one answered, so the lines order the stages and prove the
/// threading at once.
#[test]
fn a_stage_sees_the_graph_the_stage_before_it_answered() {
    let chain = pass_chain(vec![
        Box::new(Append {
            name: "first",
            added: 1,
        }),
        Box::new(Append {
            name: "second",
            added: 0,
        }),
    ]);
    let mut lines = Vec::new();
    let rewritten = chain.apply(&one_node(), &extent, &mut lines);
    assert_eq!(
        rewritten.as_ref().map(Graph::len),
        Some(2),
        "the chain answers the last rewrite, which is the only stage that wrote one"
    );
    assert_eq!(
        lines,
        vec!["first: saw 1 node(s)", "second: saw 2 node(s)"],
        "the lines are the stages' own in stage order, and the second stage was handed the first \
         stage's two-node graph rather than the one the chain started from"
    );
}

/// A chain whose stages all rewrite nothing answers no graph, and says so per stage.
///
/// # Invariant
/// The second stage's `saw 1` is the contract's `None` case: a stage that rewrote nothing
/// leaves the graph as it was, so the next stage sees the original rather than a stale clone.
#[test]
fn a_chain_of_stages_that_rewrite_nothing_answers_no_graph() {
    let chain = pass_chain(vec![
        Box::new(Append {
            name: "first",
            added: 0,
        }),
        Box::new(Append {
            name: "second",
            added: 0,
        }),
    ]);
    let mut lines = Vec::new();
    assert!(
        chain.apply(&one_node(), &extent, &mut lines).is_none(),
        "no stage rewrote anything, so a caller can still tell that from a rewrite"
    );
    assert_eq!(
        lines,
        vec!["first: saw 1 node(s)", "second: saw 1 node(s)"],
        "both stages were consulted, and each saw the graph as it was"
    );
}

/// The last stage's rewrite alone is enough to answer a graph.
///
/// # Invariant
/// A rewrite is not cancelled by a stage that declines after it: `None` means "leave it as it
/// was", so the answer is the one stage one wrote and no later stage rolls it back.
#[test]
fn a_chain_answers_a_graph_when_only_its_last_stage_rewrote() {
    let chain = pass_chain(vec![
        Box::new(Append {
            name: "first",
            added: 0,
        }),
        Box::new(Append {
            name: "second",
            added: 1,
        }),
    ]);
    let mut lines = Vec::new();
    let rewritten = chain.apply(&one_node(), &extent, &mut lines);
    assert_eq!(
        rewritten.as_ref().map(Graph::len),
        Some(2),
        "the answer is stage two's graph, and stage one's `None` did not answer for the chain"
    );
    assert_eq!(
        lines,
        vec!["first: saw 1 node(s)", "second: saw 1 node(s)"],
        "stage one refused and stage two still rewrote from the graph it was handed"
    );
}