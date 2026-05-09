use lichen_utils::arena::array::ArenaArray;

use crate::module::{ExprId, Module, PropertyId, Ptr};

#[derive(Debug, Clone, Copy)]
pub enum Value {
    Int(i64),
    Array(Ptr<ArenaArray<ExprId>>),
    Auto { referrer_count: usize },
    Ref { property: PropertyId },
    UnSolved,
}

impl Value {
    pub const AUTO: Self = Value::Auto { referrer_count: 1 };
    pub fn root(self) -> Value {
        match self {
            Value::Ref { property } => Module::property_value(property).root(),
            _ => self,
        }
    }
    pub fn root_mut(&mut self) -> &mut Value {
        match self {
            Value::Ref { property } => Module::property_value_mut(*property).root_mut(),
            _ => self,
        }
    }
    pub fn solve_order(self) -> (usize, usize) {
        match self.root() {
            Value::Auto { referrer_count } => (1, referrer_count),
            Value::UnSolved => (0, 0),
            _ => (2, 0),
        }
    }
}
