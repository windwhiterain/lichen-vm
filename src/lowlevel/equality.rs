use crate::{
    lowlevel::{Module, NodeId, Program},
    utils::disjoint,
};

impl<P: Program> Module<P> {
    pub fn add_equality(&mut self, a: NodeId, b: NodeId) -> NodeId {
        disjoint::union(&mut self.nodes, a, b)
    }

    pub fn equality_representative(&mut self, node: NodeId) -> NodeId {
        disjoint::find(&mut self.nodes, node)
    }
}
