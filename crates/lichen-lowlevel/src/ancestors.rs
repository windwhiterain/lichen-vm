//! The recursion-path guard the structural walks share.
//!
//! Three walks descend a node graph and must cut a cycle instead of looping:
//! the unification comparison ([`crate::equality`]), the reconciliation of a
//! forced computation against the value its class committed, and the table
//! key comparison ([`crate::table`]).  Each asks whether a **pair of nodes**
//! is on the current recursion path, and that question is an *ancestor*
//! relation, not a visited mark: a pair the walk meets again in a sibling
//! subtree is not on the path and must be compared again, so membership is
//! removed on the way out.
//!
//! The paths used to be `Vec`s probed with a linear `contains`, which is one
//! scan per recursion level — quadratic in the depth of the walk.  Pairs the
//! guard accepted are distinct (the caller tests before it inserts), so a set
//! answers the same question with the same insert/remove discipline.

use std::collections::HashSet;
use std::hash::Hash;

/// The unordered node pairs on the current recursion path.
///
/// # Contract
/// - A pair [`insert`](Self::insert)ed by a frame must be
///   [`remove`](Self::remove)d by that same frame on every exit, including
///   the early returns; a pair left behind cuts a comparison in a sibling
///   subtree that the path relation does not cover.
/// - A pair is inserted only after [`contains`](Self::contains) answered
///   `false` for it, so the pairs on the path are distinct.
pub(crate) struct AncestorPairs<K> {
    pairs: HashSet<(K, K)>,
}

impl<K: Copy + Eq + Hash> AncestorPairs<K> {
    pub(crate) fn new() -> Self {
        Self {
            pairs: HashSet::new(),
        }
    }

    /// Whether the unordered pair `{a, b}` is on the current path — the same
    /// test `path.contains(&(a, b)) || path.contains(&(b, a))` performed.
    pub(crate) fn contains(&self, a: K, b: K) -> bool {
        self.pairs.contains(&(a, b))
    }

    /// Put `{a, b}` on the path.  Both orientations are stored, which is what
    /// makes [`Self::contains`]'s single probe answer the symmetric test.
    pub(crate) fn insert(&mut self, a: K, b: K) {
        self.pairs.insert((a, b));
        self.pairs.insert((b, a));
    }

    /// Take `{a, b}` off the path.  Must name the same pair that was
    /// [`insert`](Self::insert)ed.
    pub(crate) fn remove(&mut self, a: K, b: K) {
        self.pairs.remove(&(a, b));
        self.pairs.remove(&(b, a));
    }
}
