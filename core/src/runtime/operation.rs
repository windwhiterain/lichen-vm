use crate::{plugin::Project, runtime::NodeIdLocal};

use std::fmt::Debug;

#[derive(Debug, Clone, Copy)]
pub struct Operation<P: Project> {
    pub operand: NodeIdLocal,
    pub operator: P::Operator,
}
