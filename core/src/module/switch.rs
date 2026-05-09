use crate::module::{ExprId, Module};

pub struct Switch {
    pub index: ExprId,
    pub branches: Box<[Module]>,
}
