//! Block compaction and garbage collection: what is hoisted into the
//! parent, what is released with a vacated block, and how GC walks
//! unevaluated operands.

use super::*;

#[test]
fn redundant_nodes_are_not_compacted() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let x = u128_node(&mut m, child, 5);
    let y = u128_node(&mut m, child, 7); // redundant: never referenced
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(x));

    let value = m.evaluate_node_deep(root_node, None);

    assert_eq!(u128_of(value), 5);
    assert_eq!(m.nodes.len(), 2); // root_node + child's kept return x
    assert!(!m.nodes.contains_key(y));
}
#[test]
fn u128_payload_is_relocated_into_parent_and_block_releasable() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // Marker allocated in root's arena before the child runs.  Bumpalo
    // allocates downward within a chunk, so later allocations sit at
    // lower addresses.
    let marker = m.blocks[root].arena.alloc_slice_copy(b"marker");
    let marker_start = marker.as_ptr() as usize;

    let child = m.add_block(Some(root));
    let x = u128_node(&mut m, child, 42);
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(x));

    let value = m.evaluate_node_deep(root_node, None);
    let TestValue::U128(ptr) = value else {
        panic!("expected U128")
    };
    assert_eq!(u128_of(value), 42);
    // Relocated into root's arena: the copy was made after the marker,
    // so it sits below it in the same chunk.
    let ptr = match ptr {
        AnyHandle::Dynamic(h) => h.0 as *const u8,
        AnyHandle::Static(h) => h.offset as *const u8,
    };
    assert!(ptr as usize + 16 <= marker_start);

    // The child block was released: gone from the block table, yet the
    // value still points into root's arena.
    assert!(!m.blocks.contains_key(child));
    assert_eq!(u128_of(value), 42);
}
#[test]
fn array_return_compacts_elements_into_parent() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let a = u128_node(&mut m, child, 10);
    let b = u128_node(&mut m, child, 20);
    let c = u128_node(&mut m, child, 30);
    let ret = array_node(&mut m, child, &[a, b, c], None);
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(ret));

    let value = m.evaluate_node_deep(root_node, None);

    // The element nodes keep their ids: their data was relocated into the
    // root's arena, so they stay readable after the child was released.
    assert_u128_array(&m, value, &[10, 20, 30]);
    assert_eq!(m.nodes.len(), 5); // root_node + ret + a, b, c
    assert!(m.nodes.contains_key(a));
    assert!(m.nodes.contains_key(b));
    assert!(m.nodes.contains_key(c));
}
#[test]
fn nested_scalar_return_compacts_into_grandparent() {
    let mut m = Module::new();
    let grandparent = m.add_block(None);
    let outer = m.add_block(Some(grandparent));
    let inner = m.add_block(Some(outer));
    let x = u128_node(&mut m, inner, 9);
    let ret = array_node(&mut m, outer, &[x], None); // outer's return references inner's return x
    let root_node = op_node(&mut m, grandparent, TestOperator::Id, Some(ret));

    let value = m.evaluate_node_deep(root_node, None);

    assert_u128_array(&m, value, &[9]);
    assert_eq!(m.nodes.len(), 3); // root_node + outer's kept return + inner's kept return
    assert!(m.nodes.contains_key(x));
}
#[test]
fn nested_array_return_relocates_data_twice() {
    let mut m = Module::new();
    let grandparent = m.add_block(None);
    let outer = m.add_block(Some(grandparent));
    let inner = m.add_block(Some(outer));
    let c = u128_node(&mut m, inner, 7);
    let inner_ret = array_node(&mut m, inner, &[c], None);
    let outer_ret = array_node(&mut m, outer, &[inner_ret], None);
    let root_node = op_node(&mut m, grandparent, TestOperator::Id, Some(outer_ret));

    let value = m.evaluate_node_deep(root_node, None);

    // inner's data was relocated into outer first, then into grandparent;
    // all node ids survive unchanged.
    let ids = array_ids(value);
    assert_eq!(ids.len(), 1);
    let ids = array_ids(m.nodes[ids[0]].value.unwrap());
    assert_eq!(ids.len(), 1);
    assert_eq!(u128_of(m.nodes[ids[0]].value.unwrap()), 7);
    assert_eq!(m.nodes.len(), 4); // root_node + outer_ret + inner_ret + c
    assert!(m.nodes.contains_key(c));
}
#[test]
fn compact_preserves_the_shallow_mask() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // [7, Add(1, 2)] with position 1 marked shallow, homed in the child:
    // compaction moves the ids into root's arena and must carry the mask
    // with them.
    let seven = u128_node(&mut m, child, 7);
    let one = u128_node(&mut m, child, 1);
    let two = u128_node(&mut m, child, 2);
    let add_ops = array_node(&mut m, child, &[one, two], None);
    let add = op_node(&mut m, child, TestOperator::Add, Some(add_ops));
    let ret = array_node(&mut m, child, &[seven, add], Some(&[false, true]));

    let value = m.evaluate_node_deep(ret, None);
    assert_eq!(array_mask(value), [false, true]);

    m.garbage_collect(ret);
    let value = m.nodes[ret].value.unwrap();
    assert_eq!(
        array_mask(value),
        [false, true],
        "the mask survives compaction"
    );
    assert_eq!(array_ids(value).len(), 2);
    assert_eq!(m.nodes[ret].block, root, "compacted into the parent");
    assert!(
        m.nodes[array_ids(value)[1]].value.is_none(),
        "shallow element stays lazy"
    );
}
#[test]
fn unreferenced_child_blocks_are_released() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let x = u128_node(&mut m, child, 5);
    let grandchild = m.add_block(Some(child));
    let orphan = u128_node(&mut m, grandchild, 9); // never referenced
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(x));

    let value = m.evaluate_node_deep(root_node, None);

    assert_eq!(u128_of(value), 5);
    assert_eq!(m.nodes.len(), 2); // root_node + child's kept return x
    assert!(!m.nodes.contains_key(orphan));
}
#[test]
fn block_run_pulls_outer_and_sibling_blocks() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let p = m.add_block(Some(root));
    let c = m.add_block(Some(p));
    let s = m.add_block(Some(p)); // sibling of c
    let z = u128_node(&mut m, s, 11); // s's return
    let y = op_node(&mut m, p, TestOperator::Id, Some(z)); // p's node uses sibling s
    let c_ret = op_node(&mut m, c, TestOperator::Id, Some(y)); // c references outer y
    let p_ret = op_node(&mut m, p, TestOperator::Id, Some(c_ret)); // p's return is c's result
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(p_ret));

    let value = m.evaluate_node_deep(root_node, None);

    // Running p ran c, whose resolution pulled in p's outer node y,
    // which ran sibling s; the result is compacted up to the root.
    assert_eq!(u128_of(value), 11);
    assert_eq!(m.nodes.len(), 2); // root_node + p's kept return
    assert!(!m.nodes.contains_key(z));
    assert!(!m.blocks.contains_key(s)); // sibling s was released
    assert!(!m.blocks.contains_key(c));
    assert!(!m.blocks.contains_key(p));
}
#[test]
fn deep_never_run_block_chain_releases_stack_safely() {
    let mut m = Module::new();
    let top = m.add_block(None);
    let mut prev = top;
    for _ in 0..100_000 {
        prev = m.add_block(Some(prev));
    }
    let first = m.blocks[top].children[0];
    let x = u128_node(&mut m, first, 1);

    // Running first releases its whole never-run subtree, 100_000 blocks deep.
    m.evaluate_node_deep(x, Some(top));

    assert_eq!(u128_of(m.nodes[x].value.unwrap()), 1);
    assert_eq!(m.nodes.len(), 1); // only x survived, moved to top
    assert!(m.blocks.contains_key(top));
    assert!(!m.blocks.contains_key(first));
    assert_eq!(m.blocks.len(), 1); // top is all that remains
}
#[test]
fn deep_block_chain_evaluates_stack_safely() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let mut chain = vec![root];
    for _ in 0..100_000 {
        let id = m.add_block(Some(*chain.last().unwrap()));
        chain.push(id);
    }
    // Deepest block holds the constant; each block's return op references the
    // child block's return node, so evaluation nests 100_000 block runs deep.
    let mut ret = u128_node(&mut m, *chain.last().unwrap(), 7);
    for i in (1..chain.len() - 1).rev() {
        ret = op_node(&mut m, chain[i], TestOperator::Id, Some(ret));
    }
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(ret));

    let value = m.evaluate_node_deep(root_node, None);

    assert_eq!(u128_of(value), 7);
    assert_eq!(m.blocks.len(), 1); // only root remains, chain compacted into it
    assert!(m.nodes.values().all(|n| !n.visiting));
}
#[test]
fn garbage_collect_hoists_uncompacted_descendants() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let grandchild = m.add_block(Some(child));
    let a = u128_node(&mut m, grandchild, 10);
    let b = u128_node(&mut m, grandchild, 20);
    let orphan = u128_node(&mut m, grandchild, 99); // never referenced
    let ret = array_node(&mut m, child, &[a, b], None);

    // Collect the child's return *without* deep-evaluating it first: the
    // elements still live in the un-compacted grandchild block, so the
    // move must pull them straight through to the root in one pass, and
    // the release sweeps the whole vacated subtree.
    let value = m.garbage_collect(ret).expect("evaluated return node");

    assert_u128_array(&m, value, &[10, 20]);
    assert_eq!(m.nodes[ret].block, root);
    assert_eq!(m.nodes[a].block, root); // hoisted past the child, not into it
    assert_eq!(m.nodes[b].block, root);
    // The release dropped the grandchild and its orphan, but the hoisted
    // elements survive.
    assert!(!m.blocks.contains_key(child));
    assert!(!m.blocks.contains_key(grandchild));
    assert!(!m.nodes.contains_key(orphan));
    assert_u128_array(&m, value, &[10, 20]);
}
#[test]
fn garbage_collect_leaves_sibling_and_ancestor_content_in_place() {
    let mut m = Module::new();
    let grandparent = m.add_block(None);
    let parent = m.add_block(Some(grandparent));
    let child = m.add_block(Some(parent));
    let sibling = m.add_block(Some(grandparent));
    let c_gp = u128_node(&mut m, grandparent, 1); // ancestor, not the target
    let s_node = u128_node(&mut m, sibling, 2);
    let ret = array_node(&mut m, child, &[c_gp, s_node], None);

    let value = m.garbage_collect(ret).expect("evaluated return node");

    assert_u128_array(&m, value, &[1, 2]);
    assert_eq!(m.nodes[ret].block, parent);
    assert_eq!(m.nodes[c_gp].block, grandparent); // untouched
    assert_eq!(m.nodes[s_node].block, sibling); // untouched
    assert!(!m.blocks.contains_key(child)); // only the vacated block is released
    assert!(m.blocks.contains_key(sibling));
    assert!(m.blocks.contains_key(grandparent));
}
#[test]
fn garbage_collect_rehomes_function_from_uncompacted_descendant() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let grandchild = m.add_block(Some(child));
    // f(x) = Id(x), homed in the un-compacted grandchild block.
    let ret_f = m.add_node(grandchild, None, None);
    let param_f = m.add_node(
        grandchild,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    m.nodes[ret_f].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(param_f),
    });
    let (func_node, f) = wrap_function(&mut m, grandchild, ret_f, param_f);
    let ret = array_node(&mut m, child, &[func_node], None);

    // A direct collect re-homes the function and maps its scope members
    // into the root, releasing the vacated subtree in the same call.
    let value = m.garbage_collect(ret).expect("evaluated return node");
    assert_eq!(array_ids(value), &[func_node]);

    assert_eq!(m.nodes[ret].block, root);
    assert_eq!(m.nodes[func_node].block, root);
    assert_eq!(m.functions[f].block, root); // re-homed out of the grandchild
    assert!(m.blocks[root].functions.contains(&f));
    assert_eq!(m.nodes[ret_f].block, root); // scope mapped along with it
    assert_eq!(m.nodes[param_f].block, root);
    assert_eq!(
        m.functions[f].nodes.iter().copied().collect::<HashSet<_>>(),
        HashSet::from([ret_f, param_f])
    );

    // The vacated subtree is gone, but the function is still callable.
    assert!(!m.blocks.contains_key(child));
    assert!(!m.blocks.contains_key(grandchild));
    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);
}
#[test]
fn garbage_collect_hoists_unevaluated_scalar_operand() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let grandchild = m.add_block(Some(child));
    let x = u128_node(&mut m, grandchild, 42);
    let ret = op_node(&mut m, child, TestOperator::Id, Some(x));
    let orphan = u128_node(&mut m, grandchild, 99); // never referenced

    // `ret` is unevaluated, so its operand is still live: collecting the
    // child hoists x through the operand edge instead of dropping it with
    // the vacated subtree; only the orphan, which no edge reaches, dies.
    assert!(m.garbage_collect(ret).is_none());
    assert_eq!(m.nodes[ret].block, root);
    assert!(m.nodes[ret].value.is_none());
    assert_eq!(m.nodes[x].block, root); // operand hoisted, not dropped
    assert!(!m.blocks.contains_key(child));
    assert!(!m.blocks.contains_key(grandchild));
    assert!(!m.nodes.contains_key(orphan));
    // The hoisted operand is still evaluable.
    assert_eq!(u128_of(m.evaluate_node_deep(ret, None)), 42);
}
#[test]
fn garbage_collect_enters_unevaluated_subtree_via_operand() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let grandchild = m.add_block(Some(child));
    let x = u128_node(&mut m, grandchild, 7);
    let operands = array_node(&mut m, grandchild, &[x], None);
    let ret = op_node(&mut m, child, TestOperator::Id, Some(operands));

    // The unevaluated subtree behind `ret`'s operand is entered through
    // the operand edge: the operand array is hoisted, and its array
    // element with it, so nothing the future evaluation of `ret` needs is
    // dropped with the inner block.
    assert!(m.garbage_collect(ret).is_none());
    assert_eq!(m.nodes[operands].block, root); // hoisted via the operand edge
    assert_eq!(m.nodes[x].block, root); // and its array element with it
    assert!(!m.blocks.contains_key(child));
    assert!(!m.blocks.contains_key(grandchild));
    let value = m.evaluate_node_deep(ret, None);
    assert_u128_array(&m, value, &[7]);
}
#[test]
fn garbage_collect_skips_operands_of_evaluated_nodes() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let grandchild = m.add_block(Some(child));
    let x = u128_node(&mut m, grandchild, 7);
    let operands = array_node(&mut m, grandchild, &[x], None);
    let ret = op_node(&mut m, child, TestOperator::Id, Some(operands));

    // Evaluating `ret` memoizes its result, so the operand edge is dead: a
    // later collect must not drag the operand subtree up with it.  Only
    // the value-reachable element x survives; the operand array itself is
    // dropped with the vacated block.
    let evaluated = m.evaluate_node_deep(ret, None);
    assert_u128_array(&m, evaluated, &[7]);
    let value = m.garbage_collect(ret).expect("evaluated return node");
    assert_eq!(m.nodes[ret].block, root);
    assert_eq!(m.nodes[x].block, root); // kept by the value edge
    assert!(!m.nodes.contains_key(operands)); // dead operand edge not followed
    assert!(!m.blocks.contains_key(child));
    assert!(!m.blocks.contains_key(grandchild));
    assert_u128_array(&m, value, &[7]);
}
#[test]
fn call_clones_are_compacted_with_the_calling_block() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // g(x) = Add(x, 1) lives at the root level.
    let (g_node, g_ret, _g_param) = function(&mut m, |m, ret, param| {
        let one = u128_node(m, m.nodes[ret].block, 1);
        let operands = array_node(m, m.nodes[ret].block, &[param, one], None);
        m.nodes[ret].operation = Some(Operation {
            operator: TestOperator::Add,
            operand: Some(operands),
        });
    });
    m.evaluate_node_deep(g_ret, None); // definition pass

    // The call clones g's body into the child block.
    let five = u128_node(&mut m, child, 5);
    let call = call_node(&mut m, child, g_node, five);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 6);
    assert_eq!(m.nodes[call].block, child);

    // Compacting the child moves the call node (with its cached result)
    // into the root; the clone nodes it used are released with the block.
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(call));
    assert_eq!(u128_of(m.evaluate_node_deep(root_node, None)), 6);
    assert_eq!(m.nodes[call].block, root);
    assert!(!m.blocks.contains_key(child));

    // The root-level function is untouched and still callable.
    let two = u128_node(&mut m, root, 2);
    let call2 = call_node(&mut m, root, g_node, two);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 3);
}
