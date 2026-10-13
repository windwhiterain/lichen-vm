//! What a collect's class walk covers: the whole class, rooted at its representative.

use super::*;
use lichen_utils::disjoint;

/// # Invariant
///
/// [`Module::garbage_collect`] follows "the equality classes it aliases into"
/// out of every node it reaches, so a member of a class a walked node shares is
/// itself walked and re-homed. The member list is headed by the representative,
/// so the walk starts there: entering a class at a member that is not its head
/// reaches only the members behind it and releases the ones ahead.
#[test]
fn a_collect_follows_a_reachable_class_from_its_representative() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let representative = undecided_node(&mut m, child);
    let outside = undecided_node(&mut m, root);
    let trailing = undecided_node(&mut m, child);
    // Union by size keeps the head in the collected block.
    m.unify(representative, outside);
    m.unify(representative, trailing);
    assert!(m.unify_errors.is_empty());
    assert_eq!(m.equality_representative(outside), representative);
    // The walk enters the class at `outside`, not at its head.
    let child_root = array_node(&mut m, child, &[outside], None);

    m.garbage_collect(child_root);

    assert!(!m.blocks.contains_key(child));
    assert!(
        m.blocks[root].nodes.contains(&representative),
        "the representative the walked node aliases is re-homed, not released"
    );
    assert_eq!(m.equality_representative(outside), representative);
    let members: Vec<_> = disjoint::members(&m.nodes, representative).collect();
    assert_eq!(members.len(), 3, "the class is whole: {members:?}");
}
