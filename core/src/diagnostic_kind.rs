use std::collections::HashSet;

use crate::{
    plugin::{DiagnosticKind as _, Project, principal_traits::DiagnosticKind},
    runtime::{NodeIdLocal, diagnostic::Diagnostic},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EqualityError {
    pub expected: NodeIdLocal,
}

impl EqualityError {
    pub fn from_nodes<P: Project>(nodes: &[NodeIdLocal]) -> HashSet<Diagnostic<P>> {
        let mut ret = HashSet::new();
        for i in 0..nodes.len() {
            for j in (i + 1)..nodes.len() {
                ret.insert(Diagnostic {
                    kind: P::DiagnosticKind::from_equality_error(EqualityError {
                        expected: nodes[i],
                    }),
                    node: nodes[j],
                });
                ret.insert(Diagnostic {
                    kind: P::DiagnosticKind::from_equality_error(EqualityError {
                        expected: nodes[j],
                    }),
                    node: nodes[i],
                });
            }
        }
        ret
    }
}

impl<P: Project> DiagnosticKind<P> for EqualityError {
    fn message(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}
