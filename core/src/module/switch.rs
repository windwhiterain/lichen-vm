use crate::module::{ExprId, Module};

#[derive(Debug)]
pub struct Switch {
    pub index: ExprId,
    pub branches: Box<[Module]>,
}
