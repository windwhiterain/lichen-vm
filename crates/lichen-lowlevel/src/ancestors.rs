//! The recursion-path guards the structural walks share.
//!
//! Several walks descend a node graph and must cut a cycle instead of looping:
//! the unification comparison ([`crate::equality`]), the reconciliation of a
//! forced computation against the value its class committed, the table key
//! comparison ([`crate::table`]), and the type/value printers in
//! `lichen-render`.  Each asks whether something is on the **current recursion
//! path**, and that question is an *ancestor* relation, not a visited mark: a
//! node or pair the walk meets again in a sibling subtree is not on the path
//! and must be visited again, so membership is removed on the way out.
//!
//! The paths used to be `Vec`s probed with a linear `contains`, which is one
//! scan per recursion level — quadratic in the depth of the walk.  What the
//! guard accepts is distinct (the caller tests before it inserts), so a set
//! answers the same question with the same insert/remove discipline:
//! [`AncestorNodes`] for a walk whose cycle is one node meeting itself again,
//! [`AncestorPairs`] for a comparison whose cycle is a pair meeting itself
//! again.

use std::collections::HashSet;
use std::hash::Hash;

/// The nodes on the current recursion path — the one-node guard.
///
/// The walk's cycle is a node that reaches itself, so the
/// test is whether the node is already on the path.  Its pair form is
/// [`AncestorPairs`]; the two share this set so the discipline below is
/// written once.
///
/// # Contract
/// - A node [`insert`](Self::insert)ed by a frame must be
///   [`remove`](Self::remove)d by that same frame on every exit, including
///   the early returns; a node left behind cuts a visit in a sibling subtree
///   that the path relation does not cover.
/// - A node is inserted only after [`contains`](Self::contains) answered
///   `false` for it, so the nodes on the path are distinct.
#[doc(hidden)]
pub struct AncestorNodes<K> {
    nodes: HashSet<K>,
}

impl<K> Default for AncestorNodes<K> {
    fn default() -> Self {
        Self {
            nodes: HashSet::new(),
        }
    }
}

impl<K: Copy + Eq + Hash> AncestorNodes<K> {
    pub fn new() -> Self {
        Self {
            nodes: HashSet::new(),
        }
    }

    /// Whether `node` is on the current path — the same test
    /// `path.contains(&node)` performed.
    pub fn contains(&self, node: K) -> bool {
        self.nodes.contains(&node)
    }

    /// Put `node` on the path.
    pub fn insert(&mut self, node: K) {
        self.nodes.insert(node);
    }

    /// Take `node` off the path.  Must name the node that was
    /// [`insert`](Self::insert)ed.
    pub fn remove(&mut self, node: K) {
        self.nodes.remove(&node);
    }
}

/// The unordered node pairs on the current recursion path.
///
/// # Contract
/// - A pair [`insert`](Self::insert)ed by a frame must be
///   [`remove`](Self::remove)d by that same frame on every exit, including
///   the early returns; a pair left behind cuts a comparison in a sibling
///   subtree that the path relation does not cover.
/// - A pair is inserted only after [`contains`](Self::contains) answered
///   `false` for it, so the pairs on the path are distinct.
#[doc(hidden)]
pub struct AncestorPairs<K> {
    pairs: AncestorNodes<(K, K)>,
}

impl<K> Default for AncestorPairs<K> {
    fn default() -> Self {
        Self {
            pairs: AncestorNodes::default(),
        }
    }
}

impl<K: Copy + Eq + Hash> AncestorPairs<K> {
    pub fn new() -> Self {
        Self {
            pairs: AncestorNodes::new(),
        }
    }

    /// Whether the unordered pair `{a, b}` is on the current path — the same
    /// test `path.contains(&(a, b)) || path.contains(&(b, a))` performed.
    pub fn contains(&self, a: K, b: K) -> bool {
        self.pairs.contains((a, b))
    }

    /// Put `{a, b}` on the path.  Both orientations are stored, which is what
    /// makes [`Self::contains`]'s single probe answer the symmetric test.
    pub fn insert(&mut self, a: K, b: K) {
        self.pairs.insert((a, b));
        self.pairs.insert((b, a));
    }

    /// Take `{a, b}` off the path.  Must name the same pair that was
    /// [`insert`](Self::insert)ed.
    pub fn remove(&mut self, a: K, b: K) {
        self.pairs.remove((a, b));
        self.pairs.remove((b, a));
    }
}
