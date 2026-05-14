use crate::runtime::OperationId;

#[derive(Debug)]
pub struct Equation {
    pub properties: Box<[OperationId]>,
}
