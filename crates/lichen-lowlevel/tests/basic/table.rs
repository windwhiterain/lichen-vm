//! Tables: the constant `LowValue::Table` — deep-content keys (the pure
//! coinductive structural equality plus the matching content hash), the
//! hash-sorted payload, and the `TableGet` read (a miss records an
//! [`EvalError::TableMiss`] and yields `Error`; an unforceable key is
//! dropped with a [`EvalError::TableKeyUnbound`] at build).

use super::*;
use lichen_lowlevel::{Freeze, ModuleKey, StaticNodeId};

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
    let read = m.evaluate_node_deep(get, None).unwrap();
    assert_eq!(
        read,
        TestValue::LowValue(LowValue::USize(10)),
        "the stored value"
    );
    let get = table_get(&mut m, root, t, two);
    let read = m.evaluate_node_deep(get, None).unwrap();
    assert_eq!(read, TestValue::LowValue(LowValue::USize(20)));

    // A miss is a recorded fact, not a panic: the error ledger gets an
    // entry and the read yields the computed-nothing value.
    let three = usize_node(&mut m, root, 3);
    let get = table_get(&mut m, root, t, three);
    let read = m.evaluate_node_deep(get, None).unwrap();
    assert_eq!(read, TestValue::LowValue(LowValue::Error));
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
    let read = m.evaluate_node_deep(get, None).unwrap();
    assert_eq!(read, TestValue::LowValue(LowValue::USize(7)));

    // A different content (`[1, 3]`) misses.
    let a = usize_node(&mut m, root, 1);
    let c = usize_node(&mut m, root, 3);
    let other = array_node(&mut m, root, &[a, c], None);
    let get = table_get(&mut m, root, t, other);
    let read = m.evaluate_node_deep(get, None).unwrap();
    assert_eq!(read, TestValue::LowValue(LowValue::Error));
}

#[test]
fn an_unbound_key_is_dropped_with_a_recorded_error() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let key = m.add_node(root, None, None);
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
    // SAFETY: `t` is a live node of `m`, whose block has not been dropped.
    assert_eq!(
        unsafe { payload.items() }.len(),
        0,
        "the unbound entry is dropped"
    );
    let EvalError::TableKeyUnbound { key: dropped } = m.eval_errors[0] else {
        panic!("the build records a TableKeyUnbound failure")
    };
    assert_eq!(dropped, AnyNodeId::Dynamic(key));

    // Reading with a still-unbound key does **not** miss: the key is undecided,
    // not absent, so the lookup has not happened yet and the read stays lazy
    // for a later pass, when the key is bound.  (A key that is *decided* and
    // not key content — an `Error` — does miss; see the next test.)
    let get = table_get(&mut m, root, t, key);
    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, None, "an undecided key leaves the read lazy");
    assert_eq!(
        m.eval_errors.len(),
        1,
        "and records no miss — only the build's dropped entry is reported"
    );
}

