use crate::{
    plugin::Project,
    runtime::{Module, NodeIdLocal},
};

#[derive(Debug, Clone, Copy)]
pub struct ExprId(usize);

pub trait Ast<P: Project> {
    const PROPERTIES_COUNT: usize;
    fn module(&self) -> &Module<P>;
    fn module_mut(&mut self) -> &mut Module<P>;
    fn add_auto(&mut self) -> ExprId;
    fn add_entry(&mut self, expr: &ExprId);
    fn property(&self, expr: &ExprId, offset: usize) -> NodeIdLocal;
}

impl<T: crate::plugin::principal_traits::Ast<P>, P: Project> Ast<P> for T {
    const PROPERTIES_COUNT: usize = T::PROPERTIES_COUNT;

    fn module(&self) -> &Module<P> {
        &self.impl_().module
    }

    fn module_mut(&mut self) -> &mut Module<P> {
        &mut self.impl_mut().module
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
