//! What a collected block's re-elected representative carries: the class low type.

use super::*;
use lichen_lowlevel::LowShape;

/// # Invariant
/// The low type is class-routed through the representative
/// (`docs/notes/lowlevel-low-types.md`) and is stored nowhere else, so the
/// re-election a release performs must join the released representative's shape
/// into the survivor it elects — the class value needs no such step.
#[test]
fn re_electing_a_representative_carries_the_class_low_type() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let inside = undecided_node(&mut m, child);
    let unreachable = undecided_node(&mut m, child);
    let child_root = usize_node(&mut m, child, 0);
    let outside = undecided_node(&mut m, root);
    // union by size keeps the representative in the collected block
    m.unify(inside, unreachable);
    m.unify(inside, outside);
    let int = usize_node(&mut m, root, 7);
    m.unify(outside, int);
    assert!(m.unify_errors.is_empty());
    assert_eq!(m.equality_representative(outside), inside);
    assert_eq!(m.class_low_type(outside), Some(&LowShape::USize));

    m.garbage_collect(child_root);

    let rep = m.equality_representative(outside);
    assert_ne!(rep, inside, "the old representative died with the block");
    assert_eq!(
        m.class_low_type(outside),
        Some(&LowShape::USize),
        "the re-elected representative holds the class low type"
    );
}
