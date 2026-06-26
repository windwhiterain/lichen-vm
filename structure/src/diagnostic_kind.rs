use crate::plugin::Project;

use lichen_core::plugin::principal_traits::DiagnosticKind;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MemberNameRepetition {
    pub indices: (usize, usize),
}

impl<P: Project> DiagnosticKind<P> for MemberNameRepetition {
    fn message(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MemberNameMissing {
    pub index: usize,
}

impl<P: Project> DiagnosticKind<P> for MemberNameMissing {
    fn message(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}
