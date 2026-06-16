use crate::{plugin::Project, runtime::NodeIdLocal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evaluation<P: Project> {
    Value(P::Value),
    Ref {
        node: NodeIdLocal,
        brother: Option<NodeIdLocal>,
    },
    Auto {
        referrer_count: usize,
        reference: Option<(NodeIdLocal, NodeIdLocal)>,
    },
}
