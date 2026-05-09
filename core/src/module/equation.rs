use crate::module::PropertyId;

#[derive(Debug)]
pub struct Equation {
    pub properties: Box<[PropertyId]>,
}
