//! The immutable lexical scope tree and the class summaries naming it.
//! See `docs/notes/apply-clone-ownership.md` §4.

use std::sync::Arc;

use crate::{FunctionId, Module, NodeId, Program};
use lichen_utils::disjoint;

/// The ancestor levels the lifting table keeps: enough for any depth a `u32`
/// can name.
const LEVELS: usize = 32;

/// A node of the immutable lexical scope tree.
///
/// # Invariant
/// A scope's ancestry never changes: the parent link is written once, when the
/// scope is created, and the lifting table is derived from it there. A scope
/// therefore names the same lexical place for as long as anything holds it.
/// The **owner** is the other half: a node whose owner is gone has no scope
/// and names the virtual root, so the class it belongs to is external.
#[derive(Debug)]
pub struct Scope<F: Copy + Eq + std::fmt::Debug> {
    /// The function this scope belongs to.
    function: F,
    /// The enclosing scope; `None` at top level.
    parent: Option<Arc<Scope<F>>>,
    /// The nesting depth: a top-level scope's depth is `0`.
    depth: u32,
    /// The `2^k`-th ancestor of this scope.
    ///
    /// # Invariant
    /// `up[0]` is the parent and `up[k]` is the `2^k`-th ancestor, so a jump
    /// of any length the depth allows is one table read. Every scope keeps the
    /// same number of levels, so two scopes of one depth can be lifted
    /// together.
    up: Vec<Option<Arc<Scope<F>>>>,
}

impl<F: Copy + Eq + std::fmt::Debug> Scope<F> {
    /// The scope of `function`, hanging under `parent`.
    ///
    /// # Invariant
    /// The lifting table is filled from `parent`'s own table, so the two
    /// agree on every shared ancestor.
    pub fn child(function: F, parent: Option<&Arc<Scope<F>>>) -> Arc<Scope<F>> {
        let depth = parent.map_or(0, |parent| parent.depth + 1);
        let mut up = Vec::with_capacity(LEVELS);
        up.push(parent.cloned());
        for level in 1..LEVELS {
            // The `2^level`-th ancestor is the `2^(level-1)`-th ancestor of the
            // `2^(level-1)`-th ancestor.
            let grand = up[level - 1]
                .as_ref()
                .and_then(|scope| scope.up[level - 1].clone());
            up.push(grand);
        }
        Arc::new(Scope {
            function,
            parent: parent.cloned(),
            depth,
            up,
        })
    }

    /// The function this scope belongs to.
    pub fn function(&self) -> F {
        self.function
    }

    /// The enclosing scope; `None` at top level.
    pub fn parent(&self) -> Option<&Arc<Scope<F>>> {
        self.parent.as_ref()
    }

    /// The nesting depth: a top-level scope's depth is `0`.
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Whether this scope is an ancestor-or-self of `other`.
    ///
    /// # Invariant
    /// The comparison is by the function a scope belongs to, so two chains
    /// that name the same functions answer the same way however they were
    /// built.
    pub fn is_ancestor_or_self(self: &Arc<Scope<F>>, other: &Arc<Scope<F>>) -> bool {
        if self.depth > other.depth {
            return false;
        }
        let mut current = other.clone();
        let mut difference = other.depth - self.depth;
        let mut level = 0;
        while difference > 0 {
            if difference & 1 == 1 {
                current = current.up[level]
                    .clone()
                    .expect("the lifting table covers the scope's depth");
            }
            difference >>= 1;
            level += 1;
        }
        current.function == self.function
    }

    /// The nearest common ancestor of two scopes; `None` when they are in
    /// different trees.
    ///
    /// # Invariant
    /// The result names a scope both are under, and no deeper scope is.
    pub fn lca(left: &Arc<Scope<F>>, right: &Arc<Scope<F>>) -> Option<Arc<Scope<F>>> {
        let mut x = left.clone();
        let mut y = right.clone();
        if x.depth < y.depth {
            std::mem::swap(&mut x, &mut y);
        }
        // The deeper scope climbs to the shallower one's depth.
        let mut difference = x.depth - y.depth;
        let mut level = 0;
        while difference > 0 {
            if difference & 1 == 1 {
                x = x.up[level].clone()?;
            }
            difference >>= 1;
            level += 1;
        }
        if x.function == y.function {
            return Some(x);
        }
        // Both climb together, keeping them apart, until their parents agree.
        for level in (0..LEVELS).rev() {
            let (Some(xa), Some(ya)) = (&x.up[level], &y.up[level]) else {
                continue;
            };
            if xa.function != ya.function {
                x = xa.clone();
                y = ya.clone();
            }
        }
        x.up[0].clone()
    }
}

