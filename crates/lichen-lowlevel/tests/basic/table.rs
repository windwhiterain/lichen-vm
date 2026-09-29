//! Tables: the constant `LowValue::Table` — deep-content keys (the pure
//! coinductive structural equality plus the matching content hash), the
//! hash-sorted payload, and the `TableGet` read (a miss records an
//! [`EvalError::TableMiss`] and yields `Void`; an unforceable key is
//! dropped with a [`EvalError::TableKeyUnbound`] at build).

use super::*;

/// A node holding a table value built from raw `(key, value)` node pairs.
fn table_value(
    m: &mut Module<TestProgram>,
    block: BlockId,
    entries: &[(AnyNodeId, AnyNodeId)],
) -> NodeId {
    let payload = m.build_table(entries, block);
    m.add_node(
        block,
        None,
        Some(TestValue::LowValue(LowValue::Table(payload))),
    )
}

/// A `TableGet` operation node reading `table[key]`.
fn table_get(m: &mut Module<TestProgram>, block: BlockId, table: NodeId, key: NodeId) -> NodeId {
    let operands = array_node(m, block, &[table, key], None);
    op_node(
        m,
        block,
        TestOperator::LowOperator(LowOperator::TableGet),
        Some(operands),
    )
}

#[test]
fn usize_keys_round_trip_and_misses_record_an_error() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let one = usize_node(&mut m, root, 1);
    let two = usize_node(&mut m, root, 2);
    let ten = usize_node(&mut m, root, 10);
    let twenty = usize_node(&mut m, root, 20);
    let t = table_value(
        &mut m,
        root,
        &[
            (AnyNodeId::Dynamic(one), AnyNodeId::Dynamic(ten)),
            (AnyNodeId::Dynamic(two), AnyNodeId::Dynamic(twenty)),
        ],
    );

    // A hit returns the stored value node's value.
    let get = table_get(&mut m, root, t, one);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(
        read,
        TestValue::LowValue(LowValue::USize(10)),
        "the stored value"
    );
    let get = table_get(&mut m, root, t, two);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, TestValue::LowValue(LowValue::USize(20)));

    // A miss is a recorded fact, not a panic: the error ledger gets an
    // entry and the read yields the computed-nothing value.
    let three = usize_node(&mut m, root, 3);
    let get = table_get(&mut m, root, t, three);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, TestValue::LowValue(LowValue::Void));
    assert_eq!(m.eval_errors.len(), 1);
    let EvalError::TableMiss { key, .. } = m.eval_errors[0] else {
        panic!("a missed read records a TableMiss failure")
    };
    assert_eq!(key, AnyNodeId::Dynamic(three));
}

#[test]
fn keys_are_deep_content_distinct_but_equal_structures_match() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // Two *separately built* `[1, 2]` arrays: different nodes, identical
    // content.  The deep-key semantics make them the same table key.
    let mk_key = |m: &mut Module<TestProgram>| -> NodeId {
        let a = usize_node(m, root, 1);
        let b = usize_node(m, root, 2);
        array_node(m, root, &[a, b], None)
    };
    let key1 = mk_key(&mut m);
    let key2 = mk_key(&mut m);
    assert_ne!(key1, key2, "two distinct node groups");
    let value = usize_node(&mut m, root, 7);
    let t = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(key1), AnyNodeId::Dynamic(value))],
    );

    let get = table_get(&mut m, root, t, key2);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, TestValue::LowValue(LowValue::USize(7)));

    // A different content (`[1, 3]`) misses.
    let a = usize_node(&mut m, root, 1);
    let c = usize_node(&mut m, root, 3);
    let other = array_node(&mut m, root, &[a, c], None);
    let get = table_get(&mut m, root, t, other);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, TestValue::LowValue(LowValue::Void));
}

#[test]
fn an_unbound_key_is_dropped_with_a_recorded_error() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let key = m.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let value = usize_node(&mut m, root, 1);
    let t = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(key), AnyNodeId::Dynamic(value))],
    );

    // The entry never made it into the payload.
    let TestValue::LowValue(LowValue::Table(payload)) =
        m.node_value(AnyNodeId::Dynamic(t)).unwrap()
    else {
        panic!("the table value")
    };
    assert_eq!(payload.items().len(), 0, "the unbound entry is dropped");
    let EvalError::TableKeyUnbound { key: dropped } = m.eval_errors[0] else {
        panic!("the build records a TableKeyUnbound failure")
    };
    assert_eq!(dropped, AnyNodeId::Dynamic(key));

    // Reading with a still-unbound key does **not** miss: the key is undecided,
    // not absent, so the lookup has not happened yet and the read stays lazy
    // for a later pass, when the key is bound.  (A key that is *decided* and
    // not key content — a `Void` — does miss; see the next test.)
    let get = table_get(&mut m, root, t, key);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(
        read,
        TestValue::LowValue(LowValue::Parameterized),
        "an undecided key leaves the read lazy"
    );
    assert_eq!(
        m.eval_errors.len(),
        1,
        "and records no miss — only the build's dropped entry is reported"
    );
}

