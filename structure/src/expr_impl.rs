use lichen_core::{
    ast::{Ast as _, ExprId},
    plugin::{Ast as _, Operator as _, Value as _},
    runtime::{equation::LocalEquation, evaluation::Evaluation, operation::Operation},
    value::Array as ValueArray,
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
        let operand = ValueArray::node(ast.module_mut(), [instance_structure, name_value]);
        let offset = ast.module_mut().add_operation(Operation {
            operand,
            operator: P::Operator::offset(),
        });
        let operand = ValueArray::node(ast.module_mut(), [instance_value, offset]);
        ast.module_mut()
            .operation_mut(&output_value)
            .replace(Operation {
                operand,
                operator: P::Operator::index(),
            });
        let operand = ValueArray::node(ast.module_mut(), [instance_structure, offset]);
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
        name_set: &lichen_core::ast::ExprId,
        structures: &lichen_core::ast::ExprId,
    ) {
        let name_set_value = ast.value(name_set);
        let structures_value = ast.value(structures);
        let output_value = ast.value(output);
        let output_structure = ast.structure(output);
        let operand = ValueArray::node(ast.module_mut(), [name_set_value, structures_value]);
        ast.module_mut()
            .operation_mut(&output_value)
            .replace(Operation {
                operand,
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
        name_set: &lichen_core::ast::ExprId,
        members: &lichen_core::ast::ExprId,
    ) {
        let structure_value = ast.value(structure);
        let name_set_value = ast.value(name_set);
        let members_value = ast.value(members);
        let output_value = ast.value(output);
        let output_structure = ast.structure(output);
        let operand = ValueArray::node(ast.module_mut(), [structure_value, name_set_value]);
        let layout = ast.module_mut().add_operation(Operation {
            operand,
            operator: P::Operator::r#match(),
        });
        let operand = ValueArray::node(ast.module_mut(), [layout, members_value]);
        ast.module_mut()
            .operation_mut(&output_value)
            .replace(Operation {
                operand,
                operator: P::Operator::transform(),
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
        let operand = ValueArray::node(ast.module_mut(), params);
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
    fn build<'a>(
        ast: &mut P::Ast,
        output: &lichen_core::ast::ExprId,
        element: impl IntoIterator<Item = &'a ExprId> + Copy,
    ) {
        let ast_static = unsafe { erase(ast) };
        let array = ValueArray::new(
            ast.module_mut(),
            element.into_iter().map(|x| ast_static.structure(x)),
        );
        let output_structure = ast.structure(output);
        *ast.module_mut().evaluation_mut(&output_structure) =
            Evaluation::Value(P::Value::from_array(array))
    }
}
