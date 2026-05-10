use lichen_utils::arena::{array::ArenaArray, hashmap::ArenaHashMap};

use crate::module::{OperationId, Module, Ptr, StringId};

#[derive(Debug, Clone, Copy)]
pub enum Value {
    Int(i64),
    String(StringId),
    Array(Ptr<ArenaArray<OperationId>>),
    Table(Ptr<ArenaHashMap<StringId,usize>>),
    Auto { referrer_count: usize },
    Ref(OperationId),
    UnSolved,
}

impl Value {
    pub const AUTO: Self = Value::Auto { referrer_count: 1 };
    pub fn root(self) -> Value {
        match self {
            Value::Ref(operation_id) => Module::value(operation_id).root(),
            _ => self,
        }
    }
    pub fn root_mut(&mut self) -> &mut Value {
        match self {
            Value::Ref(operation_id) => Module::value_mut(*operation_id).root_mut(),
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
