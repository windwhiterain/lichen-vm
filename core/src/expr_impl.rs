use lichen_utils::erase;

use crate::{
    ast::{Ast as _, ExprId},
    operator,
    plugin::{Ast as _, Operator as _, Project, Value as _, expr},
    runtime::{NodeIdLocal, evaluation::Evaluation, operation::Operation},
    value::{self},
};

macro_rules! expr_impl {
    (Name: $Name:ident, name: $name:ident,trait<$project_variable:ident:$project_trait:path>: $trait:path, params: [$($param:ident,)*]) => {
        pub struct $Name;
        impl<$project_variable: $project_trait> $trait for $Name {
            fn build(ast:&mut $project_variable::Ast,output:&$crate::ast::ExprId,$($param: &$crate::ast::ExprId,)*) {
                let params = [$(ast.get_value($param),)*];
                let operand = $crate::value::Tuple::node(ast.module_mut(), params);
                let output = ast.get_value(output);
                ast.module_mut().operation_mut(&output).replace($crate::runtime::operation::Operation {
                    operand,
                    operator: P::Operator::$name(),
                });
            }
        }
    };
    (Name: $Name:ident, name: $name:ident, trait<$project_variable:ident:$project_trait:path>: $trait:path, param: $param:ident) => {
        pub struct $Name;
        impl<$project_variable: $project_trait> $trait for $Name {
            fn build(ast:&mut $project_variable::Ast,output:&$crate::ast::ExprId,$param: &$crate::ast::ExprId) {
                let operand = ast.get_value($param);
                let output = ast.get_value(output);
                ast.module_mut().operation_mut(&output).replace($crate::runtime::operation::Operation {
                    operand,
                    operator: P::Operator::$name(),
                });
            }
        }
    }
}

expr_impl! {Name: Sum, name: sum, trait<P:Project>: expr::sum<P>, param: addends}

expr_impl! {Name: Find, name: find, trait<P:Project>: expr::find<P>, params: [table,name,]}

pub struct Tuple;

impl<P: Project> expr::tuple<P> for Tuple {
    fn build<'a>(
        ast: &mut <P as Project>::Ast,
        output: &crate::ast::ExprId,
        items: impl IntoIterator<Item = &'a ExprId> + Copy,
    ) {
        let ast_static = unsafe { erase(ast) };
        for i in 0..P::Ast::PROPERTIES_COUNT {
            let array = value::Tuple::new(
                ast.module_mut(),
                items
                    .into_iter()
                    .map(|expr| ast_static.get_property(expr, i)),
            );
            let output_property = ast_static.get_property(output, i);
            *ast.module_mut().evaluation_mut(&output_property) =
                Evaluation::Value(P::Value::tuple(array));
        }
    }
}

pub struct Index;

impl<P: Project> expr::index<P> for Index {
    fn build(
        ast: &mut <P as Project>::Ast,
        output: &crate::ast::ExprId,
        array: &crate::ast::ExprId,
        index: &crate::ast::ExprId,
    ) {
        let index_value = ast.get_value(index);
        for i in 0..P::Ast::PROPERTIES_COUNT {
            let array_property = ast.get_property(array, i);
            let output_property = ast.get_property(output, i);
            let operand = value::Tuple::node(ast.module_mut(), [array_property, index_value]);
            ast.module_mut()
                .operation_mut(&output_property)
                .replace(Operation {
                    operand,
                    operator: P::Operator::index(operator::Index::default()),
                });
        }
    }
}
