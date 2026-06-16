use lichen_utils::erase;

use crate::{
    plugin::{Ast as _, Operator as _, Project, Value as _, expr, principal_traits::Ast as _},
    runtime::evaluation::Evaluation,
    value,
};

#[macro_export]
macro_rules! expr_impl {
    (Name: $Name:ident, name: $name:ident,trait<$project_variable:ident:$project_trait:path>: $trait:path, params: [$($param:ident,)*]) => {
        pub struct $Name;
        impl<$project_variable: $project_trait> $trait for $Name {
            fn build(ast:&mut $project_variable::Ast,output:&$crate::ast::ExprId,$($param: &$crate::ast::ExprId,)*) {
                let params = [$(ast.value($param),)*];
                let operand = $crate::value::Array::node(&mut ast.impl_mut().module, params);
                let output = ast.value(output);
                *ast.impl_mut().module.operation_mut(&output) = Some($crate::runtime::operation::Operation {
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
                let operand = ast.value($param);
                let output = ast.value(output);
                *ast.impl_mut().module.operation_mut(&output) = Some($crate::runtime::operation::Operation {
                    operand,
                    operator: P::Operator::$name(),
                });
            }
        }
    }
}

expr_impl! {Name: Sum, name: sum, trait<P:Project>: expr::sum<P>, param: addends}

expr_impl! {Name: Index, name: index, trait<P:Project>: expr::index<P>, params: [array,index,]}

expr_impl! {Name: Find, name: find, trait<P:Project>: expr::find<P>, params: [table,name,]}

pub struct Array;

impl<P: Project> expr::array<P> for Array {
    fn build(
        ast: &mut <P as Project>::Ast,
        output: &crate::ast::ExprId,
        element: &[crate::ast::ExprId],
    ) {
        let ast_static = unsafe { erase(ast) };
        let array = value::Array::new(
            &mut ast.impl_mut().module,
            element.iter().map(|x| ast_static.value(x)),
        );
        let output = ast.value(output);
        *ast.impl_mut().module.evaluation_mut(&output) =
            Evaluation::Value(P::Value::from_array(array))
    }
}
