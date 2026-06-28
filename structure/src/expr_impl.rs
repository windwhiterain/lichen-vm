use lichen_core::{
    ast::Ast as _,
    plugin::{Ast as _, Operator as _, Value as _},
    runtime::{equation::LocalEquation, evaluation::Evaluation, operation::Operation},
    value::Array as CoreArray,
};
use lichen_utils::erase;

use crate::plugin::{Ast, Operator, Project, expr};

pub struct Member;

impl<P: Project> expr::member<P> for Member
where
    P::Ast: Ast<P>,
    P::Operator: Operator<P>,
{
    fn build(
        ast: &mut P::Ast,
        output: &lichen_core::ast::ExprId,
        instance: &lichen_core::ast::ExprId,
        name: &lichen_core::ast::ExprId,
    ) {
        let instance_value = ast.value(instance);
        let instance_structure = ast.structure(instance);
        let name_value = ast.value(name);
        let output_value = ast.value(output);
        let output_structure = ast.structure(output);
        let operand = CoreArray::node(ast.module_mut(), [instance_structure, name_value]);
        let offset = ast.module_mut().add_operation(Operation {
            operand,
            operator: P::Operator::offset(),
        });
        let operand = CoreArray::node(ast.module_mut(), [instance_value, offset]);
        ast.module_mut()
            .operation_mut(&output_value)
            .replace(Operation {
                operand,
                operator: P::Operator::index(),
            });
        let operand = CoreArray::node(ast.module_mut(), [instance_structure, offset]);
        ast.module_mut()
            .operation_mut(&output_structure)
            .replace(Operation {
                operand,
                operator: P::Operator::component(),
            });
    }
}

pub struct Compose;

impl<P: Project> expr::compose<P> for Compose
where
    P::Operator: Operator<P>,
    P::Ast: Ast<P>,
{
    fn build(
        ast: &mut <P>::Ast,
        output: &lichen_core::ast::ExprId,
        components: &lichen_core::ast::ExprId,
    ) {
        let components_value = ast.value(components);
        let output_value = ast.value(output);
        let output_structure = ast.structure(output);
        ast.module_mut()
            .operation_mut(&output_value)
            .replace(Operation {
                operand: components_value,
                operator: P::Operator::compose(),
            });
        *ast.module_mut().evaluation_mut(&output_structure) =
            Evaluation::Value(P::Value::from_unit());
    }
}

pub struct Construct;

impl<P: Project> expr::construct<P> for Construct
where
    P::Operator: Operator<P>,
    P::Ast: Ast<P>,
{
    fn build(
        ast: &mut <P>::Ast,
        output: &lichen_core::ast::ExprId,
        structure: &lichen_core::ast::ExprId,
        members: &lichen_core::ast::ExprId,
    ) {
        let structure_value = ast.value(structure);
        let members_value = ast.value(members);
        let output_value = ast.value(output);
        let output_structure = ast.structure(output);
        let operand = CoreArray::node(ast.module_mut(), [structure_value, members_value]);
        ast.module_mut()
            .operation_mut(&output_value)
            .replace(Operation {
                operand,
                operator: P::Operator::construct(),
            });
        ast.module_mut().add_equation(LocalEquation {
            nodes: Box::new([output_structure, structure_value]),
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
