use lichen_utils::arena::{array::ArenaArray};

use crate::module::{ExprId, Ptr};

pub enum Value {
    Int(i64),
    Array(Ptr<ArenaArray<ExprId>>),
}
