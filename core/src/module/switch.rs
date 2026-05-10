use crate::module::{OperationId, Module};

#[derive(Debug)]
pub struct Switch {
    pub index: OperationId,
    pub branches: Box<[Module]>,
}