#[test]
fn an_empty_key_is_never_a_phantom_hit() {
    // Two *different* failed reads both evaluate to `Error`; neither may key
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

    // The entry was dropped at build: an `Error` key is not hashable.
    let TestValue::LowValue(LowValue::Table(payload)) =
        m.node_value(AnyNodeId::Dynamic(t)).unwrap()
    else {
        panic!("the table value")
    };
    assert!(
        // SAFETY: `t` is a live node of `m`, whose block has not been dropped.
        unsafe { payload.items() }.is_empty(),
        "the `Error`-keyed entry is dropped"
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
    let read = m.evaluate_node_deep(get, None).unwrap();
    assert_eq!(read, TestValue::LowValue(LowValue::Error));
    assert!(
        m.eval_errors
            .iter()
            .any(|e| matches!(e, EvalError::TableMiss { .. })),
        "the read records a miss"
    );
}

#[test]
fn an_undecided_read_leaves_no_cycle_for_the_next_pass() {
    // A `TableGet` whose key is undecided answers nothing, and that
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
    let other_key = m.add_node(root, None, None);
    let get = table_get(&mut m, root, t, other_key);

    let read = m.evaluate_node_deep(get, None);
    assert_eq!(read, None, "an undecided key leaves the read lazy");
    assert_eq!(m.eval_errors.len(), 0, "an undecided key records no miss");

    // The second pass — the checker's statement pass then root pass, or a
    // later forced read — re-evaluates the same node.  A visiting mark leaked
    // by the first attempt panics here.
    let again = m.evaluate_node_deep(get, None);
    assert_eq!(again, None, "the re-read is still undecided, not a cycle");
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
    let read = m.evaluate_node_deep(get, None).unwrap();
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
    let read = m.evaluate_node_deep(get, None).unwrap();
    assert_eq!(read, TestValue::LowValue(LowValue::USize(42)));
    let get = table_get(&mut m, root, outer, t2);
    let read = m.evaluate_node_deep(get, None).unwrap();
    assert_eq!(read, TestValue::LowValue(LowValue::Error));
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
    let read = m.evaluate_node_deep(get, None).unwrap();
    let TestValue::U128(AnyHandle::Dynamic(handle)) = read else {
        panic!("the read forces the stored value, got {read:?}")
    };
    assert_eq!(unsafe { *handle.as_ptr() }, 5, "the forced value's content");
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
    // SAFETY: `t` is a live node of `m`, whose block has not been dropped.
    let items = unsafe { payload.items() };
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

    let result = m.evaluate_node_deep(read, None).unwrap();
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
    let read = m.evaluate_node_deep(get, None).unwrap();
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

// --- the key hash across a freeze and across a cycle's depth ----------
//
// The stored hash is a *pre-filter*: a read binary-searches the payload for
// the equal-hash run and verifies every candidate with `key_eq`, which is the
// authority.  So the hash owes one direction only — equal keys must hash
// equal — and a hash that cannot be recomputed after a reload is a permanent
// `TableMiss` for a key that *is* there.

/// Freeze `source` into a fresh importer and hand back the importer, its root
/// block and the freeze map.  The byte round-trip of the artifact container
/// lives in `lichen-language`; this is the lowlevel half of the same path —
/// the static refs and the payloads a reload reads.
fn reload_after_freeze(source: &Module<TestProgram>) -> (Module<TestProgram>, BlockId, Freeze) {
    let mut importer = Module::new();
    let root = importer.add_block(None);
    let freeze = importer.freeze_mapped(source, ModuleKey::from_raw(1), [0; 32]);
    (importer, root, freeze)
}

/// The static ref of a source node under `freeze`.
fn sref_of(freeze: &Freeze, node: NodeId) -> StaticNodeId {
    StaticNodeId {
        module: freeze.key,
        index: freeze.node_map[&node],
    }
}

/// `[1, ↺]` — a self-referential two-element array, the `[Type, ↺]` universe
/// shape.
fn cyclic_pair(m: &mut Module<TestProgram>, block: BlockId) -> NodeId {
    let node = m.add_node(block, None, None);
    let one = usize_node(m, block, 1);
    let items = m.alloc_array(
        &[
            ArrayItem::new(AnyNodeId::Dynamic(one)),
            ArrayItem::new(AnyNodeId::Dynamic(node)),
        ],
        block,
    );
    m.write_node_value(node, Some(TestValue::LowValue(LowValue::Array(items))));
    node
}

#[test]
fn a_table_key_survives_a_freeze_and_a_reload() {
    // A table used as a key: a dynamic handle's identity is its payload's
    // address, which no longer exists after a reload, so the stored hash has
    // to be a function of the table's content.
    let mut m = Module::new();
    let root = m.add_block(None);
    let inner_key = usize_node(&mut m, root, 1);
    let inner_value = usize_node(&mut m, root, 2);
    let inner = table_value(
        &mut m,
        root,
        &[(
            AnyNodeId::Dynamic(inner_key),
            AnyNodeId::Dynamic(inner_value),
        )],
    );
    let value = usize_node(&mut m, root, 42);
    let outer = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(inner), AnyNodeId::Dynamic(value))],
    );

    let get = table_get(&mut m, root, outer, inner);
    assert_eq!(
        m.evaluate_node_deep(get, None).unwrap(),
        TestValue::LowValue(LowValue::USize(42)),
        "the table key is found before the freeze"
    );

    let (mut importer, iroot, freeze) = reload_after_freeze(&m);
    let outer_leaf = importer.materialize_leaf(sref_of(&freeze, outer), iroot);
    let inner_leaf = importer.materialize_leaf(sref_of(&freeze, inner), iroot);
    let get = table_get(&mut importer, iroot, outer_leaf, inner_leaf);
    let read = importer.evaluate_node_deep(get, None).unwrap();
    assert!(
        importer.eval_errors.is_empty(),
        "{:?}",
        importer.eval_errors
    );
    assert_eq!(
        read,
        TestValue::LowValue(LowValue::USize(42)),
        "the table key is still found after a freeze and a reload"
    );
}

