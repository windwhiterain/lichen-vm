pub mod plugin;
pub mod property;
pub mod runtime;
pub mod value;


pub mod plugin_define{
    pub use crate::{
        value::{Array, Auto, Int, Table},
        runtime::{
            OperationId, StringId,     
        }
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

