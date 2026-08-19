use lichen_utils::erase;

use crate::{
    operator::Index, plugin::{Operator as _, Project, Value as _}, runtime::{Module, NodeIdLocal, operation::Operation}, value::Tuple,
};

#[derive(Debug, Clone, Copy)]
pub struct ExprId(pub NodeIdLocal);

pub trait Ast<P: Project> {
    const PROPERTIES_COUNT: usize;
    const METADATAS_COUNTS: &'static [usize];
    fn module(&self) -> &Module<P>;
    fn module_mut(&mut self) -> &mut Module<P>;
    fn add_auto(&mut self) -> ExprId;
    fn add_entry(&mut self, expr: &ExprId);
    fn get_property(&self, expr: &ExprId, index: usize) -> NodeIdLocal;
    fn get_property_dynamic(&mut self, expr: &NodeIdLocal, index: usize) -> NodeIdLocal;
}

impl<T: crate::plugin::principal_traits::Ast<P>, P: Project> Ast<P> for T {
    const PROPERTIES_COUNT: usize = T::PROPERTIES_COUNT;
    const METADATAS_COUNTS: &'static [usize] = T::METADATAS_COUNTS;

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

    fn get_property(&self, expr: &ExprId, index: usize) -> NodeIdLocal {
        self.impl_().get_property(expr, index)
    }

    fn get_property_dynamic(&mut self, expr: &NodeIdLocal, index: usize) -> NodeIdLocal {
        self.impl_mut().get_property_dynamic(expr, index)
    }
}

pub struct AstImpl<P: Project> {
    pub module: Module<P>,
}

impl<P: Project> AstImpl<P> {
    pub fn new(module: Module<P>) -> Self {
        Self { module }
    }
    pub fn get_property_uninit<'a>(&'a self,expr: &'a mut Tuple, index: usize) -> &'a mut NodeIdLocal{
        expr.0.get_mut(index).unwrap()
    }
    pub fn get_property(&self, expr: &ExprId, index: usize) -> NodeIdLocal {
        let tuple = self.module.assert_value(&expr.0).as_tuple().unwrap();
        *tuple.0.get(index).unwrap()
    }
    pub fn get_property_dynamic(&mut self, expr: &NodeIdLocal, index: usize) -> NodeIdLocal {
        self.module.add_operation(Operation{ operand: *expr, operator: P::Operator::index(Index::Static { index })})
    }
    pub fn add_uninit(&mut self) -> Tuple {
        Tuple::uninit(&mut self.module, P::Ast::PROPERTIES_COUNT)
    }
    pub fn init(&mut self, expr: Tuple) -> ExprId {
        ExprId(self.module.add_literal(P::Value::tuple(expr)))
    }
    pub fn add_auto(&mut self) -> ExprId {
        let mut tuple = self.add_uninit();
        for i in 0..P::Ast::PROPERTIES_COUNT {
            *tuple.0.get_mut(i).unwrap() = self.module.add_auto();
        }
        ExprId(self.module.add_literal(P::Value::tuple(tuple)))
    }
    pub fn add_entry(&mut self, expr: &ExprId) {
        let tuple = unsafe{erase(self).module.assert_value(&expr.0).as_tuple().unwrap()};
        for i in 0..P::Ast::PROPERTIES_COUNT {
            self.module.add_entry(*tuple.0.get(i).unwrap());
        }
    }
}
