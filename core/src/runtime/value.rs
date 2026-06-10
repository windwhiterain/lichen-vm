use lichen_utils::{
    arena::{array::ArenaArray, hashmap::ArenaHashMap},
    erase_mut,
};

use crate::{
    plugin::{Project, principal_traits::Value},
    runtime::{Module, NodeIdLocal, StringId, solve::Solver},
};

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

pub type Int = i64;
impl Value for Int {}

#[derive(Debug, Clone, Copy)]
pub struct Array(pub ArenaArray<NodeIdLocal>);

impl PartialEq for Array {
    fn eq(&self, other: &Self) -> bool {
        self.0.inner().len() == other.0.inner().len()
    }
}

impl Eq for Array {}

impl Value for Array {
    fn fields(&self) -> impl Iterator<Item = &NodeIdLocal> {
        self.0.iter()
    }
}
pub type Table = ArenaHashMap<StringId, usize>;
impl Value for Table {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unit;
impl Value for Unit {}

pub fn new_array<P: Project>(
    module: &mut Module<P>,
    nodes: impl Iterator<Item = NodeIdLocal>,
) -> Array {
    Array(ArenaArray::from_iter(&mut module.arena, nodes))
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
}

impl<P: Project> Module<P> {
    pub fn evaluation_order(&self, node: &NodeIdLocal) -> (usize, usize) {
        match *self.evaluation(node) {
            Evaluation::Value(_) => (2, 0),
            Evaluation::Ref { .. } => panic!(),
            Evaluation::Auto { referrer_count, .. } => (1, referrer_count),
        }
    }
    pub fn root(&mut self, node: &NodeIdLocal) -> NodeIdLocal {
        if let Evaluation::Ref { node: id, .. } = unsafe { erase_mut(self.evaluation_mut(node)) } {
            let ret = self.root(&id);
            *id = ret;
            ret
        } else {
            *node
        }
    }
    pub fn set_value(&mut self, node: &NodeIdLocal, value: P::Value) -> Vec<NodeIdLocal> {
        let evaluation = self.evaluation_mut(node);
        let Evaluation::Auto { reference, .. } = *evaluation else {
            panic!()
        };
        *evaluation = Evaluation::Value(value);
        let mut reference_iter = reference.map(|x| x.0);
        let mut ret = vec![*node];
        while let Some(reference) = reference_iter {
            let evaluation = self.evaluation_mut(&reference);
            let Evaluation::Ref { brother, .. } = *evaluation else {
                unreachable!();
            };
            reference_iter = brother;
            *evaluation = Evaluation::Value(value);
            ret.push(reference);
        }
        ret
    }
    pub fn set_ref(&mut self, node: &NodeIdLocal, target: &NodeIdLocal) {
        debug_assert_ne!(node, target);
        let evaluation = unsafe { erase_mut(self.evaluation_mut(node)) };
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
        } = (unsafe { erase_mut(self.evaluation_mut(target)) })
        else {
            panic!()
        };
        *referrer_count += self_referrer_count;
        if let Some(self_reference) = self_reference {
            if let Some(reference) = reference {
                let Evaluation::Ref { brother, .. } = self.evaluation_mut(&self_reference.1) else {
                    unreachable!()
                };
                *brother = Some(reference.0);
                reference.0 = *node;
            }
            *evaluation = Evaluation::Ref {
                node: *target,
                brother: Some(self_reference.0),
            };
        } else {
            if let Some(reference) = reference {
                *evaluation = Evaluation::Ref {
                    node: *target,
                    brother: Some(reference.0),
                };
                reference.0 = *node;
            } else {
                *evaluation = Evaluation::Ref {
                    node: *target,
                    brother: None,
                };
            }
        }
    }
}
