use lichen_core::{
    ast::Ast as _,
    plugin::{Ast as _, Operator as _},
    runtime::operation::Operation,
    value::Array,
};

use crate::plugin::{Ast, Operator, Project};

pub struct Member;

impl<P: Project> crate::plugin::expr::member<P> for Member
where
    P::Ast: Ast<P>,
    P::Operator: Operator<P>,
{
    fn build(
        ast: &mut P::Ast,
        output: &lichen_core::ast::ExprId,
        structure: &lichen_core::ast::ExprId,
        name: &lichen_core::ast::ExprId,
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
