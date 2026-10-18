//! A graph is what a recorded evaluation is: the nodes ran, the edges are who
//! consumed whom.
//!
//! # Invariant
//! One node kind and no second: a kernel node over an index range making device
//! buffers. A host call is not a node — a closure is a compiled artifact, so it
//! lowers to a fragment and is dispatched like one. What decides when the host
//! observes completion is the policy, not the graph. docs/notes/compute-graph-jit.md.

/// The graph itself and the nodes in it.
pub mod graph;

/// Refusals, each of which names its own cause.
pub mod refusal;

/// Running a graph against a backend.
pub mod run;

/// A value a graph node produces, and the difference between one that is ready
/// and one that is not.
pub mod value;

pub use graph::{Count, Graph, KernelNode, Node, Scalar, ValueId};
pub use refusal::{FragmentShape, GraphRefusal};
pub use run::{Policy, Runner};
pub use value::{BitsRefusal, Value};
