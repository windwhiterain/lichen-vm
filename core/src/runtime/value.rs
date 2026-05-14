use lichen_utils::arena::{array::ArenaArray, hashmap::ArenaHashMap};

use crate::{
    plugin_define::Value,
    runtime::{Module, OperationId, Ptr, StringId},
};

pub type Int = i64;
pub type Array = Ptr<ArenaArray<OperationId>>;
pub type Table = Ptr<ArenaHashMap<StringId, usize>>;

#[derive(Debug, Clone, Copy)]
pub struct Auto {
    pub referrer_count: usize,
}

pub fn root<V: Value>(value: V) -> V {
    if let Some(operation_id) = value.reference() {
        root(Module::<V>::value(operation_id))
    } else {
        value
    }
}

pub fn root_mut<V: Value>(value: &mut V) -> &mut V {
    if let Some(operation_id) = value.reference() {
        root_mut(Module::<V>::value_mut(operation_id))
    } else {
        value
    }
}

pub fn solve_order<V: Value>(value: V) -> (usize, usize) {
    if let Some(auto) = value.auto() {
        (1, auto.referrer_count)
    } else if value.none() {
        (0, 0)
    } else {
        (2, 0)
    }
}

impl Auto {
    pub fn new() -> Self {
        Self { referrer_count: 1 }
    }
}
