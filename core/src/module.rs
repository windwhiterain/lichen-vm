use lichen_utils::{arena::Arena, stable_vec::StableVec};

use crate::expr::Expr;

pub struct Module {
    pub arena: Arena,
    pub exprs: StableVec<Expr>,
}

pub struct ExprId(pub usize);
