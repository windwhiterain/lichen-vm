use std::collections::HashSet;

use crate::{
    plugin::{DiagnosticKind as _, Project, principal_traits::DiagnosticKind},
    runtime::{NodeIdLocal, diagnostic::Diagnostic, equation},
    value::Int,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexOutOfBounds {
    pub index: Int,
    pub len: usize,
}

impl<P: Project> DiagnosticKind<P> for IndexOutOfBounds {
    fn message(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "index {} out of bounds for length {}",
            self.index, self.len
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Unequality<P: Project> {
    pub expected: equation::Term<P>,
}

impl<P: Project> Unequality<P> {
    pub fn from_nodes(nodes: &[NodeIdLocal]) -> HashSet<Diagnostic<P>> {
        let mut ret = HashSet::new();
        for i in 0..nodes.len() {
            for j in (i + 1)..nodes.len() {
                ret.insert(Diagnostic {
                    kind: P::DiagnosticKind::unequality(Unequality {
                        expected: equation::Term::Node(nodes[i]),
                    }),
                    node: nodes[j],
                });
                ret.insert(Diagnostic {
                    kind: P::DiagnosticKind::unequality(Unequality {
                        expected: equation::Term::Node(nodes[j]),
                    }),
                    node: nodes[i],
                });
            }
        }
        ret
    }
}

impl<P: Project> DiagnosticKind<P> for Unequality<P> {
    fn message(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}
