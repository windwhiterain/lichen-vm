use std::fmt::Debug;

use crate::{plugin::{Project, principal_traits::DiagnosticKind}, runtime::NodeId};

#[derive(Debug)]
pub struct Diagnostic<P: Project> {
    pub kind: P::DiagnosticKind,
    pub node: NodeId<P>,
}

#[derive(Debug, Clone, Copy)]
pub struct EqualityError<P:Project>{pub expected:NodeId<P>}

impl<P:Project> DiagnosticKind<P> for EqualityError<P>{
    fn message(& self,f: &mut std::fmt::Formatter<'_>,)->std::fmt::Result {
        todo!()
    }
}
