use std::fmt::Debug;

use crate::{plugin::Project, runtime::NodeId};

pub trait DiagnosticKind<P: Project>: Debug {
    // fn message(f: &mut std::fmt::Formatter<'_>, node: NodeId<P>) -> std::fmt::Result;
}

#[derive(Debug)]
pub struct Diagnostic<P: Project> {
    pub kind: P::DiagnosticKind,
    pub node: NodeId<P>,
}

#[derive(Debug, Clone, Copy)]
pub struct EqualityError(usize);