#[test]
fn a_computed_nothing_key_is_never_a_phantom_hit() {
    // Two *different* failed reads both evaluate to `Void`; neither may key
    // a table entry, and the read must miss rather than the two residues
    // colliding on a shared hash token.
    let mut m = Module::new();
    let root = m.add_block(None);
    let oob_read = |m: &mut Module<TestProgram>| -> NodeId {
        let a = usize_node(m, root, 1);
        let arr = array_node(m, root, &[a], None);
        let idx = usize_node(m, root, 5);
        let operands = array_node(m, root, &[arr, idx], None);
        op_node(
            m,
            root,
            TestOperator::LowOperator(LowOperator::Index),
            Some(operands),
        )
    };
    let build_key = oob_read(&mut m);
    let value = usize_node(&mut m, root, 3);
    let t = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(build_key), AnyNodeId::Dynamic(value))],
    );

    // The entry was dropped at build: a `Void` key is not hashable.
    let TestValue::LowValue(LowValue::Table(payload)) =
        m.node_value(AnyNodeId::Dynamic(t)).unwrap()
    else {
        panic!("the table value")
    };
    assert!(
        payload.items().is_empty(),
        "the `Void`-keyed entry is dropped"
    );
    assert!(
        m.eval_errors
            .iter()
            .any(|e| matches!(e, EvalError::TableKeyUnbound { .. })),
        "the build records the dropped key"
    );

    // A read keyed by *another* failed read misses — no phantom hit between
    // two computed-nothing keys.
    let read_key = oob_read(&mut m);
    let get = table_get(&mut m, root, t, read_key);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, TestValue::LowValue(LowValue::Void));
    assert!(
        m.eval_errors
            .iter()
            .any(|e| matches!(e, EvalError::TableMiss { .. })),
        "the read records a miss"
    );
}

#[test]
fn an_undecided_read_leaves_no_cycle_for_the_next_pass() {
    // A `TableGet` whose key is undecided answers `Parameterized`, and that
    // answer is deliberately *not* cached — the next evaluation re-runs the
    // read.  The attempt must therefore release the node's visiting mark:
    // leaving it set makes the next evaluation see an ordinary re-read as a
    // cycle and panic (`cycle detected: node ... is being evaluated`).
    let mut m = Module::new();
    let root = m.add_block(None);
    let key = usize_node(&mut m, root, 1);
    let value = usize_node(&mut m, root, 10);
    let t = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(key), AnyNodeId::Dynamic(value))],
    );
    // A key that stays undecided: an unbound cell.
    let other_key = m.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let get = table_get(&mut m, root, t, other_key);

    let read = m.evaluate_node_deep(get, None);
    assert_eq!(
        read,
        TestValue::LowValue(LowValue::Parameterized),
        "an undecided key leaves the read lazy"
    );
    assert_eq!(m.eval_errors.len(), 0, "an undecided key records no miss");

    // The second pass — the checker's statement pass then root pass, or a
    // later forced read — re-evaluates the same node.  A visiting mark leaked
    // by the first attempt panics here.
    let again = m.evaluate_node_deep(get, None);
    assert_eq!(
        again,
        TestValue::LowValue(LowValue::Parameterized),
        "the re-read is still undecided, not a cycle"
    );
}

#[test]
fn cyclic_keys_hash_and_compare_equal() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // A self-referential `[1, ↺]` pair — the `[Type, ↺]` universe shape.
    // Two distinct cyclic values are coinductively equal and must land in
    // the same bucket.
    let mk_cycle = |m: &mut Module<TestProgram>| -> NodeId {
        let node = m.add_node(root, None, None);
        let one = usize_node(m, root, 1);
        let items = [
            ArrayItem::new(AnyNodeId::Dynamic(one)),
            ArrayItem::new(AnyNodeId::Dynamic(node)),
        ];
        m.write_node_value(
            node,
            Some(TestValue::LowValue(LowValue::Array(
                m.alloc_array(&items, root),
            ))),
        );
        node
    };
    let key1 = mk_cycle(&mut m);
    let key2 = mk_cycle(&mut m);
    let value = usize_node(&mut m, root, 9);
    let t = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(key1), AnyNodeId::Dynamic(value))],
    );

    let get = table_get(&mut m, root, t, key2);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, TestValue::LowValue(LowValue::USize(9)));
}

