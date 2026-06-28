use lichen_utils::erase_mut;

use crate::{
    plugin::Project,
    runtime::{Module, NodeIdLocal},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evaluation<P: Project> {
    Value(P::Value),
    /// linked list node
    Ref {
        node: NodeIdLocal,
        brother: Option<NodeIdLocal>,
    },
    Auto {
        referrer_count: usize,
        /// (head, tail) of referer (include indirect) linked list
        referers: Option<(NodeIdLocal, NodeIdLocal)>,
    },
}

impl<P: Project> Module<P> {
    /// include self
    pub fn referers(&self, node: &NodeIdLocal) -> impl Iterator<Item = NodeIdLocal> {
        std::iter::successors(Some(*node), |node| {
            let evaluation = self.evaluation(&node);
            if let Evaluation::Auto { referers, .. } = evaluation {
                referers.map(|x| x.0)
            } else {
                None
            }
        })
    }
    /// #Panic
    /// - `node` must has [`Evaluation::Auto`]
    /// - `target` must not has [`Evaluation::Ref`]
    pub fn set_ref(&mut self, node: &NodeIdLocal, target: &NodeIdLocal) {
        debug_assert_ne!(node, target);
        let evaluation = unsafe { erase_mut(self.evaluation_mut(node)) };
        let Evaluation::Auto {
            referrer_count: self_referrer_count,
            referers: self_reference,
            ..
        } = *evaluation
        else {
            panic!()
        };
        let (referrer_count, referers) = match unsafe { erase_mut(self.evaluation_mut(target)) } {
            Evaluation::Value { .. } => {
                *evaluation = Evaluation::Ref {
                    node: *target,
                    brother: None,
                };
                return;
            }
            Evaluation::Ref { .. } => panic!(),
            Evaluation::Auto {
                referrer_count,
                referers,
            } => (referrer_count, referers),
        };
        *referrer_count += self_referrer_count;
        if let Some(self_reference) = self_reference {
            if let Some(reference) = referers {
                let Evaluation::Ref { brother, .. } = self.evaluation_mut(&self_reference.1) else {
                    unreachable!()
                };
                *brother = Some(reference.0);
                reference.0 = *node;
            }
            *evaluation = Evaluation::Ref {
                node: *target,
                brother: Some(self_reference.0),
            };
        } else {
            if let Some(reference) = referers {
                *evaluation = Evaluation::Ref {
                    node: *target,
                    brother: Some(reference.0),
                };
                reference.0 = *node;
            } else {
                *evaluation = Evaluation::Ref {
                    node: *target,
                    brother: None,
                };
            }
        }
    }
}
