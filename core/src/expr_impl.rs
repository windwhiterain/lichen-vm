use crate::plugin::{Ast as _, Operator as _, Project, principal_traits::Ast as _};

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

expr_impl! {Name: Sum, name: sum, trait<P:Project>: crate::plugin::expr::sum<P>, param: addends}

expr_impl! {Name: Index, name: index, trait<P:Project>: crate::plugin::expr::index<P>, params: [array,index,]}

expr_impl! {Name: Find, name: find, trait<P:Project>: crate::plugin::expr::find<P>, params: [table,name,]}
