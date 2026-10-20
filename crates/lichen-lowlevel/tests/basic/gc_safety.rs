//! The collect's safety contracts: what must survive a release, and what must move.

use super::*;

/// The pointer a dynamic array value holds, for the arena-lifetime tests.
fn array_pointer(m: &Module<TestProgram>, node: NodeId) -> usize {
    match m
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|value| value.as_enum())
    {
        Some(LowValue::Array(array)) => match *array {
            AnyHandle::Dynamic(handle) => handle.as_ptr() as *const u8 as usize,
            AnyHandle::Static(_) => panic!("expected a dynamic array value"),
        },
        other => panic!("expected a dynamic array value, got {other:?}"),
    }
}

/// A function homed in `home` whose template is `nodes`, registered like a clone would be.
fn function_homed_in(
    m: &mut Module<TestProgram>,
    home: BlockId,
    nodes: &[NodeId],
    asserts: &[NodeId],
    r#return: NodeId,
) -> FunctionId {
    let function = m.functions.insert(Function {
        nodes: Vec::new(),
        r#return,
        parameter: NodeId::default(),
        return_type: NodeId::default(),
        return_marker: None,
        return_field: None,
        static_origin: None,
        asserts: Vec::new(),
        parent: None,
        block: home,
        looping: false,
    });
    for &node in nodes {
        m.register_in_function(function, node);
    }
    m.functions[function].asserts = asserts.to_vec();
    m.blocks[home].functions.push(function);
    function
}

/// A surviving function's template moves with its nodes: its home is outside the block.
#[test]
fn a_surviving_functions_template_moves_with_the_collect() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let leaf = u128_node(&mut m, child, 7);
    let child_root = op_node(&mut m, child, TestOperator::Id, Some(leaf));
    // No edge out of the root reaches this one: only the function's template names it.
    let template_only = u128_node(&mut m, child, 9);
    let function = function_homed_in(
        &mut m,
        root,
        &[child_root, leaf, template_only],
        &[],
        child_root,
    );

    m.garbage_collect(child_root);

    assert!(!m.blocks.contains_key(child));
    assert!(m.functions.contains_key(function));
    assert_eq!(m.node_block(child_root), root);
    assert!(
        m.nodes.contains_key(template_only),
        "the surviving function's template still names it"
    );
    assert_eq!(
        m.node_block(template_only),
        root,
        "and it moved out of the block"
    );
    assert_eq!(m.functions[function].nodes.len(), 3, "the list is whole");
}

/// The same for a template's assert conditions: they are part of the function's list.
#[test]
fn a_surviving_functions_assert_condition_moves_with_the_collect() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let leaf = u128_node(&mut m, child, 1);
    let condition = usize_node(&mut m, child, 9);
    let child_root = op_node(&mut m, child, TestOperator::Id, Some(leaf));
    let function = function_homed_in(
        &mut m,
        root,
        &[child_root, leaf, condition],
        &[condition],
        child_root,
    );

    m.garbage_collect(child_root);

    assert!(m.functions.contains_key(function));
    assert!(
        m.nodes.contains_key(condition),
        "the surviving function's assert condition still lives"
    );
    assert_eq!(m.functions[function].asserts, vec![condition]);
}

/// A function homed in the released block dies with its template: nothing survives to name a
/// released node.
#[test]
fn a_function_homed_in_the_released_block_dies_with_its_template() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let leaf = u128_node(&mut m, child, 7);
    let child_root = op_node(&mut m, child, TestOperator::Id, Some(leaf));
    // Only the function's template names this one, and the function dies with the block.
    let template_only = u128_node(&mut m, child, 9);
    let function = function_homed_in(
        &mut m,
        child,
        &[child_root, leaf, template_only],
        &[],
        child_root,
    );

    m.garbage_collect(child_root);

    assert!(!m.blocks.contains_key(child));
    assert!(
        !m.functions.contains_key(function),
        "it dies with the block"
    );
    assert!(
        !m.nodes.contains_key(template_only),
        "and releases what it names"
    );
}

