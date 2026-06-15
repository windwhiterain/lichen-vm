use lichen_core::{
    Ast as _,
    plugin::{Ast as _, Operator as _, principal_traits::Value},
    runtime::{
        Module, NodeIdLocal, StringId,
        operation::Operation,
        value::{Array, Table},
    },
};
use lichen_utils::arena::array::ArenaArray;

use crate::plugin::{Ast, Operator, Project};

pub mod operator;
pub mod plugin;

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

pub struct Member;

impl<P: Project> plugin::expr::member<P> for Member
where
    P::Ast: Ast<P>,
    P::Operator: Operator<P>,
{
    fn build(
        ast: &mut P::Ast,
        output: &lichen_core::ExprId,
        structure: &lichen_core::ExprId,
        name: &lichen_core::ExprId,
    ) {
        let structure_value = ast.value(structure);
        let structure_structure = ast.structure(structure);
        let name_value = ast.value(name);
        let output_value = ast.value(output);
        let output_structure = ast.structure(output);
        let operand = Array::node(
            &mut ast.impl_mut().module,
            [structure_structure, name_value],
        );
        let offset = ast.impl_mut().module.add_operation(Operation {
            operand,
            operator: P::Operator::offset(),
        });
        let operand = Array::node(&mut ast.impl_mut().module, [structure_value, offset]);
        *ast.impl_mut().module.operation_mut(&output_value) = Some(Operation {
            operand,
            operator: P::Operator::index(),
        });
        let operand = Array::node(&mut ast.impl_mut().module, [structure_structure, offset]);
        *ast.impl_mut().module.operation_mut(&output_structure) = Some(Operation {
            operand,
            operator: P::Operator::component(),
        });
    }
}
