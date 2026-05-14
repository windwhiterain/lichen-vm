pub mod plugin;
pub mod property;
pub mod runtime;


pub mod plugin_define{
    pub use crate::runtime::{
        OperationId, StringId,
        value::{Array, Auto, Int, Table},
    };
    crate::plugin! {
        value{
            int: Int,
            string: StringId,
            array: Array,
            table: Table,
            auto: Auto,
            reference: OperationId,
        }{
            none,
            unit,
        }
    }
}

