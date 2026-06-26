use lichen_utils::{
    arena::{array::ArenaArray, hashmap::ArenaHashMap},
    erase_mut,
};

use crate::{
    plugin::{Project, Value as _, principal_traits::Value},
    runtime::{Module, NodeIdLocal, evaluation::Evaluation},
};

pub type Int = i64;
impl Value for Int {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StringId(pub usize);
impl Value for StringId {}

#[derive(Debug, Clone, Copy)]
pub struct Array(pub ArenaArray<NodeIdLocal>);

impl Array {
    pub fn new<P: Project>(
        module: &mut Module<P>,
        nodes: impl IntoIterator<Item = NodeIdLocal>,
    ) -> Self {
        Array(ArenaArray::from_iter(&mut module.arena, nodes))
    }
    pub fn uninit<P: Project>(module: &mut Module<P>, len: usize) -> Self {
        Array(ArenaArray::new(&mut module.arena, len))
    }
    pub fn node<P: Project>(
        module: &mut Module<P>,
        nodes: impl IntoIterator<Item = NodeIdLocal>,
    ) -> NodeIdLocal {
        let value = Self::new(module, nodes);
        module.add_literal(P::Value::from_array(value))
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Table(pub ArenaHashMap<StringId, usize>);
impl Value for Table {}
impl Table {
    pub fn new<P: Project>(
        module: &mut Module<P>,
        names: impl IntoIterator<Item = StringId>,
    ) -> Self {
        let mut names = names.into_iter().collect::<Vec<_>>();
        names.sort();
        Self(ArenaHashMap::from_iter(
            &mut module.arena,
            names.into_iter().enumerate().map(|(i, x)| (x, i)),
        ))
    }
    pub fn uninit<P: Project>(module: &mut Module<P>, len: usize) -> Self {
        Self(ArenaHashMap::new(&mut module.arena, len))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unit;
impl Value for Unit {}

impl<P: Project> Evaluation<P> {
    pub const AUTO: Self = Self::Auto {
        referrer_count: 1,
        referers: None,
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
            let ret = self.root(id);
            *id = ret;
            ret
        } else {
            *node
        }
    }

    pub fn debug_root(&self, node: &NodeIdLocal) -> NodeIdLocal {
        if let Evaluation::Ref { node: id, .. } = self.evaluation(node) {
            self.debug_root(id)
        } else {
            *node
        }
    }
}
