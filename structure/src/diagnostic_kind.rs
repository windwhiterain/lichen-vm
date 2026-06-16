use crate::plugin::Project;

use lichen_core::{plugin::principal_traits::DiagnosticKind, value::Table};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberNameRepetition {
    pub table: Table,
    pub index_1: usize,
    pub index_2: usize,
}

impl std::hash::Hash for MemberNameRepetition {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.index_1.hash(state);
        self.index_2.hash(state);
    }
}

impl<P: Project> DiagnosticKind<P> for MemberNameRepetition {
    fn message(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "name at index {} repeats name at index {}",
            self.index_1, self.index_2,
        )
    }
}
