//! A graph is what a recorded evaluation is: the nodes ran, the edges are who
//! consumed whom.
//!
//! # Invariant
//! Two node kinds and no third: a kernel node over an index range making device
//! buffers, and a native node making host data. What decides when the host observes
//! completion is the policy, not the graph. docs/notes/compute-graph-jit.md.

/// The graph itself and the nodes in it.
pub mod graph;

/// Refusals, each of which names its own cause.
pub mod refusal;

/// Running a graph against a backend.
pub mod run;

/// A value a graph node produces, and the difference between one that is ready
/// and one that is not.
pub mod value;

pub use graph::{Count, Graph, KernelNode, Node, ValueId};
pub use refusal::GraphRefusal;
pub use run::{Policy, Runner};
pub use value::Value;