#[test]
fn a_function_key_survives_a_freeze_and_a_reload() {
    // A function used as a key: `FunctionId` is a slotmap key, so the id the
    // hash was taken from is process-local; the template it names is the only
    // thing a reload preserves.  The definition pass runs the body between the
    // build and the freeze, so a hash that read the body's memoized result
    // would drift too.
    let mut m = Module::new();
    let root = m.add_block(None);
    let (function, body, _) = function(&mut m, |m, r#return, parameter| {
        let block = m.node_block(r#return);
        let one = u128_node(m, block, 1);
        let two = u128_node(m, block, 2);
        let operands = array_node(m, block, &[one, two], None);
        let sum = op_node(m, block, TestOperator::Add, Some(operands));
        let items = m.alloc_array(
            &[
                ArrayItem::new(AnyNodeId::Dynamic(parameter)),
                ArrayItem::new(AnyNodeId::Dynamic(sum)),
            ],
            block,
        );
        m.write_node_value(r#return, Some(TestValue::LowValue(LowValue::Array(items))));
    });
    let value = usize_node(&mut m, root, 7);
    let table = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(function), AnyNodeId::Dynamic(value))],
    );

    let get = table_get(&mut m, root, table, function);
    assert_eq!(
        m.evaluate_node_deep(get, None).unwrap(),
        TestValue::LowValue(LowValue::USize(7)),
        "the function key is found before the freeze"
    );

    // The definition pass runs the template's body — `x => [x, 1 + 2]` with
    // the `Add`'s operand chain already bound.
    m.evaluate_node_deep(body, None);

    let (mut importer, iroot, freeze) = reload_after_freeze(&m);
    let table_leaf = importer.materialize_leaf(sref_of(&freeze, table), iroot);
    let function_leaf = importer.materialize_leaf(sref_of(&freeze, function), iroot);
    let get = table_get(&mut importer, iroot, table_leaf, function_leaf);
    let read = importer.evaluate_node_deep(get, None).unwrap();
    assert!(
        importer.eval_errors.is_empty(),
        "{:?}",
        importer.eval_errors
    );
    assert_eq!(
        read,
        TestValue::LowValue(LowValue::USize(7)),
        "the function key is still found after a freeze and a reload"
    );
}

