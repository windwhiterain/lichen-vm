use crate::{
    plugin::{Ast as _, Operator as _, Project},
    runtime::{Module, NodeId, NodeIdLocal, operation::Operation},
};

pub mod plugin;
pub mod property;
pub mod runtime;

#[derive(Debug, Clone, Copy)]
pub struct ExprId(usize);

pub trait Ast<'a, P: Project> {
    const PROPERTIES_LEN: usize;
    fn impl_(&self) -> &AstImpl<'a, P>;
    fn impl_mut(&mut self) -> &mut AstImpl<'a, P>;
    fn add_auto(&mut self) -> ExprId;
    fn add_entry(&mut self, expr: &ExprId);
}

pub struct AstImpl<'a, P: Project> {
    pub module: &'a mut Module<P>,
    pub expr_len: usize,
}

impl<'a, P: Project> AstImpl<'a, P> {
    pub fn new(module: &'a mut Module<P>) -> Self {
        Self {
            module,
            expr_len: 0,
        }
    }
    pub fn property(&self, expr: &ExprId, offset: usize) -> NodeIdLocal {
        NodeIdLocal(expr.0 * P::Ast::PROPERTIES_LEN + offset)
    }
    pub fn add_auto(&mut self) -> ExprId {
        let node = self.module.add_auto();
        debug_assert_eq!(node.0, self.expr_len * P::Ast::PROPERTIES_LEN);
        for _ in 1..P::Ast::PROPERTIES_LEN {
            self.module.add_auto();
        }
        let ret = ExprId(self.expr_len);
        self.expr_len += 1;
        ret
    }
    pub fn add_entry(&mut self, expr: &ExprId) {
        for i in 0..P::Ast::PROPERTIES_LEN {
            self.module.add_entry(self.property(&expr, i));
        }
    }
}

pub trait ExprImpl<P: Project> {
    fn build(ast: &mut P::Ast<'_>, input: &ExprId, output: &ExprId);
}

macro_rules! value_expr {
    ($expr_impl:ident,$operator:ident) => {
        pub struct $expr_impl;

        impl<P: Project> ExprImpl<P> for $expr_impl {
            fn build(ast: &mut P::Ast<'_>, input: &ExprId, output: &ExprId) {
                let output_value = ast.value(output);
                let input_value = ast.value(input);
                *ast.impl_mut().module.operation_mut(&output_value) = Some(Operation {
                    operand: input_value,
                    operator: P::Operator::$operator(),
                });
            }
        }
    };
}
value_expr! {Sum,sum}
value_expr! {Index,index}
value_expr! {Find,find}
