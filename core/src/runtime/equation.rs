use crate::{
    plugin::Project,
    runtime::{NodeIdLocal, solve::LocalModuleId},
};

#[derive(Debug)]
pub struct Equation<P: Project> {
    pub nodes: Box<[Term<P>]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Term<P: Project> {
    Node(NodeIdLocal),
    Value(P::Value),
}
