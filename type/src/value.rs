use lichen_core::plugin::principal_traits::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntType;

impl Value for IntType {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StringType;

impl Value for StringType {}
