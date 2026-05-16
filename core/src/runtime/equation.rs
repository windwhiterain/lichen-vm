use crate::{plugin::Project, runtime::NodeId};

#[derive(Debug)]
pub struct Equation<P: Project> {
    pub nodes: Box<[NodeId<P>]>,
}
