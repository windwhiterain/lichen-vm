//! What a failed unify records about the clash. See `docs/notes/array-attribute-view.md`.

use super::*;
use lichen_lowlevel::OperandOrder;

/// A raw unify states no operand order, so a consumer must not judge it.
///
/// # Invariant
/// `OperandOrder::Unknown` is the default a `UnifyError` is born with; only
/// `Checker::check_unify` and the apply's parameter check name an order.
#[test]
fn a_raw_unify_error_states_no_operand_order() {
    let mut m: Module<TestProgram> = Module::new();
    let block = m.add_block(None);
    let one = usize_node(&mut m, block, 1);
    let two = usize_node(&mut m, block, 2);
    let (_, errors) = m.try_unify(one, two);
    assert!(!errors.is_empty(), "two decided values clash");
    let error = &m.unify_errors[errors.start];
    assert_eq!(error.operand_order, OperandOrder::Unknown);
    assert!(error.clash.is_none(), "the operands themselves clashed");
    assert!(error.clash_attribute.is_none());
}

/// A named clash belongs to the error the failing compare added, never to an older one.
///
/// # Invariant
/// The item pair is written only on the errors the child produced: an earlier error
/// keeps the operands it was recorded with.
#[test]
fn a_named_clash_belongs_to_the_new_error() {
    let mut m: Module<TestProgram> = Module::new();
    let block = m.add_block(None);
    // An older error whose `a`/`b` are its own roots.
    let one = usize_node(&mut m, block, 1);
    let two = usize_node(&mut m, block, 2);
    let (_, old) = m.try_unify(one, two);
    assert!(!old.is_empty());
    // A nested compare: the items of two outer arrays are arrays of one decided value.
    let left_item = array_node(&mut m, block, &[one], None);
    let right_item = array_node(&mut m, block, &[two], None);
    let left = array_node(&mut m, block, &[left_item], None);
    let right = array_node(&mut m, block, &[right_item], None);
    let (_, later) = m.try_unify(left, right);
    assert!(!later.is_empty(), "the nested compare records its conflict");
    // The new error names the innermost pair the descent clashed at.
    let newest = &m.unify_errors[later.start];
    assert_eq!(newest.clash, Some((one, two)));
    // The older one is unchanged.
    let older = &m.unify_errors[old.start];
    assert!(
        older.clash.is_none(),
        "an older error keeps its own operands"
    );
    assert!(older.clash_attribute.is_none());
}

/// The default attribute array is one **shared** node per canonical position.
///
/// # Invariant
/// The node therefore states no attribute of its own: a reader that must know
/// which attribute a default slot belongs to takes it from the canonical index,
/// never from the node.
#[test]
fn the_default_attribute_array_shares_one_node_across_positions() {
    let mut m: Module<TestProgram> = Module::new();
    let block = m.add_block(None);
    let zero = usize_node(&mut m, block, 0);
    let items = vec![ArrayItem::new(AnyNodeId::Dynamic(zero)); lichen_lowlevel::EXPR_ATTR_BASE + 2];
    let attributes = m.alloc_array(&items, block);
    m.set_default_attributes(attributes);
    assert_eq!(m.default_attribute(0), Some(AnyNodeId::Dynamic(zero)));
    assert_eq!(m.default_attribute(1), Some(AnyNodeId::Dynamic(zero)));
    assert_eq!(
        m.default_attribute(0),
        m.default_attribute(1),
        "one node answers for every canonical position"
    );
}
