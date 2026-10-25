//! The graph pass: a rewrite applied to a graph once per run, before dispatch.
//! See docs/notes/graph-fusion.md §5 and §8.
//!
//! # Invariant
//! Everything here takes a `Graph` and a launch extent and answers a `Graph`, so a pass can be
//! written, chained and exercised with no `Module` in scope. The installed pass is the language
//! side's, because a host thread's session is what holds one.

use lichen_graph_ir::{Count, Graph, Node};

/// A graph's launch extent, read for the node that dispatches over it.
///
/// # Invariant
/// A closure rather than a table, so a pass asks for the one node it is about and never
/// has to know how many nodes the graph holds or which of them reads a count.
pub type LaunchExtent<'a> = dyn Fn(usize) -> Option<usize> + 'a;

/// A rewrite a host installs, applied to a graph once per run, immediately before dispatch.
///
/// # Invariant
/// The graph handed over is the one that will run, so a pass that answers a different one
/// changes what runs and a pass that answers `None` changes nothing. Every line it records
/// is reported through the run report's own channel, so a refusal is visible rather than
/// swallowed, and a launch the pass declines to serve is said rather than left to die at
/// the dispatcher.
pub trait GraphPass {
    /// The graph to dispatch, with one line recorded per node the pass looked at.
    fn apply(
        &self,
        graph: &Graph,
        extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph>;
}

/// The one pass that applies `stages` in turn, for a caller that wants several.
///
/// # Invariant
/// A stage sees the graph the stage before it answered, or the one it was handed when that
/// stage answered `None` — the trait's `None` means "leave it as it was". The lines are the
/// stages' own, in stage order and verbatim: every merge refusal names its check after
/// `refused: `, a tiling line states the sentence alone, and the chain answers `None` only
/// when no stage rewrote anything.
pub fn pass_chain(stages: Vec<Box<dyn GraphPass>>) -> Box<dyn GraphPass> {
    Box::new(PassChain { stages })
}

/// The chain: an ordered list of passes, one installed pass in its place.
struct PassChain {
    /// The stages, in the order they are applied.
    stages: Vec<Box<dyn GraphPass>>,
}

impl GraphPass for PassChain {
    /// Apply every stage in turn, each to the graph the one before it answered.
    ///
    /// # Invariant
    /// The graph running is the last answer, or the caller's own while it is `None`: a stage
    /// that rewrote nothing hands the next stage the graph it was given, which is what makes a
    /// chain of one refusing stage and one rewriting stage the second stage's own answer.
    fn apply(
        &self,
        graph: &Graph,
        extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        let mut current: Option<Graph> = None;
        for stage in &self.stages {
            // **A stage reads its own graph's extent**: a rewrite renumbers the nodes.
            let rewritten = {
                let seen = current.as_ref().unwrap_or(graph);
                stage.apply(seen, &stage_extent(seen, extent), lines)
            };
            if let Some(rewritten) = rewritten {
                current = Some(rewritten);
            }
        }
        current
    }
}

/// The extent a stage reads, resolved against the graph that stage is about.
///
/// # Invariant
/// **The node's own count is the reading**: a rewrite renumbers the nodes, so a closure built
/// over the graph the chain was handed answers about another node. A count the graph cannot
/// name is the host's to name, and the host names it against the graph it holds.
fn stage_extent<'a>(
    graph: &'a Graph,
    host: &'a LaunchExtent<'_>,
) -> impl Fn(usize) -> Option<usize> + 'a {
    move |node: usize| match graph.nodes().get(node)? {
        Node::Kernel(kernel) => match kernel.count {
            Count::Constant(count) => Some(count),
            _ => host(node),
        },
    }
}
