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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tuple(pub ArenaArray<NodeIdLocal>);

impl Tuple {
    pub fn new<P: Project>(
        module: &mut Module<P>,
        nodes: impl IntoIterator<Item = NodeIdLocal>,
    ) -> Self {
        Tuple(ArenaArray::new(&mut module.arena, nodes))
    }
    pub fn uninit<P: Project>(module: &mut Module<P>, len: usize) -> Self {
        Tuple(ArenaArray::uninit(&mut module.arena, len))
    }
    pub fn node<P: Project>(
        module: &mut Module<P>,
        nodes: impl IntoIterator<Item = NodeIdLocal>,
    ) -> NodeIdLocal {
        let value = Self::new(module, nodes);
        module.add_literal(P::Value::tuple(value))
    }
}

impl Value for Tuple {
    fn fields(&self) -> impl Iterator<Item = &NodeIdLocal> {
        self.0.iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Table(pub ArenaHashMap<StringId, usize>);
impl Value for Table {}
impl Table {
    pub fn new<P: Project>(
        module: &mut Module<P>,
        names: impl IntoIterator<Item = StringId>,
    ) -> Self {
        let mut names = names.into_iter().collect::<Vec<_>>();
        names.sort();
        Self(ArenaHashMap::new(
            &mut module.arena,
            names.into_iter().enumerate().map(|(i, x)| (x, i)),
        ))
    }
    pub fn uninit<P: Project>(module: &mut Module<P>, len: usize) -> Self {
        Self(ArenaHashMap::uninit(&mut module.arena, len))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Unit;
impl Value for Unit {}

impl<P: Project> Module<P> {
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
