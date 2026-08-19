use lichen_core::{
    ast::{Ast as _, ExprId}, plugin::{Ast as _, Value as _, Operator as _}, runtime::{evaluation::Evaluation, operation::Operation}, value::Tuple as ValueArray,
};
use lichen_utils::erase;

use crate::plugin::{Ast, Operator, Project};

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
        let type_array = ValueArray::new(
            ast.module_mut(),
            element.into_iter().map(|x| ast_static.r#type(x)),
        );
        let output_type = ast.r#type(output);
        *ast.module_mut().evaluation_mut(&output_type) =
            Evaluation::Value(P::Value::from_array(type_array))
    }
}

pub struct Index;

impl<P: Project> lichen_core::plugin::expr::index<P> for Index
where
    P::Ast: Ast<P>,
{
    fn build(
        ast: &mut P::Ast,
        output: &lichen_core::ast::ExprId,
        array: &lichen_core::ast::ExprId,
        index: &lichen_core::ast::ExprId,
    ) {
        let array_type = ast.r#type(array);
        let index_value = ast.get_value(index);
        let output_type = ast.r#type(output);
        let operand = ValueArray::node(ast.module_mut(), [array_type,index_value]);
        ast.module_mut().operation_mut(&output_type).replace(Operation{ operand, operator: P::Operator::index()});
    }
}
