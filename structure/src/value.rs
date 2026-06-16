use lichen_core::{
    plugin::principal_traits::Value,
    runtime::{Module, NodeIdLocal},
    value::{Array, StringId, Table},
};
use lichen_utils::arena::array::ArenaArray;

use crate::plugin::Project;

#[derive(Debug, Clone, Copy)]
pub struct NamedArray(pub ArenaArray<(StringId, NodeIdLocal)>);

impl PartialEq for NamedArray {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.0.inner(), other.0.inner())
    }
}

impl Eq for NamedArray {}

impl Value for NamedArray {
    fn fields(&self) -> impl Iterator<Item = &lichen_core::runtime::NodeIdLocal> {
        self.0.iter().map(|x| &x.1)
    }
}

impl NamedArray {
    pub fn new<P: Project>(
        module: &mut Module<P>,
        named_nodes: impl IntoIterator<Item = (StringId, NodeIdLocal)>,
    ) -> Self {
        Self(ArenaArray::from_iter(&mut module.arena, named_nodes))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NameSet(pub ArenaArray<StringId>);

impl PartialEq for NameSet {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.0.inner(), other.0.inner())
    }
}

impl Eq for NameSet {}

impl Value for NameSet {}

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