#[test]
fn table_values_key_by_identity() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let mk_table = |m: &mut Module<TestProgram>| -> NodeId {
        let a = usize_node(m, root, 1);
        let b = usize_node(m, root, 2);
        table_value(m, root, &[(AnyNodeId::Dynamic(a), AnyNodeId::Dynamic(b))])
    };
    let t1 = mk_table(&mut m);
    let t2 = mk_table(&mut m);
    let value = usize_node(&mut m, root, 42);
    let outer = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(t1), AnyNodeId::Dynamic(value))],
    );

    // The stored table key is found by itself, never by an equal-looking
    // distinct table.
    let get = table_get(&mut m, root, outer, t1);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, TestValue::LowValue(LowValue::USize(42)));
    let get = table_get(&mut m, root, outer, t2);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, TestValue::LowValue(LowValue::Void));
}

#[test]
fn table_values_stay_lazy_until_read() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let key = usize_node(&mut m, root, 1);
    // The value is an unevaluated `Id` computation — stored as a lazy ref.
    let operand = u128_node(&mut m, root, 5);
    let lazy_value = op_node(&mut m, root, TestOperator::Id, Some(operand));
    let t = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(key), AnyNodeId::Dynamic(lazy_value))],
    );

    assert!(
        m.node_value(AnyNodeId::Dynamic(lazy_value)).is_none(),
        "the value stays lazy until the read"
    );
    let get = table_get(&mut m, root, t, key);
    let read = m.evaluate_node_deep(get, None);
    let TestValue::U128(AnyHandle::Dynamic(handle)) = read else {
        panic!("the read forces the stored value, got {read:?}")
    };
    assert_eq!(unsafe { *handle.0 }, 5, "the forced value's content");
}

#[test]
fn the_payload_is_stored_sorted_by_hash() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let three = usize_node(&mut m, root, 3);
    let one = usize_node(&mut m, root, 1);
    let value = usize_node(&mut m, root, 0);
    let t = table_value(
        &mut m,
        root,
        &[
            (AnyNodeId::Dynamic(three), AnyNodeId::Dynamic(value)),
            (AnyNodeId::Dynamic(one), AnyNodeId::Dynamic(value)),
        ],
    );

    let TestValue::LowValue(LowValue::Table(payload)) =
        m.node_value(AnyNodeId::Dynamic(t)).unwrap()
    else {
        panic!("the table value")
    };
    let items = payload.items();
    assert_eq!(items.len(), 2);
    assert!(
        items.windows(2).all(|w| w[0].hash <= w[1].hash),
        "sorted by hash: {} then {}",
        items[0].hash,
        items[1].hash
    );
    let keys: HashSet<AnyNodeId> = items.iter().map(|item| item.key).collect();
    assert_eq!(
        keys,
        HashSet::from([AnyNodeId::Dynamic(one), AnyNodeId::Dynamic(three)]),
        "both entries survive the build"
    );
}

#[test]
fn gc_compaction_moves_table_payloads_and_entries() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let key = usize_node(&mut m, child, 1);
    let value = usize_node(&mut m, child, 5);
    let t = table_value(
        &mut m,
        child,
        &[(AnyNodeId::Dynamic(key), AnyNodeId::Dynamic(value))],
    );
    let read = op_node(&mut m, root, TestOperator::Id, Some(t));

    let result = m.evaluate_node_deep(read, None);
    assert!(
        matches!(result, TestValue::LowValue(LowValue::Table(_))),
        "the hoisted value is the table itself: {result:?}"
    );
    assert!(
        !m.blocks.contains_key(child),
        "the vacated block is released"
    );

    // The hoisted table still reads after the compaction.
    let get = table_get(&mut m, root, t, key);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, TestValue::LowValue(LowValue::USize(5)));
}

#[test]
fn tables_unify_by_identity_not_content() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let mk_table = |m: &mut Module<TestProgram>| -> NodeId {
        let a = usize_node(m, root, 1);
        let b = usize_node(m, root, 2);
        table_value(m, root, &[(AnyNodeId::Dynamic(a), AnyNodeId::Dynamic(b))])
    };
    let t1 = mk_table(&mut m);
    let t2 = mk_table(&mut m);

    // Two distinct tables are distinct values — unification compares them by
    // identity, exactly like two function values (there is no elementwise
    // table arm; a table's entries are not a positional structure).
    m.unify(t1, t2);
    assert_eq!(m.unify_errors.len(), 1, "distinct tables conflict");

    // The same table unifies with itself.
    let before = m.unify_errors.len();
    m.unify(t1, t1);
    assert_eq!(m.unify_errors.len(), before, "a table unifies with itself");
}
