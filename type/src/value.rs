use lichen_core::{plugin::principal_traits::Value, runtime::NodeId, value::Tuple};

#[macro_export]
macro_rules! primitive_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name;
        impl lichen_core::plugin::principal_traits::Value for $name {}
    };
}

primitive_type! {IntType}

primitive_type! {StringType}

primitive_type! {TableType}
