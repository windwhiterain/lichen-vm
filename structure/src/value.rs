use lichen_core::{
    plugin::principal_traits::Value,
    runtime::Module,
    value::{Array, StringId, Table},
};
use lichen_utils::arena::array::ArenaArray;

use crate::plugin::Project;

#[derive(Debug, Clone, Copy)]
pub struct NameSet(pub ArenaArray<StringId>);

impl NameSet {
    pub fn new<P: Project>(
        module: &mut Module<P>,
        iter: impl IntoIterator<Item = StringId>,
    ) -> Self {
        Self(ArenaArray::from_iter(&mut module.arena, iter))
    }
}

impl PartialEq for NameSet {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.0.inner(), other.0.inner())
    }
}

impl Eq for NameSet {}

impl Value for NameSet {}

#[derive(Debug, Clone, Copy)]
pub struct Layout(pub ArenaArray<usize>);

impl Layout {
    pub fn uninit<P: Project>(module: &mut Module<P>, len: usize) -> Self {
        Self(ArenaArray::new(&mut module.arena, len))
    }
}

impl PartialEq for Layout {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.0.inner(), other.0.inner())
    }
}

impl Eq for Layout {}

impl Value for Layout {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Structure {
    pub table: Table,
    pub components: Array,
}

impl Value for Structure {
    fn fields(&self) -> impl Iterator<Item = &lichen_core::runtime::NodeIdLocal> {
        self.components.fields()
    }
}
