//! Why a graph could not be represented without this, and where the DAG came from
//!
//! A graph is not a special case detected in the source. It is the shape of a
//! program that has been evaluated once and whose *evaluation* was recorded
//! rather than thrown away — the nodes are what ran, the edges are who consumed
//! whom, and a diamond is not a pattern the compiler looked for but a shape two
//! independent computations happened to have.
//!
//! The consequence is that the graph contains two kinds of node and **no third**:
//!
//! - a **kernel** node, which is a [`KernelFragment`] over an index range and
//!   produces device buffers;
//! - a **native** node, which is a host function and produces host data.
//!
//! There is no "fused" node, no "barrier" node and no "sync" node, and that is
//! not an omission. What decides when the host observes completion is the
//! *policy* ([`run::Policy`]), not the graph, and a graph that could name its
//! own synchronisation would be a graph whose correctness depended on where
//! somebody put a keyword. The only thing a graph says is what has to happen
//! before what.

/// The graph itself and the nodes in it.
pub mod graph;

/// Refusals, each of which names its own cause.
pub mod refusal;

/// Running a graph against a backend.
pub mod run;

/// A value a graph node produces, and the difference between one that is ready
/// and one that is not.
pub mod value;

pub use graph::{Count, Graph, KernelNode, NativeCall, NativeNode, Node, ValueId};
pub use refusal::GraphRefusal;
pub use run::{Policy, Runner};
pub use value::{Native, NativeValueId, Value};
