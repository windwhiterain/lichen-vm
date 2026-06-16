use lichen_core::{
    ast::Ast as _,
    plugin::{Ast as _, Operator as _, Value as _},
    runtime::{evaluation::Evaluation, operation::Operation},
    value::Array as CoreArray,
};
use lichen_utils::erase;

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
        let operand = CoreArray::node(ast.module_mut(), [structure_structure, name_value]);
        let offset = ast.module_mut().add_operation(Operation {
            operand,
            operator: P::Operator::offset(),
        });
        let operand = CoreArray::node(ast.module_mut(), [structure_value, offset]);
        ast.module_mut()
            .operation_mut(&output_value)
            .replace(Operation {
                operand,
                operator: P::Operator::index(),
            });
        let operand = CoreArray::node(ast.module_mut(), [structure_structure, offset]);
        ast.module_mut()
            .operation_mut(&output_structure)
            .replace(Operation {
                operand,
                operator: P::Operator::component(),
            });
    }
}

pub struct Index;

impl<P: Project> lichen_core::plugin::expr::index<P> for Index
where
    P::Ast: Ast<P>,
    P::Operator: Operator<P>,
{
    fn build(
        ast: &mut P::Ast,
        output: &lichen_core::ast::ExprId,
        array: &lichen_core::ast::ExprId,
        index: &lichen_core::ast::ExprId,
    ) {
        let params = [ast.structure(array), ast.value(index)];
        let operand = CoreArray::node(ast.module_mut(), params);
        let output_structure = ast.structure(output);
        ast.module_mut()
            .operation_mut(&output_structure)
            .replace(Operation {
                operand,
                operator: P::Operator::index(),
            });
    }
}

pub struct Sum;

impl<P: Project> lichen_core::plugin::expr::sum<P> for Sum
where
    P::Ast: Ast<P>,
{
    fn build(
        ast: &mut P::Ast,
        output: &lichen_core::ast::ExprId,
        _addends: &lichen_core::ast::ExprId,
    ) {
        let output_structure = ast.structure(output);
        *ast.module_mut().evaluation_mut(&output_structure) =
            Evaluation::Value(P::Value::from_unit())
    }
}

pub struct Find;

impl<P: Project> lichen_core::plugin::expr::find<P> for Find
where
    P::Ast: Ast<P>,
{
    fn build(
        ast: &mut P::Ast,
        output: &lichen_core::ast::ExprId,
        _table: &lichen_core::ast::ExprId,
        _name: &lichen_core::ast::ExprId,
    ) {
        let output_structure = ast.structure(output);
        *ast.module_mut().evaluation_mut(&output_structure) =
            Evaluation::Value(P::Value::from_unit())
    }
}

pub struct Array;

impl<P: Project> lichen_core::plugin::expr::array<P> for Array
where
    P::Ast: Ast<P>,
{
    fn build(
        ast: &mut P::Ast,
        output: &lichen_core::ast::ExprId,
        element: &[lichen_core::ast::ExprId],
    ) {
        let ast_static = unsafe { erase(ast) };
        let array = CoreArray::new(
            ast.module_mut(),
            element.iter().map(|x| ast_static.structure(x)),
        );
        let output_structure = ast.structure(output);
        *ast.module_mut().evaluation_mut(&output_structure) =
            Evaluation::Value(P::Value::from_array(array))
    }
}