#[test]
fn a_key_of_the_program_s_own_value_vocabulary_is_hashed_not_refused() {
    // The program's own value variants are key content like any other: a
    // vocabulary with handle-carrying values reaches this path from ordinary
    // source (a type constant as a table key), and it used to be an
    // `unreachable!`.  The lowlevel cannot unfold a variant it does not know,
    // so equal values match through `key_eq` in the equal-hash run and a
    // different one misses.
    let mut m = Module::new();
    let root = m.add_block(None);
    let five = u128_node(&mut m, root, 5);
    let value = usize_node(&mut m, root, 3);
    let table = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(five), AnyNodeId::Dynamic(value))],
    );

    let same = u128_node(&mut m, root, 5);
    let get = table_get(&mut m, root, table, same);
    assert_eq!(
        m.evaluate_node_deep(get, None).unwrap(),
        TestValue::LowValue(LowValue::USize(3)),
        "an equal value of the program's own vocabulary is the same key"
    );

    let other = u128_node(&mut m, root, 6);
    let get = table_get(&mut m, root, table, other);
    assert_eq!(
        m.evaluate_node_deep(get, None).unwrap(),
        TestValue::LowValue(LowValue::Error),
        "a different value misses"
    );
    assert!(
        m.eval_errors
            .iter()
            .any(|error| matches!(error, EvalError::TableMiss { .. })),
        "the miss is recorded: {:?}",
        m.eval_errors
    );
}

#[test]
fn coinductively_equal_cyclic_keys_hash_equal_across_depth() {
    // `[1, ↺]` and `[1, [1, ↺]]` are the same infinite structure, so `key_eq`
    // calls them equal; a cycle token that carries the revisit depth does not,
    // and under a pre-filter that disagreement is a miss.
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = cyclic_pair(&mut m, root);
    // `b = [1, [1, b]]`.
    let b = m.add_node(root, None, None);
    let one = usize_node(&mut m, root, 1);
    let inner = array_node(&mut m, root, &[one, b], None);
    let items = m.alloc_array(
        &[
            ArrayItem::new(AnyNodeId::Dynamic(one)),
            ArrayItem::new(AnyNodeId::Dynamic(inner)),
        ],
        root,
    );
    m.write_node_value(b, Some(TestValue::LowValue(LowValue::Array(items))));

    let value = usize_node(&mut m, root, 9);
    let table = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(a), AnyNodeId::Dynamic(value))],
    );
    let get = table_get(&mut m, root, table, b);
    let read = m.evaluate_node_deep(get, None).unwrap();
    assert!(m.eval_errors.is_empty(), "{:?}", m.eval_errors);
    assert_eq!(
        read,
        TestValue::LowValue(LowValue::USize(9)),
        "a coinductively equal key must be found"
    );
}

#[test]
fn a_cyclic_key_is_found_across_the_static_boundary() {
    // The stored key is a cycle inside a frozen module; the lookup key closes
    // the same cycle one level deeper and through a static ref.  The two are
    // equal under `key_eq` and — with a depth-bounded unfolding — under the
    // hash as well.
    let mut m = Module::new();
    let root = m.add_block(None);
    let key = cyclic_pair(&mut m, root);
    let value = usize_node(&mut m, root, 5);
    let table = table_value(
        &mut m,
        root,
        &[(AnyNodeId::Dynamic(key), AnyNodeId::Dynamic(value))],
    );
    let get = table_get(&mut m, root, table, key);
    assert_eq!(
        m.evaluate_node_deep(get, None).unwrap(),
        TestValue::LowValue(LowValue::USize(5)),
        "the cyclic key is found before the freeze"
    );

    let (mut importer, iroot, freeze) = reload_after_freeze(&m);
    let table_leaf = importer.materialize_leaf(sref_of(&freeze, table), iroot);
    let frozen_key = importer.materialize_leaf(sref_of(&freeze, key), iroot);
    // `[1, [1, frozen_key]]` — coinductively equal to the stored `[1, ↺]`.
    let one = usize_node(&mut importer, iroot, 1);
    let inner = array_node(&mut importer, iroot, &[one, frozen_key], None);
    let query = array_node(&mut importer, iroot, &[one, inner], None);

    let get = table_get(&mut importer, iroot, table_leaf, query);
    let read = importer.evaluate_node_deep(get, None).unwrap();
    assert!(
        importer.eval_errors.is_empty(),
        "{:?}",
        importer.eval_errors
    );
    assert_eq!(
        read,
        TestValue::LowValue(LowValue::USize(5)),
        "the cyclic key is still found through the static module"
    );
}
