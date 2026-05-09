use std::collections::HashMap;

use lichen_utils::{arena::Arena, stable_vec::StableVec};

use crate::{expr::Expr, value::Value};

pub struct Module {
    pub arena: Arena,
    pub exprs: StableVec<Expr>,
    pub property_values: StableVec<Value>,
    pub properties: HashMap<StringId,usize>,
}

pub struct Ptr<T>(pub *const T);
pub struct ExprId{
    pub module: usize,
    pub local: usize,
}
pub struct StringId(pub usize);
