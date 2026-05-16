use crate::{plugin::Project, runtime::OperationId};

#[derive(Debug)]
pub struct Equation<P:Project> {
    pub operation_ids: Box<[OperationId<P>]>,
}
