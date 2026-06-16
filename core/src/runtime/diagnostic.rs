use std::fmt::Debug;

use crate::{plugin::Project, runtime::NodeIdLocal};

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic<P: Project> {
    pub kind: P::DiagnosticKind,
    pub node: NodeIdLocal,
}