/// The summary of two class summaries: the nearest common ancestor of the
/// scopes they name.
///
/// # Invariant
/// The virtual root — the summary of a class with an unowned or invalid owner
/// — absorbs every other summary: a class that reaches outside every scope
/// reaches outside the scope of any one of them.
pub(crate) fn merge_summaries<F: Copy + Eq + std::fmt::Debug>(
    left: Option<Arc<Scope<F>>>,
    right: Option<Arc<Scope<F>>>,
) -> Option<Arc<Scope<F>>> {
    match (left, right) {
        (None, _) | (_, None) => None,
        (Some(left), Some(right)) => Scope::lca(&left, &right),
    }
}

impl<P: Program> Module<P> {
    /// The scope of `function`, or `None` when it has none.
    pub fn function_scope(&self, function: FunctionId) -> Option<Arc<Scope<FunctionId>>> {
        self.functions
            .get(function)
            .and_then(|entry| entry.scope.clone())
    }

    /// The scope the node's **owner** names, or `None` for the virtual root.
    ///
    /// # Invariant
    /// A node with no owner, or one whose owner has been collected, names the
    /// virtual root: it is outside every function's scope, so the class it
    /// belongs to is referenced rather than cloned.
    fn owner_scope(&self, node: NodeId) -> Option<Arc<Scope<FunctionId>>> {
        let owner = self.nodes[node].function?;
        self.function_scope(owner)
    }

    /// The scope summary of the class `node` belongs to.
    ///
    /// # Invariant
    /// The summary is the nearest common ancestor of the owner scopes of every
    /// member of the class, and it is held by the class's representative.
    /// `None` is the virtual root: the class reaches outside every scope.
    pub fn class_scope_summary(&self, node: NodeId) -> Option<Arc<Scope<FunctionId>>> {
        self.nodes.get(self.class_root(node))?.scope_summary.clone()
    }

    /// Whether every member of the class `node` belongs to is inside `scope`.
    ///
    /// # Invariant
    /// The verdict is a fact of the **class**, not of the node: every member
    /// of one class reads the same answer, so a clone never splits a class.
    pub fn class_inside_scope(&self, node: NodeId, scope: &Arc<Scope<FunctionId>>) -> bool {
        let Some(summary) = self.class_scope_summary(node) else {
            return false; // the virtual root is inside no scope
        };
        scope.is_ancestor_or_self(&summary)
    }

    /// Recompute the class's summary from its members; the one writer of it.
    ///
    /// # Invariant
    /// The summary is the nearest common ancestor of the members' owner
    /// scopes, so a member whose owner changes cannot leave a stale summary
    /// behind.
    pub(crate) fn refresh_class_summary(&mut self, node: NodeId) {
        let root = disjoint::find(&mut self.nodes, node);
        let next = self.nodes[root].equality.next();
        if next.is_none() {
            // A lone member: its own owner is the whole summary.
            let summary = self.owner_scope(root);
            self.nodes[root].scope_summary = summary;
            return;
        }
        let mut summary: Option<Arc<Scope<FunctionId>>> = None;
        let mut seen = false;
        let mut member = Some(root);
        while let Some(current) = member {
            member = self.nodes[current].equality.next();
            let owner = self.owner_scope(current);
            summary = if seen {
                merge_summaries(summary, owner)
            } else {
                seen = true;
                owner
            };
        }
        self.nodes[root].scope_summary = summary;
    }

    /// The one write of a node's **owner** tag.
    ///
    /// # Invariant
    /// The owner and the class's summary are written together: a member whose
    /// owner changes moves its class's summary with it.
    pub(crate) fn set_node_owner(&mut self, node: NodeId, owner: Option<FunctionId>) {
        self.nodes[node].function = owner;
        self.refresh_class_summary(node);
    }

    /// Merge two classes' scope summaries; the write side of a union.
    ///
    /// # Invariant
    /// The merged class's summary is the nearest common ancestor of the two
    /// classes' summaries, which is the nearest common ancestor of every
    /// member's owner scope.
    pub(crate) fn merge_class_summaries(
        &mut self,
        left: NodeId,
        right: NodeId,
        representative: NodeId,
    ) {
        let left = self.nodes[left].scope_summary.clone();
        let right = self.nodes[right].scope_summary.clone();
        self.nodes[representative].scope_summary = merge_summaries(left, right);
    }
}
