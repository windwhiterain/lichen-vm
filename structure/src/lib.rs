use lichen_core::{
    property::{ExprId, Module},
    runtime::value::Value,
};

pub fn unit(module: &mut Module) -> ExprId {
    module.add_unit_expr(Value::Unit)
}
pub fn named_array() {}
