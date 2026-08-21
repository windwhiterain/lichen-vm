//! Node evaluation: operator execution, the cycle guard, and the
//! visiting / parameterized markers left behind by `evaluate_node_deep`.

use super::*;

#[test]
fn add_sums_u128_operands() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 3);
    let b = u128_node(&mut m, root, 4);
    let operands = array_node(&mut m, root, &[a, b]);
    let add = op_node(&mut m, root, Operator::Ext(TestOperator::Add), Some(operands));

    let value = m.evaluate_node_deep(add,None);

    assert_eq!(u128_of(value), 7);
}
#[test]
fn concat_joins_string_operands() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = str_node(&mut m, root, &['a', 'b']);
    let b = str_node(&mut m, root, &['c', 'd']);
    let operands = array_node(&mut m, root, &[a, b]);
    let concat = op_node(&mut m, root, Operator::Ext(TestOperator::Concat), Some(operands));

    let value = m.evaluate_node_deep(concat,None);

    assert_eq!(string_of(value), vec!['a', 'b', 'c', 'd']);
}
#[test]
fn index_selects_array_element() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 10);
    let b = u128_node(&mut m, root, 20);
    let arr = array_node(&mut m, root, &[a, b]);
    let idx = usize_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[arr, idx]);
    let index = op_node(&mut m, root, Operator::Index, Some(operands));

    let value = m.evaluate_node_deep(index,None);

    assert_eq!(u128_of(value), 20);
}
#[test]
#[should_panic(expected = "cycle")]
fn cyclic_operations_panic_instead_of_looping() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = op_node(&mut m, root, Operator::Ext(TestOperator::Id), None);
    let b = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(a));
    // Close the loop a -> b -> a through the public operation fields; the
    // evaluating state marks b as in-progress on re-entry and panics.
    m.nodes[a].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Id),
        operand: Some(b),
    });
    m.evaluate_node_deep(b, None);
}
#[test]
fn visiting_markers_are_cleared_after_evaluation() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 3);
    let b = u128_node(&mut m, root, 4);
    let operands = array_node(&mut m, root, &[a, b]);
    let add = op_node(&mut m, root, Operator::Ext(TestOperator::Add), Some(operands));

    m.evaluate_node_deep(add, None);

    assert!(m.nodes.values().all(|n| !n.visiting));
}
#[test]
fn parameterized_deep_marks_subtrees_with_parameters() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let p = m.add_node(root, None, Some(Value::Parameterized));
    let x = u128_node(&mut m, root, 5);
    let arr = array_node(&mut m, root, &[x, p]);
    let id_arr = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(arr));
    let id_p = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(p));

    m.evaluate_node_deep(id_arr, None);

    // The parameter node itself and everything reachable from it is flagged;
    // plain constants are not.
    assert_eq!(m.nodes[p].parameterized_deep, Some(true));
    assert_eq!(m.nodes[arr].parameterized_deep, Some(true));
    assert_eq!(m.nodes[id_arr].parameterized_deep, Some(true));
    assert_eq!(m.nodes[x].parameterized_deep, Some(false));
    assert_eq!(m.nodes[id_p].parameterized_deep, None); // not yet evaluated

    m.evaluate_node_deep(id_p, None);
    assert_eq!(m.nodes[id_p].parameterized_deep, Some(true));
}
#[test]
fn sub_eq_lt_operators_compute_concrete_results() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let two = u128_node(&mut m, root, 2);
    let three = u128_node(&mut m, root, 3);
    // Sub: 3 - 2 = 1
    let sub_ops = array_node(&mut m, root, &[three, two]);
    let sub = op_node(&mut m, root, Operator::Ext(TestOperator::Sub), Some(sub_ops));
    assert_eq!(u128_of(m.evaluate_node_deep(sub, None)), 1);
    // Eq: 3 == 3
    let eq_ops = array_node(&mut m, root, &[three, three]);
    let eq = op_node(&mut m, root, Operator::Ext(TestOperator::Eq), Some(eq_ops));
    assert!(matches!(m.evaluate_node_deep(eq, None), Value::USize(1)));
    // Lt: 3 < 2 is false
    let lt_ops = array_node(&mut m, root, &[three, two]);
    let lt = op_node(&mut m, root, Operator::Ext(TestOperator::Lt), Some(lt_ops));
    assert!(matches!(m.evaluate_node_deep(lt, None), Value::USize(0)));
}
