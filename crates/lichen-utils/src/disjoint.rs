//! An intrusive disjoint-set (union-find) with a member list per set.
//!
//! Each set is a `parent`-pointer tree plus a singly-linked list of all its
//! members headed by the representative, so walking `next` from a root visits
//! every node of the set. [`find`] compresses paths without touching the
//! list, [`union`] splices the two member lists with O(1) pointer surgery,
//! and neither allocates: the metadata lives inside the caller's nodes and
//! every operation is plain field reads and writes.

use slotmap::{Key, SlotMap};
use stacksafe::stacksafe;

/// The disjoint-set metadata embedded in a node.
///
/// # Contract
/// - Call [`make_set`] right after inserting the node, before any other
///   operation touches it.
/// - `tail` and `size` are meaningful only on the representative of a set;
///   they are never read elsewhere.
/// - The member list is acyclic: [`union`] splices a root (a None-terminated
///   list head) onto the tail of another root's list, so `next` never loops.
#[derive(Clone, Copy, Debug, Default)]
pub struct Meta<K: Copy> {
    /// The representative of the set; `None` means this node is its own root.
    pub parent: Option<K>,
    /// The next member in the set's member list, headed by the representative.
    pub next: Option<K>,
    /// Valid only at a representative: the last member of its list.
    pub tail: Option<K>,
    /// Valid only at a representative: the number of members in the set.
    pub size: u32,
}

/// A node type that carries a [`Meta`] for a disjoint-set.
pub trait Node: Sized {
    /// The node's key in the [`SlotMap`] that stores it.
    type Key: Copy;
    fn meta(&self) -> &Meta<Self::Key>;
    fn meta_mut(&mut self) -> &mut Meta<Self::Key>;
}

/// Initialize `key` as the singleton representative of its own set.
///
/// Must be called once, right after `key` is inserted into `nodes`, before
/// the key participates in any other operation.
pub fn make_set<K, V>(nodes: &mut SlotMap<K, V>, key: K)
where
    K: Key,
    V: Node<Key = K>,
{
    let set = nodes[key].meta_mut();
    set.parent = None;
    set.next = None;
    set.tail = Some(key);
    set.size = 1;
}

/// Return the representative of `key`'s set.
///
/// Compresses the path from `key` to the root: every node visited is
/// re-pointed directly at the root, so later finds are shorter. Only the
/// `parent` field is rewritten — the member list is left untouched, which
/// keeps it valid at no extra cost. Recursion depth is bounded by the path
/// length and grows the stack on demand.
#[stacksafe]
pub fn find<K, V>(nodes: &mut SlotMap<K, V>, key: K) -> K
where
    K: Key,
    V: Node<Key = K>,
{
    let Some(parent) = nodes[key].meta().parent else {
        return key;
    };
    let root = find(nodes, parent);
    if root != parent {
        nodes[key].meta_mut().parent = Some(root);
    }
    root
}

/// Merge the sets of `a` and `b` and return the representative of the merged
/// set.
///
/// The smaller set is attached under the larger one (by member count), which
/// bounds the tree depth by the logarithm of the set size, and its member
/// list is spliced onto the tail of the other root's list in O(1). Nothing
/// is allocated.
pub fn union<K, V>(nodes: &mut SlotMap<K, V>, a: K, b: K) -> K
where
    K: Key,
    V: Node<Key = K>,
{
    let ra = find(nodes, a);
    let rb = find(nodes, b);
    if ra == rb {
        return ra;
    }
    let (ra, rb) = if nodes[ra].meta().size < nodes[rb].meta().size {
        (rb, ra)
    } else {
        (ra, rb)
    };
    // Read every target before writing, so the writes below are independent.
    let Some(ta) = nodes[ra].meta().tail else {
        unreachable!("representative {ra:?} lacks a member-list tail")
    };
    let Some(tb) = nodes[rb].meta().tail else {
        unreachable!("representative {rb:?} lacks a member-list tail")
    };
    let size = nodes[ra].meta().size + nodes[rb].meta().size;
    nodes[ta].meta_mut().next = Some(rb);
    nodes[rb].meta_mut().parent = Some(ra);
    nodes[ra].meta_mut().tail = Some(tb);
    nodes[ra].meta_mut().size = size;
    ra
}

/// Iterate over every member of the set represented by `root`, starting with
/// `root` itself, in member-list order.
///
/// The iterator borrows `nodes` and allocates nothing.
pub fn members<'n, K, V>(nodes: &'n SlotMap<K, V>, root: K) -> impl Iterator<Item = K> + 'n
where
    K: Key,
    V: Node<Key = K>,
{
    std::iter::successors(Some(root), |&key| nodes[key].meta().next)
}
