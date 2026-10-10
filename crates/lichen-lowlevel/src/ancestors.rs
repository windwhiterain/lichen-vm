//! The recursion-path guards: an **ancestor** relation, not a visited mark.

use std::collections::HashSet;
use std::hash::Hash;

/// The nodes on the current recursion path — the one-node guard.  Its pair form is
/// [`AncestorPairs`].
///
/// # Invariant
/// - A node [`insert`](Self::insert)ed by a frame must be
///   [`remove`](Self::remove)d by that same frame on every exit, including the
///   early returns; a node left behind cuts a visit in a sibling subtree the
///   path relation does not cover.
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

    /// Whether `node` is on the current path.
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
/// # Invariant
/// - A pair [`insert`](Self::insert)ed by a frame must be
///   [`remove`](Self::remove)d by that same frame on every exit, including the
///   early returns; a pair left behind cuts a comparison in a sibling subtree
///   the path relation does not cover.
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

    /// Whether the unordered pair `{a, b}` is on the current path.
    pub fn contains(&self, a: K, b: K) -> bool {
        self.pairs.contains((a, b))
    }

    /// Put `{a, b}` on the path.  Both orientations are stored, so
    /// [`Self::contains`] probes once.
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
