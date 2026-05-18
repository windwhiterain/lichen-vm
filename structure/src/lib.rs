use lichen_core::{
    plugin::Project,
    property::{ExprId, Module},
    runtime::{self, NodeId, NodeIdRaw, StringId},
};
use lichen_utils::arena::array::ArenaArray;

pub type NamedArray = ArenaArray<(StringId, NodeIdRaw)>;
pub fn new_named_array<P: Project>(
    module: &mut runtime::Module<P>,
    iter: impl Iterator<Item = (StringId, NodeIdRaw)>,
) -> NamedArray {
    ArenaArray::from_iter(&mut module.arena, iter)
}

pub fn construct<P: Project<Value: as_plugin::Value>>(
    operand: P::Value,
    node: NodeId<P>,
) -> Option<P::Value> {
    todo!()
}

::lichen_core::plugin! {
    value{
        named_array: crate::NamedArray,
    }{}
    operator{
        construct: crate::construct,
    }
}
