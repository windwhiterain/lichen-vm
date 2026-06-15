use lichen_utils::erase;

use crate::{
    plugin::{Ast as _, Operator as _, Project},
    runtime::{Module, NodeIdLocal, operation::Operation, solve::LocalNodeId, value::Array},
};

pub mod plugin;
pub mod property;
pub mod runtime;

#[derive(Debug, Clone, Copy)]
pub struct ExprId(usize);

pub trait Ast<P: Project> {
    const PROPERTIES_COUNT: usize;
    fn impl_(&self) -> &AstImpl<P>;
    fn impl_mut(&mut self) -> &mut AstImpl<P>;
    fn add_auto(&mut self) -> ExprId;
    fn add_entry(&mut self, expr: &ExprId);
    fn property(&self, expr: &ExprId, offset: usize) -> NodeIdLocal;
}

impl<T: plugin::principal_traits::Ast<P>, P: Project> Ast<P> for T {
    const PROPERTIES_COUNT: usize = T::PROPERTIES_COUNT;

    fn impl_(&self) -> &AstImpl<P> {
        self.impl_()
    }

    fn impl_mut(&mut self) -> &mut AstImpl<P> {
        self.impl_mut()
    }

    fn add_auto(&mut self) -> ExprId {
        self.impl_mut().add_auto()
    }

    fn add_entry(&mut self, expr: &ExprId) {
        self.impl_mut().add_entry(expr);
    }

    fn property(&self, expr: &ExprId, offset: usize) -> NodeIdLocal {
        self.impl_().property(expr, offset)
    }
}

pub struct AstImpl<P: Project> {
    pub module: Module<P>,
}

impl<P: Project> AstImpl<P> {
    pub fn new(module: Module<P>) -> Self {
        Self { module }
    }
    pub fn property(&self, expr: &ExprId, offset: usize) -> NodeIdLocal {
        NodeIdLocal(expr.0 + offset)
    }
    pub fn add_auto(&mut self) -> ExprId {
        let node = self.module.add_auto();
        for _ in 1..P::Ast::PROPERTIES_COUNT {
            self.module.add_auto();
        }
        ExprId(node.0)
    }
    pub fn add_entry(&mut self, expr: &ExprId) {
        for i in 0..P::Ast::PROPERTIES_COUNT {
            self.module.add_entry(self.property(&expr, i));
        }
    }
}

#[macro_export]
macro_rules! expr_impl {
    (Name: $Name:ident, name: $name:ident,trait<$project_variable:ident:$project_trait:path>: $trait:path, params: [$($param:ident,)*]) => {
        pub struct $Name;
        impl<$project_variable: $project_trait> $trait for $Name {
            fn build(ast:&mut $project_variable::Ast,output:&$crate::ExprId,$($param: &$crate::ExprId,)*) {
                let params = [$(ast.value($param),)*];
                let operand = $crate::runtime::value::Array::node(&mut ast.impl_mut().module, params);
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
            fn build(ast:&mut $project_variable::Ast,output:&$crate::ExprId,$param: &$crate::ExprId) {
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

expr_impl! {Name: Sum, name: sum, trait<P:Project>: plugin::expr::sum<P>, param: addends}

expr_impl! {Name: Index, name: index, trait<P:Project>: plugin::expr::index<P>, params: [array,index,]}

expr_impl! {Name: Find, name: find, trait<P:Project>: plugin::expr::find<P>, params: [table,name,]}
