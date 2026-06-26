use crate::{plugin::Project, runtime::NodeIdLocal};

use std::fmt::Debug;

#[derive(Debug, Clone, Copy)]
pub struct Operation<P: Project> {
    pub operand: NodeIdLocal,
    pub operator: P::Operator,
}

#[allow(type_alias_bounds)]
pub type Option<P: Project> = core::option::Option<Some<P>>;
pub enum Some<P: Project> {
    Value(P::Value),
    Ref(NodeIdLocal),
}