/// A class write shares one payload: an outside member's payload must move with it.
#[test]
fn a_class_members_payload_moves_out_of_the_released_arena() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let seven = u128_node(&mut m, child, 7);
    let array = array_node(&mut m, child, &[seven], None);
    let outside = undecided_node(&mut m, root);
    m.unify(outside, array);
    assert!(m.unify_errors.is_empty());
    let before = array_pointer(&m, outside);
    let child_root = op_node(&mut m, child, TestOperator::Id, Some(array));

    m.garbage_collect(child_root);

    assert!(m.nodes.contains_key(outside));
    let after = array_pointer(&m, outside);
    assert_ne!(
        before, after,
        "the member's payload was re-allocated, not left in the freed arena"
    );
    assert_eq!(
        array_ids(m.node_value(AnyNodeId::Dynamic(outside))),
        vec![seven],
        "and it still holds what it held"
    );
}

/// A payload a collect moved once moves again: the next collect frees that arena.
#[test]
fn a_class_members_payload_moves_again_with_the_next_collect() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let middle = m.add_block(Some(root));
    let inner = m.add_block(Some(middle));
    let seven = u128_node(&mut m, inner, 7);
    let array = array_node(&mut m, inner, &[seven], None);
    let outside = undecided_node(&mut m, middle);
    m.unify(outside, array);
    assert!(m.unify_errors.is_empty());
    let inner_root = op_node(&mut m, inner, TestOperator::Id, Some(array));
    m.garbage_collect(inner_root);
    let first = array_pointer(&m, outside);
    let middle_root = op_node(&mut m, middle, TestOperator::Id, Some(inner_root));

    m.garbage_collect(middle_root);

    assert!(!m.blocks.contains_key(middle));
    assert_eq!(m.node_block(outside), root);
    let second = array_pointer(&m, outside);
    assert_ne!(first, second, "the second collect released the first arena");
    assert_eq!(
        array_ids(m.node_value(AnyNodeId::Dynamic(outside))),
        vec![seven]
    );
}

/// A value cycle inside the scope terminates the walk and moves both nodes.
#[test]
fn a_value_cycle_inside_the_scope_survives_the_collect() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let second = undecided_node(&mut m, child);
    let first = array_node(&mut m, child, &[second], None);
    let items = [ArrayItem {
        node: AnyNodeId::Dynamic(first),
        shallow: false,
    }];
    let value = TestValue::LowValue(LowValue::Array(m.alloc_array(&items, child).into()));
    m.write_node_value(second, Some(value));

    m.garbage_collect(first);

    assert!(!m.blocks.contains_key(child));
    assert!(m.nodes.contains_key(first) && m.nodes.contains_key(second));
    assert_eq!(m.node_block(second), root);
    assert_eq!(
        array_ids(m.node_value(AnyNodeId::Dynamic(first))),
        vec![second],
        "the cycle is still one"
    );
}

/// A table entry is an edge like an array item: the node it names moves with the value.
#[test]
fn a_table_entry_naming_a_scope_node_moves_with_the_collect() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let key = usize_node(&mut m, child, 1);
    let value_node = u128_node(&mut m, child, 7);
    let hash = 42;
    let items = [lichen_lowlevel::TableItem {
        hash,
        key: AnyNodeId::Dynamic(key),
        value: AnyNodeId::Dynamic(value_node),
    }];
    let table = TestValue::LowValue(LowValue::Table(m.alloc_table(&items, child)));
    let table_node = m.add_node(child, None, Some(table));

    m.garbage_collect(table_node);

    assert!(!m.blocks.contains_key(child));
    assert_eq!(m.node_block(key), root);
    assert_eq!(m.node_block(value_node), root);
}

/// A hand-built module may break the block boundary; the engine does not validate it.
#[test]
#[ignore = "P1-52: a parent-block payload may name a non-root scope node"]
fn a_parent_block_array_naming_a_scope_node_keeps_it() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let scope_root = u128_node(&mut m, child, 5);
    let sibling = u128_node(&mut m, child, 7);
    // The parent holds `[scope_root, sibling]`; nothing inside the child reaches the array.
    let _outside = array_node(&mut m, root, &[scope_root, sibling], None);
    let read = op_node(&mut m, root, TestOperator::Id, Some(scope_root));

    m.evaluate_node_deep(read, None);

    assert!(!m.blocks.contains_key(child));
    assert!(
        m.nodes.contains_key(sibling),
        "a live parent-block array still names it"
    );
}
