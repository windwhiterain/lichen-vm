use std::fmt::Debug;

use crate::{
    plugin::{Project, principal_traits::DiagnosticKind},
    runtime::NodeId,
};

#[derive(Debug)]
pub struct Diagnostic<P: Project> {
    pub kind: P::DiagnosticKind,
    pub node: NodeId,
}

#[derive(Debug, Clone, Copy)]
pub struct EqualityError {
    pub expected: NodeId,
}

impl<P: Project> DiagnosticKind<P> for EqualityError {
    fn message(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}
