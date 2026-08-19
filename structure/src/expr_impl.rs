use lichen_core::{
    ast::{Ast as _, ExprId}, expr_impl::Index, plugin::{Ast as _, Operator as _, Value as _, expr::index}, runtime::{equation::{self, Equation}, evaluation::Evaluation, operation::Operation}, value::Tuple as ValueArray,
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
        let instance_structure = ast.structure(instance);
        let name_value = ast.get_value(name);
        let operand = ValueArray::node(ast.module_mut(), [instance_structure, name_value]);
        let offset = ast.module_mut().add_operation(Operation {
            operand,
            operator: P::Operator::find(),
        });
        Index::build::<P>(ast, output, instance, offset);
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
        let name_set_value = ast.get_value(name_set);
        let structures_value = ast.get_value(structures);
        let output_value = ast.get_value(output);
        let output_structure = ast.structure(output);
        let operand = ValueArray::node(ast.module_mut(), [name_set_value, structures_value]);
        ast.module_mut()
            .operation_mut(&output_value)
            .replace(Operation {
                operand,
                operator: P::Operator::compose(),
            });
        *ast.module_mut().evaluation_mut(&output_structure) =
            Evaluation::Value(P::Value::unit());
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
        let structure_value = ast.get_value(structure);
        let name_set_value = ast.get_value(name_set);
        let members_value = ast.get_value(members);
        let members_structure = ast.structure(members);
        let output_value = ast.get_value(output);
        let output_structure = ast.structure(output);
        let operand = ValueArray::node(ast.module_mut(), [structure_value, name_set_value]);
        let offsets = ast.module_mut().add_operation(Operation {
            operand,
            operator: P::Operator::r#match(),
        });
        let operand = ValueArray::node(ast.module_mut(), [offsets, members_value]);
        ast.module_mut()
            .operation_mut(&output_value)
            .replace(Operation {
                operand,
                operator: P::Operator::transform(),
            });
        ast.module_mut().add_equation(Equation {
            nodes: Box::new([equation::Term::Node(output_structure), equation::Term::Node(structure_value)]),
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
            Evaluation::Value(P::Value::unit())
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
