pub mod plugin;
pub mod property;
pub mod runtime;
pub mod value;


pub mod plugin_define{
    pub use crate::{
        value::{Array, Int, Table},
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
        }{
            unit,
        }
        operation{
            sum:crate::runtime::operation::sum,
            index:crate::runtime::operation::index,
            find:crate::runtime::operation::find,
        }
    }
}

