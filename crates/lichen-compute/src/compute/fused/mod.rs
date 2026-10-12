//! A fused kernel's landing in a graph (`docs/notes/graph-fusion.md` §5).
//!
//! # Invariant
//! The landing reads through the runtime's own reader and replaces nothing until
//! that read has run to completion, so a refused text leaves the graph exactly as it
//! was. The artifact itself is data and lives in `lichen-runtime`, which is the side
//! of the seam that takes no `Module`; this half is the language side, because a
//! graph is where a recorded dispatch is being rewritten.

pub use lichen_runtime::fused::schema;
pub use lichen_runtime::{
    FusedKernel, FusedRefusal, malformed_at, read_fused_kernel, write_fused_kernel,
};

use lichen_graph_ir::{Graph, GraphRefusal};

/// Why a fused kernel's text did not land in a graph.
///
/// # Invariant
/// One variant per side that refused, and each says which in its message: a caller
/// reading a run report has to know whether to fix the text or the node it named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandingRefusal {
    /// The reader refused the text, before the graph was touched.
    Read(FusedRefusal),
    /// The graph refused the fragment the text named.
    Graph(GraphRefusal),
}

impl LandingRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl std::fmt::Display for LandingRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LandingRefusal::Read(refusal) => write!(
                formatter,
                "the fused kernel's text was refused, so the graph is untouched: {refusal}"
            ),
            LandingRefusal::Graph(refusal) => write!(
                formatter,
                "the graph refused the fused kernel's fragment: {refusal}"
            ),
        }
    }
}

/// Read `text` as a fused kernel and put its fragment into `graph`'s node.
///
/// # Invariant
/// The read runs to completion — parse, convert, both IR gates and `check_plan` —
/// before the graph is touched, so a refused text leaves the graph exactly as it was.
/// The replacement is data-in/data-out because a node holds its fragment as data and
/// `run.rs` dispatches it with `LaunchSet::single`.
pub fn land_fused_kernel(
    graph: &mut Graph,
    node: usize,
    text: &str,
) -> Result<FusedKernel, LandingRefusal> {
    let kernel = read_fused_kernel(text).map_err(LandingRefusal::Read)?;
    graph
        .replace_kernel(node, kernel.fragment.clone())
        .map_err(LandingRefusal::Graph)?;
    Ok(kernel)
}
