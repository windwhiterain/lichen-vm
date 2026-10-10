//! An intrusive disjoint-set (union-find): a `parent`-pointer tree per set, plus
//! a member list from the representative.

use slotmap::{Key, SlotMap};
use stacksafe::stacksafe;

/// The disjoint-set metadata embedded in a node; the fields are private.
///
/// # Invariant
///
/// [`make_set`] runs right after the node is inserted, before anything else
/// touches it; `tail` and `size` are meaningful only on a representative; and
/// the member list is acyclic, because [`union`] splices a None-terminated list
/// head onto another root's tail, so `next` never loops.
#[derive(Clone, Copy, Debug, Default)]
pub struct Meta<K: Copy> {
    /// The representative of the set; `None` means this node is its own root.
    parent: Option<K>,
    /// The next member in the member list headed by the representative.
    next: Option<K>,
    /// Valid only at a representative: the last member of its list.
    tail: Option<K>,
    /// Valid only at a representative: the number of members in the set.
    size: u32,
}

impl<K: Copy> Meta<K> {
    /// Assemble metadata from four links from outside this module — freezing or
    /// decoding remaps a node's links.
    pub fn new(parent: Option<K>, next: Option<K>, tail: Option<K>, size: u32) -> Self {
        Self {
            parent,
            next,
            tail,
            size,
        }
    }

    /// The representative of the set; `None` means this node is its own root.
    pub fn parent(&self) -> Option<K> {
        self.parent
    }

    /// The next member in the set's member list, headed by the representative.
    pub fn next(&self) -> Option<K> {
        self.next
    }

    /// Valid only at a representative: the last member of its list.
    pub fn tail(&self) -> Option<K> {
        self.tail
    }

    /// Valid only at a representative: the number of members in the set.
    pub fn size(&self) -> u32 {
        self.size
    }
}

/// The permit [`Node::meta_mut`] requires.  The field is private, so only this
/// module mints one.
pub struct MetaPermit(());

/// A node type that carries a [`Meta`] for a disjoint-set.
pub trait Node: Sized {
    /// The node's key in the [`SlotMap`] that stores it.
    type Key: Copy;
    fn meta(&self) -> &Meta<Self::Key>;
    /// The node's metadata, writable only under a minted [`MetaPermit`].
    fn meta_mut(&mut self, permit: MetaPermit) -> &mut Meta<Self::Key>;
}

/// Initialize `key` as the singleton representative of its own set, once, right
/// after insertion.
pub fn make_set<K, V>(nodes: &mut SlotMap<K, V>, key: K)
where
    K: Key,
    V: Node<Key = K>,
{
    let set = nodes[key].meta_mut(MetaPermit(()));
    set.parent = None;
    set.next = None;
    set.tail = Some(key);
    set.size = 1;
}

/// Return the representative of `key`'s set.
///
/// # Invariant
///
/// Every node on the path is re-pointed at the root, so later finds are
/// shorter.  Only `parent` is written, never the member list.
#[stacksafe]
pub fn find<K, V>(nodes: &mut SlotMap<K, V>, key: K) -> K
where
    K: Key,
    V: Node<Key = K>,
{
    let Some(parent) = nodes[key].meta().parent() else {
        return key;
    };
    let root = find(nodes, parent);
    if root != parent {
        nodes[key].meta_mut(MetaPermit(())).parent = Some(root);
    }
    root
}

/// Merge the sets of `a` and `b`, returning the representative of the merged
/// set.
///
/// # Invariant
///
/// The smaller set is attached under the larger, bounding tree depth by the
/// logarithm of the size; the member lists are spliced in O(1) and nothing is
/// allocated.
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
    let (ra, rb) = if nodes[ra].meta().size() < nodes[rb].meta().size() {
        (rb, ra)
    } else {
        (ra, rb)
    };
    // Read every target before writing, so the writes below are independent.
    let Some(ta) = nodes[ra].meta().tail() else {
        unreachable!("representative {ra:?} lacks a member-list tail")
    };
    let Some(tb) = nodes[rb].meta().tail() else {
        unreachable!("representative {rb:?} lacks a member-list tail")
    };
    let size = nodes[ra].meta().size() + nodes[rb].meta().size();
    nodes[ta].meta_mut(MetaPermit(())).next = Some(rb);
    nodes[rb].meta_mut(MetaPermit(())).parent = Some(ra);
    nodes[ra].meta_mut(MetaPermit(())).tail = Some(tb);
    nodes[ra].meta_mut(MetaPermit(())).size = size;
    ra
}

/// Rewrite a set's member list to exactly `members`, in the order given.
///
/// # Invariant
///
/// `members[0]` becomes the representative and every other member's parent is
/// re-pointed straight at it, flattening the tree.  Members left out keep their
/// metadata; an empty `members` is a no-op.
pub fn rebuild<K, V>(nodes: &mut SlotMap<K, V>, members: &[K])
where
    K: Key,
    V: Node<Key = K>,
{
    let Some((&representative, _)) = members.split_first() else {
        return;
    };
    let mut previous: Option<K> = None;
    for &member in members {
        if let Some(previous) = previous {
            nodes[previous].meta_mut(MetaPermit(())).next = Some(member);
        }
        nodes[member].meta_mut(MetaPermit(())).parent =
            (member != representative).then_some(representative);
        previous = Some(member);
    }
    let last = previous.expect("the member list is non-empty");
    nodes[last].meta_mut(MetaPermit(())).next = None;
    let meta = nodes[representative].meta_mut(MetaPermit(()));
    meta.tail = Some(last);
    meta.size = members.len() as u32;
}

/// Iterate over every member of `root`'s set, `root` first, in list order.
pub fn members<'n, K, V>(nodes: &'n SlotMap<K, V>, root: K) -> impl Iterator<Item = K> + 'n
where
    K: Key,
    V: Node<Key = K>,
{
    std::iter::successors(Some(root), |&key| nodes[key].meta().next())
}
