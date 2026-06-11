use std::marker::PhantomData;

use crate::{plugin::Project, runtime::{Module, NodeId, NodeIdLocal}};

pub mod plugin;
pub mod property;
pub mod runtime;

pub struct ExprId(usize);

pub trait Ast {
    const PROPERTIES_LEN: usize;
}

pub struct AstImpl<'a, P:Project>{
    module: &'a mut Module<P>
}

impl<P:Project> AstImpl<'_,P>{
    pub fn property(&self,expr:&ExprId,offset:usize)->NodeIdLocal{
        NodeIdLocal(expr.0*P::Ast::PROPERTIES_LEN + offset)
    }
}

pub trait ExprImpl<P:Project>{
    fn build(ast:&mut P::Ast,input: &ExprId,output:&ExprId);
}

pub struct Sum;

impl<P:Project> ExprImpl<P> for Sum{
    fn build(ast:&mut P::Ast,input: &ExprId,output:&ExprId) {
        
    }
}