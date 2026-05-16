use lichen_utils::arena::{array::ArenaArray, hashmap::ArenaHashMap};

use crate::{
    plugin::Project,
    runtime::{Module, NodeId, NodeIdRaw, StringId, solve::Solver},
};

#[derive(Debug, Clone, Copy)]
pub enum Evaluation<P: Project> {
    Value(P::Value),
    Ref {
        node: NodeId<P>,
        brother: Option<NodeId<P>>,
    },
    Auto {
        referrer_count: usize,
        reference: Option<(NodeId<P>, NodeId<P>)>,
    },
}

pub type Int = i64;
pub type Array = ArenaArray<NodeIdRaw>;
pub type Table = ArenaHashMap<StringId, usize>;

pub fn new_array<P: Project>(
    module: &mut Module<P>,
    nodes: impl Iterator<Item = NodeId<P>>,
) -> Array {
    Array::from_iter(&mut module.arena, nodes.map(|x| x.raw()))
}
pub fn new_table<P: Project>(
    module: &mut Module<P>,
    entries: impl Iterator<Item = (StringId, usize)>,
) -> Table {
    Table::from_iter(&mut module.arena, entries)
}

impl<P: Project> Evaluation<P> {
    pub const AUTO: Self = Self::Auto {
        referrer_count: 1,
        reference: None,
    };
    pub fn evaluation_order(self) -> (usize, usize) {
        match self {
            Evaluation::Value(_) => (2, 0),
            Evaluation::Ref { node: id, .. } => id.evaluation().evaluation_order(),
            Evaluation::Auto { referrer_count, .. } => (1, referrer_count),
        }
    }
}

impl<P: Project> NodeId<P> {
    pub fn root(self) -> NodeId<P> {
        if let Evaluation::Ref { node: id, .. } = self.evaluation_mut() {
            let ret = id.root();
            *id = ret;
            ret
        } else {
            self
        }
    }
    pub fn set_value(self, value: P::Value) {
        let evaluation = self.evaluation_mut();
        let Evaluation::Auto { reference, .. } = *evaluation else {
            panic!()
        };
        let mut reference_iter = reference.map(|x| x.0);
        while let Some(reference) = reference_iter {
            let evaluation = reference.evaluation_mut();
            let Evaluation::Ref { brother, .. } = *evaluation else {
                unreachable!();
            };
            reference_iter = brother;
            Solver::set_node_value(reference, value);
        }
        Solver::set_node_value(self, value);
    }
    pub fn set_ref(self, node: NodeId<P>) {
        let evaluation = self.evaluation_mut();
        let Evaluation::Auto {
            referrer_count: self_referrer_count,
            reference: self_reference,
            ..
        } = *evaluation
        else {
            panic!()
        };
        let Evaluation::Auto {
            referrer_count,
            reference,
        } = node.evaluation_mut()
        else {
            panic!()
        };
        *referrer_count += self_referrer_count;
        if let Some(self_reference) = self_reference {
            if let Some(reference) = reference {
                let Evaluation::Ref { brother, .. } = self_reference.1.evaluation_mut() else {
                    unreachable!()
                };
                *brother = Some(reference.0);
                reference.0 = self;
            }
            *evaluation = Evaluation::Ref {
                node,
                brother: Some(self_reference.0),
            };
        } else {
            if let Some(reference) = reference {
                *evaluation = Evaluation::Ref {
                    node,
                    brother: Some(reference.0),
                };
                reference.0 = self;
            } else {
                *evaluation = Evaluation::Ref {
                    node,
                    brother: None,
                };
            }
        }
    }
}
