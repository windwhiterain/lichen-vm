use crate::runtime::{NodeIdLocal, solve::LocalModuleId};

#[derive(Debug)]
pub struct LocalEquation {
    pub nodes: Box<[NodeIdLocal]>,
}

#[derive(Debug)]
pub struct Equation {
    pub module: LocalModuleId,
    pub nodes: Box<[NodeIdLocal]>,
}
