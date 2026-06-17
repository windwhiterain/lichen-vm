use lichen_core::{
    ast::Ast as _,
    plugin::Value as _,
    runtime::evaluation::Evaluation,
    value::Array as CoreArray,
};
use lichen_utils::erase;

use crate::plugin::{Ast, Project};

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
        let type_array = CoreArray::new(
            ast.module_mut(),
            element.iter().map(|x| ast_static.r#type(x)),
        );
        let output_type = ast.r#type(output);
        *ast.module_mut().evaluation_mut(&output_type) =
            Evaluation::Value(P::Value::from_array(type_array))
    }
}
