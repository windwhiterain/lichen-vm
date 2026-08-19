use lichen_core::{
    plugin::principal_traits::Value,
    runtime::Module,
    value::{Tuple, StringId, Table},
};
use lichen_utils::arena::array::ArenaArray;

use crate::plugin::Project;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NameSet(pub ArenaArray<StringId>);

impl NameSet {
    pub fn new<P: Project>(
        module: &mut Module<P>,
        iter: impl IntoIterator<Item = StringId>,
    ) -> Self {
        Self(ArenaArray::new(&mut module.arena, iter))
    }
}

impl Value for NameSet {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Offsets(pub ArenaArray<usize>);

impl Offsets {
    pub fn uninit<P: Project>(module: &mut Module<P>, len: usize) -> Self {
        Self(ArenaArray::uninit(&mut module.arena, len))
    }
}

impl Value for Offsets {}
