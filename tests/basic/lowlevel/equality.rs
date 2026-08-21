//! The DSU equivalence classes behind `unify`, and how clones start fresh.

use super::*;

#[test]
fn add_equality_merges_equivalence_classes() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 1);
    let b = u128_node(&mut m, root, 2);
    let c = u128_node(&mut m, root, 3);

    // Fresh nodes are their own representatives.
    assert_eq!(m.equality_representative(a), a);
    assert_eq!(m.equality_representative(b), b);

    let rep = m.add_equality(a, b);
    assert_eq!(m.equality_representative(a), rep);
    assert_eq!(m.equality_representative(b), rep);
    assert_ne!(m.equality_representative(c), rep);

    // Equality is transitive: merging b's class with c pulls a in too.
    m.add_equality(b, c);
    assert_eq!(m.equality_representative(a), rep);
    assert_eq!(m.equality_representative(c), rep);
}
#[test]
fn root_node_compresses_deep_paths() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let nodes: Vec<_> = (0..5).map(|i| u128_node(&mut m, root, i as u128)).collect();
    // Build a deliberately deep parent chain 0 <- 1 <- 2 <- 3 <- 4 by hand
    // (union by size would attach every new node directly under the root).
    for pair in nodes.windows(2) {
        m.nodes[pair[1]].equality.parent = Some(pair[0]);
    }

    let rep = m.equality_representative(nodes[4]);

    assert_eq!(rep, nodes[0]);
    // The whole path was flattened onto the representative.
    for &n in &nodes {
        assert_eq!(m.nodes[n].equality.parent, (n != nodes[0]).then_some(nodes[0]));
    }
}
#[test]
fn cloned_function_nodes_start_in_their_own_equality_class() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, ret, _param) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Id),
            operand: Some(param),
        });
    });

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    m.evaluate_node_deep(call, None);

    // A clone is a fresh node, so it starts as a singleton class unrelated
    // to the body's nodes: find the call's clone of ret (the Id op whose
    // operand is the clone of the parameter, unified with the argument) in
    // the call node's block.
    let candidates: Vec<NodeId> = m.blocks[root]
        .nodes
        .iter()
        .copied()
        .filter(|&id| m.nodes[id].operation.is_some())
        .collect();
    let clone_ret = candidates
        .into_iter()
        .find(|&id| {
            let operand = m.nodes[id].operation.unwrap().operand.unwrap();
            m.equality_representative(operand) == m.equality_representative(arg)
        })
        .expect("the call clone of ret");
    assert_eq!(m.equality_representative(clone_ret), clone_ret);
    assert_ne!(m.equality_representative(clone_ret), m.equality_representative(ret));
}
