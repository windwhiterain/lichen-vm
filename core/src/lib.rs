pub mod plugin;
pub mod property;
pub mod runtime;

pub mod plugin_define {
    pub use crate::runtime::{
        NodeId, StringId,
        value::{Array, Int, Table},
    };
    crate::plugin! {
        value{
            int: Int,
            string: StringId,
            array: Array,
            table: Table,
        }{
            unit,
        }
        operator{
            sum:crate::runtime::operation::sum,
            index:crate::runtime::operation::index,
            find:crate::runtime::operation::find,
        }
    }
}
